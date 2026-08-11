//! Song-structure detection: section novelty, build-up, and drop (A18 #1469).
//!
//! Nothing else in the engine sees structure beyond a single beat, so the drop and section
//! changes — the most valuable VJ moments — have to be hand-triggered. This stage adds a
//! cheap, decimated (~10 Hz) analysis on top of the per-hop features:
//!
//! - `section_novelty` — a **Foote** self-similarity novelty. Each tick appends a compact
//!   timbre vector (7 bands + MFCC 1–8, each block RMS-equalized then unit-normalized) to a
//!   60 s ring; a Gaussian-tapered **checkerboard kernel** slid along the self-similarity
//!   diagonal peaks where the block structure changes (a new section). Causal, so it reports
//!   a boundary ~`KERNEL_SECONDS` after it happens. Normalized **absolutely**, by the
//!   kernel's own weight — steady material reads near zero rather than saturating (#2080).
//! - `boundary` — a 1-tick pulse carrying confidence, from a causal peak-pick over that
//!   curve. This is what turns the novelty into events: the curve alone was computed and
//!   published for four releases without anything ever thresholding it, which is why the
//!   `/section` stream emitted 3.7 boundaries against 12.3 references. The boundary it
//!   reports sits [`BOUNDARY_LAG_SECONDS`] in the past, published on the wire alongside it.
//! - `buildup` — a logistic combination of loudness rise (A10 `loudness_trend`), spectral
//!   **brightening** (centroid rise), **onset-density** rise (A6 onset stream), and sub-bass
//!   **withdrawal** (the classic EDM pre-drop high-pass sweep). A superb global-intensity
//!   driver (auto camera push-in, tension).
//! - `drop` — a 1-frame pulse: fires when `buildup` has been sustained high, then a broadband
//!   loudness jump lands together with the sub-bass returning; 16 s refractory afterward.
//!   Counter-latched by the audio thread (like `beat`/`downbeat`) so it can't be missed.
//!
//! Reads the **pre-normalization** features (the adaptive normalizer would flatten exactly
//! the loudness/sub-bass dynamics this stage keys on) plus the beat result. Fills three
//! reserved shader fields with **zero ABI churn** (#1505). The hot-loop weights and drop
//! thresholds are exposed as a runtime-tunable [`StructureConfig`] (audio-panel sliders,
//! #1510); the sizing windows (ring/kernel/tick/baseline) stay compile-time consts because
//! they allocate. Heuristics tuned for electronic music.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::beat::BeatResult;
use super::features::AudioFeatures;

/// Compact timbre-vector dimension: 7 frequency bands + MFCC coefficients 1..=8.
const VEC_DIM: usize = 15;
/// Heavy-analysis rate (decimated from the ~86 Hz analysis frame rate).
const TICK_HZ: f32 = 10.0;
/// Self-similarity ring length (novelty context window), seconds.
const RING_SECONDS: f32 = 60.0;
/// Foote checkerboard-kernel half-width, seconds. Also the causal latency of
/// `section_novelty`; a modest value trades boundary sharpness for lower latency.
const KERNEL_SECONDS: f32 = 3.0;

/// Per-block scale equalization for the timbre vector (#2204).
///
/// The seven bands and MFCC 1..=8 live on wildly different scales — measured per-dim RMS
/// over 120 Harmonix tracks is 0.342 for the bands against 6.197 for the MFCCs. Unit-norming
/// the raw concatenation therefore hands MFCC ~97% of the vector's energy and the bands are
/// decoration: the "timbre vector" was an MFCC-only detector wearing seven extra dimensions.
///
/// Dividing each block by its own measured RMS before the unit-norm makes the weights below
/// mean what they say. The constants are fixed rather than tracked: across those 120 tracks
/// the per-block RMS varies with coefficient of variation 0.14 (bands) and 0.13 (MFCC), so
/// these are properties of the feature definitions, not of the music — and a fixed scale
/// keeps the detector deterministic, which the offline bench and the loop-export golden gate
/// both rely on.
const BAND_RMS: f32 = 0.342;
const MFCC_RMS: f32 = 6.197;
/// Block weights after equalization, chosen on the Q4 tune half (`bench/sweep_structure.py
/// --stage blocks`). Equal weighting wins; the two blocks carry comparable and partly
/// independent boundary information once they are on the same scale.
const W_BANDS: f32 = 1.0;
const W_MFCC: f32 = 1.0;

/// Peak confirmation half-width, seconds: how long the detector waits before calling a
/// novelty sample a local maximum.
///
/// Deliberately independent of [`KERNEL_SECONDS`]. The kernel's own centring lag is
/// irreducible — the novelty value produced at tick `t` describes the material at
/// `t - KERNEL_SECONDS` — but the confirmation delay is a free parameter, and tying the two
/// together would double the announcement latency for no accuracy. Total detection lag is
/// `KERNEL_SECONDS + CONFIRM_SECONDS`, published on the wire so a consumer can back-date.
const CONFIRM_SECONDS: f32 = 3.0;
/// Trailing window the boundary threshold's mean/stddev are measured over, seconds.
const PEAK_STAT_SECONDS: f32 = 45.0;
/// A peak must clear `mean + PEAK_SIGMA * stddev` of that trailing window.
const PEAK_SIGMA: f32 = 1.0;
/// Minimum musical distance between two boundaries, seconds. Shorter than this is a fill or
/// a turnaround, not a section (`analyze::structure_offline` uses the same idea at 8 s).
const MIN_SECTION_SECONDS: f32 = 6.0;
/// Divisor mapping `(peak - mean) / stddev` into the reported 0..1 confidence, so 8 sigma
/// of excess reads full confidence.
///
/// Calibrated, not guessed: over 948 boundaries on the tune half the sigma excess runs p25
/// 1.8 / p50 3.0 / p95 7.8, so a 4-sigma span (the obvious first choice) pinned 34% of
/// events at exactly 1.0 and threw away the discrimination. At 8 only 3.9% saturate.
///
/// And the value earns its name. Over the 4,144 boundaries this detector emits across the
/// full 374-track Harmonix set, precision rises monotonically with it: .384 in the bottom
/// third, .493 in the middle, .656 in the top. Gating at 0.5 keeps 31% of events and lifts
/// precision .511 -> .664.
const CONF_SIGMA_SPAN: f32 = 8.0;
/// Display gain for the bindable `section_novelty`, so the feature keeps a usable 0..1 range.
///
/// The absolute novelty is genuinely small on real music — pooled over 60 Harmonix tracks
/// its median is 0.003 and its p95 is 0.024 — because a unit-norm self-similarity block is
/// mostly diagonal. 33 puts that p95 at 0.80, leaving the median near 0.10: steady material
/// sits low (which is the entire point of dropping the running max) and only genuine change
/// approaches the top. 3.6% of frames clamp, and they are section changes and transients,
/// not stable passages.
///
/// Applied to the PUBLISHED feature only. The peak-picker reads the unclamped, ungained
/// curve: it thresholds on `mean + sigma*stddev`, which is scale-invariant, but clamping is
/// not — a clamped peak is indistinguishable from any other clamped peak, which would blind
/// the local-max test exactly on the strongest boundaries.
const NOVELTY_GAIN: f32 = 33.0;

/// Long window (seconds) for the build-up slope/decline references.
const SLOPE_SECONDS: f32 = 8.0;
/// Fast onset-density EMA window (seconds); its excess over the slow one is "onsets rising".
const ONSET_FAST_SECONDS: f32 = 1.0;
/// Decay window (seconds) for the sub-bass reference peak (the drop's "return" target).
const SUBBASS_REF_SECONDS: f32 = 10.0;
/// Build-up output smoothing (EMA) time constant, seconds.
const BUILD_TAU: f32 = 0.5;
/// Gains mapping the raw centroid / onset rises into ~0..1 before weighting.
const CENTROID_RISE_GAIN: f32 = 6.0;
const ONSET_RISE_GAIN: f32 = 4.0;

/// Build-up logistic: `buildup = σ(BIAS + Σ wᵢ·fᵢ)`, each fᵢ in ~0..1.
const BUILD_BIAS: f32 = -2.2;
const BUILD_W_LOUD: f32 = 2.2;
const BUILD_W_CENTROID: f32 = 1.4;
const BUILD_W_ONSET: f32 = 1.2;
const BUILD_W_SUBBASS: f32 = 1.6;

/// Drop state machine.
/// `buildup` must exceed this...
const DROP_ARM_BUILDUP: f32 = 0.6;
/// ...continuously for at least this long (seconds) to arm the drop.
const DROP_ARM_SUSTAIN: f32 = 4.0;
/// Window (seconds) the loudness-jump baseline (a running minimum) spans — roughly two
/// beats at typical tempo.
const DROP_BASELINE_SECONDS: f32 = 1.5;
/// Loudness jump (in `loudness_m`'s 0..1 = −60..0 LUFS mapping) that counts as a drop. 0.08
/// ≈ 5 LU; a real drop is a broadband loudness leap.
const DROP_LOUD_JUMP: f32 = 0.08;
/// Sub-bass must return to at least this fraction of its reference peak at the drop.
const DROP_SUBBASS_RETURN: f32 = 0.5;
/// No further drop for this long (seconds) after one fires.
const DROP_REFRACTORY: f32 = 16.0;

/// Runtime-tunable A18 thresholds (task #1510). Only the hot-loop build-up weights and
/// drop-machine thresholds are exposed — a VJ tunes build-up sensitivity and drop firing
/// live from the audio panel. The sizing consts (`TICK_HZ`, `RING_SECONDS`, `KERNEL_SECONDS`,
/// `DROP_BASELINE_SECONDS`) are *not* here: they allocate rings/kernels at construction, so
/// changing them needs a rebuild, not a per-tick read. Shared with the audio thread via
/// `Arc<Mutex<_>>` and snapshotted once per hop (no pipeline rebuild). Defaults mirror the
/// module consts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StructureConfig {
    /// Build-up logistic bias (base tension; more negative = harder to trigger build-up).
    pub buildup_bias: f32,
    /// Build-up weight on loudness rise (A10 `loudness_trend`).
    pub buildup_w_loud: f32,
    /// Build-up weight on spectral brightening (centroid rise).
    pub buildup_w_centroid: f32,
    /// Build-up weight on onset-density rise.
    pub buildup_w_onset: f32,
    /// Build-up weight on sub-bass withdrawal (the EDM high-pass sweep).
    pub buildup_w_subbass: f32,
    /// Drop arm: build-up level that must be sustained to arm the drop.
    pub drop_arm_buildup: f32,
    /// Drop arm: seconds build-up must stay above the arm level.
    pub drop_arm_sustain: f32,
    /// Drop fire: broadband loudness jump (0..1 = −60..0 LUFS; 0.08 ≈ 5 LU).
    pub drop_loud_jump: f32,
    /// Drop fire: fraction of the sub-bass reference peak that must return.
    pub drop_subbass_return: f32,
    /// Drop: seconds of suppression after one fires (refractory).
    pub drop_refractory: f32,
}

impl Default for StructureConfig {
    fn default() -> Self {
        Self {
            buildup_bias: BUILD_BIAS,
            buildup_w_loud: BUILD_W_LOUD,
            buildup_w_centroid: BUILD_W_CENTROID,
            buildup_w_onset: BUILD_W_ONSET,
            buildup_w_subbass: BUILD_W_SUBBASS,
            drop_arm_buildup: DROP_ARM_BUILDUP,
            drop_arm_sustain: DROP_ARM_SUSTAIN,
            drop_loud_jump: DROP_LOUD_JUMP,
            drop_subbass_return: DROP_SUBBASS_RETURN,
            drop_refractory: DROP_REFRACTORY,
        }
    }
}

/// Per-frame structure outputs, copied onto `AudioFeatures`.
pub struct StructureResult {
    /// Section-boundary novelty, absolute 0..1 (see [`StructureTracker::foote_novelty`]).
    pub section_novelty: f32,
    /// Build-up / tension estimate, 0..1.
    pub buildup: f32,
    /// 1.0 on the frame a drop is detected, else 0.0 (a trigger, counter-latched upstream).
    pub drop: f32,
    /// Confidence 0..1 on the tick a section boundary is confirmed, else 0.0. A trigger,
    /// like `drop` — the boundary it reports happened [`BOUNDARY_LAG_SECONDS`] earlier.
    pub boundary: f32,
}

/// How far in the past a confirmed boundary actually sits, seconds. The kernel's centring
/// lag plus the peak-confirmation delay; both are constants, so this is exact rather than
/// estimated, and it is published on the wire so consumers can place the boundary in
/// musical time instead of at the moment of announcement.
pub const BOUNDARY_LAG_SECONDS: f32 = KERNEL_SECONDS + CONFIRM_SECONDS;

pub struct StructureTracker {
    tick_interval: f64,
    /// Precomputed checkerboard kernel `K(i,j) = g(i,j)·sgn(i)·sgn(j)` over `-L..=L`,
    /// row-major of side `2L+1`.
    kernel: Vec<f32>,
    kernel_half: usize,
    /// Reciprocal of the kernel's total absolute weight — the absolute novelty's divisor.
    kernel_weight_recip: f32,
    ring: VecDeque<[f32; VEC_DIM]>,
    ring_cap: usize,
    cur_novelty: f32,

    // Boundary peak-picking. `nov_hist` is the trailing absolute-novelty curve; the
    // candidate sits `confirm_ticks` back from its newest entry so the local-max test only
    // ever reads samples the detector has already seen.
    nov_hist: VecDeque<f32>,
    nov_hist_cap: usize,
    confirm_ticks: usize,
    /// Sample-clock time of the last confirmed boundary (its back-dated musical time).
    last_boundary_time: f64,

    // Build-up references (updated every frame).
    onset_fast: f32,
    onset_slow: f32,
    centroid_slow: f32,
    subbass_slow: f32,
    subbass_ref: f32,
    buildup_ema: f32,
    cur_buildup: f32,

    // Drop state machine (updated every tick).
    high_duration: f32,
    loud_ring: VecDeque<f32>,
    loud_ring_cap: usize,
    refractory_until: f64,

    last_frame_time: f64,
    last_tick_time: f64,
    started: bool,
    /// Live-tunable thresholds, refreshed from the shared config each `process` call (#1510).
    cfg: StructureConfig,
}

impl StructureTracker {
    /// `hop_rate_hz` is the analysis frame rate (`sr / ANALYSIS_HOP`), used to size the
    /// frame-rate loudness-baseline ring.
    pub fn new(hop_rate_hz: f32) -> Self {
        let hop = hop_rate_hz.max(1.0);
        let kernel_half = (KERNEL_SECONDS * TICK_HZ).round().max(1.0) as usize;
        let side = 2 * kernel_half + 1;
        let sigma = kernel_half as f32 / 2.0;
        let mut kernel = vec![0.0f32; side * side];
        for di in 0..side {
            for dj in 0..side {
                let i = di as isize - kernel_half as isize;
                let j = dj as isize - kernel_half as isize;
                let g = (-((i * i + j * j) as f32) / (2.0 * sigma * sigma)).exp();
                kernel[di * side + dj] = g * sign(i) * sign(j);
            }
        }
        let kernel_weight = kernel.iter().map(|k| k.abs()).sum::<f32>().max(1e-6);
        let confirm_ticks = (CONFIRM_SECONDS * TICK_HZ).round().max(1.0) as usize;
        let nov_hist_cap = (PEAK_STAT_SECONDS * TICK_HZ) as usize + confirm_ticks + 1;
        Self {
            tick_interval: (1.0 / TICK_HZ) as f64,
            kernel,
            kernel_half,
            kernel_weight_recip: 1.0 / kernel_weight,
            ring: VecDeque::with_capacity((RING_SECONDS * TICK_HZ) as usize + 1),
            ring_cap: (RING_SECONDS * TICK_HZ) as usize,
            cur_novelty: 0.0,
            nov_hist: VecDeque::with_capacity(nov_hist_cap),
            nov_hist_cap,
            confirm_ticks,
            last_boundary_time: f64::NEG_INFINITY,
            onset_fast: 0.0,
            onset_slow: 0.0,
            centroid_slow: 0.0,
            subbass_slow: 0.0,
            subbass_ref: 0.0,
            buildup_ema: 0.0,
            cur_buildup: 0.0,
            high_duration: 0.0,
            loud_ring: VecDeque::new(),
            loud_ring_cap: (DROP_BASELINE_SECONDS * hop).round().max(1.0) as usize,
            refractory_until: 0.0,
            last_frame_time: -1.0,
            last_tick_time: -1.0,
            started: false,
            cfg: StructureConfig::default(),
        }
    }

    /// Called every audio frame with the **pre-normalization** features and the beat result.
    /// Cheap per-frame references update every call; the Foote novelty, build-up logistic and
    /// drop machine run only on the decimated ~`TICK_HZ` tick (their outputs are held between).
    pub fn process(
        &mut self,
        cfg: StructureConfig,
        pre_norm: &AudioFeatures,
        beat: &BeatResult,
        timestamp: f64,
    ) -> StructureResult {
        self.cfg = cfg;
        let frame_dt = if self.started {
            (timestamp - self.last_frame_time).clamp(0.0, 0.1) as f32
        } else {
            1.0 / TICK_HZ
        };
        self.last_frame_time = timestamp;
        self.started = true;
        self.update_refs(pre_norm, beat, frame_dt);

        let mut drop = 0.0;
        let mut boundary = 0.0;
        if self.last_tick_time < 0.0 || timestamp - self.last_tick_time >= self.tick_interval {
            self.last_tick_time = timestamp;
            let ticked = self.tick(pre_norm, timestamp);
            drop = ticked.0;
            boundary = ticked.1;
        }

        StructureResult {
            section_novelty: self.cur_novelty,
            buildup: self.cur_buildup,
            drop,
            boundary,
        }
    }

    /// Per-frame reference EMAs feeding the build-up features.
    fn update_refs(&mut self, pre_norm: &AudioFeatures, beat: &BeatResult, dt: f32) {
        let a_fast = 1.0 - (-dt / ONSET_FAST_SECONDS).exp();
        let a_slow = 1.0 - (-dt / SLOPE_SECONDS).exp();
        self.onset_fast += (beat.onset_strength - self.onset_fast) * a_fast;
        self.onset_slow += (beat.onset_strength - self.onset_slow) * a_slow;
        self.centroid_slow += (pre_norm.centroid - self.centroid_slow) * a_slow;
        self.subbass_slow += (pre_norm.sub_bass - self.subbass_slow) * a_slow;
        // Sub-bass reference: a slowly-decaying peak (the level the drop's sub-bass returns to).
        let decay = (-dt / SUBBASS_REF_SECONDS).exp();
        self.subbass_ref = (self.subbass_ref * decay).max(pre_norm.sub_bass);
    }

    /// Returns `(drop pulse, boundary confidence)` for this tick.
    fn tick(&mut self, pre_norm: &AudioFeatures, timestamp: f64) -> (f32, f32) {
        // --- section_novelty (Foote) ---
        let v = timbre_vector(pre_norm);
        if self.ring.len() == self.ring_cap {
            self.ring.pop_front();
        }
        self.ring.push_back(v);
        // Absolute normalization: divide by the kernel's own total weight. The ring vectors
        // are unit-norm, so every similarity is in -1..1 and the kernel-weighted sum is
        // bounded by that weight — this is a real curve, and steady material reads near
        // zero. The previous decaying-running-max normalization saturated to 1.0 in exactly
        // that case (#1973), which is why nothing downstream could threshold it.
        let abs_novelty = self.foote_novelty() * self.kernel_weight_recip;
        self.cur_novelty = (abs_novelty * NOVELTY_GAIN).clamp(0.0, 1.0);

        // --- buildup (logistic, EMA-smoothed at tick rate) ---
        let build_raw = self.buildup_logistic(pre_norm);
        let a = 1.0 - (-(1.0 / TICK_HZ) / BUILD_TAU).exp();
        self.buildup_ema += (build_raw - self.buildup_ema) * a;
        self.cur_buildup = self.buildup_ema;

        let boundary = self.update_boundary(abs_novelty, timestamp);

        // --- drop state machine ---
        (self.update_drop(pre_norm, timestamp), boundary)
    }

    /// Causal peak-pick over the absolute novelty curve. Returns the boundary confidence on
    /// the tick a peak is confirmed, else 0.0.
    ///
    /// The candidate is `confirm_ticks` behind the newest sample, so its local-max test reads
    /// only samples already observed. It must also clear `mean + PEAK_SIGMA * stddev` of the
    /// trailing window — an adaptive floor, so a busy track needs a bigger jump than a sparse
    /// one — and sit at least `MIN_SECTION_SECONDS` after the last boundary in musical time.
    fn update_boundary(&mut self, abs_novelty: f32, timestamp: f64) -> f32 {
        // Only record ticks whose novelty is real. Before the ring holds a full kernel,
        // `foote_novelty` returns a placeholder 0.0 that means "not computed yet", not
        // "nothing is happening" — feeding those into the trailing mean and stddev would
        // let the first 45 s of every track be thresholded against a fiction.
        if self.ring.len() < 2 * self.kernel_half + 1 {
            return 0.0;
        }
        if self.nov_hist.len() == self.nov_hist_cap {
            self.nov_hist.pop_front();
        }
        self.nov_hist.push_back(abs_novelty);

        let n = self.nov_hist.len();
        // Need the candidate plus a full confirmation window on each side of it, and enough
        // history for the trailing statistics to mean anything.
        if n < 2 * self.confirm_ticks + 20 {
            return 0.0;
        }
        let cand = n - 1 - self.confirm_ticks;
        let v = self.nov_hist[cand];

        // Trailing mean/stddev up to and including the candidate. Two passes over the
        // deque rather than a collected slice: this runs on the analysis thread, where a
        // per-tick heap allocation is exactly the kind of thing the zero-alloc probes exist
        // to catch.
        let count = (cand + 1) as f32;
        let mean = self.nov_hist.iter().take(cand + 1).sum::<f32>() / count;
        let var = self
            .nov_hist
            .iter()
            .take(cand + 1)
            .map(|x| (x - mean) * (x - mean))
            .sum::<f32>()
            / count;
        let std = var.sqrt();
        if std < 1e-9 || v < mean + PEAK_SIGMA * std {
            return 0.0;
        }
        // Local maximum over the confirmation window.
        let lo = cand.saturating_sub(self.confirm_ticks);
        for i in lo..n {
            if self.nov_hist[i] > v {
                return 0.0;
            }
        }
        // Back-date to the boundary's musical time before the dwell test, so the minimum
        // section length is measured in the music rather than in announcements.
        let boundary_time = timestamp - f64::from(BOUNDARY_LAG_SECONDS);
        if boundary_time - self.last_boundary_time < f64::from(MIN_SECTION_SECONDS) {
            return 0.0;
        }
        self.last_boundary_time = boundary_time;
        ((v - mean) / (std * CONF_SIGMA_SPAN)).clamp(0.0, 1.0)
    }

    /// Checkerboard-kernel novelty at the point `kernel_half` ticks behind the newest (so the
    /// full symmetric kernel fits inside the ring — causal). Vectors are unit-normalized, so
    /// their similarity is a plain dot product.
    fn foote_novelty(&self) -> f32 {
        let l = self.kernel_half;
        let side = 2 * l + 1;
        let n = self.ring.len();
        if n < side {
            return 0.0;
        }
        let center = n - 1 - l;
        let mut acc = 0.0f32;
        for di in 0..side {
            let a = &self.ring[center + di - l];
            for dj in 0..side {
                let k = self.kernel[di * side + dj];
                if k == 0.0 {
                    continue;
                }
                let b = &self.ring[center + dj - l];
                acc += k * dot(a, b);
            }
        }
        acc.max(0.0)
    }

    fn buildup_logistic(&self, pre_norm: &AudioFeatures) -> f32 {
        let f_loud = pre_norm.loudness_trend.clamp(0.0, 1.0);
        let f_centroid =
            ((pre_norm.centroid - self.centroid_slow) * CENTROID_RISE_GAIN).clamp(0.0, 1.0);
        let f_onset = ((self.onset_fast - self.onset_slow) * ONSET_RISE_GAIN).clamp(0.0, 1.0);
        // Sub-bass withdrawal: how far current sub-bass sits below its ~8 s average.
        let f_subbass_gone = if self.subbass_slow > 1e-6 {
            ((self.subbass_slow - pre_norm.sub_bass) / self.subbass_slow).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let x = self.cfg.buildup_bias
            + self.cfg.buildup_w_loud * f_loud
            + self.cfg.buildup_w_centroid * f_centroid
            + self.cfg.buildup_w_onset * f_onset
            + self.cfg.buildup_w_subbass * f_subbass_gone;
        sigmoid(x)
    }

    /// Returns 1.0 on the tick a drop is detected.
    fn update_drop(&mut self, pre_norm: &AudioFeatures, timestamp: f64) -> f32 {
        let tick_dt = self.tick_interval as f32;
        // Sustained-high build-up arms the drop; brief dips decay the timer rather than reset it.
        if self.cur_buildup > self.cfg.drop_arm_buildup {
            self.high_duration += tick_dt;
        } else {
            self.high_duration = (self.high_duration - 2.0 * tick_dt).max(0.0);
        }

        // Loudness-jump baseline: running minimum over ~DROP_BASELINE_SECONDS.
        if self.loud_ring.len() == self.loud_ring_cap {
            self.loud_ring.pop_front();
        }
        self.loud_ring.push_back(pre_norm.loudness_m);
        let baseline = self.loud_ring.iter().copied().fold(f32::INFINITY, f32::min);
        let jump = pre_norm.loudness_m - baseline;
        let subbass_returning = pre_norm.sub_bass > self.cfg.drop_subbass_return * self.subbass_ref;

        let armed = self.high_duration >= self.cfg.drop_arm_sustain;
        let in_refractory = timestamp < self.refractory_until;
        if armed && !in_refractory && jump >= self.cfg.drop_loud_jump && subbass_returning {
            self.refractory_until = timestamp + self.cfg.drop_refractory as f64;
            self.high_duration = 0.0;
            return 1.0;
        }
        0.0
    }
}

/// Build the unit-normalized compact timbre vector (7 bands + MFCC 1..=8), each block first
/// divided by its own measured RMS so the two contribute at their intended weights (#2204).
fn timbre_vector(f: &AudioFeatures) -> [f32; VEC_DIM] {
    let b = W_BANDS / BAND_RMS;
    let m = W_MFCC / MFCC_RMS;
    let mut v = [
        f.sub_bass * b,
        f.bass * b,
        f.low_mid * b,
        f.mid * b,
        f.upper_mid * b,
        f.presence * b,
        f.brilliance * b,
        f.mfcc[1] * m,
        f.mfcc[2] * m,
        f.mfcc[3] * m,
        f.mfcc[4] * m,
        f.mfcc[5] * m,
        f.mfcc[6] * m,
        f.mfcc[7] * m,
        f.mfcc[8] * m,
    ];
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-6 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

#[inline]
fn dot(a: &[f32; VEC_DIM], b: &[f32; VEC_DIM]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[inline]
fn sign(x: isize) -> f32 {
    match x.cmp(&0) {
        std::cmp::Ordering::Greater => 1.0,
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
    }
}

#[inline]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOP: f32 = 86.0;

    fn beat(onset: f32) -> BeatResult {
        BeatResult {
            onset_strength: onset,
            beat: 0.0,
            beat_phase: 0.0,
            bpm: 128.0,
            beat_strength: 0.0,
            beat_time: 0.0,
            beat_index: 0,
        }
    }

    /// Feed a timbre profile for `secs` seconds. `feat` is refreshed each frame from the
    /// closure so callers can ramp inputs. Returns (drops fired, **max** section_novelty over
    /// the interval, final buildup).
    fn drive(
        t: &mut StructureTracker,
        clock: &mut f64,
        secs: f32,
        mut feat: impl FnMut(f32) -> (AudioFeatures, f32),
    ) -> (usize, f32, f32) {
        let dt = 1.0 / HOP as f64;
        let n = (secs * HOP) as usize;
        let (mut drops, mut max_nov, mut last_build) = (0, 0.0f32, 0.0);
        for i in 0..n {
            let (f, onset) = feat(i as f32 / HOP);
            let r = t.process(StructureConfig::default(), &f, &beat(onset), *clock);
            if r.drop > 0.5 {
                drops += 1;
            }
            max_nov = max_nov.max(r.section_novelty);
            last_build = r.buildup;
            *clock += dt;
        }
        (drops, max_nov, last_build)
    }

    /// As [`drive`], plus the count of boundary events fired over the interval (#2080).
    fn drive_b(
        t: &mut StructureTracker,
        clock: &mut f64,
        secs: f32,
        mut feat: impl FnMut(f32) -> (AudioFeatures, f32),
    ) -> (usize, f32, f32, usize) {
        let dt = 1.0 / HOP as f64;
        let n = (secs * HOP) as usize;
        let (mut drops, mut max_nov, mut last_build, mut bounds) = (0, 0.0f32, 0.0, 0);
        for i in 0..n {
            let (f, onset) = feat(i as f32 / HOP);
            let r = t.process(StructureConfig::default(), &f, &beat(onset), *clock);
            if r.drop > 0.5 {
                drops += 1;
            }
            if r.boundary > 0.0 {
                bounds += 1;
            }
            max_nov = max_nov.max(r.section_novelty);
            last_build = r.buildup;
            *clock += dt;
        }
        (drops, max_nov, last_build, bounds)
    }

    fn feat_with(loudness_m: f32, sub_bass: f32, centroid: f32, loud_trend: f32) -> AudioFeatures {
        AudioFeatures {
            sub_bass,
            bass: 0.4,
            low_mid: 0.3,
            mid: 0.3,
            centroid,
            loudness_m,
            loudness_trend: loud_trend,
            ..Default::default()
        }
    }

    #[test]
    fn steady_state_low_buildup_no_drop() {
        let mut t = StructureTracker::new(HOP);
        let mut clock = 0.0;
        let (drops, _, build) = drive(&mut t, &mut clock, 20.0, |_| {
            (feat_with(0.5, 0.6, 0.4, 0.0), 0.3)
        });
        assert_eq!(drops, 0, "steady music must not fire a drop");
        assert!(build < 0.3, "steady build-up should stay low, got {build}");
    }

    #[test]
    fn build_then_drop_fires_once() {
        let mut t = StructureTracker::new(HOP);
        let mut clock = 0.0;
        // Baseline.
        drive(&mut t, &mut clock, 3.0, |_| {
            (feat_with(0.5, 0.6, 0.35, 0.0), 0.2)
        });
        // ~7 s riser: loudness trend up, brightening, onsets denser, sub-bass withdrawn.
        let (d_build, _, build) = drive(&mut t, &mut clock, 7.0, |s| {
            let p = (s / 7.0).min(1.0);
            let f = feat_with(0.5, 0.6 - 0.4 * p, 0.35 + 0.35 * p, 0.7);
            (f, 0.2 + 0.6 * p)
        });
        assert_eq!(d_build, 0, "no drop should fire during the build");
        assert!(
            build > DROP_ARM_BUILDUP,
            "riser should raise buildup, got {build}"
        );
        // The drop: broadband loudness leap + sub-bass returns.
        let (d_drop, _, _) = drive(&mut t, &mut clock, 3.0, |_| {
            (feat_with(0.75, 0.9, 0.6, 0.2), 0.9)
        });
        assert_eq!(d_drop, 1, "exactly one drop should fire");
        // Refractory: a second identical build+drop within 16 s must not fire.
        drive(&mut t, &mut clock, 7.0, |s| {
            let p = (s / 7.0).min(1.0);
            (
                feat_with(0.5, 0.6 - 0.4 * p, 0.35 + 0.35 * p, 0.7),
                0.2 + 0.6 * p,
            )
        });
        let (d_again, _, _) = drive(&mut t, &mut clock, 2.0, |_| {
            (feat_with(0.75, 0.9, 0.6, 0.2), 0.9)
        });
        assert_eq!(
            d_again, 0,
            "refractory must suppress a second drop within 16 s"
        );
    }

    #[test]
    fn section_change_spikes_novelty() {
        let mut t = StructureTracker::new(HOP);
        let mut clock = 0.0;
        // Section A: bass-heavy timbre.
        let section_a = |_: f32| {
            let mut f = feat_with(0.5, 0.8, 0.2, 0.0);
            f.brilliance = 0.05;
            f.presence = 0.05;
            (f, 0.3)
        };
        drive(&mut t, &mut clock, 32.0, section_a);
        let (_, nov_steady, _) = drive(&mut t, &mut clock, 2.0, section_a);
        // Section B: bright timbre — a clear boundary.
        let section_b = |_: f32| {
            let mut f = feat_with(0.5, 0.1, 0.8, 0.0);
            f.brilliance = 0.8;
            f.presence = 0.7;
            (f, 0.3)
        };
        // The causal kernel reports the boundary ~KERNEL_SECONDS into section B; drive long
        // enough to cover the peak, taking the max novelty over the transition.
        let (_, nov_peak, _) = drive(&mut t, &mut clock, 8.0, section_b);

        // The load-bearing half of this test is the STEADY reading, not the peak (#2080).
        // Under the old decaying-running-max normalization steady material read a hard 1.0 —
        // the curve self-normalized to its own recent maximum, so a song that never changed
        // pinned the meter and nothing downstream could threshold it (#1973). Absolute
        // normalization is what makes "nothing is happening" representable, so assert that
        // first: this bound is what fails if the running max ever comes back.
        assert!(
            nov_steady < 0.05,
            "steady material must read near zero, got {nov_steady}"
        );
        assert!(
            nov_peak > 0.1 && nov_peak > nov_steady * 10.0,
            "novelty should spike at the section change (peak {nov_peak}, steady {nov_steady})"
        );
    }

    /// A section change produces exactly one boundary event, and steady material produces
    /// none — the two halves of turning the novelty curve into wire events (#2080).
    #[test]
    fn section_change_fires_one_boundary() {
        let mut t = StructureTracker::new(HOP);
        let mut clock = 0.0;
        let section_a = |_: f32| {
            let mut f = feat_with(0.5, 0.8, 0.2, 0.0);
            f.brilliance = 0.05;
            f.presence = 0.05;
            (f, 0.3)
        };
        let section_b = |_: f32| {
            let mut f = feat_with(0.5, 0.1, 0.8, 0.0);
            f.brilliance = 0.8;
            f.presence = 0.7;
            (f, 0.3)
        };
        // Long enough to fill the ring and the peak-statistics window.
        let (_, _, _, b_steady) = drive_b(&mut t, &mut clock, 60.0, section_a);
        assert_eq!(b_steady, 0, "unchanging material must not fire a boundary");

        let (_, _, _, b_change) = drive_b(&mut t, &mut clock, 30.0, section_b);
        assert_eq!(
            b_change, 1,
            "one section change must fire exactly one boundary"
        );
    }

    /// The dwell gate suppresses a second peak inside `MIN_SECTION_SECONDS`: a fill or a
    /// turnaround is not a section.
    #[test]
    fn dwell_suppresses_a_rapid_second_boundary() {
        let mut t = StructureTracker::new(HOP);
        let mut clock = 0.0;
        let quiet = |_: f32| {
            let mut f = feat_with(0.5, 0.8, 0.2, 0.0);
            f.brilliance = 0.05;
            f.presence = 0.05;
            (f, 0.3)
        };
        let bright = |_: f32| {
            let mut f = feat_with(0.5, 0.1, 0.8, 0.0);
            f.brilliance = 0.8;
            f.presence = 0.7;
            (f, 0.3)
        };
        drive_b(&mut t, &mut clock, 60.0, quiet);
        // Two changes only 4 s apart — well inside the 8 s minimum section length.
        let (_, _, _, first) = drive_b(&mut t, &mut clock, 4.0, bright);
        let (_, _, _, second) = drive_b(&mut t, &mut clock, 4.0, quiet);
        let (_, _, _, settle) = drive_b(&mut t, &mut clock, 12.0, quiet);
        assert!(
            first + second + settle <= 1,
            "two changes {MIN_SECTION_SECONDS} s apart must yield at most one boundary, \
             got {first}+{second}+{settle}"
        );
    }

    #[test]
    fn config_raised_jump_threshold_suppresses_drop() {
        // Same build→drop scenario as `build_then_drop_fires_once`, driven twice: with the
        // default config the drop fires once; with `drop_loud_jump` raised past the scenario's
        // ~0.25 loudness leap it must NOT fire. Proves the shared StructureConfig actually
        // drives the detector at runtime (#1510), not just the compile-time consts.
        fn run<F: FnMut(f32) -> (AudioFeatures, f32)>(
            t: &mut StructureTracker,
            clock: &mut f64,
            cfg: StructureConfig,
            secs: f32,
            mut feat: F,
            drops: &mut usize,
        ) {
            let dt = 1.0 / HOP as f64;
            let n = (secs * HOP) as usize;
            for i in 0..n {
                let (f, onset) = feat(i as f32 / HOP);
                if t.process(cfg, &f, &beat(onset), *clock).drop > 0.5 {
                    *drops += 1;
                }
                *clock += dt;
            }
        }

        fn count_drops(cfg: StructureConfig) -> usize {
            let mut t = StructureTracker::new(HOP);
            let mut clock = 0.0f64;
            let mut drops = 0usize;
            run(
                &mut t,
                &mut clock,
                cfg,
                3.0,
                |_| (feat_with(0.5, 0.6, 0.35, 0.0), 0.2),
                &mut drops,
            );
            run(
                &mut t,
                &mut clock,
                cfg,
                7.0,
                |s| {
                    let p = (s / 7.0).min(1.0);
                    (
                        feat_with(0.5, 0.6 - 0.4 * p, 0.35 + 0.35 * p, 0.7),
                        0.2 + 0.6 * p,
                    )
                },
                &mut drops,
            );
            run(
                &mut t,
                &mut clock,
                cfg,
                3.0,
                |_| (feat_with(0.75, 0.9, 0.6, 0.2), 0.9),
                &mut drops,
            );
            drops
        }

        assert_eq!(
            count_drops(StructureConfig::default()),
            1,
            "default config must fire exactly one drop"
        );
        let hard = StructureConfig {
            drop_loud_jump: 0.5,
            ..StructureConfig::default()
        };
        assert_eq!(
            count_drops(hard),
            0,
            "a raised drop_loud_jump must suppress the same drop"
        );
    }

    #[test]
    fn timbre_vector_is_unit_norm() {
        let f = feat_with(0.5, 0.6, 0.4, 0.0);
        let v = timbre_vector(&f);
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((n - 1.0).abs() < 1e-5, "expected unit norm, got {n}");
    }
}
