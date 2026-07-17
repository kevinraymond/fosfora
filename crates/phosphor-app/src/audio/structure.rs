//! Song-structure detection: section novelty, build-up, and drop (A18 #1469).
//!
//! Nothing else in the engine sees structure beyond a single beat, so the drop and section
//! changes — the most valuable VJ moments — have to be hand-triggered. This stage adds a
//! cheap, decimated (~10 Hz) analysis on top of the per-hop features:
//!
//! - `section_novelty` — a **Foote** self-similarity novelty. Each tick appends a compact
//!   timbre vector (7 bands + MFCC 1–8, unit-normalized) to a 60 s ring; a Gaussian-tapered
//!   **checkerboard kernel** slid along the self-similarity diagonal peaks where the block
//!   structure changes (a new section). Causal, so it reports a boundary ~`KERNEL_SECONDS`
//!   after it happens.
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
/// Running-max forgetting window (seconds) for self-normalizing `section_novelty`.
const NOVELTY_MAX_SECONDS: f32 = 30.0;

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
    /// Section-boundary novelty, self-normalized 0..1.
    pub section_novelty: f32,
    /// Build-up / tension estimate, 0..1.
    pub buildup: f32,
    /// 1.0 on the frame a drop is detected, else 0.0 (a trigger, counter-latched upstream).
    pub drop: f32,
}

pub struct StructureTracker {
    tick_interval: f64,
    /// Precomputed checkerboard kernel `K(i,j) = g(i,j)·sgn(i)·sgn(j)` over `-L..=L`,
    /// row-major of side `2L+1`.
    kernel: Vec<f32>,
    kernel_half: usize,
    ring: VecDeque<[f32; VEC_DIM]>,
    ring_cap: usize,
    novelty_max: f32,
    cur_novelty: f32,

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
        Self {
            tick_interval: (1.0 / TICK_HZ) as f64,
            kernel,
            kernel_half,
            ring: VecDeque::with_capacity((RING_SECONDS * TICK_HZ) as usize + 1),
            ring_cap: (RING_SECONDS * TICK_HZ) as usize,
            novelty_max: 1e-6,
            cur_novelty: 0.0,
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
        if self.last_tick_time < 0.0 || timestamp - self.last_tick_time >= self.tick_interval {
            self.last_tick_time = timestamp;
            drop = self.tick(pre_norm, timestamp);
        }

        StructureResult {
            section_novelty: self.cur_novelty,
            buildup: self.cur_buildup,
            drop,
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

    fn tick(&mut self, pre_norm: &AudioFeatures, timestamp: f64) -> f32 {
        // --- section_novelty (Foote) ---
        let v = timbre_vector(pre_norm);
        if self.ring.len() == self.ring_cap {
            self.ring.pop_front();
        }
        self.ring.push_back(v);
        let raw_novelty = self.foote_novelty();
        let max_decay = (-(1.0 / TICK_HZ) / NOVELTY_MAX_SECONDS).exp();
        self.novelty_max = (self.novelty_max * max_decay).max(raw_novelty).max(1e-6);
        self.cur_novelty = (raw_novelty / self.novelty_max).clamp(0.0, 1.0);

        // --- buildup (logistic, EMA-smoothed at tick rate) ---
        let build_raw = self.buildup_logistic(pre_norm);
        let a = 1.0 - (-(1.0 / TICK_HZ) / BUILD_TAU).exp();
        self.buildup_ema += (build_raw - self.buildup_ema) * a;
        self.cur_buildup = self.buildup_ema;

        // --- drop state machine ---
        self.update_drop(pre_norm, timestamp)
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

/// Build the unit-normalized compact timbre vector (7 bands + MFCC 1..=8).
fn timbre_vector(f: &AudioFeatures) -> [f32; VEC_DIM] {
    let mut v = [
        f.sub_bass,
        f.bass,
        f.low_mid,
        f.mid,
        f.upper_mid,
        f.presence,
        f.brilliance,
        f.mfcc[1],
        f.mfcc[2],
        f.mfcc[3],
        f.mfcc[4],
        f.mfcc[5],
        f.mfcc[6],
        f.mfcc[7],
        f.mfcc[8],
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
        assert!(
            nov_peak > 0.3 && nov_peak > nov_steady + 0.1,
            "novelty should spike at the section change (peak {nov_peak}, steady {nov_steady})"
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
