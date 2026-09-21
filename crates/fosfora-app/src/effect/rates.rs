//! Rate parameters for `.pfx` layer effects (#2984).
//!
//! A shader that computes `u.time * speed` multiplies every CHANGE in speed by
//! how long the app has been running, so a speed moved by a binding or a drag
//! jumps the picture by (change x uptime) instead of changing how fast it moves.
//! Measured on the GPU, a 0.01 nudge cost 884x-12,000x more at a 300 s clock than
//! at a fresh one. The frame-correct quantity is the running integral of the
//! speed, which only the engine can keep, because it lives across frames.
//!
//! An effect lists such parameters under `"rates"` in its `.pfx`. Each frame the
//! engine adds `value * dt` to each one's integral and writes the integral into
//! its own slot AFTER the packed params, in the order `"rates"` names them
//! (a Point2D takes two). The parameter's own slot is left alone: unlike trama,
//! where the integral replaces the value, a `.pfx` shader often needs the value
//! too — Chromatica's rotation speed is also a gain on the beat, Tunnel's twist
//! also sets the rib twist, Frost's drift also sets the smear direction.
//!
//! A shader that remapped the value (`t * (a * p + b)`) keeps its remap by
//! writing `a * integral + b * u.time`, since the integral of `a * p + b` is
//! exactly that.
//!
//! The integral advances by the app's frame `dt`, which is clamped to 50 ms;
//! `u.time` is not. After a longer stall the integrated part therefore lags the
//! `b * u.time` part by the excess, once — a small fixed offset, not a jump, and
//! the same "slow motion through a stall" trade the particle sims make.
//!
//! Where the rate is not one param — a product of params (Tunnel's rib screw)
//! or a function of live audio (Frost's wander) — the engine cannot integrate
//! it; a small feedback pass does, with `phase_pack` / `phase_unpack`
//! (lib/chronoflow.wgsl).

use crate::params::{ParamDef, ParamStore, ParamValue};

/// One rate parameter's place in the packed uniform buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateSlot {
    pub name: String,
    /// Where the param's own value is packed.
    pub src: usize,
    /// Where its integral is written.
    pub dst: usize,
    /// 1 for a Float, 2 for a Point2D.
    pub width: usize,
}

/// Resolve `"rates"` against an effect's params. Refuses a name that is not a
/// Float or Point2D param, a name listed twice, and a layout that would run past
/// the 16 packed slots — each would otherwise hand a shader a slot that silently
/// holds something else.
pub fn layout(inputs: &[ParamDef], rates: &[String]) -> Result<Vec<RateSlot>, String> {
    let (offsets, mut next) = ParamStore::packed_offsets(inputs);
    let mut out: Vec<RateSlot> = Vec::with_capacity(rates.len());
    for name in rates {
        if out.iter().any(|r| r.name == *name) {
            return Err(format!("`rates` lists `{name}` twice"));
        }
        let Some(i) = inputs.iter().position(|d| d.name() == name) else {
            return Err(format!("`rates` names `{name}`, which is not a parameter"));
        };
        let width = match inputs[i].default_value() {
            ParamValue::Float(_) => 1,
            ParamValue::Point2D(_) => 2,
            _ => {
                return Err(format!(
                    "`rates` names `{name}`, which is not a Float or Point2D parameter"
                ));
            }
        };
        let Some(src) = offsets[i] else {
            return Err(format!("`rates` names `{name}`, which is not packed"));
        };
        if next + width > 16 {
            return Err(format!(
                "`rates` needs slots {next}..{} for `{name}`, past the 16 available",
                next + width
            ));
        }
        out.push(RateSlot {
            name: name.clone(),
            src,
            dst: next,
            width,
        });
        next += width;
    }
    Ok(out)
}

/// A layer's rate integrals. Runtime-only, like modulation state: never saved,
/// and started from zero whenever an effect is loaded.
#[derive(Debug, Clone, Default)]
pub struct RateState {
    slots: Vec<RateSlot>,
    /// One running total per float, in slot order. f64 on the CPU so the sum
    /// does not lose the step; the shader receives it as f32, the same
    /// precision `u.time` already has.
    phases: Vec<f64>,
}

impl RateState {
    pub fn new(slots: Vec<RateSlot>) -> Self {
        let n = slots.iter().map(|s| s.width).sum();
        Self {
            slots,
            phases: vec![0.0; n],
        }
    }

    /// Integrate this frame's packed values and write the totals into their
    /// slots. Call after the params are packed and before upload.
    pub fn advance(&mut self, buf: &mut [f32; 16], dt: f32) {
        let mut k = 0;
        for s in &self.slots {
            for c in 0..s.width {
                self.phases[k] += f64::from(buf[s.src + c]) * f64::from(dt);
                buf[s.dst + c] = self.phases[k] as f32;
                k += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn float(name: &str) -> ParamDef {
        ParamDef::Float {
            name: name.into(),
            default: 0.5,
            min: 0.0,
            max: 1.0,
        }
    }

    fn point(name: &str) -> ParamDef {
        ParamDef::Point2D {
            name: name.into(),
            default: [0.0, 0.0],
            min: [-1.0, -1.0],
            max: [1.0, 1.0],
        }
    }

    #[test]
    fn integrals_go_after_the_packed_params_in_declared_order() {
        let inputs = [float("a"), point("drift"), float("speed")];
        let got = layout(&inputs, &["speed".into(), "drift".into()]).unwrap();
        // a=0, drift=1..3, speed=3 -> integrals from 4.
        assert_eq!(
            got,
            [
                RateSlot {
                    name: "speed".into(),
                    src: 3,
                    dst: 4,
                    width: 1
                },
                RateSlot {
                    name: "drift".into(),
                    src: 1,
                    dst: 5,
                    width: 2
                },
            ]
        );
    }

    #[test]
    fn layout_refuses_what_would_hand_a_shader_the_wrong_slot() {
        let inputs = [
            float("speed"),
            ParamDef::Bool {
                name: "on".into(),
                default: true,
            },
        ];
        let err = |r: &[&str]| {
            let r: Vec<String> = r.iter().map(|s| s.to_string()).collect();
            layout(&inputs, &r).unwrap_err()
        };
        assert!(err(&["nope"]).contains("not a parameter"));
        assert!(err(&["on"]).contains("not a Float or Point2D"));
        assert!(err(&["speed", "speed"]).contains("twice"));

        let full: Vec<ParamDef> = (0..16).map(|i| float(&format!("p{i}"))).collect();
        assert!(
            layout(&full, &["p0".into()])
                .unwrap_err()
                .contains("past the 16")
        );
    }

    // The property the whole mechanism exists for: a change in the rate changes
    // the NEXT step only. With `t * speed`, moving speed from 0.5 to 0.51 at
    // t=300 moved the phase by 3.0; integrated, it moves the step by 0.01 * dt.
    #[test]
    fn a_change_in_rate_changes_only_the_next_step() {
        let inputs = [float("speed")];
        let mut st = RateState::new(layout(&inputs, &["speed".into()]).unwrap());
        let dt = 1.0 / 60.0;
        let mut buf = [0.0f32; 16];
        buf[0] = 0.5;
        for _ in 0..18_000 {
            st.advance(&mut buf, dt);
        }
        let before = buf[1];
        assert!(
            (f64::from(before) - 150.0).abs() < 1e-2,
            "300 s at 0.5 -> {before}"
        );

        buf[0] = 0.51;
        st.advance(&mut buf, dt);
        let step = f64::from(buf[1] - before);
        assert!(
            (step - 0.51 * f64::from(dt)).abs() < 1e-4,
            "one frame after the change the phase moved {step}, not 0.51 * dt"
        );
        // The value's own slot is untouched: shaders may still read it.
        assert_eq!(buf[0], 0.51);
    }

    #[test]
    fn a_point2d_rate_integrates_each_axis() {
        let inputs = [point("drift")];
        let mut st = RateState::new(layout(&inputs, &["drift".into()]).unwrap());
        let mut buf = [0.0f32; 16];
        buf[0] = 1.0;
        buf[1] = -0.5;
        for _ in 0..60 {
            st.advance(&mut buf, 1.0 / 60.0);
        }
        assert!((buf[2] - 1.0).abs() < 1e-5 && (buf[3] + 0.5).abs() < 1e-5);
    }

    // A particle sim receives only slots 0-7 (frame_prep forwards p[0..8];
    // particle_lib's param() reads two vec4s). An integral placed at 8 or above
    // compiles, and the sim silently reads past its params — the reason Tesla,
    // which packs all eight, is not converted yet (board #3039).
    #[test]
    fn particle_sims_only_read_rate_slots_they_receive() {
        let shaders = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/shaders");
        for effect in crate::effect::loader::shipped_effects_for_test() {
            let Some(sim) = effect
                .particles
                .as_ref()
                .map(|p| p.compute_shader.clone())
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let src = std::fs::read_to_string(shaders.join(&sim)).unwrap_or_default();
            for s in layout(&effect.inputs, &effect.rates).unwrap_or_default() {
                for slot in s.dst..s.dst + s.width {
                    if src.contains(&format!("param({slot}u)")) {
                        assert!(
                            slot < 8,
                            "{}: {sim} reads the `{}` integral at slot {slot}, which \
                             particle sims never receive",
                            effect.name,
                            s.name
                        );
                    }
                }
            }
        }
    }

    // Every shipped effect's `"rates"` must resolve; a load would fail on it
    // otherwise, and only on the machine of whoever picks that effect.
    #[test]
    fn every_shipped_rate_layout_resolves() {
        for effect in crate::effect::loader::shipped_effects_for_test() {
            if let Err(e) = layout(&effect.inputs, &effect.rates) {
                panic!("{}: {e}", effect.name);
            }
        }
    }
}
