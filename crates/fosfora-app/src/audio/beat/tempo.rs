//! Stage 2: autocorrelation tempo estimation with Kalman tracking and octave anchoring.

use rustfft::FftPlanner;
use rustfft::num_complex::Complex;
use std::sync::Arc;

use super::buffer::CircularBuffer;
use super::control::{
    AUTO_PRIOR_MAX_BPM, AUTO_PRIOR_MIN_BPM, AUTO_PRIOR_MIN_CONFIDENCE, AUTO_PRIOR_RATE, BPM_MAX,
    BPM_MIN, MAX_PRIOR_SIGMA, MIN_PRIOR_SIGMA, TempoConfig, prior_center_log2,
};
use super::kalman::KalmanBpm;

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
pub(super) const LOCK_INNOVATION_GATE_LOG2: f64 = 0.057;
pub(super) const LOCK_INNOVATION_R_MULT: f64 = 6.0;
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
/// log2 of {2, ½, 3/2, ⅔, 4/3, ¾}. The two octaves come first (see [`is_octave_level`]).
const RELATED_RATIOS_LOG2: [f64; 6] = [
    1.0,
    -1.0,
    0.584_962_500_721_156,
    -0.584_962_500_721_156,
    0.415_037_499_278_844,
    -0.415_037_499_278_844,
];

/// Is `level` (an index into [`RELATED_RATIOS_LOG2`]) one of the octaves, ×2 or ×½?
fn is_octave_level(level: usize) -> bool {
    level < 2
}

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
pub(super) fn acf_at(acf: &[f64], x: f64) -> f64 {
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
pub(super) fn comb_score(acf: &[f64], lag: f64) -> f64 {
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

/// One tempo update's internals, for the dev-only beat sidecar (#4153). Filled
/// by plain reads of values the update computes anyway; nothing reads it back.
#[derive(Clone, Copy, Debug, Default)]
pub struct TempoDebug {
    /// Winner after the octave override.
    pub raw_bpm: f64,
    pub conf: f64,
    /// The winner's comb score before prior and continuity weighting.
    pub comb: f64,
    /// ACF at the winning lag, and the median floor confidence is judged against.
    pub peak: f64,
    pub floor: f64,
    /// ACF at 2x, 3x, 4x the winning lag; `None` past the end of the ACF.
    pub c2: Option<f64>,
    pub c3: Option<f64>,
    pub c4: Option<f64>,
    /// The measurement after folding onto the anchor level, and the filter output.
    pub meas_bpm: f64,
    pub filt_bpm: f64,
    pub support: f64,
    pub locked: bool,
    pub seeded: bool,
    pub anchor_bpm: Option<f64>,
    pub challenge: f64,
    pub pub_bpm: f64,
}

pub(super) struct TempoEstimator {
    bpm_range: (f32, f32),

    /// The SuperFlux combined flux, one value per hop: what the seed gate reads.
    onset_history: CircularBuffer,
    /// #4250: the beat activation, hop-aligned with `onset_history`. Once the flux has
    /// seeded, the activation's autocorrelation chooses the period (see [`Self::update`]).
    act_history: CircularBuffer,
    frame_rate: f64,
    frame_time: f64,
    frame_count: u32,

    pub(super) current_bpm: f64,
    current_confidence: f64,
    current_period_frames: f64,

    // FFT-based generalized autocorrelation
    fft_forward: Arc<dyn rustfft::Fft<f64>>,
    fft_inverse: Arc<dyn rustfft::Fft<f64>>,
    fft_size: usize,

    // Genre-aware tempo prior (log-Gaussian)
    prior_center_log2: f64,
    pub(super) prior_sigma: f64,
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
    pub(super) support: f64,
    /// Q3: hysteretic lock state derived from `support`.
    pub(super) locked: bool,
    /// Q2b: log2 of the earned tempo level. See [`ANCHOR_EARN_SECS`].
    pub(super) anchor_log2: Option<f64>,
    /// Q2b: hop stamp of the anchor earn — non-octave displacement is allowed
    /// within [`ANCHOR_PROBATION_SECS`] of it. `None` = no probation (tap).
    anchor_earned_at: Option<u32>,
    /// Q2b: (level, hop stamp) of the last abandoned anchor, kept only when
    /// it had [`GHOST_MIN_TENURE_SECS`]. See [`GHOST_ANCHOR_TTL_SECS`].
    pub(super) ghost_anchor: Option<(f64, u32)>,
    /// Q2b: hop stamp of the current anchor's original earn (displacement
    /// keeps the lineage) — the ghost veto's tenure clock.
    anchor_born_at: Option<u32>,
    /// Q2b: hop stamp of the false→true lock transition (anchor-earn clock).
    locked_since: Option<u32>,
    /// Q2b: EMA of confident winners at the one related level in
    /// `challenge_level`; switching levels restarts it.
    challenge: f64,
    /// Q2b: the level currently challenging, as an index into
    /// [`RELATED_RATIOS_LOG2`]; `None` until one has.
    challenge_level: Option<usize>,
    /// Q2b: hop stamp when `challenge` first cleared [`DISPLACE_CHALLENGE`].
    challenge_above_since: Option<u32>,
    /// Q2b (F3): last tempo published while locked — held on the wire through
    /// unlocks so the readout never blanks or glides mid-set.
    last_locked_bpm: f64,
    /// Buffers `compute_tempo` reuses across its every-few-hops runs (#75).
    acf: AcfScratch,
    /// #4153 sidecar: the last update's internals, whether the ACF ran this
    /// hop, and the state transitions since the last drain (name, extra).
    /// Events are only collected while `dbg` is set.
    pub(super) last_debug: TempoDebug,
    pub(super) ran_this_hop: bool,
    pub(super) dbg: bool,
    pub(super) dbg_events: Vec<(&'static str, f64)>,
}

/// Working storage for one autocorrelation tempo estimate, kept on the estimator so a
/// run allocates nothing after the first.
#[derive(Default)]
struct AcfScratch {
    history: Vec<f64>,
    fft_buf: Vec<Complex<f64>>,
    fft_scratch: Vec<Complex<f64>>,
    autocorr: Vec<f64>,
    peaks: Vec<(f64, f64)>,
    floor: Vec<f64>,
}

impl TempoEstimator {
    pub(super) fn new(history_seconds: f64, frame_rate: f64, config: TempoConfig) -> Self {
        let history_size = (history_seconds * frame_rate).ceil() as usize;
        let frame_time = 1.0 / frame_rate;

        let fft_size = (2 * history_size).next_power_of_two();
        let mut planner = FftPlanner::<f64>::new();
        let fft_forward = planner.plan_fft_forward(fft_size);
        let fft_inverse = planner.plan_fft_inverse(fft_size);

        Self {
            bpm_range: (BPM_MIN as f32, BPM_MAX as f32),
            onset_history: CircularBuffer::new(history_size),
            act_history: CircularBuffer::new(history_size),
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
            challenge_level: None,
            challenge_above_since: None,
            last_locked_bpm: 0.0,
            acf: AcfScratch::default(),
            last_debug: TempoDebug::default(),
            ran_this_hop: false,
            dbg: false,
            dbg_events: Vec::new(),
        }
    }

    /// #4153 sidecar event; a no-op unless `dbg` is set.
    fn dbg_event(&mut self, name: &'static str, extra: f64) {
        if self.dbg {
            self.dbg_events.push((name, extra));
        }
    }

    /// #4153: snapshot the update's outcome and the state it left behind.
    fn fill_debug(&mut self, raw_bpm: f64, conf: f64, meas_bpm: f64, filt_bpm: f64) {
        let pub_bpm = self.published_bpm();
        let d = &mut self.last_debug;
        d.raw_bpm = raw_bpm;
        d.conf = conf;
        d.meas_bpm = meas_bpm;
        d.filt_bpm = filt_bpm;
        d.support = self.support;
        d.locked = self.locked;
        d.seeded = self.kalman.initialized;
        d.anchor_bpm = self.anchor_log2.map(|a| 2.0f64.powf(a));
        d.challenge = self.challenge;
        d.pub_bpm = pub_bpm;
    }

    /// Apply live config from the shared [`TempoControl`] (A7 #1458). In auto mode the
    /// estimator owns `prior_center_bpm`, so the incoming centre is ignored — the audio
    /// thread publishes ours back instead (see `prior_center_bpm`).
    pub(super) fn set_config(&mut self, config: TempoConfig) {
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
    pub(super) fn prior_center_bpm(&self) -> f32 {
        2.0f64.powf(self.prior_center_log2) as f32
    }

    /// Q2b (F3): tempo for the wire — the filtered value while locked, held at
    /// the last locked value through unlocks (breakdowns, re-acquisition
    /// wobble), and 0.0 only before the first lock is ever earned. The readout
    /// never blanks or glides mid-set.
    pub(super) fn published_bpm(&self) -> f64 {
        if self.locked {
            self.current_bpm
        } else {
            self.last_locked_bpm
        }
    }

    /// A7 (#1458): force the reported tempo up/down an octave. Shifts both the offset
    /// (so it sticks) and the filter state (so the readout moves now, not after the
    /// filter reconverges). Rejected when the result would leave the BPM range.
    pub(super) fn shift_octave(&mut self, direction: i32) {
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
    pub(super) fn tap(&mut self, bpm: f64) {
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

    /// Update the tempo estimate from the hop's combined flux (`onset_value`) and beat
    /// activation (`act_value`). Returns (bpm, confidence, period_seconds).
    ///
    /// #4250: two autocorrelations, two jobs. The flux's ACF decides WHEN a tempo is
    /// earned (the seed gate below; its path is unchanged from before the round, so
    /// beatless audio seeds exactly as it did). Once seeded, the activation's ACF
    /// decides WHAT period the measurement carries, with its own confidence, and the
    /// flux's reading is the fallback whenever the activation has no in-range peak.
    /// The activation alone must never seed: it is sparse on beatless material, and a
    /// zero-lag-normalized ACF reads that sparse texture as periodic.
    ///
    /// A5 (#1456): the analysis hop is fixed (`ANALYSIS_HOP` samples), so `frame_rate` and
    /// `frame_time` are exact from construction — the old runtime re-estimation of frame
    /// timing from wall-clock `timestamp` deltas (a workaround for the jittery variable
    /// hop) is gone, and with it this stage's dependence on `timestamp`.
    pub(super) fn update(&mut self, onset_value: f64, act_value: f64) -> (f64, f64, f64) {
        self.onset_history.push(onset_value);
        self.act_history.push(act_value);
        self.frame_count += 1;
        self.ran_this_hop = false;

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
        self.ran_this_hop = true;

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
                self.fill_debug(raw_bpm, confidence, 0.0, 0.0);
                return (0.0, self.current_confidence, 0.0);
            }
            self.pending_seed = None;
            self.dbg_event("seed", f64::NAN);
        }

        // #4250: the flux decided WHEN (the gate above); from here the activation's ACF
        // decides WHAT period, with the flux's reading as the fallback when it has no
        // in-range peak. The octave offset was already walked above, so it is applied
        // here as a plain factor.
        let (raw_bpm, confidence) = {
            let (a_bpm, a_conf, _) = self.compute_tempo_activation();
            let a_bpm = a_bpm * 2.0f64.powi(self.octave_offset);
            if a_bpm >= self.bpm_range.0 as f64 && a_bpm <= self.bpm_range.1 as f64 {
                (a_bpm, a_conf)
            } else {
                (raw_bpm, confidence)
            }
        };

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
                } else if let Some(level) = RELATED_RATIOS_LOG2
                    .iter()
                    .position(|r| (d - r).abs() <= RELATED_TOL_LOG2)
                {
                    measurement_bpm = 2.0f64.powf(raw_bpm.log2() - RELATED_RATIOS_LOG2[level]);
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
                    if in_probation && !is_octave_level(level) {
                        if self.challenge_level != Some(level) {
                            self.challenge = 0.0;
                            self.challenge_level = Some(level);
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
                // `challenge` only rises once a level is set, so `level` is
                // always present here.
                if f64::from(self.frame_count.wrapping_sub(since)) * self.frame_time
                    >= DISPLACE_SPAN_SECS
                    && filtered_bpm > 0.0
                    && let Some(level) = self.challenge_level
                {
                    // Displace from the ANCHOR, not the filter state — the
                    // anchor is the slow consensus; the filter can be dragged
                    // a few percent by transition mush, and the challenger's
                    // claim is "the true level is anchor × 2^r".
                    let new_bpm = 2.0f64.powf(
                        self.anchor_log2.unwrap_or(filtered_bpm.log2())
                            + RELATED_RATIOS_LOG2[level],
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
                    self.dbg_event("anchor_displaced", f64::NAN);
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
                self.dbg_event("anchor_abandoned", f64::NAN);
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
                    self.dbg_event("unlock", f64::NAN);
                }
            } else if self.support > LOCK_ENTER_SUPPORT {
                self.locked = true;
                self.dbg_event("lock", f64::NAN);
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
                    self.dbg_event("anchor_earned", f64::NAN);
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

        self.fill_debug(raw_bpm, confidence, measurement_bpm, filtered_bpm);
        let period_s = self.current_period_frames * self.frame_time;
        (self.current_bpm, self.current_confidence, period_s)
    }

    fn compute_tempo(&mut self) -> (f64, f64, f64) {
        let mut acf = std::mem::take(&mut self.acf);
        let result = self.compute_tempo_with(&mut acf);
        self.acf = acf;
        result
    }

    fn compute_tempo_with(&mut self, acf: &mut AcfScratch) -> (f64, f64, f64) {
        self.onset_history.copy_values_into(&mut acf.history);
        self.compute_tempo_on(acf)
    }

    /// #4250: the same selection (peaks x ratios x comb x prior) over the activation
    /// history.
    fn compute_tempo_activation(&mut self) -> (f64, f64, f64) {
        let mut acf = std::mem::take(&mut self.acf);
        self.act_history.copy_values_into(&mut acf.history);
        let result = self.compute_tempo_on(&mut acf);
        self.acf = acf;
        result
    }

    /// One ACF tempo estimate over whatever `acf.history` holds.
    fn compute_tempo_on(&mut self, acf: &mut AcfScratch) -> (f64, f64, f64) {
        // #4153: an early return leaves the ACF fields at zero, not stale.
        self.last_debug = TempoDebug::default();
        let history = &acf.history;
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
        let buffer = &mut acf.fft_buf;
        buffer.clear();
        buffer.extend(history.iter().map(|&v| Complex::new(v - mean, 0.0)));
        buffer.resize(self.fft_size, Complex::new(0.0, 0.0));
        let scratch_len = self
            .fft_forward
            .get_inplace_scratch_len()
            .max(self.fft_inverse.get_inplace_scratch_len());
        acf.fft_scratch.resize(scratch_len, Complex::new(0.0, 0.0));

        self.fft_forward
            .process_with_scratch(buffer, &mut acf.fft_scratch);

        // Power spectrum |X|^2 — standard autocorrelation (Wiener-Khinchin)
        for c in buffer.iter_mut() {
            let power = c.norm_sqr();
            *c = Complex::new(power, 0.0);
        }

        self.fft_inverse
            .process_with_scratch(buffer, &mut acf.fft_scratch);

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
        let autocorr = &mut acf.autocorr;
        autocorr.clear();
        autocorr.extend(
            buffer[..acr_len]
                .iter()
                .enumerate()
                .map(|(lag, c)| (c.re * scale / zero_lag) * (n as f64 / (n - lag) as f64).min(4.0)),
        );
        let autocorr = &*autocorr;
        let acr_max = autocorr.len() - 1;

        // Q2: candidates live in CONTINUOUS lag from here on. Adjacent-bin
        // spacing is 1/lag — 2.5-3.4% at EDM tempi against the 4% Acc1
        // tolerance — so integer-rounded candidate lags could not represent the
        // true tempo at all. The parabolic vertex is only valid at a true local
        // max, so refine each ACF peak at its own bin, then project metrical
        // ratios in float.
        let range_end = max_lag.min(acr_max.saturating_sub(1));
        let peaks = &mut acf.peaks; // (refined lag, refined height)
        peaks.clear();
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

        let mut best: Option<(f64, f64, f64)> = None; // (lag, weighted score, comb)
        for &(peak_lag, _) in peaks.iter() {
            for &(num, den) in &ratios {
                let cand = peak_lag * num / den;
                if cand < min_lag_f || cand > max_lag_f {
                    continue;
                }
                let bpm = 60.0 / (cand * self.frame_time);
                let comb = comb_score(autocorr, cand);
                let weighted = comb * self.tempo_prior_weight(bpm) * self.continuity_weight(bpm);
                if best.is_none_or(|(_, b, _)| weighted > b) {
                    best = Some((cand, weighted, comb));
                }
            }
        }
        let Some((best_lag_f, _, best_comb)) = best else {
            return (0.0, 0.0, 0.0);
        };
        self.last_debug.comb = best_comb;

        let bpm = 60.0 / (best_lag_f * self.frame_time);

        // Confidence: the winner's interpolated height relative to the median
        // floor across the candidate range.
        let confidence = {
            let range_end = max_lag.min(acr_max);
            let floor = &mut acf.floor;
            floor.clear();
            floor.extend_from_slice(&autocorr[min_lag..=range_end]);
            let mid = floor.len() / 2;
            // Upper median, by selection rather than a sort.
            let noise_floor = *floor.select_nth_unstable_by(mid, f64::total_cmp).1;
            let peak = acf_at(autocorr, best_lag_f);
            self.last_debug.peak = peak;
            self.last_debug.floor = noise_floor;
            ((peak - noise_floor) / (1.0 - noise_floor).max(1e-6)).clamp(0.0, 1.0)
        };
        // #4153: the winner's harmonics, where the ACF reaches them.
        let harmonic = |h: f64| {
            let x = best_lag_f * h;
            (x <= acr_max as f64).then(|| acf_at(autocorr, x))
        };
        self.last_debug.c2 = harmonic(2.0);
        self.last_debug.c3 = harmonic(3.0);
        self.last_debug.c4 = harmonic(4.0);

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
    pub(super) fn tempo_prior_weight(&self, bpm: f64) -> f64 {
        if bpm <= 0.0 {
            return 0.0;
        }
        let log2_bpm = bpm.log2();
        let diff = log2_bpm - self.prior_center_log2;
        (-0.5 * (diff / self.prior_sigma).powi(2)).exp()
    }
}
