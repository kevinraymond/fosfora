//! 3-stage beat detection pipeline: OnsetDetector → TempoEstimator → BeatScheduler.
//! Ported from easey-glyph's Python implementation.

use rustfft::FftPlanner;
use rustfft::num_complex::Complex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// A7 (#1458): tempo prior configuration + manual octave/tap overrides
// ---------------------------------------------------------------------------

/// Lowest/highest BPM the tempo estimator will report. Manual octave shifts and tap
/// tempo are rejected when they would land outside this window.
pub const BPM_MIN: f64 = 40.0;
pub const BPM_MAX: f64 = 300.0;

/// Auto-prior adaptation (A7 #1458). The rate is per tempo update — the estimator runs
/// one every 6 frames (~14 Hz), so 0.001 is a ~70s time constant: slow enough that a
/// transient mis-lock can't drag the prior with it. Bounds are tighter than
/// `BPM_MIN`/`BPM_MAX`: the *prior centre* has no business out at 40 or 300.
const AUTO_PRIOR_RATE: f64 = 0.001;
const AUTO_PRIOR_MIN_CONFIDENCE: f64 = 0.5;
const AUTO_PRIOR_MIN_BPM: f64 = 60.0;
const AUTO_PRIOR_MAX_BPM: f64 = 200.0;

/// Bounds on the prior width, in octaves. Zero would make the prior a delta function that
/// rejects every candidate; the upper bound is already effectively "no opinion".
const MIN_PRIOR_SIGMA: f64 = 0.05;
const MAX_PRIOR_SIGMA: f64 = 4.0;

/// Prior centre in log2 space, clamped to the range the estimator can actually report.
fn prior_center_log2(bpm: f32) -> f64 {
    (bpm as f64).clamp(BPM_MIN, BPM_MAX).log2()
}

/// User-tunable tempo prior (A7 #1458). The estimator scores metrical-ratio candidates
/// with a log-Gaussian centred on `prior_center_bpm`, so this is what decides whether a
/// 172 BPM DnB track reads as 172 or folds to 86.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TempoConfig {
    /// Centre of the log-Gaussian tempo prior, in BPM.
    pub prior_center_bpm: f32,
    /// Prior width in octaves (log2 BPM). Small = a strong opinion about the octave.
    pub prior_sigma: f32,
    /// When set, the estimator slowly walks `prior_center_bpm` toward the tempo it is
    /// actually locking onto. The audio thread publishes the adapted value back into the
    /// shared config, so the UI reads it live and it freezes in place when auto is off.
    pub auto_prior: bool,
}

impl Default for TempoConfig {
    fn default() -> Self {
        // The pre-A7 hardcoded values — upgrading users get identical detection until
        // they pick a preset.
        Self {
            prior_center_bpm: 150.0,
            prior_sigma: 1.0,
            auto_prior: false,
        }
    }
}

/// Genre presets for the tempo prior (A7 #1458).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempoPreset {
    Neutral,
    Wide,
    House,
    DrumAndBass,
    HipHop,
    Ambient,
}

impl TempoPreset {
    pub const ALL: &[TempoPreset] = &[
        TempoPreset::Neutral,
        TempoPreset::Wide,
        TempoPreset::House,
        TempoPreset::DrumAndBass,
        TempoPreset::HipHop,
        TempoPreset::Ambient,
    ];

    /// (centre BPM, sigma in octaves).
    pub fn values(self) -> (f32, f32) {
        match self {
            Self::Neutral => (150.0, 1.0),
            Self::Wide => (140.0, 1.2),
            Self::House => (127.0, 0.35),
            Self::DrumAndBass => (172.0, 0.3),
            Self::HipHop => (90.0, 0.4),
            Self::Ambient => (70.0, 0.6),
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Neutral => "Neutral \u{00b7} 150",
            Self::Wide => "Wide \u{00b7} 140",
            Self::House => "House \u{00b7} 127",
            Self::DrumAndBass => "Drum & Bass \u{00b7} 172",
            Self::HipHop => "Hip-hop \u{00b7} 90",
            Self::Ambient => "Ambient \u{00b7} 70",
        }
    }

    /// The preset matching this config exactly, or `None` when the user has hand-tuned
    /// the sliders. Keeps the config the single source of truth — no preset field to
    /// drift out of sync with the values it names.
    pub fn from_config(cfg: &TempoConfig) -> Option<TempoPreset> {
        Self::ALL.iter().copied().find(|p| {
            let (c, s) = p.values();
            cfg.prior_center_bpm == c && cfg.prior_sigma == s
        })
    }
}

/// One-shot tempo override from the UI / MIDI / OSC (A7 #1458). Queued in
/// [`TempoControl::pending`] and drained by the audio thread each hop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TempoCommand {
    /// Force the reported tempo up (+1) or down (-1) an octave.
    ShiftOctave(i32),
    /// Lock onto a tapped tempo, in BPM (averaged UI-side — see `audio_panel`).
    Tap(f64),
}

/// Shared tempo state: live config plus a small command mailbox, both behind one mutex
/// the audio thread locks once per hop (the #1510 pattern, extended with the mailbox the
/// A7 note called for). Cloned into the audio thread and threaded through `switch_device`,
/// so user tuning survives a device change.
#[derive(Debug, Default)]
pub struct TempoControl {
    pub config: TempoConfig,
    pending: Vec<TempoCommand>,
}

impl TempoControl {
    pub fn new(config: TempoConfig) -> Self {
        Self {
            config,
            pending: Vec::new(),
        }
    }

    /// Queue a command for the audio thread. Bounded so a stalled/absent audio thread
    /// can't grow this without limit — dropping the oldest keeps the newest intent.
    pub fn push(&mut self, cmd: TempoCommand) {
        const MAX_PENDING: usize = 16;
        if self.pending.len() >= MAX_PENDING {
            self.pending.remove(0);
        }
        self.pending.push(cmd);
    }

    pub fn drain(&mut self) -> Vec<TempoCommand> {
        std::mem::take(&mut self.pending)
    }
}

// ---------------------------------------------------------------------------
// Circular buffer (fixed-size ring buffer with statistical methods)
// ---------------------------------------------------------------------------

struct CircularBuffer {
    buf: Vec<f64>,
    cap: usize,
    write: usize,
    count: usize,
}

impl CircularBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            buf: vec![0.0; capacity],
            cap: capacity,
            write: 0,
            count: 0,
        }
    }

    fn push(&mut self, value: f64) {
        self.buf[self.write] = value;
        self.write = (self.write + 1) % self.cap;
        if self.count < self.cap {
            self.count += 1;
        }
    }

    fn len(&self) -> usize {
        self.count
    }

    fn values(&self) -> Vec<f64> {
        if self.count == 0 {
            return Vec::new();
        }
        if self.count < self.cap {
            self.buf[..self.count].to_vec()
        } else {
            let start = self.write;
            let mut v = Vec::with_capacity(self.cap);
            v.extend_from_slice(&self.buf[start..]);
            v.extend_from_slice(&self.buf[..start]);
            v
        }
    }

    fn median(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let mut vals = self.values();
        vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = vals.len() / 2;
        if vals.len().is_multiple_of(2) {
            f64::midpoint(vals[mid - 1], vals[mid])
        } else {
            vals[mid]
        }
    }

    fn mad(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let med = self.median();
        let mut abs_devs: Vec<f64> = self.values().iter().map(|v| (v - med).abs()).collect();
        abs_devs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = abs_devs.len() / 2;
        if abs_devs.len().is_multiple_of(2) {
            f64::midpoint(abs_devs[mid - 1], abs_devs[mid])
        } else {
            abs_devs[mid]
        }
    }

    fn max(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        if self.count < self.cap {
            self.buf[..self.count]
                .iter()
                .cloned()
                .fold(f64::NEG_INFINITY, f64::max)
        } else {
            self.buf.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
        }
    }

    // Retained stats-utility sibling of `median`/`mad`/`max` (has its own unit test). Its
    // prod caller was the tempo estimator's runtime frame-timing average, removed in A5
    // (#1456) now that the analysis hop is fixed.
    #[allow(dead_code)]
    fn mean(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        if self.count < self.cap {
            self.buf[..self.count].iter().sum::<f64>() / self.count as f64
        } else {
            self.buf.iter().sum::<f64>() / self.cap as f64
        }
    }
}

// ---------------------------------------------------------------------------
// Stage 1: Multi-band onset detection
// ---------------------------------------------------------------------------

/// SuperFlux onset detection (A6 #1457): a log-magnitude filterbank spectral flux with a
/// frequency **maximum filter** applied to the reference frame (Böck & Widmer, DAFx 2013).
/// The max filter lets a partial drift ±`SUPERFLUX_MAX_BINS` bands between frames without
/// registering flux, which suppresses the phantom onsets that plain flux fires on vibrato
/// and pitch slides. The bands are contiguous and log-spaced, so they cover 250–500 Hz —
/// the snare/tom/male-vocal gap the old four-band detector left open.
const N_ONSET_BANDS: usize = 64;
const ONSET_F_MIN: f32 = 20.0;
const ONSET_F_MAX: f32 = 16000.0;
/// Frequency max-filter half-width, in bands, on the reference frame.
const SUPERFLUX_MAX_BINS: usize = 1;
/// Partition edges (Hz) splitting the bands into low / mid / high, and the weights that
/// combine their mean flux. Preserves the old kick/bass-vs-snare-vs-hat balance; `mid` now
/// also spans the reclaimed 250–500 Hz. Weights sum to 1.
const ONSET_LOW_HZ: f32 = 250.0;
const ONSET_HIGH_HZ: f32 = 2000.0;
const ONSET_W_LOW: f64 = 0.60;
const ONSET_W_MID: f64 = 0.28;
const ONSET_W_HIGH: f64 = 0.12;

struct OnsetDetector {
    sample_rate: f32,
    threshold_mult: f32,
    threshold_ceiling: f32,

    /// `[lo_bin, hi_bin)` in the 4096-pt spectrum for each log band (computed once the
    /// spectrum length is known), the partition (0=low,1=mid,2=high) each band belongs to,
    /// and the band count per partition (for mean-flux normalization).
    band_bins: Vec<(usize, usize)>,
    band_partition: Vec<u8>,
    partition_counts: [usize; 3],
    /// Previous frame's per-band log-magnitude (the μ=1 reference the max filter runs over).
    prev_log: Vec<f64>,

    onset_history: CircularBuffer,
    long_term_history: CircularBuffer,
    silent_frames: u32,
}

impl OnsetDetector {
    fn new(sample_rate: f32, history_size: usize, long_term_size: usize) -> Self {
        Self {
            sample_rate,
            threshold_mult: 2.0,
            threshold_ceiling: 0.5,
            band_bins: Vec::new(),
            band_partition: Vec::new(),
            partition_counts: [0; 3],
            prev_log: Vec::new(),
            onset_history: CircularBuffer::new(history_size),
            long_term_history: CircularBuffer::new(long_term_size),
            silent_frames: 0,
        }
    }

    /// Build the log-spaced filterbank the first time we see the spectrum length. Each band
    /// spans at least one FFT bin (adjacent low bands may overlap a shared bin, which is
    /// harmless); its partition is decided by the band's geometric-centre frequency.
    fn ensure_bands(&mut self, num_bins: usize) {
        if self.band_bins.len() == N_ONSET_BANDS {
            return;
        }
        let bin_hz = self.sample_rate / ((num_bins - 1) * 2) as f32;
        let ratio = (ONSET_F_MAX / ONSET_F_MIN).powf(1.0 / N_ONSET_BANDS as f32);
        self.band_bins.clear();
        self.band_partition.clear();
        self.partition_counts = [0; 3];
        for b in 0..N_ONSET_BANDS {
            let f_lo = ONSET_F_MIN * ratio.powi(b as i32);
            let f_hi = ONSET_F_MIN * ratio.powi(b as i32 + 1);
            let lo = (f_lo / bin_hz).floor() as usize;
            let hi = ((f_hi / bin_hz).ceil() as usize).max(lo + 1).min(num_bins);
            self.band_bins
                .push((lo.min(num_bins.saturating_sub(1)), hi));
            let centre = (f_lo * f_hi).sqrt();
            let part = if centre < ONSET_LOW_HZ {
                0u8
            } else if centre < ONSET_HIGH_HZ {
                1u8
            } else {
                2u8
            };
            self.band_partition.push(part);
            self.partition_counts[part as usize] += 1;
        }
        self.prev_log.clear();
    }

    /// Process the multi-resolution spectra and return (is_onset, onset_strength,
    /// combined_flux). SuperFlux runs on the 4096-pt `bass_spectrum` (its fine, consistent
    /// frequency resolution is what the max filter needs); `mid_spectrum`/`high_spectrum`
    /// are part of the detector's interface but not consumed here. `loud_silent` is the
    /// perceptual silence gate from the A10 loudness meter (replaces the old raw-RMS gate).
    fn process(
        &mut self,
        bass_spectrum: &[f32],  // 4096-pt FFT magnitudes (num_bins)
        _mid_spectrum: &[f32],  // 1024-pt FFT magnitudes (unused by SuperFlux)
        _high_spectrum: &[f32], // 512-pt FFT magnitudes (unused by SuperFlux)
        loud_silent: bool,
    ) -> (bool, f32, f64) {
        // Unified perceptual silence gate (A10 #1461).
        if loud_silent {
            self.silent_frames += 1;
            return (false, 0.0, 0.0);
        }
        self.silent_frames = 0;

        self.ensure_bands(bass_spectrum.len());

        // Current per-band log magnitude.
        let mut cur = vec![0.0f64; N_ONSET_BANDS];
        for (b, &(lo, hi)) in self.band_bins.iter().enumerate() {
            let hi = hi.min(bass_spectrum.len());
            let mut e = 0.0f64;
            for &m in &bass_spectrum[lo..hi] {
                e += m as f64;
            }
            cur[b] = (e + 1e-10).ln();
        }

        // SuperFlux: half-wave-rectified difference against the frequency-max-filtered
        // reference frame, accumulated per partition.
        let mut part = [0.0f64; 3];
        if self.prev_log.len() == N_ONSET_BANDS {
            for b in 0..N_ONSET_BANDS {
                let lo = b.saturating_sub(SUPERFLUX_MAX_BINS);
                let hi = (b + SUPERFLUX_MAX_BINS).min(N_ONSET_BANDS - 1);
                let mut reference = f64::MIN;
                for &v in &self.prev_log[lo..=hi] {
                    reference = reference.max(v);
                }
                let flux = (cur[b] - reference).max(0.0);
                part[self.band_partition[b] as usize] += flux;
            }
        }
        self.prev_log.clone_from(&cur);

        // Mean flux per partition, weighted into one onset value (weights sum to 1).
        let mean = |sum: f64, count: usize| if count > 0 { sum / count as f64 } else { 0.0 };
        let combined_flux = ONSET_W_LOW * mean(part[0], self.partition_counts[0])
            + ONSET_W_MID * mean(part[1], self.partition_counts[1])
            + ONSET_W_HIGH * mean(part[2], self.partition_counts[2]);

        self.onset_history.push(combined_flux);
        self.long_term_history.push(combined_flux);

        // Adaptive threshold: median + k * MAD (unchanged).
        let threshold = self.compute_threshold();
        let is_onset = combined_flux > threshold;

        // Normalize onset strength to 0-1.
        let recent_max = self.long_term_history.max();
        let onset_strength = (combined_flux / recent_max.max(1e-6)).min(1.0) as f32;

        (is_onset, onset_strength, combined_flux)
    }

    fn compute_threshold(&self) -> f64 {
        let median = self.onset_history.median();
        let mad = self.onset_history.mad();
        let base_threshold = median + self.threshold_mult as f64 * mad;

        let min_threshold = 0.001;

        // Cap at proportion of long-term max
        let mut max_threshold = f64::INFINITY;
        if self.long_term_history.len() > self.onset_history.len() {
            let lt_max = self.long_term_history.max();
            max_threshold = lt_max * self.threshold_ceiling as f64;
        }

        // Also cap at 80% of recent max
        let recent_max = self.onset_history.max();
        let recent_ceiling = recent_max * 0.8;

        let capped = base_threshold.min(max_threshold).min(recent_ceiling);
        capped.max(min_threshold)
    }

    fn is_sustained_silence(&self) -> bool {
        self.silent_frames >= 30
    }
}

// ---------------------------------------------------------------------------
// Kalman filter for BPM tracking in log2-BPM space
// ---------------------------------------------------------------------------

struct KalmanBpm {
    state: f64,         // log2(BPM)
    variance: f64,      // estimation uncertainty
    q: f64,             // process noise
    r: f64,             // measurement noise
    diverge_count: u32, // leaky count of recent divergent frames
    initialized: bool,
}

impl KalmanBpm {
    fn new() -> Self {
        Self {
            state: 0.0,
            variance: 1.0,
            q: 0.001,
            r: 0.1,
            diverge_count: 0,
            initialized: false,
        }
    }

    /// Jump the state to `bpm` with high certainty (A7 #1458: tap tempo).
    fn force(&mut self, bpm: f64) {
        if bpm <= 0.0 {
            return;
        }
        self.state = bpm.log2();
        self.variance = 0.01;
        self.diverge_count = 0;
        self.initialized = true;
    }

    /// Shift the filtered state by `direction` octaves (A7 #1458).
    fn shift_octave(&mut self, direction: i32) {
        if !self.initialized {
            return;
        }
        self.state += direction as f64;
    }

    /// Update with a raw BPM measurement and confidence. Returns filtered BPM.
    /// `locked` engages the innovation gate — see the update step below.
    /// `anchored` (Q2b) hardens the divergence path: the hard reset needs ~3 s
    /// of sustained deviation instead of ~1 s, and process noise stays low so
    /// transition mush cannot glide the state off the anchor — an ACF window
    /// flushing between metrical levels produces exactly such mush, and it
    /// must not read as "a genuinely different song".
    fn update(&mut self, raw_bpm: f64, confidence: f64, locked: bool, anchored: bool) -> f64 {
        if raw_bpm <= 0.0 {
            return if self.initialized {
                2.0f64.powf(self.state)
            } else {
                0.0
            };
        }

        if !self.initialized {
            self.state = raw_bpm.log2();
            self.variance = 1.0;
            self.initialized = true;
            return raw_bpm;
        }

        let current_bpm = 2.0f64.powf(self.state);

        // Q2: no octave-snap preprocessing here anymore. Octave decisions live in
        // `compute_tempo`'s candidate scoring (continuity weight), where they are
        // made on graded evidence — the old snap-then-count-to-30 loop reset on any
        // single non-snapping frame, which is how tracks got permanently stuck on
        // the wrong octave.
        let measurement = raw_bpm.log2();

        // Divergence detection: sustained large deviation -> hard reset. The count
        // leaks instead of zeroing so one in-range frame cannot indefinitely
        // postpone a genuine tempo change.
        let bpm_deviation = (raw_bpm - current_bpm).abs() / current_bpm.max(1.0);
        if bpm_deviation > 0.10 {
            self.diverge_count += 1;
        } else {
            self.diverge_count = self.diverge_count.saturating_sub(1);
        }

        let reset_limit = if anchored { 45 } else { 15 };
        if self.diverge_count >= reset_limit {
            log::debug!(
                "Kalman hard reset: {:.1} -> {:.1} BPM (diverged for {} frames)",
                current_bpm,
                raw_bpm,
                self.diverge_count
            );
            self.state = raw_bpm.log2();
            self.variance = 1.0;
            self.diverge_count = 0;
            return raw_bpm;
        }

        // Adaptive noise: R = f(confidence), Q = f(stability)
        self.r = 0.01 + (1.0 - confidence) * 0.5;
        // Escalate process noise only once divergence is SUSTAINED — a single
        // divergent frame with q = 0.1 would let 2-3 noisy frames drag the
        // state most of the way to a false octave. While anchored, never: the
        // hard reset (above) is the only unrelated-change path, so the state
        // cannot be dragged off the anchor by mush.
        self.q = if self.diverge_count >= 5 && !anchored {
            0.1
        } else {
            0.001
        };

        // Kalman predict (constant model: state unchanged)
        self.variance += self.q;

        // Kalman update
        let innovation = measurement - self.state;
        // Q3 lock dynamics: once the estimator is locked, a strongly-disagreeing
        // measurement is far more likely a transient (breakdown, fill, half-bar
        // of percussion dropout) than a tempo change, so its gain is cut instead
        // of letting a ~2 s wobble drag the state out of the ±4% band — measured
        // as up to 10 excursions per GiantSteps track, each resetting the
        // trailing-lock clock. A GENUINE change still escapes: the divergence
        // counter above accumulates on the raw deviation regardless of gain and
        // hard-resets after ~1 s of sustained disagreement.
        let r = if locked && innovation.abs() > LOCK_INNOVATION_GATE_LOG2 {
            self.r * LOCK_INNOVATION_R_MULT
        } else {
            self.r
        };
        let s = self.variance + r;
        let k = self.variance / s;
        self.state += k * innovation;
        self.variance *= 1.0 - k;

        2.0f64.powf(self.state)
    }
}

// ---------------------------------------------------------------------------
// Stage 2: FFT-based tempo estimation with Kalman tracking
// ---------------------------------------------------------------------------

/// Q2 seed gate: the first measurement the Kalman adopts defines the octave the
/// whole track then argues against, so it must be earned — either one confident
/// reading, or two readings that agree within 5% across at least
/// [`SEED_AGREE_SPAN_SECS`] of NEW audio. The span is load-bearing: consecutive
/// tempo updates are 70 ms apart on a multi-second ACF window, so "two
/// consecutive agree" is satisfied by any persistent noise artifact (measured:
/// white noise holds conf ≈ 0.26 and self-agrees indefinitely).
const SEED_MIN_CONFIDENCE: f64 = 0.35;
const SEED_AGREE_MIN_CONFIDENCE: f64 = 0.2;
const SEED_AGREE_SPAN_SECS: f64 = 2.0;
/// Continuity bonus for candidates near the tracked tempo (log2 σ ≈ 4%): enough
/// hysteresis that near-tied octave scores don't flap frame to frame, weak
/// enough that a genuinely better octave still wins on evidence.
const CONTINUITY_BONUS: f64 = 0.25;
const CONTINUITY_SIGMA_LOG2: f64 = 0.06;
/// Q3 lock dynamics: an EMA of winner-agrees-with-tracked-tempo (within 4%,
/// confidence-scaled) with hysteresis. One noisy update moves the EMA a few
/// percent — unlike the old consecutive counters, which any single frame reset.
const SUPPORT_TAU_SECS: f64 = 2.5;
const LOCK_ENTER_SUPPORT: f64 = 0.6;
const LOCK_EXIT_SUPPORT: f64 = 0.35;
/// When locked, innovations beyond ~4% in log2 pay this measurement-noise
/// multiplier — brief wobbles stop dragging the published BPM out of the
/// tolerance band, while the divergence hard reset still follows real changes.
const LOCK_INNOVATION_GATE_LOG2: f64 = 0.057;
const LOCK_INNOVATION_R_MULT: f64 = 6.0;
/// Q2b metrical anchor: a lock held this long earns an anchor level. Once
/// anchored, ACF winners at a metrically-related level (±octave, 3:2, 4:3 and
/// inverses) are FOLDED onto the anchor level before the filter — they carry
/// fine tempo information about the same grid — instead of feeding the
/// divergence hard-reset, which is how double-kick-over-four-on-the-floor
/// material teleported the filter between levels (live smoke 2026-08-08).
/// Displacing the anchor takes sustained confident evidence at ONE related
/// level — a challenge EMA above [`DISPLACE_CHALLENGE`] for
/// [`DISPLACE_SPAN_SECS`] — and is possible ONLY during the post-earn
/// probation ([`ANCHOR_PROBATION_SECS`]): once a level survives probation it
/// is settled for the track's lifetime (owner-ruled, live run 3 — sustained
/// half-level evidence mid-track is a half-time FEEL, not a tempo change).
/// Unrelated tempos (a genuinely different song) skip all of this and keep
/// the fast divergence path; tap and the octave override are the operator's
/// escape for a set that truly changes level.
const ANCHOR_EARN_SECS: f64 = 8.0;
const RELATED_TOL_LOG2: f64 = 0.08;
/// log2 of {2, ½, 3/2, ⅔, 4/3, ¾}.
const RELATED_RATIOS_LOG2: [f64; 6] = [
    1.0,
    -1.0,
    0.584_962_500_721_156,
    -0.584_962_500_721_156,
    0.415_037_499_278_844,
    -0.415_037_499_278_844,
];
const CHALLENGE_TAU_SECS: f64 = 3.0;
const DISPLACE_CHALLENGE: f64 = 0.7;
const DISPLACE_SPAN_SECS: f64 = 10.0;
/// Slow anchor re-center toward the filtered tempo while locked, so gradual
/// drift/automation tracks without ever letting a level flip masquerade as it.
const ANCHOR_RECENTER_TAU_SECS: f64 = 10.0;
/// For this long after an anchor is EARNED, non-octave related levels may
/// still displace it: an intro can lock ≥8 s at a wrong ¾/⅔ level (measured:
/// dev-subset immabewolf, two GiantSteps tracks), and the sustained true-level
/// evidence that follows must be able to correct the mistake. After probation,
/// displacement hardens to octave-only. Tap anchors skip probation — the user
/// is authoritative.
const ANCHOR_PROBATION_SECS: f64 = 30.0;
/// A filter state this far from the anchor (log2) means the filter has escaped
/// the fold regime — related winners fold to within [`RELATED_TOL_LOG2`] of the
/// anchor and displacement moves the anchor itself, so only the unrelated
/// divergence path (a genuine new tempo, e.g. 128 → 148, log2 0.209) gets here.
/// The anchor is stale at that point and re-earns at the new tempo.
const ANCHOR_ABANDON_LOG2: f64 = 0.12;
/// An abandoned anchor lingers as a GHOST this long. If the replacement anchor
/// earns at a non-octave relative (⅔, ¾ …) of the ghost, the earn snaps to the
/// ghost's level instead: a level war's mush can fire the unrelated reset,
/// abandon the anchor, and let the wrong level lock long enough to re-earn —
/// measured on the 2026-08-08 re-listen, where the orchestral section did
/// exactly this (172 → mush → abandon → ⅔ re-earn, then held wrong by
/// design). Non-octave anchors are unrecoverable post-probation, so a wrong
/// one must never be minted off a fresh abandon. ALL related re-earns fold —
/// octave included: if the material genuinely half-timed, the octave
/// displacement path re-displaces from the restored level within ~10 s, so
/// folding costs nothing; not folding lets a mixed-window artifact level
/// (measured: a 60.09 BPM earn off the mush/⅔ boundary) become the anchor.
/// A ghost-restored anchor carries no probation — its level was already
/// probated in its first life and is settled. Unrelated re-earns are a
/// genuine new song and keep their level.
const GHOST_ANCHOR_TTL_SECS: f64 = 60.0;
/// The ghost comparison asks a coarse question — "same metrical family?" —
/// so its band is wider than the fold tolerance (the measured artifact earn
/// sat 0.006 log2 outside RELATED_TOL_LOG2's ±0.08).
const GHOST_RELATED_TOL_LOG2: f64 = 0.12;
/// The ghost veto only applies when the dead anchor had at least this much
/// tenure: an anchor that survived a minute of music is a veteran whose level
/// war deserves to be lost by the challenger, but a first anchor that died
/// young (GiantSteps 30 s clips: wrong intro locks, ~10 s tenure) is exactly
/// the mistake the re-earn is correcting — measured both ways on the dev
/// subset (ghost-without-tenure reverted 4191591's fix).
const GHOST_MIN_TENURE_SECS: f64 = 30.0;

/// Linearly interpolated ACF value at a continuous lag. `x` must be within
/// `[0, len-1]`; callers guarantee it via the float lag bounds.
fn acf_at(acf: &[f64], x: f64) -> f64 {
    let i = x.floor() as usize;
    if i + 1 >= acf.len() {
        return acf[acf.len() - 1];
    }
    let frac = x - i as f64;
    acf[i] * (1.0 - frac) + acf[i + 1] * frac
}

/// Comb evidence for a candidate period: the 1/h-weighted MEAN of the ACF at
/// h·lag for h = 1..4, over the harmonics that fit inside the ACF. A mean, not
/// a sum: with a sum, a candidate whose harmonics ran past the end of the ACF
/// scored fewer terms than its double-tempo rival — at the 2 s seed a 120 BPM
/// candidate summed 4 terms while 60 BPM got 2, which is exactly when the old
/// ungated seed was taken.
fn comb_score(acf: &[f64], lag: f64) -> f64 {
    let acr_max = (acf.len() - 1) as f64;
    let mut num = 0.0;
    let mut wsum = 0.0;
    for h in 1..=4u32 {
        let x = lag * f64::from(h);
        if x <= acr_max {
            let w = 1.0 / f64::from(h);
            num += w * acf_at(acf, x);
            wsum += w;
        }
    }
    if wsum > 0.0 { num / wsum } else { 0.0 }
}

struct TempoEstimator {
    bpm_range: (f32, f32),

    onset_history: CircularBuffer,
    frame_rate: f64,
    frame_time: f64,
    frame_count: u32,

    current_bpm: f64,
    current_confidence: f64,
    current_period_frames: f64,

    // FFT-based generalized autocorrelation
    fft_forward: Arc<dyn rustfft::Fft<f64>>,
    fft_inverse: Arc<dyn rustfft::Fft<f64>>,
    fft_size: usize,

    // Genre-aware tempo prior (log-Gaussian)
    prior_center_log2: f64,
    prior_sigma: f64,
    /// A7 (#1458): when set, `prior_center_log2` walks toward the locked tempo.
    auto_prior: bool,
    /// A7 (#1458): user octave override, in octaves. Applied to every raw measurement
    /// before the Kalman rather than to the filter state alone — a one-shot state nudge
    /// would be undone within ~2s by the snap-escape counter, since the autocorrelation
    /// keeps reporting the octave the user just rejected.
    octave_offset: i32,

    // Kalman filter replaces EMA + stability tracking
    kalman: KalmanBpm,
    /// Q2: last unseeded measurement that cleared [`SEED_AGREE_MIN_CONFIDENCE`]
    /// with the frame it was taken at, waiting for a confirming reading that
    /// agrees within 5% at least [`SEED_AGREE_SPAN_SECS`] later.
    pending_seed: Option<(f64, u32)>,
    /// Q3: EMA of "this update's winner agrees with the tracked tempo".
    support: f64,
    /// Q3: hysteretic lock state derived from `support`.
    locked: bool,
    /// Q2b: log2 of the earned tempo level. See [`ANCHOR_EARN_SECS`].
    anchor_log2: Option<f64>,
    /// Q2b: hop stamp of the anchor earn — non-octave displacement is allowed
    /// within [`ANCHOR_PROBATION_SECS`] of it. `None` = no probation (tap).
    anchor_earned_at: Option<u32>,
    /// Q2b: (level, hop stamp) of the last abandoned anchor, kept only when
    /// it had [`GHOST_MIN_TENURE_SECS`]. See [`GHOST_ANCHOR_TTL_SECS`].
    ghost_anchor: Option<(f64, u32)>,
    /// Q2b: hop stamp of the current anchor's original earn (displacement
    /// keeps the lineage) — the ghost veto's tenure clock.
    anchor_born_at: Option<u32>,
    /// Q2b: hop stamp of the false→true lock transition (anchor-earn clock).
    locked_since: Option<u32>,
    /// Q2b: EMA of confident winners at the one related level in
    /// `challenge_ratio`; switching levels restarts it.
    challenge: f64,
    /// Q2b: log2 ratio (vs anchor) of the level currently challenging.
    challenge_ratio: f64,
    /// Q2b: hop stamp when `challenge` first cleared [`DISPLACE_CHALLENGE`].
    challenge_above_since: Option<u32>,
    /// Q2b (F3): last tempo published while locked — held on the wire through
    /// unlocks so the readout never blanks or glides mid-set.
    last_locked_bpm: f64,
}

impl TempoEstimator {
    fn new(history_seconds: f64, frame_rate: f64, config: TempoConfig) -> Self {
        let history_size = (history_seconds * frame_rate).ceil() as usize;
        let frame_time = 1.0 / frame_rate;

        let fft_size = (2 * history_size).next_power_of_two();
        let mut planner = FftPlanner::<f64>::new();
        let fft_forward = planner.plan_fft_forward(fft_size);
        let fft_inverse = planner.plan_fft_inverse(fft_size);

        Self {
            bpm_range: (BPM_MIN as f32, BPM_MAX as f32),
            onset_history: CircularBuffer::new(history_size),
            frame_rate,
            frame_time,
            frame_count: 0,
            current_bpm: 0.0,
            current_confidence: 0.0,
            current_period_frames: 0.0,
            fft_forward,
            fft_inverse,
            fft_size,
            prior_center_log2: prior_center_log2(config.prior_center_bpm),
            prior_sigma: (config.prior_sigma as f64).clamp(MIN_PRIOR_SIGMA, MAX_PRIOR_SIGMA),
            auto_prior: config.auto_prior,
            octave_offset: 0,
            kalman: KalmanBpm::new(),
            pending_seed: None,
            support: 0.0,
            locked: false,
            anchor_log2: None,
            anchor_earned_at: None,
            ghost_anchor: None,
            anchor_born_at: None,
            locked_since: None,
            challenge: 0.0,
            challenge_ratio: 0.0,
            challenge_above_since: None,
            last_locked_bpm: 0.0,
        }
    }

    /// Apply live config from the shared [`TempoControl`] (A7 #1458). In auto mode the
    /// estimator owns `prior_center_bpm`, so the incoming centre is ignored — the audio
    /// thread publishes ours back instead (see `prior_center_bpm`).
    fn set_config(&mut self, config: TempoConfig) {
        self.auto_prior = config.auto_prior;
        // Both values are clamped rather than trusted: the UI can only produce sane ones, but
        // `settings.json` is hand-editable, and a centre of 0 would make every candidate weight
        // exp(-inf) = 0 — the prior would silently stop discriminating instead of failing loudly.
        self.prior_sigma = (config.prior_sigma as f64).clamp(MIN_PRIOR_SIGMA, MAX_PRIOR_SIGMA);
        if !config.auto_prior {
            self.prior_center_log2 = prior_center_log2(config.prior_center_bpm);
        }
    }

    /// Current prior centre in BPM — what auto mode has adapted to.
    fn prior_center_bpm(&self) -> f32 {
        2.0f64.powf(self.prior_center_log2) as f32
    }

    /// Q2b (F3): tempo for the wire — the filtered value while locked, held at
    /// the last locked value through unlocks (breakdowns, re-acquisition
    /// wobble), and 0.0 only before the first lock is ever earned. The readout
    /// never blanks or glides mid-set.
    fn published_bpm(&self) -> f64 {
        if self.locked {
            self.current_bpm
        } else {
            self.last_locked_bpm
        }
    }

    /// A7 (#1458): force the reported tempo up/down an octave. Shifts both the offset
    /// (so it sticks) and the filter state (so the readout moves now, not after the
    /// filter reconverges). Rejected when the result would leave the BPM range.
    fn shift_octave(&mut self, direction: i32) {
        let current = self.current_bpm;
        if current > 0.0 {
            let shifted = current * 2.0f64.powi(direction);
            if shifted < self.bpm_range.0 as f64 || shifted > self.bpm_range.1 as f64 {
                log::debug!("Octave shift rejected: {shifted:.1} BPM out of range");
                return;
            }
        }
        self.octave_offset += direction;
        self.kalman.shift_octave(direction);
        if current > 0.0 {
            self.current_bpm = current * 2.0f64.powi(direction);
            self.current_period_frames = 60.0 / (self.current_bpm * self.frame_time);
        }
        // Q2b: the anchor and the held wire value ride the shift — the user just
        // told us which level is correct.
        if let Some(a) = &mut self.anchor_log2 {
            *a += f64::from(direction);
        }
        if self.last_locked_bpm > 0.0 {
            self.last_locked_bpm *= 2.0f64.powi(direction);
        }
        log::info!(
            "Tempo octave shift {:+} -> offset {}",
            direction,
            self.octave_offset
        );
    }

    /// A7 (#1458): lock onto a tapped tempo. Also re-aims `octave_offset` when the tap is
    /// a clean octave off the raw reading, so the estimator keeps agreeing with the tap
    /// instead of drifting back to the octave the user just corrected.
    fn tap(&mut self, bpm: f64) {
        if bpm < self.bpm_range.0 as f64 || bpm > self.bpm_range.1 as f64 {
            log::debug!("Tap tempo rejected: {bpm:.1} BPM out of range");
            return;
        }
        // What the detector reads before any offset — the octave the autocorrelation
        // will keep insisting on.
        let raw = self.current_bpm * 2.0f64.powi(-self.octave_offset);
        if raw > 0.0 {
            let octaves = (bpm / raw).log2().round();
            if octaves.abs() <= 3.0 && (bpm / (raw * 2.0f64.powf(octaves)) - 1.0).abs() < 0.06 {
                self.octave_offset = octaves as i32;
            }
        }
        self.kalman.force(bpm);
        self.current_bpm = bpm;
        self.current_confidence = 1.0;
        self.support = 1.0;
        self.locked = true;
        // Q2b: a tap is the strongest possible level evidence — anchor there now.
        self.anchor_log2 = Some(bpm.log2());
        self.anchor_earned_at = None;
        self.anchor_born_at = Some(self.frame_count);
        self.locked_since = Some(self.frame_count);
        self.last_locked_bpm = bpm;
        self.challenge = 0.0;
        self.challenge_above_since = None;
        self.current_period_frames = 60.0 / (bpm * self.frame_time);
        log::info!(
            "Tap tempo: {:.1} BPM (octave offset {})",
            bpm,
            self.octave_offset
        );
    }

    /// Update tempo estimate. Returns (bpm, confidence, period_seconds).
    ///
    /// A5 (#1456): the analysis hop is fixed (`ANALYSIS_HOP` samples), so `frame_rate` and
    /// `frame_time` are exact from construction — the old runtime re-estimation of frame
    /// timing from wall-clock `timestamp` deltas (a workaround for the jittery variable
    /// hop) is gone, and with it this stage's dependence on `timestamp`.
    fn update(&mut self, onset_value: f64) -> (f64, f64, f64) {
        self.onset_history.push(onset_value);
        self.frame_count += 1;

        // Need enough history (at least 2s)
        let min_frames = (2.0 * self.frame_rate).ceil() as usize;
        if self.onset_history.len() < min_frames {
            return (0.0, 0.0, 0.0);
        }

        // Compute autocorrelation every ~6 frames (~16Hz update rate)
        if !self.frame_count.is_multiple_of(6) {
            let period_s = self.current_period_frames * self.frame_time;
            return (self.current_bpm, self.current_confidence, period_s);
        }

        let (raw_bpm, confidence, _raw_period_frames) = self.compute_tempo();

        // A7 (#1458): honour the user's octave override before filtering.
        let raw_bpm = self.apply_octave_offset(raw_bpm);

        // Q2 seed gate. The old code adopted the very first post-2s measurement
        // with NO confidence requirement (the low-confidence hold below only
        // applied once current_bpm > 0), then defended that garbage seed
        // indefinitely. Until a seed is earned, keep honestly reporting silence —
        // the scheduler stays in its no-tempo path rather than free-running on
        // a fiction. Post-seed, every measurement flows to the filter: the
        // adaptive R already shrinks the gain to near-zero at low confidence,
        // which is what the deleted `< 0.15` early-return was redundantly (and
        // harmfully — it also froze the published BPM forever) doing.
        if !self.kalman.initialized {
            let span = (SEED_AGREE_SPAN_SECS * self.frame_rate) as u32;
            let confirmed = raw_bpm > 0.0
                && self.pending_seed.is_some_and(|(p, f0)| {
                    (raw_bpm / p - 1.0).abs() < 0.05 && self.frame_count.wrapping_sub(f0) >= span
                });
            let earned = raw_bpm > 0.0
                && (confidence >= SEED_MIN_CONFIDENCE
                    || (confidence >= SEED_AGREE_MIN_CONFIDENCE && confirmed));
            if !earned {
                if raw_bpm > 0.0 && confidence >= SEED_AGREE_MIN_CONFIDENCE {
                    match self.pending_seed {
                        // An agreeing reading keeps the ORIGINAL stamp — the span
                        // measures how long the hypothesis has held, not how
                        // recently it was repeated.
                        Some((p, _)) if (raw_bpm / p - 1.0).abs() < 0.05 => {}
                        _ => self.pending_seed = Some((raw_bpm, self.frame_count)),
                    }
                }
                self.current_confidence = confidence;
                return (0.0, self.current_confidence, 0.0);
            }
            self.pending_seed = None;
        }

        // Q2b metrical anchor: classify the winner against the anchor level.
        // Related-level winners (±octave, 3:2, 4:3) are folded onto the anchor
        // level — a ⅔-level reading is fine tempo information about the same
        // grid — so they refine the filter instead of feeding its divergence
        // reset. Only ONE related level accumulates displacement evidence at a
        // time; switching levels restarts the clock.
        let mut measurement_bpm = raw_bpm;
        if let Some(anchor) = self.anchor_log2 {
            let alpha = 1.0 - (-(6.0 * self.frame_time) / CHALLENGE_TAU_SECS).exp();
            // No-evidence frames (silence, weak windows) HOLD the challenge:
            // they argue neither for the challenger nor for the anchor.
            // Confident evidence moves it — toward 1 at the challenging level,
            // toward 0 at the anchor level or an unrelated tempo.
            let confident = raw_bpm > 0.0 && confidence >= SEED_AGREE_MIN_CONFIDENCE;
            if confident {
                let d = raw_bpm.log2() - anchor;
                if d.abs() <= RELATED_TOL_LOG2 {
                    self.challenge += alpha * (0.0 - self.challenge);
                } else if let Some(r) = RELATED_RATIOS_LOG2
                    .iter()
                    .copied()
                    .find(|r| (d - r).abs() <= RELATED_TOL_LOG2)
                {
                    measurement_bpm = 2.0f64.powf(raw_bpm.log2() - r);
                    // Displacement evidence accumulates ONLY during probation.
                    // Post-probation the level is settled for the track's
                    // lifetime — owner-ruled after live run 3 (2026-08-08):
                    // the mid-section's sustained half-level evidence is a
                    // half-time FEEL, not a tempo change, and following it
                    // (172→86→172→86 ping-pong) is wrong. Related winners
                    // still fold, so a half-feel section keeps supporting the
                    // real grid; a genuinely different song escapes via the
                    // unrelated divergence path; tap / octave override remain
                    // the operator's escape for a set that truly half-times.
                    let in_probation = self.anchor_earned_at.is_some_and(|f| {
                        f64::from(self.frame_count.wrapping_sub(f)) * self.frame_time
                            < ANCHOR_PROBATION_SECS
                    });
                    // Octave never displaces, probation included (owner-ruled
                    // after live run 4's logged chain: the intro's ½-level
                    // evidence displaced a CORRECT young anchor at +26 s and
                    // the wrong lineage then out-tenured the truth). Probation
                    // displacement exists to fix the unrecoverable class —
                    // wrong NON-octave anchors; an octave-wrong young anchor
                    // is benign (beats nest) and the override corrects it.
                    if in_probation && r.abs() != 1.0 {
                        if r != self.challenge_ratio {
                            self.challenge = 0.0;
                            self.challenge_ratio = r;
                            self.challenge_above_since = None;
                        }
                        self.challenge += alpha * (1.0 - self.challenge);
                    }
                } else {
                    // Unrelated tempo: leave the raw measurement for the
                    // divergence path — a genuinely different song follows fast.
                    self.challenge += alpha * (0.0 - self.challenge);
                }
            } else if raw_bpm > 0.0 {
                // Low-confidence winners still fold (the filter should not see
                // level jumps) but don't move the challenge.
                let d = raw_bpm.log2() - anchor;
                if let Some(r) = RELATED_RATIOS_LOG2
                    .iter()
                    .copied()
                    .find(|r| (d - r).abs() <= RELATED_TOL_LOG2)
                {
                    measurement_bpm = 2.0f64.powf(raw_bpm.log2() - r);
                }
            }
        }

        // Kalman filter update. The innovation gate holds while locked OR
        // anchored — losing the support lock must not free the filter to glide
        // (that glide was the live-smoke flap mechanism).
        let anchored = self.anchor_log2.is_some();
        let mut filtered_bpm = self.kalman.update(
            measurement_bpm,
            confidence,
            self.locked || anchored,
            anchored,
        );

        // Q2b displacement: one related level with sustained confident evidence
        // takes the anchor with it — a snap, never a glide through the gap.
        if self.anchor_log2.is_some() {
            if self.challenge > DISPLACE_CHALLENGE {
                let since = *self.challenge_above_since.get_or_insert(self.frame_count);
                if f64::from(self.frame_count.wrapping_sub(since)) * self.frame_time
                    >= DISPLACE_SPAN_SECS
                    && filtered_bpm > 0.0
                {
                    // Displace from the ANCHOR, not the filter state — the
                    // anchor is the slow consensus; the filter can be dragged
                    // a few percent by transition mush, and the challenger's
                    // claim is "the true level is anchor × 2^r".
                    let new_bpm = 2.0f64.powf(
                        self.anchor_log2.unwrap_or(filtered_bpm.log2()) + self.challenge_ratio,
                    );
                    log::info!(
                        "Tempo anchor displaced: {:.1} -> {:.1} BPM after sustained challenge",
                        filtered_bpm,
                        new_bpm
                    );
                    self.kalman.force(new_bpm);
                    self.anchor_log2 = Some(new_bpm.log2());
                    self.challenge = 0.0;
                    self.challenge_above_since = None;
                    filtered_bpm = new_bpm;
                }
            } else {
                self.challenge_above_since = None;
            }
            // The unrelated divergence path teleported the filter: a genuine
            // new tempo. The anchor is stale — drop it and re-earn.
            if filtered_bpm > 0.0
                && self
                    .anchor_log2
                    .is_some_and(|a| (filtered_bpm.log2() - a).abs() > ANCHOR_ABANDON_LOG2)
            {
                log::info!("Tempo anchor abandoned at {filtered_bpm:.1} BPM (unrelated change)");
                let tenure = self.anchor_born_at.map_or(0.0, |b| {
                    f64::from(self.frame_count.wrapping_sub(b)) * self.frame_time
                });
                self.ghost_anchor = if tenure >= GHOST_MIN_TENURE_SECS {
                    self.anchor_log2.map(|a| (a, self.frame_count))
                } else {
                    None
                };
                self.anchor_log2 = None;
                self.anchor_earned_at = None;
                self.anchor_born_at = None;
                self.locked_since = None;
                self.challenge = 0.0;
                self.challenge_above_since = None;
            }
        }

        // Q3 lock dynamics: support = EMA of "the winner agrees with what we're
        // tracking", scaled by confidence, at the ~14.4 Hz tempo cadence.
        // Q2b: agreement is judged on the FOLDED measurement — a related-level
        // winner is evidence FOR the anchor grid (its onsets subdivide it), so
        // it must not drain the lock and mute beats mid-groove.
        if filtered_bpm > 0.0 {
            let agrees =
                measurement_bpm > 0.0 && (measurement_bpm / filtered_bpm - 1.0).abs() <= 0.04;
            let alpha = 1.0 - (-(6.0 * self.frame_time) / SUPPORT_TAU_SECS).exp();
            // Binary target with a confidence floor, NOT confidence-scaled: a
            // clean signal's absolute confidence sits near 0.5, so scaling
            // would cap support below any usable lock threshold. Support reads
            // "fraction of recent updates that were confident-enough
            // agreement".
            let target = if agrees && confidence >= 0.2 {
                1.0
            } else {
                0.0
            };
            self.support += alpha * (target - self.support);
            if self.locked {
                if self.support < LOCK_EXIT_SUPPORT {
                    self.locked = false;
                }
            } else if self.support > LOCK_ENTER_SUPPORT {
                self.locked = true;
            }
        }

        // Q2b: earn the anchor after a sustained lock; while locked at the
        // anchor level, re-center it slowly so gradual drift/automation tracks
        // without a level flip ever passing as drift.
        if self.locked {
            let since = *self.locked_since.get_or_insert(self.frame_count);
            if self.anchor_log2.is_none() {
                if filtered_bpm > 0.0
                    && f64::from(self.frame_count.wrapping_sub(since)) * self.frame_time
                        >= ANCHOR_EARN_SECS
                {
                    let mut level = filtered_bpm.log2();
                    let mut folded_to_ghost = false;
                    // A fresh ghost vetoes a non-octave-related re-earn — the
                    // level war that killed the old anchor must not mint an
                    // unrecoverable wrong one (see GHOST_ANCHOR_TTL_SECS).
                    if let Some((ghost, at)) = self.ghost_anchor {
                        let fresh = f64::from(self.frame_count.wrapping_sub(at)) * self.frame_time
                            < GHOST_ANCHOR_TTL_SECS;
                        let rel = RELATED_RATIOS_LOG2
                            .iter()
                            .copied()
                            .find(|r| (level - ghost - r).abs() <= GHOST_RELATED_TOL_LOG2);
                        if fresh && rel.is_some() {
                            log::info!(
                                "Tempo anchor re-earn folded to ghost level: {:.1} -> {:.1} BPM",
                                filtered_bpm,
                                2.0f64.powf(ghost)
                            );
                            level = ghost;
                            folded_to_ghost = true;
                            self.kalman.force(2.0f64.powf(ghost));
                        }
                    }
                    log::info!("Tempo anchor earned: {:.1} BPM", 2.0f64.powf(level));
                    self.anchor_log2 = Some(level);
                    // A ghost-folded earn is a veteran level — no probation,
                    // or the same ⅔ evidence that lost the level war would
                    // immediately displace the anchor it just failed to earn.
                    self.anchor_earned_at = if folded_to_ghost {
                        None
                    } else {
                        Some(self.frame_count)
                    };
                    self.anchor_born_at = Some(self.frame_count);
                    self.ghost_anchor = None;
                }
            } else if let Some(a) = &mut self.anchor_log2 {
                if filtered_bpm > 0.0 && (filtered_bpm.log2() - *a).abs() <= RELATED_TOL_LOG2 {
                    let alpha = 1.0 - (-(6.0 * self.frame_time) / ANCHOR_RECENTER_TAU_SECS).exp();
                    *a += alpha * (filtered_bpm.log2() - *a);
                }
            }
            if filtered_bpm > 0.0 {
                self.last_locked_bpm = filtered_bpm;
            }
        } else {
            self.locked_since = None;
        }

        // A7 (#1458): auto prior — walk the centre toward the tempo we're locking onto, so
        // the prior stops fighting a track whose real tempo sits far from it. Gated on high
        // confidence: the prior steers octave selection and this steers the prior back, so a
        // confident lock is what keeps that loop from cementing a wrong octave. The slow rate
        // (~70s time constant at this update cadence) and the clamp bound the damage if it does.
        if self.auto_prior && confidence >= AUTO_PRIOR_MIN_CONFIDENCE && filtered_bpm > 0.0 {
            let target = filtered_bpm.log2();
            self.prior_center_log2 += AUTO_PRIOR_RATE * (target - self.prior_center_log2);
            self.prior_center_log2 = self
                .prior_center_log2
                .clamp(AUTO_PRIOR_MIN_BPM.log2(), AUTO_PRIOR_MAX_BPM.log2());
        }

        self.current_bpm = filtered_bpm;
        self.current_confidence = confidence;
        self.current_period_frames = if filtered_bpm > 0.0 {
            60.0 / (filtered_bpm * self.frame_time)
        } else {
            0.0
        };

        let period_s = self.current_period_frames * self.frame_time;
        (self.current_bpm, self.current_confidence, period_s)
    }

    fn compute_tempo(&mut self) -> (f64, f64, f64) {
        let history = self.onset_history.values();
        let n = history.len();

        // Convert BPM range to lag range (frames)
        let max_lag = ((60.0 / (self.bpm_range.0 as f64 * self.frame_time)) as usize).min(n / 2);
        let min_lag = ((60.0 / (self.bpm_range.1 as f64 * self.frame_time)) as usize).max(1);

        if max_lag <= min_lag {
            return (0.0, 0.0, 0.0);
        }

        // FFT-based autocorrelation via Wiener-Khinchin: zero-pad -> FFT -> |X|^2 -> IFFT
        // Using power spectrum (exponent 2) instead of amplitude (exponent 1) because
        // the power spectrum gives the fundamental period a clear height advantage over
        // subharmonics, reducing octave ambiguity.
        // Mean-subtract to remove DC offset — critical for autocorrelation contrast.
        let mean = history.iter().sum::<f64>() / n as f64;
        let mut buffer: Vec<Complex<f64>> = vec![Complex::new(0.0, 0.0); self.fft_size];
        for (i, &v) in history.iter().enumerate() {
            buffer[i] = Complex::new(v - mean, 0.0);
        }

        self.fft_forward.process(&mut buffer);

        // Power spectrum |X|^2 — standard autocorrelation (Wiener-Khinchin)
        for c in &mut buffer {
            let power = c.norm_sqr();
            *c = Complex::new(power, 0.0);
        }

        self.fft_inverse.process(&mut buffer);

        // Normalize by fft_size (rustfft doesn't normalize) and by zero-lag
        let scale = 1.0 / self.fft_size as f64;
        let zero_lag = buffer[0].re * scale;
        if zero_lag <= 0.0 {
            return (0.0, 0.0, 0.0);
        }

        // Extract autocorrelation up to 4*max_lag for harmonic scoring.
        //
        // Q2: unbiased estimate. The zero-padded ACF sums (n − lag) products, so
        // long lags were attenuated by exactly (n − lag)/n — a permanent
        // structural bias toward double-time (~+16% for the 2:1 pair at full
        // history). Divide it out; clamp the factor so the noisy far tail (few
        // products per bin) cannot explode.
        let acr_len = (4 * max_lag + 1).min(n).min(self.fft_size);
        let autocorr: Vec<f64> = buffer[..acr_len]
            .iter()
            .enumerate()
            .map(|(lag, c)| (c.re * scale / zero_lag) * (n as f64 / (n - lag) as f64).min(4.0))
            .collect();
        let acr_max = autocorr.len() - 1;

        // Q2: candidates live in CONTINUOUS lag from here on. Adjacent-bin
        // spacing is 1/lag — 2.5-3.4% at EDM tempi against the 4% Acc1
        // tolerance — so integer-rounded candidate lags could not represent the
        // true tempo at all. The parabolic vertex is only valid at a true local
        // max, so refine each ACF peak at its own bin, then project metrical
        // ratios in float.
        let range_end = max_lag.min(acr_max.saturating_sub(1));
        let mut peaks: Vec<(f64, f64)> = Vec::new(); // (refined lag, refined height)
        for lag in min_lag.max(1)..=range_end {
            let (alpha, beta, gamma) = (autocorr[lag - 1], autocorr[lag], autocorr[lag + 1]);
            if beta < alpha || beta < gamma || beta <= 0.0 {
                continue;
            }
            let curvature = alpha - 2.0 * beta + gamma; // ≤ 0 at a local max
            let (rl, rv) = if curvature < -1e-12 {
                let p = (0.5 * (alpha - gamma) / curvature).clamp(-0.5, 0.5);
                (lag as f64 + p, beta - 0.25 * (alpha - gamma) * p)
            } else {
                (lag as f64, beta)
            };
            peaks.push((rl, rv));
        }
        // Strongest few peaks are enough — every metrical level of a real
        // rhythm is a local max, and the ratio projection reaches the rest.
        peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        peaks.truncate(5);
        if peaks.is_empty() {
            return (0.0, 0.0, 0.0);
        }

        // Metrical ratios: 1:3 and 1:4 are needed because a peak can land on the
        // 3rd or 4th subharmonic; without them the projection can only step to
        // 2T, never reaching T directly.
        let ratios: [(f64, f64); 9] = [
            (1.0, 4.0),
            (1.0, 3.0),
            (1.0, 2.0),
            (2.0, 3.0),
            (3.0, 4.0),
            (1.0, 1.0),
            (4.0, 3.0),
            (3.0, 2.0),
            (2.0, 1.0),
        ];
        // Float lag bounds from the BPM range itself — a candidate at, say, lag
        // 17.5 (295 BPM) is legal even though no integer bin represents it. This
        // also removes the old silent (0,0,0) discard when a rounded ratio
        // landed just outside the range.
        let min_lag_f = 60.0 / (self.bpm_range.1 as f64 * self.frame_time);
        let max_lag_f = 60.0 / (self.bpm_range.0 as f64 * self.frame_time);

        let mut best: Option<(f64, f64)> = None; // (lag, weighted score)
        for &(peak_lag, _) in &peaks {
            for &(num, den) in &ratios {
                let cand = peak_lag * num / den;
                if cand < min_lag_f || cand > max_lag_f {
                    continue;
                }
                let bpm = 60.0 / (cand * self.frame_time);
                let weighted = comb_score(&autocorr, cand)
                    * self.tempo_prior_weight(bpm)
                    * self.continuity_weight(bpm);
                if best.is_none_or(|(_, b)| weighted > b) {
                    best = Some((cand, weighted));
                }
            }
        }
        let Some((best_lag_f, _)) = best else {
            return (0.0, 0.0, 0.0);
        };

        let bpm = 60.0 / (best_lag_f * self.frame_time);

        // Confidence: the winner's interpolated height relative to the median
        // floor across the candidate range.
        let confidence = {
            let range_end = max_lag.min(acr_max);
            let mut sorted_vals: Vec<f64> = autocorr[min_lag..=range_end].to_vec();
            sorted_vals
                .sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let noise_floor = sorted_vals[sorted_vals.len() / 2]; // median
            let peak = acf_at(&autocorr, best_lag_f);
            ((peak - noise_floor) / (1.0 - noise_floor).max(1e-6)).clamp(0.0, 1.0)
        };

        // Safety net only — the float bounds above already confine candidates.
        if bpm < self.bpm_range.0 as f64 || bpm > self.bpm_range.1 as f64 {
            return (0.0, 0.0, 0.0);
        }

        log::debug!(
            "Tempo estimate: {:.1} BPM (confidence {:.2}, lag {:.1})",
            bpm,
            confidence,
            best_lag_f
        );

        (bpm, confidence, best_lag_f)
    }

    /// Q2 octave hysteresis (replaces the Kalman's snap counter): a candidate
    /// within a few percent of the tempo already being tracked gets a modest
    /// bonus, so near-tied octave scores don't flap frame to frame, while a
    /// genuinely better octave (> ~25% comb advantage) still wins immediately.
    fn continuity_weight(&self, bpm: f64) -> f64 {
        if self.current_bpm <= 0.0 || bpm <= 0.0 {
            return 1.0;
        }
        let d = bpm.log2() - self.current_bpm.log2();
        1.0 + CONTINUITY_BONUS * (-0.5 * (d / CONTINUITY_SIGMA_LOG2).powi(2)).exp()
    }

    /// Apply the user's octave override to a raw measurement (A7 #1458). Walks the offset
    /// back toward zero if the tempo has since moved somewhere the shifted value can't
    /// legally sit, so an override taken at 90 BPM can't strand a later 200 BPM track
    /// outside the range.
    fn apply_octave_offset(&mut self, raw_bpm: f64) -> f64 {
        if raw_bpm <= 0.0 {
            return raw_bpm;
        }
        while self.octave_offset != 0 {
            let shifted = raw_bpm * 2.0f64.powi(self.octave_offset);
            if shifted >= self.bpm_range.0 as f64 && shifted <= self.bpm_range.1 as f64 {
                return shifted;
            }
            self.octave_offset -= self.octave_offset.signum();
        }
        raw_bpm
    }

    /// Log-Gaussian tempo prior weight centered at prior_center_bpm.
    fn tempo_prior_weight(&self, bpm: f64) -> f64 {
        if bpm <= 0.0 {
            return 0.0;
        }
        let log2_bpm = bpm.log2();
        let diff = log2_bpm - self.prior_center_log2;
        (-0.5 * (diff / self.prior_sigma).powi(2)).exp()
    }
}

// ---------------------------------------------------------------------------
// Stage 3: Beat scheduler — a phase-locked grid oscillator (Q1 rewrite)
// ---------------------------------------------------------------------------
//
// Beats FIRE at predicted grid instants; onsets are evidence that corrects the
// grid's phase, never fire triggers (with one bounded exception while the
// tempo is still unlocked). This inverts the old state machine, whose "a beat
// is an event that happens at the hop where we decide it" assumption produced
// every measured defect at once: firing on every gated onset below the
// confidence gate (n_est/n_ref 1.5-1.7×), a 0.9×period backup free-running
// 11% fast, missed beats fired 80-92 ms late at the hop that noticed them, and
// the grid re-anchored onto whichever late tail onset happened to confirm.
//
// Modes, keyed off the estimator's hysteretic lock (Q3):
// - LOCKED, supported (freerun): fire ON the grid; `beat_time` is the grid
//   instant itself, so n_est/n_ref ≈ 1 and timing error ≈ phase-correction
//   residual by construction. In-window onsets apply a two-gain PI correction.
// - LOCKED, unsupported (muted freerun): after enough unsupported beats the
//   grid keeps advancing silently — a breakdown mutes the strobe instead of
//   spraying it — and the first supported beats resume emission on-grid with
//   no re-acquisition.
// - UNLOCKED with a provisional period (acquisition): fire only where onset
//   and grid agree, at the onset's own time, at most one per 0.6 period.
// - NO TEMPO (bootstrap, or sustained silence): no beats. /onset and the kick
//   stem events keep carrying transient reactivity (user-approved behavior).

/// Onset-to-grid window: ±10% of the period, capped at the bench tolerance.
const PLL_WINDOW_PERIOD_FRAC: f64 = 0.10;
const PLL_WINDOW_MAX_SECS: f64 = 0.07;
/// Bid capture reach on BOTH sides, as a period fraction. Wide, so a
/// desynced grid can still see the true beat (a tight late window measured a
/// self-sustaining −60 ms dead-zone desync on the fixture: real evidence
/// couldn't bid, and 0.005-strength flux ripples kept the grid early).
const PLL_CAPTURE_PERIOD_FRAC: f64 = 0.25;
/// Bid score = √strength × exp(−½(e/σ)²). The root softens loudness wars
/// (a build's crescendo roll is genuinely louder just BEFORE the bar line
/// than on it); the proximity kernel makes far bids need a real strength
/// advantage. σ is in seconds of grid error.
const PLL_BID_PROXIMITY_SIGMA_SECS: f64 = 0.06;
/// Bids below this onset strength are flux ripples, not evidence.
const PLL_MIN_BID_STRENGTH: f32 = 0.05;

/// Two-gain PI correction (Ellis-style): phase pulls the upcoming beat, the
/// trim nudges the grid rate between tempo updates.
const PLL_PHASE_GAIN: f64 = 0.3;
const PLL_TRIM_GAIN: f64 = 0.02;
const PLL_TRIM_CLAMP: f64 = 0.02;
/// Per-beat trim relaxation. The trim exists to track a sustained rate
/// mismatch; without decay, the corrections of a one-off phase convergence
/// wind it to the clamp and the grid then free-runs off-rate faster than the
/// tight late window can recapture (measured: −20 ms/beat runaway).
const PLL_TRIM_DECAY_SUPPORTED: f64 = 0.85;
const PLL_TRIM_DECAY_UNSUPPORTED: f64 = 0.5;
/// Per-beat support EMA and the mute hysteresis it drives.
const PLL_SUPPORT_ALPHA: f64 = 0.25;
const PLL_MUTE_ENTER_SUPPORT: f64 = 0.2;
const PLL_MUTE_EXIT_SUPPORT: f64 = 0.4;
/// Acquisition-mode minimum spacing, in periods.
const PLL_ACQ_MIN_SPACING: f64 = 0.6;
/// Unlocked and nothing fired for this many periods → the anchor is stale;
/// re-anchor on the next onset instead of letting a dead grid veto agreement.
const PLL_ACQ_STALE_PERIODS: f64 = 4.0;

struct BeatScheduler {
    /// Q3: the estimator's hysteretic lock state — selects the firing mode.
    tempo_locked: bool,
    bpm: f64,
    period: f64,
    tempo_confidence: f64,

    /// The next grid instant on the sample clock; 0.0 = no grid anchored.
    next_beat_time: f64,
    /// Multiplicative grid-rate trim from the PI loop, clamped ±[`PLL_TRIM_CLAMP`].
    period_trim: f64,
    /// Time of the most recently fired (or silently passed) grid beat.
    last_beat_time: f64,
    /// Time of the last EMITTED beat (grid or acquisition).
    last_fired_time: f64,
    /// Was any in-window onset seen since the last grid beat?
    onset_support_pending: bool,
    /// Best EARLY-side bid (score, error) for the upcoming grid instant —
    /// held until the fire so a weak far hit cannot pre-empt the actual
    /// beat-carrying hit behind it.
    slot_early_best: Option<(f64, f64)>,
    /// The just-fired slot, still collecting late-side bids until its
    /// deadline: (deadline, best bid so far from either side).
    closing_slot: Option<(f64, Option<(f64, f64)>)>,
    /// EMA of per-beat onset support; drives mute hysteresis.
    beat_support: f64,
    /// Running count of grid instants passed — INCLUDING muted ones — so the
    /// downbeat tracker's bar rotation stays phase-coherent through mutes and
    /// unlocked gaps (counting only fired beats slips the modulo every gap).
    grid_beat_count: u64,
    muted: bool,
    beat_strength: f64,
}

impl BeatScheduler {
    fn new() -> Self {
        Self {
            tempo_locked: false,
            bpm: 0.0,
            period: 0.0,
            tempo_confidence: 0.0,
            next_beat_time: 0.0,
            period_trim: 0.0,
            last_beat_time: 0.0,
            last_fired_time: 0.0,
            onset_support_pending: false,
            slot_early_best: None,
            closing_slot: None,
            beat_support: 0.0,
            grid_beat_count: 0,
            muted: false,
            beat_strength: 0.0,
        }
    }

    fn update_tempo(&mut self, bpm: f64, period: f64, confidence: f64, locked: bool) {
        self.tempo_locked = locked;
        self.bpm = bpm;
        self.period = period;
        self.tempo_confidence = confidence;
    }

    fn grid_period(&self) -> f64 {
        self.period * (1.0 + self.period_trim)
    }

    /// Two-gain PI step: e > 0 means the music runs late vs the grid.
    fn apply_phase_correction(&mut self, e: f64, period: f64) {
        self.next_beat_time += PLL_PHASE_GAIN * e;
        self.period_trim =
            (self.period_trim + PLL_TRIM_GAIN * e / period).clamp(-PLL_TRIM_CLAMP, PLL_TRIM_CLAMP);
    }

    fn window(&self) -> f64 {
        (self.grid_period() * PLL_WINDOW_PERIOD_FRAC).min(PLL_WINDOW_MAX_SECS)
    }

    /// Main beat decision. Returns (is_beat, beat_time, beat_phase, bpm) —
    /// `beat_time` is the beat's true instant, at or before `timestamp`.
    fn process(
        &mut self,
        is_onset: bool,
        onset_strength: f32,
        timestamp: f64,
        is_silence: bool,
    ) -> (bool, f64, f64, f64) {
        // Sustained silence: mute and drop the grid — re-entry re-acquires
        // fresh instead of trusting a phase that free-ran through the gap.
        if is_silence {
            self.next_beat_time = 0.0;
            self.period_trim = 0.0;
            self.beat_support = 0.0;
            self.muted = false;
            self.slot_early_best = None;
            self.closing_slot = None;
            return (false, 0.0, 0.0, self.bpm);
        }

        if self.period <= 0.0 {
            // Bootstrap: no tempo hypothesis yet — no grid, no beats.
            return (false, 0.0, 0.0, self.bpm);
        }
        let period = self.grid_period();

        // No grid anchored yet: anchor on the first gated onset. Locked or
        // not, the first beat needs an event to phase against.
        if self.next_beat_time == 0.0 {
            if is_onset {
                self.beat_strength = f64::from(onset_strength);
                self.last_beat_time = timestamp;
                self.last_fired_time = timestamp;
                self.next_beat_time = timestamp + period;
                self.onset_support_pending = false;
                self.grid_beat_count += 1;
                return (true, timestamp, 0.0, self.bpm);
            }
            return (false, 0.0, 0.0, self.bpm);
        }

        // Onset evidence: measure against the NEAREST grid instant (the
        // upcoming one or the one just passed) and BID to correct that slot.
        // One correction per slot, decided at the slot's deadline by the best
        // bid. Bid score = √strength × proximity — three measured failure
        // modes shaped this: first-captured-wins let roll hits drag the grid
        // fast; strength-alone let a build's crescendo (louder just before
        // the bar line than on it) walk the grid early; and a tight late
        // window created a dead zone where a desynced grid could no longer
        // see the true beat at all. SUPPORT (mute hysteresis) stays tight:
        // only onsets inside the ±window count as "this beat had backing".
        if is_onset && onset_strength >= PLL_MIN_BID_STRENGTH {
            let prev = self.next_beat_time - period;
            let e_next = timestamp - self.next_beat_time;
            let e_prev = timestamp - prev;
            let e = if e_prev.abs() < e_next.abs() {
                e_prev
            } else {
                e_next
            };
            let capture = period * PLL_CAPTURE_PERIOD_FRAC;
            if e.abs() <= capture {
                if e.abs() <= self.window() {
                    self.beat_strength = f64::from(onset_strength);
                    self.onset_support_pending = true;
                }
                let prox = (-0.5 * (e / PLL_BID_PROXIMITY_SIGMA_SECS).powi(2)).exp();
                let score = f64::from(onset_strength).sqrt() * prox;
                if e == e_next && e < 0.0 {
                    // Early side of the upcoming instant: hold until its fire.
                    if self.slot_early_best.is_none_or(|(b, _)| score > b) {
                        self.slot_early_best = Some((score, e));
                    }
                } else if e == e_prev && e >= 0.0 {
                    // Late side of the just-fired slot: bid into its window.
                    if let Some((_, best)) = &mut self.closing_slot {
                        if best.is_none_or(|(b, _)| score > b) {
                            *best = Some((score, e));
                        }
                    }
                }
            }
        }

        let mut fired = false;
        let mut beat_time = 0.0;

        if self.tempo_locked {
            // Settle an expired decision window: its closest attack corrects
            // the grid now, in time to shape the very next instant.
            if let Some((deadline, best)) = self.closing_slot {
                if timestamp > deadline {
                    if let Some((_, e)) = best {
                        self.apply_phase_correction(e, period);
                    }
                    self.closing_slot = None;
                }
            }
            // Grid instants due this hop fire (or pass silently while muted).
            if self.next_beat_time <= timestamp {
                // Degenerate transitional slot: around a mode flip, a pending
                // negative correction can leave the next grid instant at or
                // before the beat just emitted (measured once in 374 tracks:
                // a −1.6 ms inversion after an acquisition fire). That instant
                // IS the beat already fired — advance past it silently so
                // /beat stays strictly monotonic.
                if self.next_beat_time <= self.last_fired_time + 0.3 * period {
                    self.next_beat_time += period;
                    let phase = ((timestamp - self.last_beat_time) / period).rem_euclid(1.0);
                    return (false, 0.0, phase, self.bpm);
                }
                let supported = self.onset_support_pending;
                self.beat_support +=
                    PLL_SUPPORT_ALPHA * (f64::from(supported as u8) - self.beat_support);
                self.onset_support_pending = false;
                self.period_trim *= if supported {
                    PLL_TRIM_DECAY_SUPPORTED
                } else {
                    PLL_TRIM_DECAY_UNSUPPORTED
                };
                // Open the fired slot's decision window: early-side evidence
                // already collected bids against late-side attacks still to
                // come; the closest wins when the window expires. (A pending
                // unresolved slot is settled first — periods are ≫ windows, so
                // it has necessarily expired by now.)
                if let Some((_, Some((_, e)))) = self.closing_slot.take() {
                    self.apply_phase_correction(e, period);
                }
                self.closing_slot = Some((
                    self.next_beat_time + period * PLL_CAPTURE_PERIOD_FRAC,
                    self.slot_early_best.take(),
                ));
                if self.muted {
                    if self.beat_support > PLL_MUTE_EXIT_SUPPORT {
                        self.muted = false;
                    }
                } else if self.beat_support < PLL_MUTE_ENTER_SUPPORT {
                    self.muted = true;
                }

                beat_time = self.next_beat_time;
                self.last_beat_time = self.next_beat_time;
                self.next_beat_time += period;
                self.grid_beat_count += 1;
                if !supported {
                    self.beat_strength = 0.5;
                }
                if !self.muted {
                    fired = true;
                    self.last_fired_time = beat_time;
                }
            }
        } else {
            // Acquisition: evidence-gated fires only — an in-window onset, far
            // enough from the previous fire. The onset is the better clock
            // while unlocked, so the beat carries the onset's own time.
            if is_onset
                && self.onset_support_pending
                && timestamp - self.last_fired_time >= PLL_ACQ_MIN_SPACING * period
            {
                fired = true;
                beat_time = timestamp;
                self.last_fired_time = timestamp;
                self.last_beat_time = timestamp;
                self.grid_beat_count += 1;
                // Fold the grid onto this fire so phase reads from it.
                while self.next_beat_time <= timestamp {
                    self.next_beat_time += period;
                }
            } else if is_onset
                && !self.onset_support_pending
                && timestamp - self.last_fired_time >= PLL_ACQ_STALE_PERIODS * period
            {
                // Stale anchor: nothing has agreed for several periods —
                // re-anchor on this onset rather than let a dead grid veto
                // every candidate forever.
                self.beat_strength = f64::from(onset_strength);
                self.last_beat_time = timestamp;
                self.last_fired_time = timestamp;
                self.next_beat_time = timestamp + period;
                self.onset_support_pending = false;
                self.grid_beat_count += 1;
                return (true, timestamp, 0.0, self.bpm);
            }
            // Advance the provisional grid past due instants (no emission —
            // unsupported unlocked beats are exactly the old over-firing).
            while self.next_beat_time <= timestamp {
                self.last_beat_time = self.next_beat_time;
                self.next_beat_time += period;
                self.onset_support_pending = false;
                self.grid_beat_count += 1;
            }
            self.onset_support_pending = is_onset && self.onset_support_pending;
        }

        let phase = ((timestamp - self.last_beat_time) / period).rem_euclid(1.0);
        (fired, beat_time, phase, self.bpm)
    }
}

// ---------------------------------------------------------------------------
// Main BeatDetector facade
// ---------------------------------------------------------------------------

/// Result from beat detection for one frame.
pub struct BeatResult {
    pub onset_strength: f32,
    pub beat: f32,
    pub beat_phase: f32,
    pub bpm: f32,
    pub beat_strength: f32,
    /// The beat's event time on the sample clock, seconds. Only meaningful when
    /// `beat > 0.5`. The PLL scheduler fires grid beats at their predicted
    /// instants, so this sits at or (by up to one hop + one correction step)
    /// before the hop timestamp that emits it.
    pub beat_time: f64,
    /// The scheduler's grid index of this beat — counts every grid instant,
    /// including MUTED ones, so bar rotation downstream stays phase-coherent
    /// through breakdown mutes and unlocked gaps. Meaningful when `beat > 0.5`.
    pub beat_index: u64,
}

/// 3-stage beat detection pipeline.
pub struct BeatDetector {
    onset_detector: OnsetDetector,
    tempo_estimator: TempoEstimator,
    beat_scheduler: BeatScheduler,

    // Onset hold+decay
    held_onset: f32,
    onset_decay_tau: f32,
    last_timestamp: f64,

    // Onset cooldown
    onset_cooldown: f64,
    last_onset_time: f64,
}

impl BeatDetector {
    pub fn new(sample_rate: f32, tempo: TempoConfig) -> Self {
        // A5 (#1456): exact frame rate from the fixed analysis hop (sr / ANALYSIS_HOP),
        // e.g. ~86.1 Hz @ 44.1 kHz. Replaces the old hardcoded ~100 Hz approximation that
        // the tempo estimator then had to correct at runtime.
        let frame_rate = sample_rate as f64 / super::ANALYSIS_HOP as f64;

        let history_size = (0.5 * frame_rate) as usize; // ~0.5s
        let long_term_size = (4.0 * frame_rate) as usize; // ~4s

        Self {
            onset_detector: OnsetDetector::new(sample_rate, history_size, long_term_size),
            tempo_estimator: TempoEstimator::new(8.0, frame_rate, tempo),
            beat_scheduler: BeatScheduler::new(),
            held_onset: 0.0,
            onset_decay_tau: 0.20,
            last_timestamp: 0.0,
            onset_cooldown: 0.05,
            last_onset_time: 0.0,
        }
    }

    /// Apply live tempo config (A7 #1458), snapshotted from the shared `TempoControl`.
    pub fn set_tempo_config(&mut self, config: TempoConfig) {
        self.tempo_estimator.set_config(config);
    }

    /// Apply a one-shot tempo override (A7 #1458).
    pub fn apply_tempo_command(&mut self, cmd: TempoCommand) {
        match cmd {
            TempoCommand::ShiftOctave(dir) => self.tempo_estimator.shift_octave(dir),
            TempoCommand::Tap(bpm) => self.tempo_estimator.tap(bpm),
        }
    }

    /// Prior centre in BPM — published back to the shared config in auto mode (A7 #1458).
    pub fn prior_center_bpm(&self) -> f32 {
        self.tempo_estimator.prior_center_bpm()
    }

    /// Process one frame of audio data.
    ///
    /// Arguments:
    /// - bass_spectrum: magnitude spectrum from 4096-pt FFT (num_bins)
    /// - mid_spectrum: magnitude spectrum from 1024-pt FFT
    /// - high_spectrum: magnitude spectrum from 512-pt FFT
    /// - timestamp: current time in seconds
    /// - loud_silent: perceptual silence gate from the A10 loudness meter (#1457). Gates
    ///   the phase freeze; took over from an `rms < 1e-4` test that misfired on loud audio
    ///   (see the freeze site below).
    pub fn process(
        &mut self,
        bass_spectrum: &[f32],
        mid_spectrum: &[f32],
        high_spectrum: &[f32],
        timestamp: f64,
        loud_silent: bool,
    ) -> BeatResult {
        let dt = if self.last_timestamp > 0.0 {
            (timestamp - self.last_timestamp).max(0.0)
        } else {
            0.0
        };
        self.last_timestamp = timestamp;

        // Stage 1: Onset detection
        let (is_onset, onset_strength, combined_flux) =
            self.onset_detector
                .process(bass_spectrum, mid_spectrum, high_spectrum, loud_silent);

        // Apply onset cooldown
        let mut onset_gated = is_onset;
        if is_onset && (timestamp - self.last_onset_time) < self.onset_cooldown {
            onset_gated = false;
        }
        if onset_gated {
            self.last_onset_time = timestamp;
        }

        // Stage 2: Tempo estimation
        let (bpm, confidence, period_s) = self.tempo_estimator.update(combined_flux);

        // Stage 3: Beat scheduling
        self.beat_scheduler
            .update_tempo(bpm, period_s, confidence, self.tempo_estimator.locked);
        let (is_beat, beat_time, beat_phase, _smoothed_bpm) = self.beat_scheduler.process(
            onset_gated,
            onset_strength,
            timestamp,
            self.onset_detector.is_sustained_silence(),
        );

        // Onset hold+decay (instant attack, exponential release)
        if onset_strength > self.held_onset {
            self.held_onset = onset_strength;
        } else if dt > 0.0 {
            self.held_onset *= (-dt as f32 / self.onset_decay_tau).exp();
        }

        // Freeze phase at 0 during silence.
        //
        // Gate on the A10 perceptual flag, not on `rms`: by this point `rms` has been
        // through the adaptive normalizer, which maps it to `(v − P5) / (P95 − P5)` and so
        // floors it at exactly 0.0 whenever the signal touches the bottom of its recent
        // range. On any rhythmic material that is the trough between every hit, on
        // perfectly loud audio — which used to punch a spurious 1-hop `beat_phase` dropout
        // to 0 several times a beat (found while verifying A8 #1459).
        let phase = if loud_silent { 0.0 } else { beat_phase as f32 };

        BeatResult {
            onset_strength: self.held_onset,
            beat: if is_beat { 1.0 } else { 0.0 },
            beat_phase: phase,
            // Q2b (F3): the wire carries the held tempo, not the raw filter —
            // see `TempoEstimator::published_bpm`.
            bpm: self.tempo_estimator.published_bpm() as f32,
            beat_strength: if is_beat {
                self.beat_scheduler.beat_strength as f32
            } else {
                0.0
            },
            beat_time: if is_beat { beat_time } else { timestamp },
            beat_index: self.beat_scheduler.grid_beat_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }
    fn approx_eq_f64(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    // ---- CircularBuffer tests ----

    #[test]
    fn circular_buffer_new_empty() {
        let buf = CircularBuffer::new(10);
        assert_eq!(buf.len(), 0);
        assert!(buf.values().is_empty());
    }

    #[test]
    fn circular_buffer_push_under_capacity() {
        let mut buf = CircularBuffer::new(5);
        buf.push(1.0);
        buf.push(2.0);
        buf.push(3.0);
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.values(), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn circular_buffer_push_wrap_around() {
        let mut buf = CircularBuffer::new(3);
        buf.push(1.0);
        buf.push(2.0);
        buf.push(3.0);
        buf.push(4.0); // wraps, oldest (1.0) overwritten
        assert_eq!(buf.len(), 3);
        let vals = buf.values();
        assert_eq!(vals, vec![2.0, 3.0, 4.0]);
    }

    #[test]
    fn circular_buffer_median_odd() {
        let mut buf = CircularBuffer::new(5);
        for v in [3.0, 1.0, 4.0, 1.0, 5.0] {
            buf.push(v);
        }
        // sorted: [1.0, 1.0, 3.0, 4.0, 5.0], median = 3.0
        assert!(approx_eq_f64(buf.median(), 3.0, 1e-10));
    }

    #[test]
    fn circular_buffer_median_even() {
        let mut buf = CircularBuffer::new(4);
        for v in [1.0, 2.0, 3.0, 4.0] {
            buf.push(v);
        }
        // sorted: [1.0, 2.0, 3.0, 4.0], median = (2.0+3.0)/2 = 2.5
        assert!(approx_eq_f64(buf.median(), 2.5, 1e-10));
    }

    #[test]
    fn circular_buffer_median_empty() {
        let buf = CircularBuffer::new(5);
        assert_eq!(buf.median(), 0.0);
    }

    #[test]
    fn circular_buffer_mad() {
        let mut buf = CircularBuffer::new(5);
        for v in [1.0, 2.0, 3.0, 4.0, 5.0] {
            buf.push(v);
        }
        // median=3.0, deviations=[2,1,0,1,2], sorted=[0,1,1,2,2], mad=1.0
        assert!(approx_eq_f64(buf.mad(), 1.0, 1e-10));
    }

    #[test]
    fn circular_buffer_max() {
        let mut buf = CircularBuffer::new(5);
        for v in [1.0, 5.0, 3.0] {
            buf.push(v);
        }
        assert!(approx_eq_f64(buf.max(), 5.0, 1e-10));
    }

    #[test]
    fn circular_buffer_max_empty() {
        let buf = CircularBuffer::new(5);
        assert_eq!(buf.max(), 0.0);
    }

    #[test]
    fn circular_buffer_mean() {
        let mut buf = CircularBuffer::new(5);
        for v in [2.0, 4.0, 6.0] {
            buf.push(v);
        }
        assert!(approx_eq_f64(buf.mean(), 4.0, 1e-10));
    }

    // ---- KalmanBpm tests ----

    #[test]
    fn kalman_first_measurement_returns_raw() {
        let mut k = KalmanBpm::new();
        let bpm = k.update(120.0, 0.5, false, false);
        assert!(approx_eq_f64(bpm, 120.0, 1e-6));
    }

    #[test]
    fn kalman_stable_input_stays_near() {
        let mut k = KalmanBpm::new();
        k.update(120.0, 0.8, false, false);
        for _ in 0..50 {
            let bpm = k.update(120.0, 0.8, false, false);
            assert!((bpm - 120.0).abs() < 5.0, "got {}", bpm);
        }
    }

    /// Q2 replaced the snap-then-count-to-30 octave preprocessing with graded
    /// evidence upstream (continuity weight in candidate scoring). At the
    /// filter, brief octave noise is only *damped* now — the small gain keeps a
    /// couple of wild frames from moving the estimate far — and the track
    /// recovers as soon as sane measurements resume.
    #[test]
    fn kalman_brief_octave_noise_is_damped() {
        let mut k = KalmanBpm::new();
        k.update(120.0, 0.8, false, false);
        for _ in 0..10 {
            k.update(120.0, 0.8, false, false);
        }
        for _ in 0..3 {
            let bpm = k.update(240.0, 0.8, false, false);
            assert!(
                bpm < 160.0,
                "3 noisy frames must not reach the far octave, got {bpm}"
            );
        }
        let mut bpm = 0.0;
        for _ in 0..20 {
            bpm = k.update(120.0, 0.8, false, false);
        }
        assert!(
            (bpm - 120.0).abs() < 6.0,
            "must recover after noise, got {bpm}"
        );
    }

    /// A sustained, genuine change to the double tempo is followed promptly —
    /// the leaky divergence counter hard-resets after ~15 divergent frames
    /// (~1 s at the tempo cadence) instead of chasing through the filter gain.
    #[test]
    fn kalman_sustained_octave_change_is_followed() {
        let mut k = KalmanBpm::new();
        k.update(120.0, 0.8, false, false);
        for _ in 0..20 {
            k.update(240.0, 0.8, false, false);
        }
        let bpm = k.update(240.0, 0.8, false, false);
        assert!((bpm - 240.0).abs() < 12.0, "expected near 240, got {}", bpm);
    }

    #[test]
    fn kalman_divergence_reset() {
        let mut k = KalmanBpm::new();
        k.update(120.0, 0.8, false, false);
        for _ in 0..5 {
            k.update(120.0, 0.8, false, false);
        }
        // Feed completely different BPM — should reset after 15 frames
        for _ in 0..20 {
            k.update(80.0, 0.8, false, false);
        }
        let bpm = k.update(80.0, 0.8, false, false);
        assert!((bpm - 80.0).abs() < 15.0, "expected near 80, got {}", bpm);
    }

    // ---- OnsetDetector tests ----

    #[test]
    fn onset_silence_gate() {
        let mut od = OnsetDetector::new(44100.0, 50, 400);
        let bass = vec![0.0; 2049]; // 4096-pt fft
        let mid = vec![0.0; 513]; // 1024-pt fft
        let high = vec![0.0; 257]; // 512-pt fft
        // Perceptual silence gate (A10): loud_silent = true → no onset.
        let (is_onset, strength, _) = od.process(&bass, &mid, &high, true);
        assert!(!is_onset);
        assert!(approx_eq(strength, 0.0, 1e-6));
    }

    #[test]
    fn onset_sustained_silence() {
        let mut od = OnsetDetector::new(44100.0, 50, 400);
        let bass = vec![0.0; 2049];
        let mid = vec![0.0; 513];
        let high = vec![0.0; 257];
        for _ in 0..40 {
            od.process(&bass, &mid, &high, true);
        }
        assert!(od.is_sustained_silence());
    }

    #[test]
    fn superflux_fires_on_broadband_onset_not_vibrato() {
        // Bands are built on first process; a broadband magnitude jump must produce flux,
        // while a partial merely sliding ±1 band (vibrato) must be suppressed by the freq
        // max filter.
        let mut od = OnsetDetector::new(44100.0, 50, 400);
        let bins = 2049;
        let (mid, high) = (vec![0.0; 513], vec![0.0; 257]);
        let quiet = vec![0.01f32; bins];
        // Warm up on a steady quiet spectrum (fills prev_log, no flux).
        for _ in 0..4 {
            od.process(&quiet, &mid, &high, false);
        }
        // Broadband jump → onset flux well above the quiet baseline.
        let mut loud = vec![0.01f32; bins];
        for m in loud.iter_mut().take(400).skip(4) {
            *m = 3.0;
        }
        let (_, _, onset_flux) = od.process(&loud, &mid, &high, false);

        // A single tone that slides up one FFT bin each frame (vibrato) should barely register.
        let mut vib = OnsetDetector::new(44100.0, 50, 400);
        let mut vibrato_flux = 0.0;
        for k in 0..6 {
            let mut s = vec![0.01f32; bins];
            s[40 + k] = 3.0; // partial drifts one bin/frame
            let (_, _, f) = vib.process(&s, &mid, &high, false);
            vibrato_flux = f;
        }
        assert!(
            onset_flux > vibrato_flux * 3.0,
            "broadband onset ({onset_flux}) should dominate vibrato flux ({vibrato_flux})"
        );
    }

    // ---- BeatScheduler tests ----

    /// Q1 PLL: the old scheduler fired a beat on EVERY gated onset whenever
    /// tempo confidence was low — the mechanical source of the 1.5-1.7×
    /// over-firing. With no tempo hypothesis there is now no grid and no beat;
    /// /onset still carries the transient (user-approved pre-lock behavior).
    #[test]
    fn scheduler_no_tempo_means_no_beats() {
        let mut bs = BeatScheduler::new();
        bs.update_tempo(0.0, 0.0, 0.0, false);
        for i in 0..8 {
            let (is_beat, _, _, _) = bs.process(true, 0.8, 1.0 + i as f64 * 0.3, false);
            assert!(!is_beat, "onset {i} must not fire without a tempo");
        }
    }

    #[test]
    fn scheduler_silence_no_beat() {
        let mut bs = BeatScheduler::new();
        bs.update_tempo(120.0, 0.5, 0.8, false);
        let (is_beat, _, phase, _) = bs.process(false, 0.0, 1.0, true);
        assert!(!is_beat);
        assert!(approx_eq_f64(phase, 0.0, 1e-6));
    }

    #[test]
    fn scheduler_phase_in_range() {
        let mut bs = BeatScheduler::new();
        bs.update_tempo(120.0, 0.5, 0.8, false);
        // Anchor the grid
        bs.process(true, 0.8, 1.0, false);
        // Advance time
        for i in 1..100 {
            let t = 1.0 + (i as f64) * 0.01;
            let (_, _, phase, _) = bs.process(false, 0.0, t, false);
            assert!((0.0..=1.0).contains(&phase), "phase={} at t={}", phase, t);
        }
    }

    /// Q1 PLL, locked freerun: beats fire AT grid instants (beat_time carries
    /// the instant, not the hop that noticed it), exactly one per period.
    #[test]
    fn scheduler_locked_fires_on_grid() {
        let mut bs = BeatScheduler::new();
        bs.update_tempo(120.0, 0.5, 0.9, true);
        bs.process(true, 0.9, 1.0, false); // anchor
        let mut beats = Vec::new();
        let dt = 512.0 / 44100.0;
        let mut t: f64 = 1.0;
        while t < 9.0 {
            t += dt;
            // Supporting onsets exactly on the (true) beat grid at 120 BPM.
            let near_beat = ((t - 1.0) / 0.5 - ((t - 1.0) / 0.5).round()).abs() * 0.5 < dt * 0.5;
            let (fired, bt, _, _) = bs.process(near_beat, 0.8, t, false);
            if fired {
                beats.push(bt);
            }
        }
        assert!(
            (14..=17).contains(&beats.len()),
            "expected ~16 beats in 8 s at 120 BPM, got {}",
            beats.len()
        );
        for pair in beats.windows(2) {
            let gap = pair[1] - pair[0];
            assert!(
                (gap - 0.5).abs() < 0.05,
                "grid beats must be one period apart, got {gap}"
            );
        }
    }

    /// Q1 PLL, muted freerun: a breakdown (no onsets) mutes emission after a
    /// few unsupported beats, the grid keeps phase, and the return of support
    /// resumes on-grid without re-acquisition.
    #[test]
    fn scheduler_breakdown_mutes_then_resumes_on_grid() {
        let mut bs = BeatScheduler::new();
        bs.update_tempo(120.0, 0.5, 0.9, true);
        let dt = 512.0 / 44100.0;
        let on_grid = |t: f64| ((t - 1.0) / 0.5 - ((t - 1.0) / 0.5).round()).abs() * 0.5 < dt * 0.5;
        bs.process(true, 0.9, 1.0, false); // anchor
        let mut t = 1.0;
        // 8 s supported.
        while t < 9.0 {
            t += dt;
            bs.process(on_grid(t), 0.8, t, false);
        }
        // 8 s breakdown: count emissions — must stop after a few beats.
        let mut gap_fires = 0;
        while t < 17.0 {
            t += dt;
            let (fired, _, _, _) = bs.process(false, 0.0, t, false);
            gap_fires += fired as u32;
        }
        assert!(
            gap_fires <= 8,
            "breakdown must mute freerun, got {gap_fires} fires in 16 bars"
        );
        // Support returns: emission resumes, and the resumed beats sit on the
        // ORIGINAL grid (phase preserved through the gap).
        let mut resumed = Vec::new();
        while t < 25.0 {
            t += dt;
            let (fired, bt, _, _) = bs.process(on_grid(t), 0.8, t, false);
            if fired {
                resumed.push(bt);
            }
        }
        assert!(
            resumed.len() >= 12,
            "must resume firing, got {}",
            resumed.len()
        );
        for &bt in &resumed[2..] {
            let phase_err = ((bt - 1.0) / 0.5 - ((bt - 1.0) / 0.5).round()).abs() * 0.5;
            assert!(
                phase_err < 0.08,
                "resumed beat at {bt} is {phase_err}s off the original grid"
            );
        }
    }

    /// Q1 PLL invariant: emitted beat_times are STRICTLY increasing and never
    /// closer than 0.3 period, across lock flips, corrections, silences and
    /// mode churn. (A pending negative slot correction around an acquisition
    /// fire once produced a −1.6 ms inversion that crashed mir_eval.)
    #[test]
    fn scheduler_beat_times_strictly_monotonic_through_mode_churn() {
        let mut bs = BeatScheduler::new();
        let dt = 512.0 / 44100.0;
        let period = 0.5;
        let mut rng: u64 = 0xBEEF;
        let mut emitted: Vec<f64> = Vec::new();
        let mut t = 1.0;
        for step in 0..(120.0 / dt) as usize {
            t += dt;
            rng = rng
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let r = (rng >> 40) as f64 / (1u64 << 24) as f64;
            // Lock flaps every ~2 s; onsets arrive ~5/s at pseudo-random
            // instants; occasional silence patches.
            let locked = (step / 172) % 2 == 0;
            bs.update_tempo(120.0, period, if locked { 0.8 } else { 0.3 }, locked);
            let onset = r < 0.06;
            let silence = (step / 800) % 7 == 6;
            let (fired, bt, _, _) = bs.process(onset, 0.5 + (r * 0.5) as f32, t, silence);
            if fired {
                if let Some(&prev) = emitted.last() {
                    assert!(
                        bt > prev + 0.3 * period - 1e-9,
                        "beat at {bt:.4} too close to previous {prev:.4} (step {step})"
                    );
                }
                emitted.push(bt);
            }
        }
        assert!(
            emitted.len() > 40,
            "churn drive should still emit beats, got {}",
            emitted.len()
        );
    }

    /// Q1 PLL, acquisition: unlocked fires need onset+grid agreement and are
    /// spaced at least 0.6 period — a hi-hat run cannot spray beats.
    #[test]
    fn scheduler_acquisition_is_evidence_gated_and_spaced() {
        let mut bs = BeatScheduler::new();
        bs.update_tempo(120.0, 0.5, 0.3, false);
        bs.process(true, 0.8, 1.0, false); // anchor fire
        // Onsets every 100 ms (16th-note spam at 150 BPM): only those agreeing
        // with the 0.5 s grid may fire, so ≤ 1 fire per 0.3 s minimum spacing.
        let mut fires = 0;
        let dt = 512.0 / 44100.0;
        let mut t = 1.0;
        let mut next_onset = 1.1;
        while t < 6.0 {
            t += dt;
            let onset = t + dt * 0.5 >= next_onset;
            if onset {
                next_onset += 0.1;
            }
            let (fired, _, _, _) = bs.process(onset, 0.7, t, false);
            fires += fired as u32;
        }
        assert!(
            fires <= 11,
            "acquisition must not fire above ~1 per period on onset spam, got {fires} in 5 s"
        );
    }

    // ---- Integration test ----

    /// Per-frame band magnitude for a kick train with sub-hop placement. Kick
    /// `i` (0-based) lands at `(i+1) * interval`; its magnitude rises to
    /// `amp(i)` GEOMETRICALLY across the two frames the instant falls between,
    /// so the SuperFlux log-domain rise splits (1−frac)/frac — which is how the
    /// real 87.5%-overlap analysis window encodes sub-hop onset position in the
    /// flux envelope. Binary single-frame impulses instead quantize an off-bin
    /// tempo like 175 BPM into a hard 29/30-frame alternation whose
    /// every-other-kick lag is EXACTLY 59 frames, making half tempo genuinely
    /// sharper in the synthetic signal than the tempo it claims to carry.
    fn kick_amplitude_track(
        num_frames: usize,
        dt: f64,
        interval: f64,
        amp: impl Fn(usize) -> f32,
    ) -> Vec<f32> {
        const BASE: f64 = 0.001;
        let mut track = vec![BASE as f32; num_frames];
        let mut i = 0usize;
        loop {
            let f = (i + 1) as f64 * interval / dt;
            let idx = f as usize;
            if idx + 1 >= num_frames {
                return track;
            }
            let frac = f - idx as f64;
            let a = f64::from(amp(i));
            let first = BASE * (a / BASE).powf(1.0 - frac);
            track[idx] = track[idx].max(first as f32);
            track[idx + 1] = track[idx + 1].max(a as f32);
            i += 1;
        }
    }

    /// Run a BPM convergence test with synthetic kicks at the given tempo.
    /// Returns the detected BPM after `duration_secs` seconds.
    fn run_bpm_convergence_test(target_bpm: f64, duration_secs: f64) -> f32 {
        run_bpm_convergence_with(target_bpm, duration_secs, TempoConfig::default())
    }

    fn run_bpm_convergence_with(target_bpm: f64, duration_secs: f64, tempo: TempoConfig) -> f32 {
        let sample_rate = 44100.0;
        let mut detector = BeatDetector::new(sample_rate, tempo);

        let bass_len = 2049; // 4096/2 + 1
        let mid_len = 513; // 1024/2 + 1
        let high_len = 257; // 512/2 + 1

        // Frames must be spaced at the detector's own clock. Since A5 (#1456) that is derived
        // from the fixed analysis hop (sr / ANALYSIS_HOP ~= 86.1 Hz @ 44.1 kHz), not the 100 Hz
        // this harness used to assume — at 100 Hz every `target_bpm` below actually reached the
        // estimator 13.9% low, and only the +/-15% tolerance bands hid it.
        let dt = crate::audio::ANALYSIS_HOP as f64 / sample_rate as f64;
        let kick_interval = 60.0 / target_bpm;
        let num_frames = (duration_secs / dt) as usize;
        // Sub-hop onset placement, like a real windowed analyzer sees it: a kick
        // between two hops splits its energy across both in proportion to its
        // fractional position. Single-frame impulses instead quantize an off-bin
        // tempo like 175 BPM (29.53 frames) into a hard 29/30 alternation whose
        // every-other-kick lag is EXACTLY 59 — making half tempo genuinely
        // sharper in the synthetic signal than the tempo it claims to carry.
        let kick_amp = kick_amplitude_track(num_frames, dt, kick_interval, |_| 2.0);

        let mut last_bpm = 0.0f32;

        for (frame, &amp) in kick_amp.iter().enumerate() {
            let t = frame as f64 * dt;

            let mut bass = vec![0.001f32; bass_len];
            let mid = vec![0.001f32; mid_len];
            let high = vec![0.001f32; high_len];

            for bin in 1..12 {
                bass[bin] = amp;
            }

            let result = detector.process(&bass, &mid, &high, t, false);
            last_bpm = result.bpm;
        }

        eprintln!(
            "BPM convergence: target={target_bpm}, detected={last_bpm}, duration={duration_secs}s"
        );
        last_bpm
    }

    /// Like `run_bpm_convergence_with`, but alternating strong/weak hits — the
    /// strong-hits-only grid (half tempo) carries real ACF evidence of its own,
    /// like a kick/snare backbeat does.
    fn run_backbeat_convergence(target_bpm: f64, duration_secs: f64, preset: TempoPreset) -> f32 {
        let (center, sigma) = preset.values();
        let tempo = TempoConfig {
            prior_center_bpm: center,
            prior_sigma: sigma,
            auto_prior: false,
        };
        let sample_rate = 44100.0;
        let mut detector = BeatDetector::new(sample_rate, tempo);
        let (bass_len, mid_len, high_len) = (2049, 513, 257);
        let dt = crate::audio::ANALYSIS_HOP as f64 / sample_rate as f64;
        let kick_interval = 60.0 / target_bpm;
        let num_frames = (duration_secs / dt) as usize;
        let kick_amp = kick_amplitude_track(num_frames, dt, kick_interval, |i| {
            if i % 2 == 0 { 2.0 } else { 0.8 }
        });
        let mut last_bpm = 0.0f32;
        for (frame, &amp) in kick_amp.iter().enumerate() {
            let t = frame as f64 * dt;
            let mut bass = vec![0.001f32; bass_len];
            let mid = vec![0.001f32; mid_len];
            let high = vec![0.001f32; high_len];
            for bin in 1..12 {
                bass[bin] = amp;
            }
            last_bpm = detector.process(&bass, &mid, &high, t, false).bpm;
        }
        eprintln!("Backbeat convergence: target={target_bpm}, detected={last_bpm}");
        last_bpm
    }

    // Q2: bands tightened from the ±15-20% that hid the lag quantization and the
    // sign-flipped parabola to ±5% — continuous-lag scoring lands well inside.
    #[test]
    fn bpm_converges_120() {
        let bpm = run_bpm_convergence_test(120.0, 8.0);
        assert!(
            (f64::from(bpm) - 120.0).abs() / 120.0 < 0.05,
            "120 BPM: expected within 5%, got {bpm}"
        );
    }

    #[test]
    fn bpm_converges_90() {
        let bpm = run_bpm_convergence_test(90.0, 10.0);
        assert!(
            (f64::from(bpm) - 90.0).abs() / 90.0 < 0.05,
            "90 BPM: expected within 5%, got {bpm}"
        );
    }

    #[test]
    fn bpm_converges_140() {
        let bpm = run_bpm_convergence_test(140.0, 10.0);
        assert!(
            (f64::from(bpm) - 140.0).abs() / 140.0 < 0.05,
            "140 BPM: expected within 5%, got {bpm}"
        );
    }

    #[test]
    fn bpm_converges_170() {
        let bpm = run_bpm_convergence_test(170.0, 10.0);
        assert!(
            (f64::from(bpm) - 170.0).abs() / 170.0 < 0.05,
            "170 BPM: expected within 5%, got {bpm}"
        );
    }

    #[test]
    fn bpm_converges_200() {
        let bpm = run_bpm_convergence_test(200.0, 10.0);
        assert!(
            (f64::from(bpm) - 200.0).abs() / 200.0 < 0.05,
            "200 BPM: expected within 5%, got {bpm}"
        );
    }

    #[test]
    fn bpm_converges_230() {
        let bpm = run_bpm_convergence_test(230.0, 10.0);
        // Accept 230 or half-tempo 115 (prior centered at 150 favors the lower octave)
        let b = f64::from(bpm);
        let in_range = (b - 230.0).abs() / 230.0 < 0.05 || (b - 115.0).abs() / 115.0 < 0.05;
        assert!(
            in_range,
            "230 BPM: expected within 5% of 230 or 115, got {bpm}"
        );
    }

    #[test]
    fn bpm_no_octave_double_145() {
        let bpm = run_bpm_convergence_test(145.0, 10.0);
        assert!(
            (f64::from(bpm) - 145.0).abs() / 145.0 < 0.05,
            "145 BPM: expected within 5% (not 290 octave double), got {bpm}"
        );
    }

    /// Q2: 175 BPM sits at lag 29.53 — between ACF bins that are 3.4% apart at
    /// this tempo, against a 4% Acc1 tolerance. Integer-lag candidates could
    /// not represent it (the old GiantSteps floor); continuous-lag candidates
    /// with peak interpolation must land within 1%.
    #[test]
    fn off_bin_tempo_locks_within_one_percent() {
        let bpm = run_bpm_convergence_test(175.0, 10.0);
        assert!(
            (f64::from(bpm) - 175.0).abs() / 175.0 < 0.01,
            "175 BPM: expected within 1%, got {bpm}"
        );
    }

    /// Q2 seed gate: white-noise onset input produces low-confidence ACF
    /// readings only, and the estimator must never adopt one as its seed — the
    /// old code seeded on the very first post-2s measurement unconditionally
    /// and then defended it.
    #[test]
    fn tempo_seed_requires_confidence() {
        let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
        let mut rng: u64 = 12345;
        for _ in 0..(6.0 * 86.13) as usize {
            rng = rng
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let v = (rng >> 32) as f64 / (1u64 << 32) as f64;
            let (bpm, _conf, _period) = est.update(v);
            assert_eq!(bpm, 0.0, "white-noise onsets must never seed a tempo");
        }
    }

    /// Drive a TempoEstimator with an impulse train at the given period (in
    /// frames, fractional OK) for `secs`, returning the last published BPM.
    fn drive_estimator(est: &mut TempoEstimator, period_frames: f64, secs: f64) -> f64 {
        let mut bpm = 0.0;
        let mut next = period_frames;
        for i in 0..(secs * 86.13) as usize {
            let v = if i as f64 + 0.5 >= next {
                next += period_frames;
                1.0
            } else {
                0.05
            };
            bpm = est.update(v).0;
        }
        bpm
    }

    /// Q3 lock dynamics: a locked estimator rides out a brief burst of
    /// contradictory readings (a fill, a breakdown bar) without its published
    /// BPM leaving the ±4% band — the excursion class that reset the bench's
    /// trailing-lock clock up to 10× per track — and re-locks seamlessly.
    #[test]
    fn tempo_lock_survives_brief_wobble() {
        let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
        // ~128 BPM: period 40.4 frames.
        let bpm = drive_estimator(&mut est, 40.4, 20.0);
        assert!(
            est.locked,
            "20 s of a clean train must lock (support {})",
            est.support
        );
        assert!(
            (bpm - 128.0).abs() / 128.0 < 0.04,
            "locked near 128, got {bpm}"
        );
        // 2.5 s of white noise — support dips, published BPM must hold the band.
        let mut rng: u64 = 777;
        for _ in 0..(2.5 * 86.13) as usize {
            rng = rng
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let v = (rng >> 32) as f64 / (1u64 << 32) as f64;
            let (b, _, _) = est.update(v);
            assert!(
                (b - 128.0).abs() / 128.0 < 0.04,
                "published BPM left the band during a brief wobble: {b}"
            );
        }
        // Train resumes: still (or again) locked shortly after.
        drive_estimator(&mut est, 40.4, 5.0);
        assert!(
            est.locked,
            "must re-lock after the wobble (support {})",
            est.support
        );
    }

    /// Q2b: a brief stretch of winners at a metrically-related level (the
    /// double-kick-over-four-on-the-floor case from the 2026-08-08 live smoke)
    /// folds into the anchor grid instead of teleporting the filter — the
    /// published tempo never leaves the band and the lock holds.
    #[test]
    fn anchor_ignores_brief_related_level_flip() {
        let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
        drive_estimator(&mut est, 40.4, 20.0); // ~128 BPM
        assert!(est.locked && est.anchor_log2.is_some(), "anchor earned");
        // 5 s at the ½ level (~64 BPM, period 80.8 frames): pre-Q2b this fed
        // the divergence hard reset within ~1 s.
        drive_estimator(&mut est, 80.8, 5.0);
        let published = est.published_bpm();
        assert!(
            (published - 128.0).abs() / 128.0 < 0.04,
            "published held the anchor level, got {published}"
        );
        assert!(est.locked, "related-level winners must not drain the lock");
        assert!(est.anchor_log2.is_some(), "anchor survives");
    }

    /// Q2b: during probation, sustained NON-octave evidence displaces — this
    /// is the correction path for the unrecoverable class (a wrong ⅔/¾ first
    /// anchor), the measured GiantSteps win.
    #[test]
    fn nonoctave_evidence_displaces_during_probation() {
        let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
        drive_estimator(&mut est, 40.4, 16.0); // ~128 BPM, anchor ~13 s in
        assert!(est.anchor_log2.is_some());
        // ⅔ level (~85.3 BPM, period 60.6): flush + challenge + span lands
        // inside the 30 s probation window.
        let bpm = drive_estimator(&mut est, 60.6, 24.0);
        assert!(
            (bpm - 85.3).abs() / 85.3 < 0.05,
            "non-octave evidence in probation must displace, got {bpm}"
        );
    }

    /// Q2b, owner-ruled after live run 4: OCTAVE evidence never displaces —
    /// not even during probation. Run 4's logged chain: ½-level intro
    /// evidence displaced a CORRECT 26 s-old anchor, the wrong lineage
    /// out-tenured the truth, and the ghost then defended 115 against a
    /// genuine 172 detection. An octave-wrong young anchor is benign; a
    /// displaced correct one is not.
    #[test]
    fn octave_evidence_folds_even_in_probation() {
        let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
        drive_estimator(&mut est, 40.4, 16.0); // ~128 BPM, probation active
        assert!(est.anchor_log2.is_some());
        let bpm = drive_estimator(&mut est, 80.8, 24.0);
        assert!(
            (bpm - 128.0).abs() / 128.0 < 0.04,
            "octave evidence must fold even in probation, got {bpm}"
        );
    }

    /// Q2b, owner-ruled after live run 3: once probation ends the level is
    /// settled — even sustained OCTAVE evidence folds instead of displacing
    /// (the 172→86→172→86 ping-pong followed a half-time feel, not a tempo
    /// change). Escape hatches: unrelated divergence, tap, octave override.
    #[test]
    fn post_probation_octave_evidence_folds_not_displaces() {
        let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
        // ~128 BPM well past probation (earn ~13 s + 30 s probation).
        drive_estimator(&mut est, 40.4, 50.0);
        assert!(est.locked && est.anchor_log2.is_some());
        // 20 s of sustained ½-level winners: pre-ruling this displaced.
        let bpm = drive_estimator(&mut est, 80.8, 20.0);
        assert!(
            (bpm - 128.0).abs() / 128.0 < 0.04,
            "post-probation octave evidence must fold, got {bpm}"
        );
        assert!(
            est.locked,
            "folded octave evidence keeps supporting the lock"
        );
    }

    /// Q2b ghost anchor: a level war (anchor → unrelated mush → abandon → a
    /// related level locks and re-earns) must not mint an unrecoverable
    /// non-octave anchor — the re-earn folds back to the ghost's level. This
    /// is the 2026-08-08 re-listen failure: the orchestral section's mush
    /// abandoned the 172 anchor and the ⅔ level (115) re-earned, then held
    /// wrong by design for 90 s.
    #[test]
    fn ghost_anchor_vetoes_related_reearn() {
        let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
        // ~128 BPM long enough for the anchor to be a VETERAN — the ghost
        // only forms with GHOST_MIN_TENURE_SECS of tenure (young first
        // anchors are the mistake a re-earn corrects, not a level to defend).
        drive_estimator(&mut est, 40.4, 45.0);
        assert!(est.anchor_log2.is_some());
        // ~12 s of unrelated MUSH — alternating ~105 / ~93 BPM so the winner
        // keeps deviating >10% from the anchor (the hard reset fires) but
        // support can never build 8 s of lock (no intermediate anchor may
        // earn, or it would launder the ghost — the exact live failure).
        for _ in 0..4 {
            drive_estimator(&mut est, 49.2, 1.5);
            drive_estimator(&mut est, 55.4, 1.5);
        }
        assert!(est.anchor_log2.is_none(), "mush must abandon the anchor");
        assert!(est.ghost_anchor.is_some(), "abandon leaves a ghost");
        // Sustained ⅔ level (~85.3 BPM): locks and re-earns — the ghost must
        // fold the earn back to ~128, not let 85.3 stick.
        drive_estimator(&mut est, 60.6, 22.0);
        let anchor_bpm = est.anchor_log2.map(|a| 2.0f64.powf(a)).unwrap_or(0.0);
        assert!(
            (anchor_bpm - 128.0).abs() / 128.0 < 0.06,
            "re-earn must fold to the ghost level, anchored at {anchor_bpm}"
        );
        let published = est.published_bpm();
        assert!(
            (published - 128.0).abs() / 128.0 < 0.06,
            "published must return to the ghost level, got {published}"
        );
    }

    /// Q2b (F3): the wire holds the last locked tempo through an unlock and
    /// reads 0 only before the first lock is ever earned.
    #[test]
    fn published_bpm_holds_through_unlock_and_zeroes_before_first_lock() {
        let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
        // Fresh estimator + noise: nothing published.
        let mut rng: u64 = 42;
        for _ in 0..(3.0 * 86.13) as usize {
            rng = rng
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            est.update((rng >> 32) as f64 / (1u64 << 32) as f64);
        }
        assert_eq!(est.published_bpm(), 0.0, "no lock ever -> wire silent");
        // Lock at ~128, then starve support with silence-grade input.
        drive_estimator(&mut est, 40.4, 20.0);
        assert!(est.locked);
        for _ in 0..(6.0 * 86.13) as usize {
            est.update(0.0);
        }
        let published = est.published_bpm();
        assert!(
            (published - 128.0).abs() / 128.0 < 0.04,
            "wire must hold the last locked tempo, got {published}"
        );
    }

    /// Q3: lock resists wobble but NOT a real change — a sustained new tempo
    /// unlocks (or hard-resets) and is adopted within a few seconds.
    #[test]
    fn tempo_lock_releases_on_sustained_change() {
        let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
        drive_estimator(&mut est, 40.4, 20.0); // ~128 BPM
        assert!(est.locked);
        // Genuine move to ~148 BPM (period 34.9 frames) for 12 s.
        let bpm = drive_estimator(&mut est, 34.9, 12.0);
        assert!(
            (bpm - 148.0).abs() / 148.0 < 0.05,
            "sustained new tempo must be adopted, got {bpm}"
        );
    }

    /// Q3: the lock state reaches the scheduler alongside confidence — the
    /// Stage 4 PLL keys its modes off this flag.
    #[test]
    fn scheduler_receives_lock_state() {
        let mut bs = BeatScheduler::new();
        assert!(!bs.tempo_locked);
        bs.update_tempo(128.0, 60.0 / 128.0, 0.9, true);
        assert!(bs.tempo_locked);
        bs.update_tempo(128.0, 60.0 / 128.0, 0.2, false);
        assert!(!bs.tempo_locked);
    }

    /// Q2: flat ACF ⇒ every candidate scores identically no matter how many of
    /// its harmonics fit inside the array. The old SUM gave a candidate with 4
    /// in-range harmonics up to 2× the score of an equally-supported longer
    /// period — the double-time seed bias.
    #[test]
    fn comb_score_is_truncation_fair() {
        let acf = vec![0.5; 101];
        let full = comb_score(&acf, 20.0); // h=1..4 → 20,40,60,80 all inside
        let cut = comb_score(&acf, 40.0); // h=3,4 → 120,160 truncated
        assert!((full - cut).abs() < 1e-12, "{full} vs {cut}");
    }

    #[test]
    fn acf_at_interpolates_linearly() {
        let acf = vec![0.0, 1.0, 0.0];
        assert!((acf_at(&acf, 0.5) - 0.5).abs() < 1e-12);
        assert!((acf_at(&acf, 1.25) - 0.75).abs() < 1e-12);
    }

    // ---- A7 (#1458): tempo prior, octave override, tap tempo ----

    #[test]
    fn tempo_config_default_matches_pre_a7_hardcoding() {
        // Upgrading users must get byte-identical detection until they touch a preset.
        let c = TempoConfig::default();
        assert_eq!(c.prior_center_bpm, 150.0);
        assert_eq!(c.prior_sigma, 1.0);
        assert!(!c.auto_prior);
    }

    #[test]
    fn tempo_preset_round_trips_through_config() {
        for &p in TempoPreset::ALL {
            let (center, sigma) = p.values();
            let cfg = TempoConfig {
                prior_center_bpm: center,
                prior_sigma: sigma,
                auto_prior: false,
            };
            assert_eq!(TempoPreset::from_config(&cfg), Some(p));
        }
    }

    #[test]
    fn hand_tuned_config_matches_no_preset() {
        let cfg = TempoConfig {
            prior_center_bpm: 133.0,
            prior_sigma: 0.5,
            auto_prior: false,
        };
        assert_eq!(TempoPreset::from_config(&cfg), None);
    }

    #[test]
    fn prior_center_decides_the_octave() {
        // The A7 payoff, stated as something this harness can actually prove: the same 172 BPM
        // signal reads as 172 under the default prior and folds to a metrical division under a
        // prior centred low. That is the prior steering metrical-ratio selection — the mechanism
        // the genre presets exist to drive.
        //
        // Q2 note: a clean impulse train's subharmonics all carry identical normalized comb
        // evidence (every division of the grid IS a valid slower reading of it), so under a low
        // prior the winner among 86/57.4/43 is decided by prior proximity alone — the fold may
        // land on ÷2 or ÷3. `backbeat_fold_prefers_the_supported_octave` pins the realistic
        // case where rhythm evidence breaks the tie.
        let default_bpm = run_bpm_convergence_with(172.0, 10.0, TempoConfig::default());
        assert!(
            (default_bpm - 172.0).abs() / 172.0 < 0.04,
            "default prior should hold 172 within 4%, got {default_bpm}"
        );

        let (center, sigma) = TempoPreset::Ambient.values();
        let low_bpm = run_bpm_convergence_with(
            172.0,
            10.0,
            TempoConfig {
                prior_center_bpm: center,
                prior_sigma: sigma,
                auto_prior: false,
            },
        );
        assert!(
            low_bpm > 40.0 && low_bpm < 95.0,
            "a prior centred at {center} should fold 172 to a metrical division, got {low_bpm}"
        );
    }

    /// Q2: when the rhythm itself carries octave evidence — strong hits on the
    /// half-tempo grid, weaker ones between (a backbeat) — a low prior folds to
    /// the SUPPORTED division (86), not whichever division sits closest to the
    /// prior centre. This is the realistic fold `prior_center_decides_the_octave`
    /// admits its clean impulse train cannot reproduce.
    #[test]
    fn backbeat_fold_prefers_the_supported_octave() {
        let bpm = run_backbeat_convergence(172.0, 10.0, TempoPreset::Ambient);
        assert!(
            (bpm - 86.0).abs() / 86.0 < 0.05,
            "backbeat at 172 under a low prior should fold to 86, got {bpm}"
        );
    }

    #[test]
    fn tempo_control_mailbox_drains_once() {
        let mut ctl = TempoControl::default();
        ctl.push(TempoCommand::ShiftOctave(1));
        ctl.push(TempoCommand::Tap(128.0));
        assert_eq!(
            ctl.drain(),
            vec![TempoCommand::ShiftOctave(1), TempoCommand::Tap(128.0)]
        );
        assert!(ctl.drain().is_empty(), "commands must not be redelivered");
    }

    #[test]
    fn tempo_control_mailbox_is_bounded() {
        // A stalled/absent audio thread must not let the mailbox grow without limit.
        let mut ctl = TempoControl::default();
        for _ in 0..100 {
            ctl.push(TempoCommand::ShiftOctave(1));
        }
        assert!(ctl.drain().len() <= 16);
    }

    /// Drive the estimator to a stable lock, then hand back the estimator for override tests.
    fn locked_estimator(target_bpm: f64) -> TempoEstimator {
        let frame_rate = 100.0;
        let mut est = TempoEstimator::new(8.0, frame_rate, TempoConfig::default());
        let dt = 1.0 / frame_rate;
        let interval = 60.0 / target_bpm;
        let mut last = -1.0f64;
        for frame in 0..1000 {
            let t = frame as f64 * dt;
            let onset = if (t - last) >= interval - dt * 0.5 && t >= interval {
                last = t;
                1.0
            } else {
                0.0
            };
            est.update(onset);
        }
        est
    }

    #[test]
    fn octave_shift_survives_the_snap_escape() {
        // The regression this guards: shifting only the Kalman state is undone within ~30
        // updates by the snap-escape counter, because the autocorrelation keeps reporting the
        // octave the user just rejected. The offset must make the override stick.
        let mut est = locked_estimator(120.0);
        let before = est.current_bpm;
        assert!(before > 100.0 && before < 140.0, "setup: got {before}");

        est.shift_octave(-1);
        assert!(
            (est.current_bpm - before / 2.0).abs() < 5.0,
            "shift should halve the readout immediately, got {}",
            est.current_bpm
        );

        // Keep feeding the same 120 BPM signal well past the 30-update escape threshold.
        let dt = 0.01;
        let interval = 0.5;
        let mut last = -1.0f64;
        for frame in 0..800 {
            let t = frame as f64 * dt;
            let onset = if (t - last) >= interval - dt * 0.5 && t >= interval {
                last = t;
                1.0
            } else {
                0.0
            };
            est.update(onset);
        }
        assert!(
            est.current_bpm < 80.0,
            "octave override must hold, got {} BPM back at full tempo",
            est.current_bpm
        );
    }

    #[test]
    fn octave_shift_out_of_range_is_rejected() {
        let mut est = locked_estimator(170.0);
        let before = est.current_bpm;
        est.shift_octave(1); // 340 BPM > BPM_MAX
        assert_eq!(
            est.current_bpm, before,
            "out-of-range shift must be a no-op"
        );
    }

    #[test]
    fn tap_tempo_locks_across_an_octave() {
        // A tap an octave away from the estimate is exactly the case the user is correcting,
        // so it must bypass the Kalman's octave-snap preprocessing rather than be swallowed.
        let mut est = locked_estimator(86.0);
        assert!(est.current_bpm < 100.0, "setup: got {}", est.current_bpm);
        est.tap(172.0);
        assert!(
            (est.current_bpm - 172.0).abs() < 1.0,
            "tap must win, got {}",
            est.current_bpm
        );
    }

    #[test]
    fn tap_tempo_out_of_range_is_rejected() {
        let mut est = locked_estimator(120.0);
        let before = est.current_bpm;
        est.tap(700.0);
        assert_eq!(est.current_bpm, before, "out-of-range tap must be a no-op");
    }

    #[test]
    fn auto_prior_walks_toward_the_detected_tempo() {
        let frame_rate = 100.0;
        let mut est = TempoEstimator::new(
            8.0,
            frame_rate,
            TempoConfig {
                prior_center_bpm: 150.0,
                prior_sigma: 0.4,
                auto_prior: true,
            },
        );
        let start = est.prior_center_bpm();
        let dt = 1.0 / frame_rate;
        let interval = 60.0 / 96.0;
        let mut last = -1.0f64;
        for frame in 0..12000 {
            let t = frame as f64 * dt;
            let onset = if (t - last) >= interval - dt * 0.5 && t >= interval {
                last = t;
                1.0
            } else {
                0.0
            };
            est.update(onset);
        }
        let end = est.prior_center_bpm();
        assert!(
            end < start - 1.0,
            "auto prior should drift down from {start} toward ~96, ended at {end}"
        );
        assert!(
            end >= AUTO_PRIOR_MIN_BPM as f32 && end <= AUTO_PRIOR_MAX_BPM as f32,
            "auto prior must stay clamped, got {end}"
        );
    }

    #[test]
    fn nonsense_config_values_are_clamped_not_trusted() {
        // settings.json is hand-editable; a 0 centre would make every prior weight exp(-inf).
        let mut est = TempoEstimator::new(8.0, 100.0, TempoConfig::default());
        est.set_config(TempoConfig {
            prior_center_bpm: 0.0,
            prior_sigma: 0.0,
            auto_prior: false,
        });
        assert!(
            est.prior_center_bpm().is_finite() && est.prior_center_bpm() >= BPM_MIN as f32,
            "centre must stay finite and in range, got {}",
            est.prior_center_bpm()
        );
        assert!(est.prior_sigma >= MIN_PRIOR_SIGMA);
        // The prior must still discriminate between candidates.
        assert!(est.tempo_prior_weight(BPM_MIN) > est.tempo_prior_weight(BPM_MAX * 0.9));
    }

    #[test]
    fn auto_prior_ignores_the_config_center() {
        // In auto mode the estimator owns the centre — a stale UI value must not stomp it.
        let mut est = TempoEstimator::new(8.0, 100.0, TempoConfig::default());
        est.set_config(TempoConfig {
            prior_center_bpm: 70.0,
            prior_sigma: 0.5,
            auto_prior: true,
        });
        assert_eq!(est.prior_center_bpm(), 150.0);
        assert_eq!(est.prior_sigma, 0.5, "sigma must still track the config");
    }
}
