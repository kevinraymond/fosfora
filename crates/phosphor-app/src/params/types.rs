use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
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
