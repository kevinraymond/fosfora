use serde::{Deserialize, Serialize};

pub type BindingId = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BindingScope {
    Preset,
    Global,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Binding {
    pub id: BindingId,
    pub name: String,
    pub enabled: bool,
    pub scope: BindingScope,
    /// Source identifier, e.g. "audio.kick", "midi.MPD218.cc.0.42", "osc./foo", "ws.mediapipe.left_thumb_y"
    pub source: String,
    /// Target identifier, e.g. "param.{effect}.{name}", "layer.0.opacity", "scene.transport.go"
    pub target: String,
    pub transforms: Vec<TransformDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum TransformDef {
    #[serde(rename = "remap")]
    Remap {
        in_lo: f32,
        in_hi: f32,
        out_lo: f32,
        out_hi: f32,
    },
    #[serde(rename = "smooth")]
    Smooth { factor: f32 },
    #[serde(rename = "invert")]
    Invert,
    #[serde(rename = "quantize")]
    Quantize { steps: u32 },
    #[serde(rename = "deadzone")]
    Deadzone { lo: f32, hi: f32 },
    #[serde(rename = "curve")]
    Curve { curve_type: String },
    #[serde(rename = "gate")]
    Gate { threshold: f32 },
    #[serde(rename = "scale")]
    Scale { factor: f32 },
    #[serde(rename = "offset")]
    Offset { value: f32 },
    #[serde(rename = "clamp")]
    Clamp { lo: f32, hi: f32 },
}

/// Per-binding runtime state (not serialized).
pub struct BindingRuntime {
    pub smooth_state: f32,
    pub last_input: Option<f32>,
    pub last_output: Option<f32>,
    pub last_raw: Option<SourceRaw>,
}

impl BindingRuntime {
    pub fn new() -> Self {
        Self {
            smooth_state: 0.0,
            last_input: None,
            last_output: None,
            last_raw: None,
        }
    }
}

/// Original value before normalization (UI diagnostics only).
#[derive(Debug, Clone)]
pub struct SourceRaw {
    pub display: String,
    #[allow(dead_code)]
    pub numeric: f64,
}

/// What the binding bus is currently learning.
pub struct LearnState {
    pub binding_id: BindingId,
    pub field: LearnField,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LearnField {
    Source,
    #[allow(dead_code)]
    Target,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_serde_roundtrip() {
        let b = Binding {
            id: "b_001".into(),
            name: "Kick to warp".into(),
            enabled: true,
            scope: BindingScope::Global,
            source: "audio.kick".into(),
            target: "param.Phosphor.warp_intensity".into(),
            transforms: vec![
                TransformDef::Gate { threshold: 0.5 },
                TransformDef::Smooth { factor: 0.8 },
                TransformDef::Remap {
                    in_lo: 0.0,
                    in_hi: 1.0,
                    out_lo: 0.2,
                    out_hi: 1.0,
                },
            ],
        };

        let json = serde_json::to_string_pretty(&b).unwrap();
        let b2: Binding = serde_json::from_str(&json).unwrap();

        assert_eq!(b2.id, "b_001");
        assert_eq!(b2.name, "Kick to warp");
        assert_eq!(b2.scope, BindingScope::Global);
        assert_eq!(b2.transforms.len(), 3);
    }

    #[test]
    fn transform_serde_all_variants() {
        let transforms = vec![
            TransformDef::Remap {
                in_lo: 0.0,
                in_hi: 1.0,
                out_lo: 0.0,
                out_hi: 1.0,
            },
            TransformDef::Smooth { factor: 0.9 },
            TransformDef::Invert,
            TransformDef::Quantize { steps: 8 },
            TransformDef::Deadzone { lo: 0.1, hi: 0.9 },
            TransformDef::Curve {
                curve_type: "ease_in".into(),
            },
            TransformDef::Gate { threshold: 0.5 },
            TransformDef::Scale { factor: 2.0 },
            TransformDef::Offset { value: -0.5 },
            TransformDef::Clamp { lo: 0.0, hi: 1.0 },
        ];

        let json = serde_json::to_string(&transforms).unwrap();
        let parsed: Vec<TransformDef> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 10);
    }

    #[test]
    fn scope_serde() {
        let s = BindingScope::Global;
        let json = serde_json::to_string(&s).unwrap();
        let s2: BindingScope = serde_json::from_str(&json).unwrap();
        assert_eq!(s2, BindingScope::Global);

        let s = BindingScope::Preset;
        let json = serde_json::to_string(&s).unwrap();
        let s2: BindingScope = serde_json::from_str(&json).unwrap();
        assert_eq!(s2, BindingScope::Preset);
    }
}
