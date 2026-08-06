//! A15 (#1466): monophonic fundamental-frequency (f0) estimation via YIN.
//!
//! Classic YIN (de Cheveigné & Kawahara 2002) on the analyzer's raw, un-windowed time-domain
//! window — no extra FFT, and DC-immune (the difference function cancels any constant offset):
//! 1. **Difference function** `d(τ) = Σ_j (x[j] − x[j+τ])²` over an integration window `W`.
//! 2. **Cumulative mean normalized difference** `d'(τ) = d(τ) / ((1/τ)·Σ_{k≤τ} d(k))` — `d'(0) ≡ 1`,
//!    which suppresses the octave-too-*high* (τ = T/2) error the raw autocorrelation makes.
//! 3. **Absolute threshold**: the *first* dip below [`YIN_THRESHOLD`] (followed down to its local
//!    minimum) is the fundamental period — taking the first qualifying τ, not the global minimum,
//!    is what biases toward T over its multiples 2T, 3T (the octave-too-*low* error).
//! 4. **Parabolic interpolation** around that τ for a sub-sample period → sub-cent f0 accuracy.
//!
//! The output `pitch` is a producer-normalized 0..1 **log-frequency** (schema policy `Passthrough`,
//! so it survives `normalize`/`smooth` unrescaled): `PITCH_F_MIN` (55 Hz, A1) → 0.0,
//! `PITCH_F_MAX` (1760 Hz, A6) → 1.0, a clean 5 octaves so an octave is always 0.2. `confidence`
//! is the YIN periodicity `1 − aperiodicity` (`aperiodicity = d'` at the chosen τ). The estimate is
//! **held through unvoiced gaps** (confidence gated to 0) so a pitch-keyed visual doesn't snap to
//! the lowest note on every rest.

/// Full analysis window pulled from the analyzer's 4096-sample time-domain buffer.
const WINDOW: usize = 4096;
/// YIN integration window. `2·W = WINDOW`, so the difference function reaches `τ = W` samples back
/// with a full-length window at every lag (`j+τ` maxes at `2·W−2 = 4094 < WINDOW`).
const W: usize = 2048;
/// Shortest lag searched (~2000 Hz at 44.1 kHz). Below this is percussion/harmonics, not melody.
const TAU_MIN: usize = 22;
/// Absolute-threshold aperiodicity below which a dip counts as a voiced pitch (de Cheveigné §4).
const YIN_THRESHOLD: f32 = 0.15;
/// AC-energy floor under which the window is treated as silent/DC (guards the `d'` 0/0 → NaN).
const ENERGY_EPS: f64 = 1e-9;

/// Bottom of the log-frequency map — A1. Also the anchor the OSC Hz de-normalization inverts.
pub const PITCH_F_MIN: f32 = 55.0;
/// Octave span of the log-frequency map — 5 octaves above [`PITCH_F_MIN`] lands on 1760 Hz (A6),
/// so `pitch = 1.0` ⇔ 1760 Hz and each octave is exactly 0.2.
pub const PITCH_OCTAVES: f32 = 5.0;

/// The two pitch features, already mapped for the shader ABI. Unlike the stateless HPSS/stereo
/// fields there is no `NEUTRAL` constant — the analyzer *holds* its last pitch across unvoiced
/// frames (see [`PitchAnalyzer::last_pitch`]) rather than returning a fixed neutral.
#[derive(Debug, Clone, Copy)]
pub struct PitchFeatures {
    /// Fundamental frequency as a 0..1 log-frequency (55–1760 Hz); held on unvoiced frames.
    pub pitch: f32,
    /// YIN periodicity `1 − aperiodicity`, 0..1; 0 when no voiced pitch is present.
    pub pitch_confidence: f32,
}

/// Monophonic YIN f0 tracker. Owns preallocated scratch so `process` never allocates.
pub struct PitchAnalyzer {
    sample_rate: f32,
    /// Last voiced pitch (0..1 log-frequency), held through unvoiced gaps.
    last_pitch: f32,
    /// Difference function `d(τ)`, `τ ∈ [0, W)`.
    diff: Box<[f32]>,
    /// Cumulative mean normalized difference `d'(τ)`.
    cmnd: Box<[f32]>,
}

impl PitchAnalyzer {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            last_pitch: 0.0,
            diff: vec![0.0; W].into_boxed_slice(),
            cmnd: vec![0.0; W].into_boxed_slice(),
        }
    }

    /// Estimate f0 over the last [`WINDOW`] samples of `time_domain` (newest at the end, raw and
    /// un-windowed). Returns the held pitch with zero confidence on silence, DC, or an unvoiced
    /// frame; updates and returns a fresh pitch on a voiced frame.
    pub fn process(&mut self, time_domain: &[f32], loud_silent: bool) -> PitchFeatures {
        if loud_silent || time_domain.len() < WINDOW {
            return PitchFeatures {
                pitch: self.last_pitch,
                pitch_confidence: 0.0,
            };
        }
        let x = &time_domain[time_domain.len() - WINDOW..];

        // 1. Difference function d(τ) = Σ_{j<W} (x[j] − x[j+τ])². f64 accumulate: W squared f32
        //    diffs sum to ~10³ and the ratio in step 2 is scale-sensitive.
        self.diff[0] = 0.0;
        for tau in 1..W {
            let mut sum = 0.0f64;
            for j in 0..W {
                let d = (x[j] - x[j + tau]) as f64;
                sum += d * d;
            }
            self.diff[tau] = sum as f32;
        }

        // Silence/DC guard: no AC energy ⇒ d(τ) ≡ 0 ⇒ d'(τ) is 0/0. Hold, don't emit NaN.
        let total: f64 = self.diff[1..W].iter().map(|&d| d as f64).sum();
        if total < ENERGY_EPS {
            return PitchFeatures {
                pitch: self.last_pitch,
                pitch_confidence: 0.0,
            };
        }

        // 2. Cumulative mean normalized difference. d'(0) ≡ 1; the earliest lags (where the running
        //    mean is still ~0) are below TAU_MIN and never searched, so hold them at 1 (no dip).
        self.cmnd[0] = 1.0;
        let mut running = 0.0f64;
        for tau in 1..W {
            running += self.diff[tau] as f64;
            self.cmnd[tau] = if running < ENERGY_EPS {
                1.0
            } else {
                (self.diff[tau] as f64 * tau as f64 / running) as f32
            };
        }

        // 3. Absolute threshold: first dip below the threshold, followed down to its local minimum
        //    — the fundamental period. No qualifying dip ⇒ global minimum with (low) confidence.
        let mut best_tau = 0usize;
        let mut tau = TAU_MIN;
        while tau < W {
            if self.cmnd[tau] < YIN_THRESHOLD {
                while tau + 1 < W && self.cmnd[tau + 1] < self.cmnd[tau] {
                    tau += 1;
                }
                best_tau = tau;
                break;
            }
            tau += 1;
        }
        let voiced = best_tau != 0;
        if !voiced {
            let mut min_tau = TAU_MIN;
            for t in (TAU_MIN + 1)..W {
                if self.cmnd[t] < self.cmnd[min_tau] {
                    min_tau = t;
                }
            }
            best_tau = min_tau;
        }

        // 4. Parabolic interpolation of the minimum for a sub-sample period (guarded at the edges).
        let period = if best_tau > TAU_MIN && best_tau + 1 < W {
            let s0 = self.cmnd[best_tau - 1] as f64;
            let s1 = self.cmnd[best_tau] as f64;
            let s2 = self.cmnd[best_tau + 1] as f64;
            let denom = s0 - 2.0 * s1 + s2;
            if denom.abs() > 1e-12 {
                best_tau as f64 + 0.5 * (s0 - s2) / denom
            } else {
                best_tau as f64
            }
        } else {
            best_tau as f64
        };

        let aperiodicity = self.cmnd[best_tau];
        if !voiced {
            // Global-minimum fallback: no clear pitch — hold and report no confidence.
            return PitchFeatures {
                pitch: self.last_pitch,
                pitch_confidence: 0.0,
            };
        }
        let f0 = self.sample_rate / period as f32;
        self.last_pitch = norm_from_hz(f0);
        PitchFeatures {
            pitch: self.last_pitch,
            pitch_confidence: (1.0 - aperiodicity).clamp(0.0, 1.0),
        }
    }
}

impl Default for PitchAnalyzer {
    fn default() -> Self {
        // 44.1 kHz — overwritten by `new(sample_rate)` in the analysis loop.
        Self::new(44_100.0)
    }
}

/// Map an f0 in Hz to the 0..1 log-frequency the shader ABI carries (clamped to [`PITCH_F_MIN`]…
/// `PITCH_F_MIN·2^PITCH_OCTAVES`). Producer-owned, so the value passes through the normalizer.
fn norm_from_hz(f0: f32) -> f32 {
    ((f0.log2() - PITCH_F_MIN.log2()) / PITCH_OCTAVES).clamp(0.0, 1.0)
}

/// Invert [`norm_from_hz`] — the single source of truth for the OSC `/pitch_hz` de-normalization
/// (mirrors the `bpm` / `key` convention of emitting the physical unit alongside the 0..1 value).
pub fn norm_to_hz(norm: f32) -> f32 {
    PITCH_F_MIN * 2f32.powf(PITCH_OCTAVES * norm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SR: f32 = 44_100.0;

    /// A pure sine of `n` samples at 44.1 kHz.
    fn sine(freq: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (TAU * freq * i as f32 / SR).sin()).collect()
    }

    /// A band-unlimited sawtooth (rich harmonics) — the real octave-error stress test.
    fn saw(freq: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let phase = (freq * i as f32 / SR).fract();
                2.0 * phase - 1.0
            })
            .collect()
    }

    /// Deterministic white-ish noise via a tiny LCG in [-1, 1).
    fn noise(n: usize) -> Vec<f32> {
        let mut s: u32 = 0x1234_5678;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 8) as f32 / (1u32 << 23) as f32 - 1.0
            })
            .collect()
    }

    fn detect(signal: &[f32]) -> PitchFeatures {
        PitchAnalyzer::new(SR).process(signal, false)
    }

    #[test]
    fn norm_maps_octaves_cleanly() {
        assert!((norm_from_hz(55.0) - 0.0).abs() < 1e-4);
        assert!((norm_from_hz(220.0) - 0.4).abs() < 1e-4);
        assert!((norm_from_hz(440.0) - 0.6).abs() < 1e-4);
        assert!((norm_from_hz(1760.0) - 1.0).abs() < 1e-4);
        // Round-trip through the OSC de-normalization.
        assert!((norm_to_hz(0.4) - 220.0).abs() < 0.5);
        assert!((norm_to_hz(0.6) - 440.0).abs() < 1.0);
    }

    #[test]
    fn pure_sines_track_pitch_with_confidence() {
        for (freq, expected) in [(110.0, 0.2), (220.0, 0.4), (440.0, 0.6), (880.0, 0.8)] {
            let f = detect(&sine(freq, WINDOW));
            assert!(
                (f.pitch - expected).abs() < 0.02,
                "{freq} Hz → pitch {} (want {expected})",
                f.pitch
            );
            assert!(
                f.pitch_confidence > 0.85,
                "{freq} Hz confidence {}",
                f.pitch_confidence
            );
        }
    }

    #[test]
    fn octave_spacing_is_one_fifth() {
        // log2(1760/55) = 5, so one octave is always 0.2 of the range.
        let a = detect(&sine(110.0, WINDOW)).pitch;
        let b = detect(&sine(220.0, WINDOW)).pitch;
        assert!((b - a - 0.2).abs() < 0.02, "octave spacing {}", b - a);
    }

    #[test]
    fn harmonically_rich_saw_picks_the_fundamental() {
        // A 220 Hz sawtooth has strong energy at 440/660/… — YIN must lock the 220 fundamental,
        // not an octave up (0.6) or down (0.2).
        let f = detect(&saw(220.0, WINDOW));
        assert!(
            (f.pitch - 0.4).abs() < 0.02,
            "saw 220 Hz → pitch {} (want 0.4, fundamental)",
            f.pitch
        );
        assert!(
            f.pitch_confidence > 0.8,
            "confidence {}",
            f.pitch_confidence
        );
    }

    #[test]
    fn sub_sample_accuracy_within_cents() {
        // 443 Hz sits between integer lags (τ ≈ 99.5); parabolic interp must resolve it to a few
        // cents. 5 cents ≈ 0.0289·(1/60) in normalized units → assert Hz round-trips tightly.
        let f = detect(&sine(443.0, WINDOW));
        let hz = norm_to_hz(f.pitch);
        assert!((hz - 443.0).abs() < 3.0, "443 Hz resolved to {hz} Hz");
    }

    #[test]
    fn dc_offset_is_immune() {
        // YIN's difference function cancels a constant — a 220 Hz sine ridden on +0.5 DC still reads
        // 220, proving no accidental mean handling broke it.
        let biased: Vec<f32> = sine(220.0, WINDOW).iter().map(|s| s + 0.5).collect();
        let f = detect(&biased);
        assert!(
            (f.pitch - 0.4).abs() < 0.02,
            "DC-biased 220 Hz → {}",
            f.pitch
        );
        assert!(f.pitch_confidence > 0.85);
    }

    #[test]
    fn low_and_high_extremes_saturate_not_wrap() {
        // 40 Hz (below the 55 Hz anchor) clamps to 0.0, never negative.
        assert_eq!(detect(&sine(40.0, WINDOW)).pitch, 0.0);
        // 1600 Hz sits just under the 1760 Hz top anchor.
        let hi = detect(&sine(1600.0, WINDOW)).pitch;
        assert!(hi > 0.9 && hi <= 1.0, "1600 Hz → {hi}");
    }

    #[test]
    fn white_noise_is_low_confidence() {
        assert!(
            detect(&noise(WINDOW)).pitch_confidence < 0.5,
            "noise confidence"
        );
    }

    #[test]
    fn all_zeros_is_unvoiced_no_nan() {
        // The 0/0 guard: an all-zeros frame that is *not* flagged silent must not emit NaN.
        let f = PitchAnalyzer::new(SR).process(&vec![0.0; WINDOW], false);
        assert_eq!(f.pitch_confidence, 0.0);
        assert!(f.pitch.is_finite());
    }

    #[test]
    fn loud_silent_holds_pitch_confidence_zero() {
        let mut a = PitchAnalyzer::new(SR);
        let voiced = a.process(&sine(220.0, WINDOW), false);
        assert!((voiced.pitch - 0.4).abs() < 0.02);
        // A silent frame holds the last pitch but drops confidence to 0.
        let held = a.process(&sine(220.0, WINDOW), true);
        assert_eq!(held.pitch, voiced.pitch);
        assert_eq!(held.pitch_confidence, 0.0);
    }

    #[test]
    fn unvoiced_gap_holds_then_reacquires() {
        let mut a = PitchAnalyzer::new(SR);
        let first = a.process(&sine(220.0, WINDOW), false);
        // Noise gap: pitch holds, confidence collapses.
        let gap = a.process(&noise(WINDOW), false);
        assert_eq!(gap.pitch, first.pitch, "pitch held through the gap");
        assert!(gap.pitch_confidence < 0.5);
        // A new note re-acquires.
        let second = a.process(&sine(440.0, WINDOW), false);
        assert!(
            (second.pitch - 0.6).abs() < 0.02,
            "reacquired {}",
            second.pitch
        );
        assert!(second.pitch_confidence > 0.85);
    }

    #[test]
    fn partial_buffer_is_neutral() {
        // Startup: fewer than WINDOW samples must not panic.
        let f = PitchAnalyzer::new(SR).process(&sine(220.0, 1000), false);
        assert_eq!(f.pitch_confidence, 0.0);
    }
}
