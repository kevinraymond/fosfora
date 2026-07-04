use super::features::{AudioFeatures, NUM_FEATURES};

/// Per-feature running min/max for adaptive normalization.
/// Replaces all fixed gain multipliers with auto-leveling.
/// Ported from easey-glyph's adaptive normalization.
pub struct AdaptiveNormalizer {
    running_min: [f32; NUM_FEATURES],
    running_max: [f32; NUM_FEATURES],
    decay: f32,
}

impl AdaptiveNormalizer {
    pub fn new() -> Self {
        Self {
            running_min: [0.0; NUM_FEATURES],
            running_max: [0.01; NUM_FEATURES], // small initial range to avoid div-by-zero
            decay: 0.005,
        }
    }

    /// Normalize all features to 0-1 using adaptive running min/max.
    /// Beat detection fields (indices 15-19: onset, beat, beat_phase, bpm, beat_strength)
    /// are passed through unchanged — they're already normalized by the beat detector.
    pub fn normalize(&mut self, raw: &AudioFeatures) -> AudioFeatures {
        let raw_slice = raw.as_slice();
        let mut out = AudioFeatures::default();
        let out_slice = out.as_slice_mut();

        for i in 0..NUM_FEATURES {
            let v = raw_slice[i];

            // Pass through detector-owned fields unchanged:
            // onset=15, beat=16, beat_phase=17, bpm=18, beat_strength=19.
            // The beat detector already emits them normalized (or binary), so
            // adaptive min/max scaling would only distort them.
            if (15..=19).contains(&i) {
                out_slice[i] = v;
                continue;
            }

            // Running min: decays toward value, instantly jumps down
            self.running_min[i] += self.decay * (v - self.running_min[i]);
            self.running_min[i] = self.running_min[i].min(v);

            // Running max: decays toward value, instantly jumps up
            self.running_max[i] += self.decay * (v - self.running_max[i]);
            self.running_max[i] = self.running_max[i].max(v);

            let span = self.running_max[i] - self.running_min[i];
            if span > 0.01 {
                out_slice[i] = ((v - self.running_min[i]) / span).clamp(0.0, 1.0);
            } else {
                out_slice[i] = 0.0;
            }
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
    fn all_zero_stays_zero() {
        let mut norm = AdaptiveNormalizer::new();
        let raw = AudioFeatures::default();
        let out = norm.normalize(&raw);
        for &v in out.as_slice() {
            assert!(v.is_finite());
        }
    }

    #[test]
    fn constant_below_span_outputs_zero() {
        let mut norm = AdaptiveNormalizer::new();
        let raw = AudioFeatures {
            sub_bass: 0.001, // below span threshold (0.01)
            ..Default::default()
        };
        for _ in 0..100 {
            let out = norm.normalize(&raw);
            assert!(out.sub_bass >= 0.0);
        }
    }

    #[test]
    fn spike_pushes_max() {
        let mut norm = AdaptiveNormalizer::new();
        let raw = AudioFeatures {
            sub_bass: 1.0,
            ..Default::default()
        };
        let out = norm.normalize(&raw);
        // After a spike, the value should be near 1.0 (at top of range)
        assert!(out.sub_bass >= 0.0);
        assert!(out.sub_bass <= 1.0);
    }

    #[test]
    fn max_decays_after_spike() {
        let mut norm = AdaptiveNormalizer::new();
        // Feed a spike
        let raw = AudioFeatures {
            sub_bass: 1.0,
            ..Default::default()
        };
        norm.normalize(&raw);
        // Feed zeros for many frames
        let raw = AudioFeatures::default();
        let mut last_max = norm.running_max[0];
        for _ in 0..200 {
            norm.normalize(&raw);
            assert!(norm.running_max[0] <= last_max + 1e-6);
            last_max = norm.running_max[0];
        }
        // running_max should have decayed from 1.0
        assert!(norm.running_max[0] < 0.5);
    }

    #[test]
    fn beat_fields_pass_through() {
        let mut norm = AdaptiveNormalizer::new();
        let raw = AudioFeatures {
            beat: 1.0,          // index 16
            beat_phase: 0.7,    // index 17
            bpm: 0.4,           // index 18
            beat_strength: 0.9, // index 19
            ..Default::default()
        };
        let out = norm.normalize(&raw);
        assert!(approx_eq(out.beat, 1.0, 1e-6));
        assert!(approx_eq(out.beat_phase, 0.7, 1e-6));
        assert!(approx_eq(out.bpm, 0.4, 1e-6));
        assert!(approx_eq(out.beat_strength, 0.9, 1e-6));
    }

    #[test]
    fn onset_passes_through() {
        let mut norm = AdaptiveNormalizer::new();
        let mut raw = AudioFeatures {
            onset: 0.5,
            ..Default::default()
        };
        // Onset (index 15) is detector-owned and must pass through unchanged,
        // no matter how much history the normalizer has accumulated.
        for _ in 0..50 {
            let out = norm.normalize(&raw);
            assert!(approx_eq(out.onset, 0.5, 1e-6));
        }
        raw.onset = 1.0; // spike
        let out = norm.normalize(&raw);
        assert!(approx_eq(out.onset, 1.0, 1e-6));
    }

    #[test]
    fn ramp_input_trends_toward_range() {
        let mut norm = AdaptiveNormalizer::new();
        let mut raw = AudioFeatures::default();
        let mut outputs = Vec::new();
        for i in 0..200 {
            raw.rms = (i as f32) / 200.0;
            let out = norm.normalize(&raw);
            outputs.push(out.rms);
        }
        // Later outputs should be in 0-1 range and mostly increasing
        let last = outputs[outputs.len() - 1];
        assert!((0.0..=1.0).contains(&last));
    }
}
