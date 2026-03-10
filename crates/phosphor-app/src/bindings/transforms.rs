use super::types::{BindingRuntime, TransformDef};

/// Apply a chain of transforms to a normalized input value.
/// Updates runtime state (e.g. smooth EMA). No allocations.
pub fn apply_chain(value: f32, transforms: &[TransformDef], runtime: &mut BindingRuntime) -> f32 {
    let mut v = value;
    for t in transforms {
        v = apply_single(v, t, runtime);
    }
    v
}

fn apply_single(value: f32, transform: &TransformDef, runtime: &mut BindingRuntime) -> f32 {
    match transform {
        TransformDef::Remap {
            in_lo,
            in_hi,
            out_lo,
            out_hi,
        } => {
            if (in_hi - in_lo).abs() < f32::EPSILON {
                *out_lo
            } else {
                let t = ((value - in_lo) / (in_hi - in_lo)).clamp(0.0, 1.0);
                out_lo + (out_hi - out_lo) * t
            }
        }
        TransformDef::Smooth { factor } => {
            let f = factor.clamp(0.0, 0.999);
            runtime.smooth_state = runtime.smooth_state * f + value * (1.0 - f);
            runtime.smooth_state
        }
        TransformDef::Invert => 1.0 - value,
        TransformDef::Quantize { steps } => {
            if *steps == 0 {
                return value;
            }
            let s = *steps as f32;
            (value * s).round() / s
        }
        TransformDef::Deadzone { lo, hi } => {
            if value >= *lo && value <= *hi {
                0.0
            } else if value < *lo {
                // Rescale [0, lo) -> [0, lo/(lo + 1-hi))
                if *lo > f32::EPSILON {
                    value / *lo * (*lo / (*lo + 1.0 - *hi))
                } else {
                    0.0
                }
            } else {
                // Rescale (hi, 1] -> (lo/(lo+1-hi), 1]
                let live_range = *lo + 1.0 - *hi;
                if live_range > f32::EPSILON {
                    let offset = *lo / live_range;
                    offset + (value - *hi) / (1.0 - *hi) * (1.0 - offset)
                } else {
                    1.0
                }
            }
        }
        TransformDef::Curve { curve_type } => apply_curve(value, curve_type),
        TransformDef::Gate { threshold } => {
            if value >= *threshold {
                1.0
            } else {
                0.0
            }
        }
        TransformDef::Scale { factor } => value * factor,
        TransformDef::Offset { value: offset } => value + offset,
        TransformDef::Clamp { lo, hi } => value.clamp(*lo, *hi),
    }
}

fn apply_curve(value: f32, curve_type: &str) -> f32 {
    let v = value.clamp(0.0, 1.0);
    match curve_type {
        "linear" => v,
        "ease_in" => v * v,
        "ease_out" => 1.0 - (1.0 - v) * (1.0 - v),
        "ease_in_out" => v * v * (3.0 - 2.0 * v), // smoothstep
        "log" => v.sqrt(),
        "exp" => v * v * v,
        _ => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt() -> BindingRuntime {
        BindingRuntime::new()
    }

    #[test]
    fn test_remap() {
        let mut r = rt();
        let t = [TransformDef::Remap {
            in_lo: 0.0,
            in_hi: 1.0,
            out_lo: 0.2,
            out_hi: 0.8,
        }];
        assert!((apply_chain(0.0, &t, &mut r) - 0.2).abs() < 1e-5);
        assert!((apply_chain(1.0, &t, &mut r) - 0.8).abs() < 1e-5);
        assert!((apply_chain(0.5, &t, &mut r) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn test_remap_clamped() {
        let mut r = rt();
        let t = [TransformDef::Remap {
            in_lo: 0.2,
            in_hi: 0.8,
            out_lo: 0.0,
            out_hi: 1.0,
        }];
        // Below in_lo clamps to out_lo
        assert!((apply_chain(0.0, &t, &mut r) - 0.0).abs() < 1e-5);
        // Above in_hi clamps to out_hi
        assert!((apply_chain(1.0, &t, &mut r) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_smooth() {
        let mut r = rt();
        let t = [TransformDef::Smooth { factor: 0.5 }];
        // First call: 0 * 0.5 + 1.0 * 0.5 = 0.5
        assert!((apply_chain(1.0, &t, &mut r) - 0.5).abs() < 1e-5);
        // Second call: 0.5 * 0.5 + 1.0 * 0.5 = 0.75
        assert!((apply_chain(1.0, &t, &mut r) - 0.75).abs() < 1e-5);
    }

    #[test]
    fn test_invert() {
        let mut r = rt();
        let t = [TransformDef::Invert];
        assert!((apply_chain(0.0, &t, &mut r) - 1.0).abs() < 1e-5);
        assert!((apply_chain(0.7, &t, &mut r) - 0.3).abs() < 1e-5);
    }

    #[test]
    fn test_quantize() {
        let mut r = rt();
        let t = [TransformDef::Quantize { steps: 4 }];
        assert!((apply_chain(0.1, &t, &mut r) - 0.0).abs() < 1e-5); // 0.1*4=0.4, rounds to 0
        assert!((apply_chain(0.3, &t, &mut r) - 0.25).abs() < 1e-5); // 0.3*4=1.2, rounds to 1
        assert!((apply_chain(0.6, &t, &mut r) - 0.5).abs() < 1e-5); // 0.6*4=2.4, rounds to 2
        assert!((apply_chain(0.9, &t, &mut r) - 1.0).abs() < 1e-5); // 0.9*4=3.6, rounds to 4
    }

    #[test]
    fn test_gate() {
        let mut r = rt();
        let t = [TransformDef::Gate { threshold: 0.5 }];
        assert!((apply_chain(0.49, &t, &mut r)).abs() < 1e-5);
        assert!((apply_chain(0.5, &t, &mut r) - 1.0).abs() < 1e-5);
        assert!((apply_chain(1.0, &t, &mut r) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_scale() {
        let mut r = rt();
        let t = [TransformDef::Scale { factor: 2.0 }];
        assert!((apply_chain(0.5, &t, &mut r) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_offset() {
        let mut r = rt();
        let t = [TransformDef::Offset { value: 0.1 }];
        assert!((apply_chain(0.5, &t, &mut r) - 0.6).abs() < 1e-5);
    }

    #[test]
    fn test_clamp() {
        let mut r = rt();
        let t = [TransformDef::Clamp { lo: 0.2, hi: 0.8 }];
        assert!((apply_chain(0.0, &t, &mut r) - 0.2).abs() < 1e-5);
        assert!((apply_chain(0.5, &t, &mut r) - 0.5).abs() < 1e-5);
        assert!((apply_chain(1.0, &t, &mut r) - 0.8).abs() < 1e-5);
    }

    #[test]
    fn test_deadzone() {
        let mut r = rt();
        let t = [TransformDef::Deadzone { lo: 0.4, hi: 0.6 }];
        // Inside deadzone -> 0
        assert!((apply_chain(0.5, &t, &mut r)).abs() < 1e-5);
        // Below deadzone -> rescaled
        let v = apply_chain(0.2, &t, &mut r);
        assert!(v > 0.0 && v < 0.5);
        // Above deadzone -> rescaled
        let v = apply_chain(0.8, &t, &mut r);
        assert!(v > 0.5 && v <= 1.0);
    }

    #[test]
    fn test_curve_ease_in_out() {
        let mut r = rt();
        let t = [TransformDef::Curve {
            curve_type: "ease_in_out".into(),
        }];
        // Smoothstep: 0 at 0, 1 at 1, 0.5 at 0.5
        assert!((apply_chain(0.0, &t, &mut r)).abs() < 1e-5);
        assert!((apply_chain(1.0, &t, &mut r) - 1.0).abs() < 1e-5);
        assert!((apply_chain(0.5, &t, &mut r) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn test_chain_composition() {
        let mut r = rt();
        // gate(0.5) -> invert -> clamp(0, 1)
        let chain = [
            TransformDef::Gate { threshold: 0.5 },
            TransformDef::Invert,
            TransformDef::Clamp { lo: 0.0, hi: 1.0 },
        ];
        // 0.3 -> gate=0 -> invert=1 -> clamp=1
        assert!((apply_chain(0.3, &chain, &mut r) - 1.0).abs() < 1e-5);
        // 0.7 -> gate=1 -> invert=0 -> clamp=0
        assert!((apply_chain(0.7, &chain, &mut r)).abs() < 1e-5);
    }
}
