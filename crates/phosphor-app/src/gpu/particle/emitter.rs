use serde::{Deserialize, Serialize};

/// Emitter shape and position definition for .pfx files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmitterDef {
    #[serde(default = "default_shape")]
    pub shape: String,
    #[serde(default)]
    pub radius: f32,
    #[serde(default)]
    pub position: [f32; 2],
}

impl Default for EmitterDef {
    fn default() -> Self {
        Self {
            shape: "point".to_string(),
            radius: 0.0,
            position: [0.0, 0.0],
        }
    }
}

fn default_shape() -> String {
    "point".to_string()
}

impl EmitterDef {
    /// Convert shape string to GPU enum value.
    pub fn shape_index(&self) -> u32 {
        match self.shape.as_str() {
            "point" => 0,
            "ring" => 1,
            "line" => 2,
            "screen" => 3,
            _ => 0,
        }
    }
}
