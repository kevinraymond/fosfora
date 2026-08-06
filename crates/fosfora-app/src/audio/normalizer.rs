use super::features::{AudioFeatures, NUM_FEATURES};
use super::ranging::PercentileWindow;
use super::schema::{FEATURES, NormPolicy};

/// Number of recent frames each Adaptive feature ranges over (~4 s at the fixed
/// 512-sample hop / 44.1 kHz). This length is the spike-recovery knob: a transient
/// leaves the window after ~4 s, so the old "if v < 0.2·hi for 30 frames" decay is gone.
const ADAPTIVE_WINDOW: usize = 344;

/// Percentile bounds for adaptive ranging. P5/P95 (not min/max) reject the outliers that
/// made the old running-max pump: one spike no longer defines the top of the range.
const P_LO: f32 = 0.05;
const P_HI: f32 = 0.95;

/// Below this P95−P5 span an Adaptive feature is treated as flat → 0 (guards div-by-zero
/// and stops quiet-room noise from being stretched to full scale).
const SPAN_EPS: f32 = 1e-6;

/// Soft-knee onset for Adaptive output: linear up to `KNEE`, then asymptote to 1.0 so
/// transients above P95 still read as "louder" instead of hard-clipping flat at 1.0.
const KNEE: f32 = 0.85;

/// EMA rate for the ZScore running mean/variance (~1.2 s time constant at 512-hop).
const Z_ALPHA: f32 = 0.01;

/// tanh softness for the ZScore → 0..1 map: ±3σ lands near 0/1, so ordinary MFCC
/// excursions use the middle of the range symmetrically about 0.5.
const Z_SOFT: f32 = 3.0;

/// Per-feature normalization to 0..1, dispatched by each slot's [`NormPolicy`]
/// (A2 #1453). Replaces the old symmetric running-min/max `AdaptiveNormalizer`, whose
/// single policy (a) stretched quiet-room noise to full scale, (b) pumped for ~2 s after
/// any spike, and (c) applied the same min/max to signed MFCCs and already-normed chroma
/// as to energy bands.
///
/// - [`Adaptive`](NormPolicy::Adaptive) — gated percentile ranging over a windowed
///   history; silence-gated to 0 with the window frozen.
/// - [`FixedRange`](NormPolicy::FixedRange) — clamp a known 0..1 feature; hold the last
///   value on silence instead of dancing to room noise.
/// - [`ZScore`](NormPolicy::ZScore) — standardize a signed feature by a running
///   mean/variance and map through tanh.
/// - [`Passthrough`](NormPolicy::Passthrough) — copy a producer-owned field unchanged.
pub struct FeatureNormalizer {
    /// Windowed history per slot (only Adaptive slots are pushed/queried).
    windows: Vec<PercentileWindow>,
    /// Running mean/variance per slot (only ZScore slots are updated).
    z_mean: [f32; NUM_FEATURES],
    z_var: [f32; NUM_FEATURES],
    /// Last emitted value per slot — FixedRange holds this through a silence gate.
    fixed_last: [f32; NUM_FEATURES],
}

impl FeatureNormalizer {
    pub fn new() -> Self {
        Self {
            windows: (0..NUM_FEATURES)
                .map(|_| PercentileWindow::new(ADAPTIVE_WINDOW))
                .collect(),
            z_mean: [0.0; NUM_FEATURES],
            z_var: [1.0; NUM_FEATURES],
            fixed_last: [0.0; NUM_FEATURES],
        }
    }

    /// Normalize all features to 0..1 per their [`NormPolicy`]. `loud_silent` is the A10
    /// perceptual silence gate (`LoudnessMeter::is_silent`): when set, energy features read
    /// 0 with their adaptation frozen, FixedRange features hold their last value, and
    /// ZScore features read the neutral midpoint without updating their stats.
    pub fn normalize(&mut self, raw: &AudioFeatures, loud_silent: bool) -> AudioFeatures {
        let raw_slice = raw.as_slice();
        let mut out = AudioFeatures::default();
        let out_slice = out.as_slice_mut();

        for i in 0..NUM_FEATURES {
            let v = raw_slice[i];
            out_slice[i] = match FEATURES[i].norm {
                NormPolicy::Passthrough => v,

                NormPolicy::Adaptive => {
                    if loud_silent {
                        // Freeze the window (don't push silence) and read 0.
                        0.0
                    } else {
                        self.windows[i].push(v);
                        let (p_lo, p_hi) = self.windows[i].range(P_LO, P_HI);
                        soft_norm(v, p_lo, p_hi)
                    }
                }

                NormPolicy::FixedRange => {
                    if loud_silent {
                        self.fixed_last[i]
                    } else {
                        let c = v.clamp(0.0, 1.0);
                        self.fixed_last[i] = c;
                        c
                    }
                }

                NormPolicy::ZScore => {
                    if loud_silent {
                        0.5
                    } else {
                        let mean = self.z_mean[i];
                        let var = self.z_var[i];
                        let z = (v - mean) / (var + 1e-6).sqrt();
                        // Update running stats (EMA mean + EWMA variance).
                        let delta = v - mean;
                        self.z_mean[i] = mean + Z_ALPHA * delta;
                        self.z_var[i] = (1.0 - Z_ALPHA) * (var + Z_ALPHA * delta * delta);
                        0.5 + 0.5 * (z / Z_SOFT).tanh()
                    }
                }
            };
        }

        out
    }
}

/// Map `v` into 0..1 given its windowed `p_lo`/`p_hi`, with a soft knee above P95.
fn soft_norm(v: f32, p_lo: f32, p_hi: f32) -> f32 {
    let span = p_hi - p_lo;
    if span < SPAN_EPS {
        return 0.0;
    }
    soft_clip01((v - p_lo) / span)
}

/// Clamp to 0..1 but with a soft knee: linear to `KNEE`, then asymptote to 1.0.
fn soft_clip01(x: f32) -> f32 {
    if x <= 0.0 {
        0.0
    } else if x <= KNEE {
        x
    } else {
        KNEE + (1.0 - KNEE) * (1.0 - (-(x - KNEE) / (1.0 - KNEE)).exp())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn all_zero_stays_finite() {
        let mut norm = FeatureNormalizer::new();
        let out = norm.normalize(&AudioFeatures::default(), false);
        for &v in out.as_slice() {
            assert!(v.is_finite());
        }
    }

    #[test]
    fn silence_gates_energy_to_zero() {
        let mut norm = FeatureNormalizer::new();
        // Warm up the rms window with real signal so it has a range.
        for i in 0..200 {
            let raw = AudioFeatures {
                rms: (i % 50) as f32 / 50.0,
                ..Default::default()
            };
            norm.normalize(&raw, false);
        }
        // Now a loud rms but the perceptual gate says silent → 0.
        let raw = AudioFeatures {
            rms: 0.9,
            sub_bass: 0.9,
            ..Default::default()
        };
        let out = norm.normalize(&raw, true);
        assert_eq!(out.rms, 0.0);
        assert_eq!(out.sub_bass, 0.0);
    }

    #[test]
    fn adaptive_ranges_high_and_low() {
        let mut norm = FeatureNormalizer::new();
        // A varied history (sawtooth 0..~1) so P5/P95 straddle the range.
        for i in 0..ADAPTIVE_WINDOW {
            let raw = AudioFeatures {
                rms: (i % 100) as f32 / 100.0,
                ..Default::default()
            };
            norm.normalize(&raw, false);
        }
        let hi = norm
            .normalize(
                &AudioFeatures {
                    rms: 0.99,
                    ..Default::default()
                },
                false,
            )
            .rms;
        let lo = norm
            .normalize(
                &AudioFeatures {
                    rms: 0.0,
                    ..Default::default()
                },
                false,
            )
            .rms;
        assert!(hi > 0.6, "top of range should read high, got {hi}");
        assert!(lo < 0.2, "bottom of range should read low, got {lo}");
        assert!((0.0..=1.0).contains(&hi));
    }

    #[test]
    fn fixedrange_clamps_and_holds_on_silence() {
        let mut norm = FeatureNormalizer::new();
        // In-range value passes through; out-of-range clamps.
        assert!(approx_eq(
            norm.normalize(
                &AudioFeatures {
                    centroid: 0.7,
                    ..Default::default()
                },
                false
            )
            .centroid,
            0.7,
            1e-6
        ));
        assert!(approx_eq(
            norm.normalize(
                &AudioFeatures {
                    centroid: 2.0,
                    ..Default::default()
                },
                false
            )
            .centroid,
            1.0,
            1e-6
        ));
        // Silence holds the last value (1.0), not the new 0.1.
        let held = norm
            .normalize(
                &AudioFeatures {
                    centroid: 0.1,
                    ..Default::default()
                },
                true,
            )
            .centroid;
        assert!(approx_eq(held, 1.0, 1e-6), "held={held}");
    }

    #[test]
    fn zscore_centers_constant_mfcc() {
        let mut norm = FeatureNormalizer::new();
        let mut raw = AudioFeatures::default();
        raw.mfcc[0] = 5.0;
        let mut out = 0.0;
        for _ in 0..500 {
            out = norm.normalize(&raw, false).mfcc[0];
        }
        // A constant coefficient converges to the running mean → z≈0 → 0.5.
        assert!(
            approx_eq(out, 0.5, 0.05),
            "constant mfcc should center, got {out}"
        );
    }

    #[test]
    fn passthrough_untouched() {
        let mut norm = FeatureNormalizer::new();
        let mut raw = AudioFeatures {
            beat: 1.0,
            beat_phase: 0.7,
            bpm: 0.4,
            beat_strength: 0.9,
            dominant_chroma: 0.55,
            ..Default::default()
        };
        raw.chroma[3] = 0.8;
        for _ in 0..50 {
            let out = norm.normalize(&raw, false);
            assert!(approx_eq(out.beat, 1.0, 1e-6));
            assert!(approx_eq(out.beat_phase, 0.7, 1e-6));
            assert!(approx_eq(out.bpm, 0.4, 1e-6));
            assert!(approx_eq(out.beat_strength, 0.9, 1e-6));
            assert!(approx_eq(out.dominant_chroma, 0.55, 1e-6));
            assert!(approx_eq(out.chroma[3], 0.8, 1e-6));
        }
    }
}
