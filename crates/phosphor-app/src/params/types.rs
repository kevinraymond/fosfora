use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ParamDef {
    Float {
        name: String,
        default: f32,
        min: f32,
        max: f32,
    },
    Color {
        name: String,
        default: [f32; 4],
    },
    Bool {
        name: String,
        default: bool,
    },
    Point2D {
        name: String,
        default: [f32; 2],
        min: [f32; 2],
        max: [f32; 2],
    },
}

impl ParamDef {
    pub fn name(&self) -> &str {
        match self {
            ParamDef::Float { name, .. } => name,
            ParamDef::Color { name, .. } => name,
            ParamDef::Bool { name, .. } => name,
            ParamDef::Point2D { name, .. } => name,
        }
    }

    pub fn default_value(&self) -> ParamValue {
        match self {
            ParamDef::Float { default, .. } => ParamValue::Float(*default),
            ParamDef::Color { default, .. } => ParamValue::Color(*default),
            ParamDef::Bool { default, .. } => ParamValue::Bool(*default),
            ParamDef::Point2D { default, .. } => ParamValue::Point2D(*default),
        }
    }

    /// Update the default value from a ParamValue. Type must match (ignored otherwise).
    pub fn set_default(&mut self, value: &ParamValue) {
        match (self, value) {
            (ParamDef::Float { default, .. }, ParamValue::Float(v)) => *default = *v,
            (ParamDef::Color { default, .. }, ParamValue::Color(v)) => *default = *v,
            (ParamDef::Bool { default, .. }, ParamValue::Bool(v)) => *default = *v,
            (ParamDef::Point2D { default, .. }, ParamValue::Point2D(v)) => *default = *v,
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParamValue {
    Float(f32),
    Color([f32; 4]),
    Bool(bool),
    Point2D([f32; 2]),
}

impl ParamValue {
    /// Number of f32 slots this value occupies in the uniform buffer.
    pub fn float_count(&self) -> usize {
        match self {
            ParamValue::Float(_) => 1,
            ParamValue::Color(_) => 4,
            ParamValue::Bool(_) => 1,
            ParamValue::Point2D(_) => 2,
        }
    }

    /// Interpolate between two param values. Type mismatches return `self`.
    pub fn lerp(&self, other: &ParamValue, t: f32) -> ParamValue {
        match (self, other) {
            (ParamValue::Float(a), ParamValue::Float(b)) => ParamValue::Float(a + (b - a) * t),
            (ParamValue::Color(a), ParamValue::Color(b)) => ParamValue::Color([
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
                a[3] + (b[3] - a[3]) * t,
            ]),
            (ParamValue::Point2D(a), ParamValue::Point2D(b)) => {
                ParamValue::Point2D([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t])
            }
            (ParamValue::Bool(a), ParamValue::Bool(b)) => {
                ParamValue::Bool(if t < 0.5 { *a } else { *b })
            }
            _ => self.clone(), // type mismatch fallback
        }
    }

    /// Write this value into a slice at the given offset.
    pub fn write_to(&self, buf: &mut [f32], offset: usize) {
        match self {
            ParamValue::Float(v) => buf[offset] = *v,
            ParamValue::Color(c) => buf[offset..offset + 4].copy_from_slice(c),
            ParamValue::Bool(b) => buf[offset] = if *b { 1.0 } else { 0.0 },
            ParamValue::Point2D(p) => buf[offset..offset + 2].copy_from_slice(p),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn param_def_name_returns_correct_string() {
        let f = ParamDef::Float {
            name: "speed".into(),
            default: 0.5,
            min: 0.0,
            max: 1.0,
        };
        assert_eq!(f.name(), "speed");
        let c = ParamDef::Color {
            name: "tint".into(),
            default: [1.0, 0.0, 0.0, 1.0],
        };
        assert_eq!(c.name(), "tint");
        let b = ParamDef::Bool {
            name: "active".into(),
            default: true,
        };
        assert_eq!(b.name(), "active");
        let p = ParamDef::Point2D {
            name: "pos".into(),
            default: [0.0, 0.0],
            min: [-1.0, -1.0],
            max: [1.0, 1.0],
        };
        assert_eq!(p.name(), "pos");
    }

    #[test]
    fn param_def_default_value_float() {
        let f = ParamDef::Float {
            name: "x".into(),
            default: 0.75,
            min: 0.0,
            max: 1.0,
        };
        match f.default_value() {
            ParamValue::Float(v) => assert!(approx_eq(v, 0.75, 1e-6)),
            _ => panic!("expected Float"),
        }
    }

    #[test]
    fn param_def_default_value_color() {
        let c = ParamDef::Color {
            name: "c".into(),
            default: [0.1, 0.2, 0.3, 0.4],
        };
        match c.default_value() {
            ParamValue::Color(v) => {
                assert!(approx_eq(v[0], 0.1, 1e-6));
                assert!(approx_eq(v[3], 0.4, 1e-6));
            }
            _ => panic!("expected Color"),
        }
    }

    #[test]
    fn param_def_default_value_bool() {
        let b = ParamDef::Bool {
            name: "b".into(),
            default: true,
        };
        match b.default_value() {
            ParamValue::Bool(v) => assert!(v),
            _ => panic!("expected Bool"),
        }
    }

    #[test]
    fn param_def_default_value_point2d() {
        let p = ParamDef::Point2D {
            name: "p".into(),
            default: [0.5, -0.5],
            min: [-1.0, -1.0],
            max: [1.0, 1.0],
        };
        match p.default_value() {
            ParamValue::Point2D(v) => {
                assert!(approx_eq(v[0], 0.5, 1e-6));
                assert!(approx_eq(v[1], -0.5, 1e-6));
            }
            _ => panic!("expected Point2D"),
        }
    }

    #[test]
    fn param_value_float_count() {
        assert_eq!(ParamValue::Float(0.0).float_count(), 1);
        assert_eq!(ParamValue::Color([0.0; 4]).float_count(), 4);
        assert_eq!(ParamValue::Bool(false).float_count(), 1);
        assert_eq!(ParamValue::Point2D([0.0; 2]).float_count(), 2);
    }

    #[test]
    fn param_value_write_to_float() {
        let mut buf = [0.0f32; 4];
        ParamValue::Float(0.42).write_to(&mut buf, 1);
        assert!(approx_eq(buf[1], 0.42, 1e-6));
    }

    #[test]
    fn param_value_write_to_color_and_bool() {
        let mut buf = [0.0f32; 8];
        ParamValue::Color([0.1, 0.2, 0.3, 0.4]).write_to(&mut buf, 2);
        assert!(approx_eq(buf[2], 0.1, 1e-6));
        assert!(approx_eq(buf[5], 0.4, 1e-6));

        ParamValue::Bool(true).write_to(&mut buf, 0);
        assert!(approx_eq(buf[0], 1.0, 1e-6));
        ParamValue::Bool(false).write_to(&mut buf, 0);
        assert!(approx_eq(buf[0], 0.0, 1e-6));
    }

    #[test]
    fn param_value_write_to_point2d() {
        let mut buf = [0.0f32; 4];
        ParamValue::Point2D([0.3, 0.7]).write_to(&mut buf, 1);
        assert!(approx_eq(buf[1], 0.3, 1e-6));
        assert!(approx_eq(buf[2], 0.7, 1e-6));
    }

    // ---- Lerp tests ----

    #[test]
    fn lerp_float() {
        let a = ParamValue::Float(0.0);
        let b = ParamValue::Float(1.0);
        match a.lerp(&b, 0.5) {
            ParamValue::Float(v) => assert!(approx_eq(v, 0.5, 1e-6)),
            _ => panic!("expected Float"),
        }
        match a.lerp(&b, 0.0) {
            ParamValue::Float(v) => assert!(approx_eq(v, 0.0, 1e-6)),
            _ => panic!("expected Float"),
        }
        match a.lerp(&b, 1.0) {
            ParamValue::Float(v) => assert!(approx_eq(v, 1.0, 1e-6)),
            _ => panic!("expected Float"),
        }
    }

    #[test]
    fn lerp_color() {
        let a = ParamValue::Color([0.0, 0.0, 0.0, 0.0]);
        let b = ParamValue::Color([1.0, 0.5, 0.2, 1.0]);
        match a.lerp(&b, 0.5) {
            ParamValue::Color(c) => {
                assert!(approx_eq(c[0], 0.5, 1e-6));
                assert!(approx_eq(c[1], 0.25, 1e-6));
                assert!(approx_eq(c[2], 0.1, 1e-6));
                assert!(approx_eq(c[3], 0.5, 1e-6));
            }
            _ => panic!("expected Color"),
        }
    }

    #[test]
    fn lerp_point2d() {
        let a = ParamValue::Point2D([0.0, 1.0]);
        let b = ParamValue::Point2D([1.0, 0.0]);
        match a.lerp(&b, 0.25) {
            ParamValue::Point2D(p) => {
                assert!(approx_eq(p[0], 0.25, 1e-6));
                assert!(approx_eq(p[1], 0.75, 1e-6));
            }
            _ => panic!("expected Point2D"),
        }
    }

    #[test]
    fn lerp_bool() {
        let a = ParamValue::Bool(false);
        let b = ParamValue::Bool(true);
        match a.lerp(&b, 0.3) {
            ParamValue::Bool(v) => assert!(!v),
            _ => panic!("expected Bool"),
        }
        match a.lerp(&b, 0.7) {
            ParamValue::Bool(v) => assert!(v),
            _ => panic!("expected Bool"),
        }
    }

    #[test]
    fn lerp_type_mismatch_returns_self() {
        let a = ParamValue::Float(0.5);
        let b = ParamValue::Bool(true);
        match a.lerp(&b, 0.5) {
            ParamValue::Float(v) => assert!(approx_eq(v, 0.5, 1e-6)),
            _ => panic!("expected Float (self)"),
        }
    }

    // ---- set_default tests ----

    #[test]
    fn set_default_float() {
        let mut def = ParamDef::Float {
            name: "x".into(),
            default: 0.5,
            min: 0.0,
            max: 1.0,
        };
        def.set_default(&ParamValue::Float(0.8));
        match def.default_value() {
            ParamValue::Float(v) => assert!(approx_eq(v, 0.8, 1e-6)),
            _ => panic!("expected Float"),
        }
    }

    #[test]
    fn set_default_color() {
        let mut def = ParamDef::Color {
            name: "c".into(),
            default: [0.0; 4],
        };
        def.set_default(&ParamValue::Color([0.1, 0.2, 0.3, 0.4]));
        match def.default_value() {
            ParamValue::Color(v) => assert!(approx_eq(v[2], 0.3, 1e-6)),
            _ => panic!("expected Color"),
        }
    }

    #[test]
    fn set_default_type_mismatch_ignored() {
        let mut def = ParamDef::Float {
            name: "x".into(),
            default: 0.5,
            min: 0.0,
            max: 1.0,
        };
        def.set_default(&ParamValue::Bool(true)); // wrong type — should be no-op
        match def.default_value() {
            ParamValue::Float(v) => assert!(approx_eq(v, 0.5, 1e-6)),
            _ => panic!("expected Float unchanged"),
        }
    }
}
