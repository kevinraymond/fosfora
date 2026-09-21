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
//! A param that is a PERIOD rather than a speed — Etch's `clear_cycle`, in
//! seconds per clear — is declared `{"param": "clear_cycle", "period": true}`:
//! the engine integrates `dt / value`, the cycles completed, which is what
//! `fract(u.time / period)` was trying to be. A period of zero or less adds
//! nothing, so a shader can keep giving it its own meaning.
//!
//! Where the rate is not one param — a product of params (Tunnel's rib screw)
//! or a function of live audio (Frost's wander) — the engine cannot integrate
//! it; a small feedback pass does, with `phase_pack` / `phase_unpack`
//! (lib/chronoflow.wgsl).

use serde::{Deserialize, Serialize};

use crate::params::{ParamDef, ParamStore, ParamValue};

/// One `"rates"` entry: a param name (integrate its value), or
/// `{"param": ..., "period": true}` (integrate its reciprocal).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RateDef {
    Name(String),
    Spec {
        param: String,
        #[serde(default)]
        period: bool,
    },
}

impl RateDef {
    pub fn param(&self) -> &str {
        match self {
            RateDef::Name(n) => n,
            RateDef::Spec { param, .. } => param,
        }
    }

    pub fn period(&self) -> bool {
        matches!(self, RateDef::Spec { period: true, .. })
    }
}

impl From<&str> for RateDef {
    fn from(name: &str) -> Self {
        RateDef::Name(name.to_string())
    }
}

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
    /// Integrate `dt / value` (a period) instead of `value * dt` (a speed).
    pub period: bool,
}

/// Resolve `"rates"` against an effect's params. Refuses a name that is not a
/// Float or Point2D param, a name listed twice, and a layout that would run past
/// the 16 packed slots — each would otherwise hand a shader a slot that silently
/// holds something else.
pub fn layout(inputs: &[ParamDef], rates: &[RateDef]) -> Result<Vec<RateSlot>, String> {
    let (offsets, mut next) = ParamStore::packed_offsets(inputs);
    let mut out: Vec<RateSlot> = Vec::with_capacity(rates.len());
    for rate in rates {
        let name = rate.param();
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
        if rate.period() && width != 1 {
            return Err(format!(
                "`rates` makes `{name}` a period, which needs a Float"
            ));
        }
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
            name: name.to_string(),
            src,
            dst: next,
            width,
            period: rate.period(),
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
                let v = f64::from(buf[s.src + c]);
                self.phases[k] += if !s.period {
                    v * f64::from(dt)
                } else if v > 0.0 {
                    f64::from(dt) / v
                } else {
                    0.0
                };
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
                    width: 1,
                    period: false
                },
                RateSlot {
                    name: "drift".into(),
                    src: 1,
                    dst: 5,
                    width: 2,
                    period: false
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
            let r: Vec<RateDef> = r.iter().map(|s| RateDef::from(*s)).collect();
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

        let period = RateDef::Spec {
            param: "drift".into(),
            period: true,
        };
        assert!(
            layout(&[point("drift")], &[period])
                .unwrap_err()
                .contains("needs a Float")
        );
    }

    // Both spellings parse, and a bare name stays a bare name when written back.
    #[test]
    fn rate_entries_parse_as_a_name_or_a_spec() {
        let got: Vec<RateDef> =
            serde_json::from_str(r#"["speed", {"param": "clear_cycle", "period": true}]"#).unwrap();
        assert_eq!(got[0].param(), "speed");
        assert!(!got[0].period());
        assert_eq!(got[1].param(), "clear_cycle");
        assert!(got[1].period());
        assert_eq!(serde_json::to_string(&got[0]).unwrap(), r#""speed""#);
    }

    // Etch's clear clock: seconds per clear in, cycles completed out. A change in
    // the period changes the pace from the next frame, and zero stops the clock
    // rather than dividing by it.
    #[test]
    fn a_period_integrates_cycles_completed() {
        let inputs = [float("clear_cycle")];
        let rates = [RateDef::Spec {
            param: "clear_cycle".into(),
            period: true,
        }];
        let mut st = RateState::new(layout(&inputs, &rates).unwrap());
        let dt = 1.0 / 60.0;
        let mut buf = [0.0f32; 16];
        buf[0] = 12.0;
        for _ in 0..3600 {
            st.advance(&mut buf, dt);
        }
        assert!(
            (buf[1] - 5.0).abs() < 1e-3,
            "60 s at 12 s per clear -> {}",
            buf[1]
        );

        let before = buf[1];
        buf[0] = 12.5;
        st.advance(&mut buf, dt);
        assert!(
            (f64::from(buf[1] - before) - f64::from(dt) / 12.5).abs() < 1e-6,
            "a new period changes the next step only"
        );

        let held = buf[1];
        buf[0] = 0.0;
        st.advance(&mut buf, dt);
        assert_eq!(
            buf[1], held,
            "a zero period must not advance (or divide by zero)"
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
