use serde::{Deserialize, Serialize};

use crate::gpu::particle::types::ParticleDef;
use crate::params::ParamDef;

/// A render pass definition within a multi-pass effect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassDef {
    pub name: String,
    pub shader: String,
    #[serde(default = "default_scale")]
    pub scale: f32,
    /// Names of previous passes whose outputs can be sampled as inputs.
    #[serde(default)]
    pub inputs: Vec<String>,
    /// Whether this pass reads its own previous frame (ping-pong feedback).
    #[serde(default)]
    pub feedback: bool,
}

fn default_scale() -> f32 {
    1.0
}

/// Per-effect post-processing overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostProcessDef {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_bloom_threshold")]
    pub bloom_threshold: f32,
    #[serde(default = "default_bloom_intensity")]
    pub bloom_intensity: f32,
    #[serde(default = "default_vignette")]
    pub vignette: f32,
}

fn default_true() -> bool {
    true
}

fn default_bloom_threshold() -> f32 {
    0.8
}

fn default_bloom_intensity() -> f32 {
    0.35
}

fn default_vignette() -> f32 {
    0.3
}

impl Default for PostProcessDef {
    fn default() -> Self {
        Self {
            enabled: true,
            bloom_threshold: 0.8,
            bloom_intensity: 0.35,
            vignette: 0.3,
        }
    }
}

/// A .pfx effect definition (JSON format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PfxEffect {
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    /// Single-pass shader (backward compatible). Ignored if `passes` is non-empty.
    #[serde(default)]
    pub shader: String,
    #[serde(default)]
    pub inputs: Vec<ParamDef>,
    /// Multi-pass pipeline definition. If empty, `shader` field is used as a single pass.
    #[serde(default)]
    pub passes: Vec<PassDef>,
    /// Per-effect post-processing overrides.
    #[serde(default)]
    pub postprocess: Option<PostProcessDef>,
    /// GPU particle system definition.
    #[serde(default)]
    pub particles: Option<ParticleDef>,
}

impl PfxEffect {
    /// Normalize: if `passes` is empty but `shader` is set, create a single-pass definition.
    pub fn normalized_passes(&self) -> Vec<PassDef> {
        if !self.passes.is_empty() {
            return self.passes.clone();
        }
        if !self.shader.is_empty() {
            vec![PassDef {
                name: "main".to_string(),
                shader: self.shader.clone(),
                scale: 1.0,
                inputs: vec![],
                feedback: false,
            }]
        } else {
            vec![]
        }
    }
}
