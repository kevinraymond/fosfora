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
    diverge_count: u32, // consecutive divergent frames
    snap_count: u32,    // consecutive octave-snapped frames
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
            snap_count: 0,
            initialized: false,
        }
    }

    /// Jump the state to `bpm` with high certainty (A7 #1458: tap tempo). Bypasses the
    /// octave-snap preprocessing in `update`, which would otherwise reject a tap that is
    /// an octave away from the current estimate — exactly the case the user is correcting.
    fn force(&mut self, bpm: f64) {
        if bpm <= 0.0 {
            return;
        }
        self.state = bpm.log2();
        self.variance = 0.01;
        self.diverge_count = 0;
        self.snap_count = 0;
        self.initialized = true;
    }

    /// Shift the filtered state by `direction` octaves (A7 #1458). Resets `snap_count` so
    /// the shift doesn't immediately trip the snap-escape counter.
    fn shift_octave(&mut self, direction: i32) {
        if !self.initialized {
            return;
        }
        self.state += direction as f64;
        self.snap_count = 0;
    }

    /// Update with a raw BPM measurement and confidence. Returns filtered BPM.
    fn update(&mut self, raw_bpm: f64, confidence: f64) -> f64 {
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

        // Octave-aware preprocessing: snap only true octave errors (2:1, 1:2)
        let ratio = raw_bpm / current_bpm;
        let mut snapped_bpm = raw_bpm;
        let mut was_snapped = false;
        for &hr in &[0.5, 2.0] {
            if (ratio - hr).abs() / hr < 0.05 {
                snapped_bpm = current_bpm;
                was_snapped = true;
                break;
            }
        }

        // Track consecutive snaps — if snapping for too long, the tempo may
        // have genuinely changed to the half/double. Accept raw measurement.
        if was_snapped {
            self.snap_count += 1;
            if self.snap_count >= 30 {
                log::debug!(
                    "Kalman snap escape: accepting {:.1} BPM after {} consecutive snaps",
                    raw_bpm,
                    self.snap_count
                );
                snapped_bpm = raw_bpm;
                // was_snapped is intentionally not read after this reassignment;
                // the snap_count reset handles the state change.
                self.snap_count = 0;
            }
        } else {
            self.snap_count = 0;
        }
        let snapped_measurement = snapped_bpm.log2();

        // Divergence detection: 5 consecutive frames >10% deviation -> hard reset
        let bpm_deviation = (snapped_bpm - current_bpm).abs() / current_bpm.max(1.0);
        if bpm_deviation > 0.10 {
            self.diverge_count += 1;
        } else {
            self.diverge_count = 0;
        }

        if self.diverge_count >= 15 {
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
        self.q = if self.diverge_count > 0 { 0.1 } else { 0.001 };

        // Kalman predict (constant model: state unchanged)
        self.variance += self.q;

        // Kalman update
        let innovation = snapped_measurement - self.state;
        let s = self.variance + self.r;
        let k = self.variance / s;
        self.state += k * innovation;
        self.variance *= 1.0 - k;

        2.0f64.powf(self.state)
    }
}

// ---------------------------------------------------------------------------
// Stage 2: FFT-based tempo estimation with Kalman tracking
// ---------------------------------------------------------------------------

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

        // Confidence gate: don't feed low-confidence garbage to the Kalman.
        // During noisy sections (off-beats, transitions), the autocorrelation
        // is unreliable and would corrupt the filter state.
        if confidence < 0.15 && self.current_bpm > 0.0 {
            self.current_confidence = confidence;
            let period_s = self.current_period_frames * self.frame_time;
            return (self.current_bpm, self.current_confidence, period_s);
        }

        // A7 (#1458): honour the user's octave override before filtering.
        let raw_bpm = self.apply_octave_offset(raw_bpm);

        // Kalman filter update
        let filtered_bpm = self.kalman.update(raw_bpm, confidence);

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

        // Extract autocorrelation up to 4*max_lag for harmonic scoring
        let acr_len = (4 * max_lag + 1).min(n).min(self.fft_size);
        let autocorr: Vec<f64> = buffer[..acr_len]
            .iter()
            .map(|c| c.re * scale / zero_lag)
            .collect();
        let acr_max = autocorr.len() - 1;

        // Find initial peak using raw autocorrelation (no prior bias)
        // Prior is applied only in the multi-ratio correction step
        let mut best_lag = min_lag;
        let mut best_value = f64::NEG_INFINITY;

        for lag in min_lag..=max_lag.min(acr_max) {
            if autocorr[lag] > best_value {
                best_value = autocorr[lag];
                best_lag = lag;
            }
        }

        // Multi-ratio octave correction
        // Check metrical ratios: for each, compute harmonic score weighted by tempo prior.
        // 1:3 and 1:4 are needed because the initial peak can land on the 3rd or 4th
        // subharmonic (e.g., lag 66 for a true period of 22), and without these ratios
        // the correction can only step down to 2T, never reaching T directly.
        let ratios: [(f64, f64); 9] = [
            (1.0, 4.0), // quarter lag -> 4x BPM
            (1.0, 3.0), // third lag -> 3x BPM
            (1.0, 2.0), // half lag -> double BPM
            (2.0, 3.0),
            (3.0, 4.0),
            (1.0, 1.0), // same
            (4.0, 3.0),
            (3.0, 2.0),
            (2.0, 1.0), // double lag -> half BPM
        ];

        let mut best_candidate_lag = best_lag;
        let mut best_score = f64::NEG_INFINITY;

        for &(num, den) in &ratios {
            let candidate_lag = ((best_lag as f64 * num / den).round() as usize).max(1);
            if candidate_lag < min_lag || candidate_lag > max_lag || candidate_lag > acr_max {
                continue;
            }

            // Harmonic score: base + sum(autocorr[h*lag] / h) for h=2,3,4
            let mut harmonic_score = autocorr[candidate_lag];
            for h in 2..=4usize {
                let h_lag = candidate_lag * h;
                if h_lag <= acr_max {
                    harmonic_score += autocorr[h_lag] / h as f64;
                }
            }

            let bpm = 60.0 / (candidate_lag as f64 * self.frame_time);
            let weight = self.tempo_prior_weight(bpm);
            let weighted_score = harmonic_score * weight;

            if weighted_score > best_score {
                best_score = weighted_score;
                best_candidate_lag = candidate_lag;
            }
        }

        if best_candidate_lag != best_lag {
            let old_bpm = 60.0 / (best_lag as f64 * self.frame_time);
            let new_bpm = 60.0 / (best_candidate_lag as f64 * self.frame_time);
            log::debug!(
                "Multi-ratio correction: lag {} ({:.1} BPM) -> lag {} ({:.1} BPM)",
                best_lag,
                old_bpm,
                best_candidate_lag,
                new_bpm
            );
        }

        best_lag = best_candidate_lag;

        // Parabolic interpolation for sub-frame precision
        let mut refined_lag = best_lag as f64;
        if best_lag > min_lag && best_lag < max_lag.min(acr_max) {
            let alpha = autocorr[best_lag - 1];
            let beta = autocorr[best_lag];
            let gamma = autocorr[best_lag + 1];
            let denom = 2.0 * (2.0 * beta - alpha - gamma);
            if denom.abs() > 1e-10 {
                let p = (alpha - gamma) / denom;
                refined_lag = best_lag as f64 + p.clamp(-0.5, 0.5);
            }
        }

        // Convert to BPM
        let period_s = refined_lag * self.frame_time;
        let bpm = if period_s > 0.0 { 60.0 / period_s } else { 0.0 };

        // Confidence: peak height relative to noise floor in the BPM range
        // Generalized autocorrelation (|FFT|^1) gives lower absolute values than
        // standard autocorrelation, so use relative measure instead
        let confidence = if best_lag <= acr_max {
            let range_end = max_lag.min(acr_max);
            let mut sorted_vals: Vec<f64> = autocorr[min_lag..=range_end].to_vec();
            sorted_vals
                .sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let noise_floor = sorted_vals[sorted_vals.len() / 2]; // median
            let peak = autocorr[best_lag];
            ((peak - noise_floor) / (1.0 - noise_floor).max(1e-6)).clamp(0.0, 1.0)
        } else {
            0.0
        };

        if bpm < self.bpm_range.0 as f64 || bpm > self.bpm_range.1 as f64 {
            return (0.0, 0.0, 0.0);
        }

        log::debug!(
            "Tempo estimate: {:.1} BPM (confidence {:.2}, lag {:.1})",
            bpm,
            confidence,
            refined_lag
        );

        (bpm, confidence, refined_lag)
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
// Stage 3: Beat scheduler (prediction + confirmation)
// ---------------------------------------------------------------------------

#[derive(PartialEq)]
enum BeatState {
    Waiting,
    Expecting,
    Confirmed,
    Missed,
}

struct BeatScheduler {
    beat_window: f64,
    refractory: f64,
    min_confidence: f64,
    phase_correction: f64,
    max_misses: u32,
    beat_timeout: f64,

    state: BeatState,
    last_beat_time: f64,
    next_predicted: f64,
    phase: f64,

    bpm: f64,
    period: f64,
    tempo_confidence: f64,

    beat_strength: f64,
    tracking_confidence: f64,
    consecutive_misses: u32,
    last_fired_time: f64,
    stored_period: f64,
}

impl BeatScheduler {
    fn new() -> Self {
        Self {
            beat_window: 0.08,
            refractory: 0.15,
            min_confidence: 0.4,
            phase_correction: 0.3,
            max_misses: 4,
            beat_timeout: 3.0,

            state: BeatState::Waiting,
            last_beat_time: 0.0,
            next_predicted: 0.0,
            phase: 0.0,

            bpm: 0.0,
            period: 0.0,
            tempo_confidence: 0.0,

            beat_strength: 0.0,
            tracking_confidence: 0.0,
            consecutive_misses: 0,
            last_fired_time: 0.0,
            stored_period: 0.0,
        }
    }

    fn update_tempo(&mut self, bpm: f64, period: f64, confidence: f64) {
        self.bpm = bpm;
        self.period = period;
        self.tempo_confidence = confidence;
        if confidence >= self.min_confidence && period > 0.0 {
            self.stored_period = period;
        }
    }

    /// Main beat decision. Returns (is_beat, beat_phase, smoothed_bpm).
    fn process(
        &mut self,
        is_onset: bool,
        onset_strength: f32,
        timestamp: f64,
        is_silence: bool,
    ) -> (bool, f64, f64) {
        let time_since_beat = if self.last_beat_time > 0.0 {
            timestamp - self.last_beat_time
        } else {
            f64::INFINITY
        };
        let in_refractory = time_since_beat < self.refractory;

        if is_onset {
            self.beat_strength = onset_strength as f64;
        }

        // Update phase and count missed predicted beats
        let missed = if self.period > 0.0 {
            self.update_phase(timestamp)
        } else {
            0
        };

        // Beat timeout recovery
        let time_since_fired = if self.last_fired_time > 0.0 {
            timestamp - self.last_fired_time
        } else {
            0.0
        };
        if time_since_fired > self.beat_timeout && self.last_fired_time > 0.0 {
            self.tracking_confidence *= 0.3;
            self.consecutive_misses = 0;
            self.state = BeatState::Waiting;
            self.last_fired_time = timestamp;
        }

        // Silence → pause beat detection
        if is_silence {
            if self.state != BeatState::Waiting {
                self.state = BeatState::Waiting;
                self.consecutive_misses = 0;
            }
            return (false, 0.0, self.bpm);
        }

        // Determine if we should fire a beat
        let is_beat = if missed > 0 && self.tempo_confidence >= self.min_confidence {
            self.beat_strength = 0.5;
            self.state = BeatState::Missed;
            true
        } else if self.tempo_confidence < self.min_confidence || self.period == 0.0 {
            // Low confidence: onset-only mode
            let mut beat = self.onset_only(is_onset, in_refractory);

            // Backup prediction
            if !beat && self.stored_period > 0.0 && self.last_fired_time > 0.0 {
                if time_since_fired >= self.stored_period * 0.9 && !in_refractory {
                    beat = true;
                    self.beat_strength = 0.5;
                }
            }
            beat
        } else {
            // High confidence: predictive mode
            self.predictive(is_onset, timestamp, in_refractory)
        };

        if is_beat {
            self.last_beat_time = timestamp;
            self.last_fired_time = timestamp;
            self.phase = 0.0;
            self.consecutive_misses = 0;
            if self.period > 0.0 {
                self.next_predicted = timestamp + self.period;
            }
            self.tracking_confidence = (self.tracking_confidence + 0.15).min(1.0);
        } else {
            self.tracking_confidence = (self.tracking_confidence - 0.0005).max(0.0);
        }

        (is_beat, self.phase, self.bpm)
    }

    fn onset_only(&mut self, is_onset: bool, in_refractory: bool) -> bool {
        self.state = BeatState::Waiting;
        if is_onset && !in_refractory {
            self.state = BeatState::Confirmed;
            return true;
        }
        false
    }

    fn predictive(&mut self, is_onset: bool, timestamp: f64, in_refractory: bool) -> bool {
        let dist = if self.next_predicted > 0.0 {
            timestamp - self.next_predicted
        } else {
            f64::INFINITY
        };
        let in_window = dist.abs() <= self.beat_window;
        let window_expired = dist > self.beat_window;

        match self.state {
            BeatState::Waiting => {
                if self.next_predicted == 0.0 {
                    if is_onset && !in_refractory {
                        self.next_predicted = timestamp + self.period;
                        self.state = BeatState::Confirmed;
                        return true;
                    }
                } else {
                    self.state = BeatState::Expecting;
                }
                false
            }
            BeatState::Expecting => {
                if is_onset && in_window && !in_refractory {
                    self.state = BeatState::Confirmed;
                    if self.period > 0.0 {
                        let phase_error = dist / self.period;
                        self.apply_phase_correction(phase_error);
                    }
                    return true;
                } else if window_expired {
                    self.state = BeatState::Missed;
                    return true;
                }
                false
            }
            BeatState::Confirmed | BeatState::Missed => {
                self.state = BeatState::Expecting;
                if is_onset && in_window && !in_refractory {
                    self.state = BeatState::Confirmed;
                    return true;
                }
                false
            }
        }
    }

    fn update_phase(&mut self, timestamp: f64) -> u32 {
        if self.last_beat_time == 0.0 || self.period == 0.0 {
            return 0;
        }

        let elapsed = timestamp - self.last_beat_time;
        self.phase = (elapsed / self.period) % 1.0;

        let mut missed = 0u32;
        while self.next_predicted > 0.0 && timestamp > self.next_predicted + self.beat_window {
            self.consecutive_misses += 1;
            missed += 1;
            self.next_predicted += self.period;
            if self.consecutive_misses == self.max_misses {
                self.tracking_confidence *= 0.7;
            }
        }
        missed
    }

    fn apply_phase_correction(&mut self, phase_error: f64) {
        let clamped = phase_error.clamp(-0.5, 0.5);
        let correction = clamped * self.period * self.phase_correction;
        self.next_predicted += correction;
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
        self.beat_scheduler.update_tempo(bpm, period_s, confidence);
        let (is_beat, beat_phase, smoothed_bpm) = self.beat_scheduler.process(
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
            bpm: if smoothed_bpm > 0.0 {
                smoothed_bpm as f32
            } else {
                0.0
            },
            beat_strength: if is_beat {
                self.beat_scheduler.beat_strength as f32
            } else {
                0.0
            },
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
        let bpm = k.update(120.0, 0.5);
        assert!(approx_eq_f64(bpm, 120.0, 1e-6));
    }

    #[test]
    fn kalman_stable_input_stays_near() {
        let mut k = KalmanBpm::new();
        k.update(120.0, 0.8);
        for _ in 0..50 {
            let bpm = k.update(120.0, 0.8);
            assert!((bpm - 120.0).abs() < 5.0, "got {}", bpm);
        }
    }

    #[test]
    fn kalman_octave_snap() {
        let mut k = KalmanBpm::new();
        k.update(120.0, 0.8);
        for _ in 0..10 {
            k.update(120.0, 0.8);
        }
        // Now feed 240 (octave double) — should snap back to 120
        let bpm = k.update(240.0, 0.8);
        assert!((bpm - 120.0).abs() < 10.0, "expected near 120, got {}", bpm);
    }

    #[test]
    fn kalman_octave_escape() {
        let mut k = KalmanBpm::new();
        k.update(120.0, 0.8);
        // Feed 240 for 60 frames — should eventually escape snap
        for _ in 0..60 {
            k.update(240.0, 0.8);
        }
        let bpm = k.update(240.0, 0.8);
        // After escape, should be near 240
        assert!((bpm - 240.0).abs() < 30.0, "expected near 240, got {}", bpm);
    }

    #[test]
    fn kalman_divergence_reset() {
        let mut k = KalmanBpm::new();
        k.update(120.0, 0.8);
        for _ in 0..5 {
            k.update(120.0, 0.8);
        }
        // Feed completely different BPM — should reset after 15 frames
        for _ in 0..20 {
            k.update(80.0, 0.8);
        }
        let bpm = k.update(80.0, 0.8);
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

    #[test]
    fn scheduler_zero_confidence_onset_fires() {
        let mut bs = BeatScheduler::new();
        bs.update_tempo(0.0, 0.0, 0.0);
        let (is_beat, _, _) = bs.process(true, 0.8, 1.0, false);
        assert!(is_beat);
    }

    #[test]
    fn scheduler_silence_no_beat() {
        let mut bs = BeatScheduler::new();
        bs.update_tempo(120.0, 0.5, 0.8);
        let (is_beat, phase, _) = bs.process(false, 0.0, 1.0, true);
        assert!(!is_beat);
        assert!(approx_eq_f64(phase, 0.0, 1e-6));
    }

    #[test]
    fn scheduler_phase_in_range() {
        let mut bs = BeatScheduler::new();
        bs.update_tempo(120.0, 0.5, 0.8);
        // Trigger a beat
        bs.process(true, 0.8, 1.0, false);
        // Advance time
        for i in 1..100 {
            let t = 1.0 + (i as f64) * 0.01;
            let (_, phase, _) = bs.process(false, 0.0, t, false);
            assert!((0.0..=1.0).contains(&phase), "phase={} at t={}", phase, t);
        }
    }

    // ---- Integration test ----

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
        let mut last_kick = -1.0f64;
        let num_frames = (duration_secs / dt) as usize;

        let mut last_bpm = 0.0f32;

        for frame in 0..num_frames {
            let t = frame as f64 * dt;

            let is_kick_frame = (t - last_kick) >= kick_interval - dt * 0.5;
            let mut bass = vec![0.001f32; bass_len];
            let mid = vec![0.001f32; mid_len];
            let high = vec![0.001f32; high_len];

            if is_kick_frame && t >= kick_interval {
                for bin in 1..12 {
                    bass[bin] = 2.0;
                }
                last_kick = t;
            }

            let result = detector.process(&bass, &mid, &high, t, false);
            last_bpm = result.bpm;
        }

        eprintln!(
            "BPM convergence: target={target_bpm}, detected={last_bpm}, duration={duration_secs}s"
        );
        last_bpm
    }

    #[test]
    fn bpm_converges_120() {
        let bpm = run_bpm_convergence_test(120.0, 8.0);
        assert!(
            bpm > 102.0 && bpm < 138.0,
            "120 BPM: expected 102-138, got {bpm}"
        );
    }

    #[test]
    fn bpm_converges_90() {
        let bpm = run_bpm_convergence_test(90.0, 10.0);
        assert!(
            bpm > 72.0 && bpm < 108.0,
            "90 BPM: expected 72-108, got {bpm}"
        );
    }

    #[test]
    fn bpm_converges_140() {
        let bpm = run_bpm_convergence_test(140.0, 10.0);
        assert!(
            bpm > 112.0 && bpm < 168.0,
            "140 BPM: expected 112-168, got {bpm}"
        );
    }

    #[test]
    fn bpm_converges_170() {
        let bpm = run_bpm_convergence_test(170.0, 10.0);
        assert!(
            bpm > 136.0 && bpm < 204.0,
            "170 BPM: expected 136-204, got {bpm}"
        );
    }

    #[test]
    fn bpm_converges_200() {
        let bpm = run_bpm_convergence_test(200.0, 10.0);
        assert!(
            bpm > 160.0 && bpm < 240.0,
            "200 BPM: expected 160-240, got {bpm}"
        );
    }

    #[test]
    fn bpm_converges_230() {
        let bpm = run_bpm_convergence_test(230.0, 10.0);
        // Accept 230 BPM or half-tempo 115 BPM (prior centered at 150 favors lower octave)
        let in_range = (bpm > 92.0 && bpm < 138.0) || (bpm > 184.0 && bpm < 276.0);
        assert!(in_range, "230 BPM: expected 92-138 or 184-276, got {bpm}");
    }

    #[test]
    fn bpm_no_octave_double_145() {
        let bpm = run_bpm_convergence_test(145.0, 10.0);
        assert!(
            bpm > 116.0 && bpm < 174.0,
            "145 BPM: expected 116-174 (not 290 octave double), got {bpm}"
        );
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
        // signal reads as 172 under the default prior and folds to half tempo under a prior
        // centred low. That is the prior steering metrical-ratio selection — the mechanism the
        // genre presets exist to drive.
        //
        // Note this harness feeds a clean impulse train, whose autocorrelation has no strong
        // half-tempo subharmonic, so it cannot reproduce the real-world 172->86 fold the task
        // describes (that needs a backbeat on 2 and 4). It proves the prior is wired and
        // effective; it does not prove the DnB preset fixes real DnB audio.
        let default_bpm = run_bpm_convergence_with(172.0, 10.0, TempoConfig::default());
        assert!(
            default_bpm > 155.0 && default_bpm < 190.0,
            "default prior should hold 172, got {default_bpm}"
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
            low_bpm > 78.0 && low_bpm < 95.0,
            "a prior centred at {center} should fold 172 to half tempo, got {low_bpm}"
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
