use super::features::{AudioFeatures, NUM_FEATURES};

/// Per-feature attack/release time constants (seconds).
struct SmoothParams {
    attack: f32,
    release: f32,
    bypass: bool, // pass-through without smoothing
}

/// Asymmetric attack/release EMA smoother for 45 audio features.
pub struct FeatureSmoother {
    state: [f32; NUM_FEATURES],
    params: [SmoothParams; NUM_FEATURES],
}

impl FeatureSmoother {
    pub fn new() -> Self {
        let params = [
            // Frequency bands (7)
            SmoothParams {
                attack: 0.02,
                release: 0.15,
                bypass: false,
            }, // sub_bass
            SmoothParams {
                attack: 0.02,
                release: 0.15,
                bypass: false,
            }, // bass
            SmoothParams {
                attack: 0.01,
                release: 0.10,
                bypass: false,
            }, // low_mid
            SmoothParams {
                attack: 0.01,
                release: 0.10,
                bypass: false,
            }, // mid
            SmoothParams {
                attack: 0.005,
                release: 0.08,
                bypass: false,
            }, // upper_mid
            SmoothParams {
                attack: 0.005,
                release: 0.08,
                bypass: false,
            }, // presence
            SmoothParams {
                attack: 0.005,
                release: 0.08,
                bypass: false,
            }, // brilliance
            // Aggregates (2)
            SmoothParams {
                attack: 0.01,
                release: 0.12,
                bypass: false,
            }, // rms
            SmoothParams {
                attack: 0.002,
                release: 0.06,
                bypass: false,
            }, // kick (fast attack)
            // Spectral shape (6)
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // centroid
            SmoothParams {
                attack: 0.005,
                release: 0.06,
                bypass: false,
            }, // flux
            SmoothParams {
                attack: 0.05,
                release: 0.20,
                bypass: false,
            }, // flatness
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // rolloff
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // bandwidth
            SmoothParams {
                attack: 0.02,
                release: 0.10,
                bypass: false,
            }, // zcr
            // Beat detection (5)
            SmoothParams {
                attack: 0.001,
                release: 0.05,
                bypass: false,
            }, // onset (very fast)
            SmoothParams {
                attack: 0.0,
                release: 0.0,
                bypass: true,
            }, // beat (pass-through)
            SmoothParams {
                attack: 0.0,
                release: 0.0,
                bypass: true,
            }, // beat_phase (pass-through)
            SmoothParams {
                attack: 0.5,
                release: 1.0,
                bypass: false,
            }, // bpm (very slow)
            SmoothParams {
                attack: 0.001,
                release: 0.08,
                bypass: false,
            }, // beat_strength (fast attack)
            // MFCC (13) — timbral content, moderate smoothing
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // mfcc[0]
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // mfcc[1]
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // mfcc[2]
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // mfcc[3]
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // mfcc[4]
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // mfcc[5]
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // mfcc[6]
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // mfcc[7]
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // mfcc[8]
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // mfcc[9]
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // mfcc[10]
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // mfcc[11]
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // mfcc[12]
            // Chroma (12) — pitch class energies, moderate smoothing
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // chroma[0] C
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // chroma[1] C#
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // chroma[2] D
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // chroma[3] D#
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // chroma[4] E
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // chroma[5] F
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // chroma[6] F#
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // chroma[7] G
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // chroma[8] G#
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // chroma[9] A
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // chroma[10] A#
            SmoothParams {
                attack: 0.03,
                release: 0.15,
                bypass: false,
            }, // chroma[11] B
            SmoothParams {
                attack: 0.05,
                release: 0.2,
                bypass: false,
            }, // dominant_chroma
        ];

        Self {
            state: [0.0; NUM_FEATURES],
            params,
        }
    }

    /// Smooth raw features with asymmetric EMA.
    /// dt is time since last call in seconds.
    pub fn smooth(&mut self, raw: &AudioFeatures, dt: f32) -> AudioFeatures {
        let raw_slice = raw.as_slice();
        let mut out = AudioFeatures::default();
        let out_slice = out.as_slice_mut();

        for i in 0..NUM_FEATURES {
            if self.params[i].bypass {
                out_slice[i] = raw_slice[i];
                self.state[i] = raw_slice[i];
                continue;
            }

            let target = raw_slice[i];
            let rising = target > self.state[i];
            let tau = if rising {
                self.params[i].attack
            } else {
                self.params[i].release
            };
            // EMA coefficient: alpha = 1 - exp(-dt/tau)
            let alpha = 1.0 - (-dt / tau.max(0.001)).exp();
            self.state[i] += alpha * (target - self.state[i]);
            out_slice[i] = self.state[i];
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn beat_and_beat_phase_bypass() {
        let mut smoother = FeatureSmoother::new();
        let mut raw = AudioFeatures::default();
        raw.beat = 1.0;
        raw.beat_phase = 0.75;
        let out = smoother.smooth(&raw, 0.01);
        assert!(approx_eq(out.beat, 1.0, 1e-6));
        assert!(approx_eq(out.beat_phase, 0.75, 1e-6));
    }

    #[test]
    fn fast_attack_rising_signal() {
        let mut smoother = FeatureSmoother::new();
        let mut raw = AudioFeatures::default();
        // sub_bass attack = 0.02s
        raw.sub_bass = 1.0;
        let out = smoother.smooth(&raw, 0.02);
        // After one frame at dt=attack_tau, alpha ≈ 1-1/e ≈ 0.632, so output ≈ 0.632
        assert!(out.sub_bass > 0.5, "got {}", out.sub_bass);
    }

    #[test]
    fn slow_release_falling_signal() {
        let mut smoother = FeatureSmoother::new();
        let mut raw = AudioFeatures::default();
        // Prime with high value
        raw.sub_bass = 1.0;
        for _ in 0..100 {
            smoother.smooth(&raw, 0.001);
        }
        // Now drop to zero — release is slow (0.15s)
        raw.sub_bass = 0.0;
        let out = smoother.smooth(&raw, 0.01);
        // Should still be near 1.0 after only 10ms with 150ms release
        assert!(out.sub_bass > 0.8, "got {}", out.sub_bass);
    }

    #[test]
    fn dt_zero_no_change() {
        let mut smoother = FeatureSmoother::new();
        let mut raw = AudioFeatures::default();
        // Prime with many frames so state converges to 0.5
        raw.rms = 0.5;
        for _ in 0..500 {
            smoother.smooth(&raw, 0.01);
        }
        let before = smoother.smooth(&raw, 0.01).rms;
        // Change target but dt=0
        raw.rms = 1.0;
        let out = smoother.smooth(&raw, 0.0);
        // exp(-0/tau) = 1, alpha = 0, so no change
        assert!(approx_eq(out.rms, before, 0.01));
    }

    #[test]
    fn steady_state_converges() {
        let mut smoother = FeatureSmoother::new();
        let mut raw = AudioFeatures::default();
        raw.mid = 0.7;
        let mut last = 0.0;
        for _ in 0..1000 {
            let out = smoother.smooth(&raw, 0.01);
            last = out.mid;
        }
        assert!(approx_eq(last, 0.7, 0.01));
    }

    #[test]
    fn all_features_finite() {
        let mut smoother = FeatureSmoother::new();
        let mut raw = AudioFeatures::default();
        raw.sub_bass = 0.5;
        raw.rms = 0.3;
        raw.onset = 0.8;
        let out = smoother.smooth(&raw, 0.016);
        for v in out.as_slice() {
            assert!(v.is_finite(), "non-finite value found");
        }
    }
}
