use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

use super::emitter::EmitterDef;

/// GPU particle: 64 bytes (4 x vec4f).
/// pos_life: xy = position (screen-space -1..1), z = reserved, w = life (1.0 = just born, 0.0 = dead)
/// vel_size: xy = velocity, z = reserved, w = size
/// color: rgba
/// flags: x = age (0..lifetime), y = lifetime, z = emitter_id, w = reserved
#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
pub struct Particle {
    pub pos_life: [f32; 4],
    pub vel_size: [f32; 4],
    pub color: [f32; 4],
    pub flags: [f32; 4],
}

/// Particle simulation uniforms: 128 bytes.
/// Separate from the main 256-byte ShaderUniforms.
#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
pub struct ParticleUniforms {
    // Simulation params (16 bytes)
    pub delta_time: f32,
    pub time: f32,
    pub max_particles: u32,
    pub emit_count: u32,

    // Emitter config (16 bytes)
    pub emitter_pos: [f32; 2],
    pub emitter_radius: f32,
    pub emitter_shape: u32, // 0=point, 1=ring, 2=line, 3=screen

    // Particle defaults (16 bytes)
    pub lifetime: f32,
    pub initial_speed: f32,
    pub initial_size: f32,
    pub size_end: f32,

    // Forces (16 bytes)
    pub gravity: [f32; 2],
    pub drag: f32,
    pub turbulence: f32,

    // Attraction (16 bytes)
    pub attraction_point: [f32; 2],
    pub attraction_strength: f32,
    pub seed: f32,

    // Audio features (32 bytes)
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub rms: f32,
    pub onset: f32,
    pub centroid: f32,
    pub flux: f32,
    pub flatness: f32,

    // Resolution (16 bytes) — needed for aspect ratio correction in orbital mechanics
    pub resolution: [f32; 2],
    pub _pad: [f32; 2],
}

/// Particle render uniforms: 16 bytes.
#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
pub struct ParticleRenderUniforms {
    pub resolution: [f32; 2],
    pub time: f32,
    pub _pad: f32,
}

/// .pfx particle definition (JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticleDef {
    #[serde(default = "default_max_count")]
    pub max_count: u32,
    /// Custom compute shader (relative to shaders dir). If empty, uses builtin.
    #[serde(default)]
    pub compute_shader: String,
    #[serde(default)]
    pub emitter: EmitterDef,
    #[serde(default = "default_lifetime")]
    pub lifetime: f32,
    #[serde(default = "default_initial_speed")]
    pub initial_speed: f32,
    #[serde(default = "default_initial_size")]
    pub initial_size: f32,
    #[serde(default)]
    pub size_end: f32,
    #[serde(default)]
    pub gravity: [f32; 2],
    #[serde(default = "default_drag")]
    pub drag: f32,
    #[serde(default)]
    pub turbulence: f32,
    #[serde(default)]
    pub attraction_strength: f32,
    #[serde(default = "default_emit_rate")]
    pub emit_rate: f32,
    #[serde(default)]
    pub burst_on_beat: u32,
}

fn default_max_count() -> u32 {
    10000
}
fn default_lifetime() -> f32 {
    3.0
}
fn default_initial_speed() -> f32 {
    0.3
}
fn default_initial_size() -> f32 {
    0.02
}
fn default_drag() -> f32 {
    0.98
}
fn default_emit_rate() -> f32 {
    100.0
}
