use super::features::{AudioFeatures, NUM_FEATURES};
use super::schema::FEATURES;

/// Asymmetric attack/release EMA smoother for the audio feature vector.
/// Per-feature attack/release constants come from [`FEATURES`], the shared
/// feature schema, so they stay aligned with the vector layout.
pub struct FeatureSmoother {
    state: [f32; NUM_FEATURES],
}

impl FeatureSmoother {
    pub fn new() -> Self {
        Self {
            state: [0.0; NUM_FEATURES],
        }
    }

    /// Smooth raw features with asymmetric EMA.
    /// dt is time since last call in seconds.
    pub fn smooth(&mut self, raw: &AudioFeatures, dt: f32) -> AudioFeatures {
        let raw_slice = raw.as_slice();
        let mut out = AudioFeatures::default();
        let out_slice = out.as_slice_mut();

        for i in 0..NUM_FEATURES {
            let p = FEATURES[i].smooth;
            if p.bypass {
                out_slice[i] = raw_slice[i];
                self.state[i] = raw_slice[i];
                continue;
            }

            let target = raw_slice[i];
            let rising = target > self.state[i];
            let tau = if rising { p.attack } else { p.release };
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
        let raw = AudioFeatures {
            beat: 1.0,
            beat_phase: 0.75,
            ..Default::default()
        };
        let out = smoother.smooth(&raw, 0.01);
        assert!(approx_eq(out.beat, 1.0, 1e-6));
        assert!(approx_eq(out.beat_phase, 0.75, 1e-6));
    }

    #[test]
    fn fast_attack_rising_signal() {
        let mut smoother = FeatureSmoother::new();
        let raw = AudioFeatures {
            // sub_bass attack = 0.02s
            sub_bass: 1.0,
            ..Default::default()
        };
        let out = smoother.smooth(&raw, 0.02);
        // After one frame at dt=attack_tau, alpha ≈ 1-1/e ≈ 0.632, so output ≈ 0.632
        assert!(out.sub_bass > 0.5, "got {}", out.sub_bass);
    }

    #[test]
    fn slow_release_falling_signal() {
        let mut smoother = FeatureSmoother::new();
        let mut raw = AudioFeatures {
            // Prime with high value
            sub_bass: 1.0,
            ..Default::default()
        };
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
        let mut raw = AudioFeatures {
            // Prime with many frames so state converges to 0.5
            rms: 0.5,
            ..Default::default()
        };
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
        let raw = AudioFeatures {
            mid: 0.7,
            ..Default::default()
        };
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
        let raw = AudioFeatures {
            sub_bass: 0.5,
            rms: 0.3,
            onset: 0.8,
            ..Default::default()
        };
        let out = smoother.smooth(&raw, 0.016);
        for v in out.as_slice() {
            assert!(v.is_finite(), "non-finite value found");
        }
    }
}
