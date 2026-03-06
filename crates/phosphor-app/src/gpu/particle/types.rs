use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

use super::emitter::EmitterDef;
use crate::media::types::DecodedFrame;

/// Size of a single SoA component buffer element (one vec4f = 16 bytes).
pub const PARTICLE_COMPONENT_STRIDE: u64 = 16;

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

/// Particle simulation uniforms: 832 bytes.
/// Separate from the main 288-byte ShaderUniforms.
#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
pub struct ParticleUniforms {
    // Simulation params (16 bytes) [0..15]
    pub delta_time: f32,
    pub time: f32,
    pub max_particles: u32,
    pub emit_count: u32,

    // Emitter config (16 bytes) [16..31]
    pub emitter_pos: [f32; 2],
    pub emitter_radius: f32,
    pub emitter_shape: u32, // 0=point, 1=ring, 2=line, 3=screen, 4=image, 5=disc, 6=cone

    // Particle defaults (16 bytes) [32..47]
    pub lifetime: f32,
    pub initial_speed: f32,
    pub initial_size: f32,
    pub size_end: f32,

    // Forces (16 bytes) [48..63]
    pub gravity: [f32; 2],
    pub drag: f32,
    pub turbulence: f32,

    // Attraction (16 bytes) [64..79]
    pub attraction_point: [f32; 2],
    pub attraction_strength: f32,
    pub seed: f32,

    // Audio features (56 bytes = 14 floats) [80..135]
    pub sub_bass: f32,
    pub bass: f32,
    pub mid: f32,
    pub rms: f32,
    pub kick: f32,
    pub onset: f32,
    pub centroid: f32,
    pub flux: f32,
    pub beat: f32,
    pub beat_phase: f32,
    pub low_mid: f32,
    pub upper_mid: f32,
    pub presence: f32,
    pub brilliance: f32,

    // Resolution (8 bytes) [136..143]
    pub resolution: [f32; 2],
    // --- 144 bytes above ---

    // Flow field params (16 bytes) [128..143]
    pub flow_strength: f32,
    pub flow_scale: f32,
    pub flow_speed: f32,
    pub flow_enabled: f32, // 0.0 or 1.0 (bool as float for GPU)

    // Trail params (16 bytes) [144..159]
    pub trail_length: u32,
    pub trail_width: f32,
    pub prev_emitter_pos: [f32; 2], // previous frame emitter position for velocity inheritance

    // --- 160 bytes above ---

    // Wind + vortex (32 bytes) [160..191]
    pub wind: [f32; 2],
    pub vortex_center: [f32; 2],
    pub vortex_strength: f32,
    pub vortex_radius: f32,
    pub ground_y: f32,
    pub ground_bounce: f32,

    // Noise params (16 bytes) [192..207]
    pub noise_octaves: u32,
    pub noise_lacunarity: f32,
    pub noise_persistence: f32,
    pub noise_mode: u32, // 0=turbulence(abs), 1=curl

    // Emitter enhancements (32 bytes) [208..239]
    pub emitter_angle: f32,
    pub emitter_spread: f32,
    pub speed_variance: f32,
    pub life_variance: f32,
    pub size_variance: f32,
    pub velocity_inherit: f32,
    pub noise_speed: f32,
    pub dominant_chroma: f32,

    // Lifetime curves (64 bytes) [240..303]
    pub size_curve: [f32; 8],    // 8-point LUT for size over lifetime
    pub opacity_curve: [f32; 8], // 8-point LUT for opacity over lifetime

    // Color gradient (32 bytes) [304..335]
    pub color_gradient: [u32; 8], // up to 8 packed RGBA colors

    // Spin + curve config (16 bytes) [336..351]
    pub spin_speed: f32,
    pub gradient_count: u32,
    pub curve_flags: u32, // bit 0 = size curve, bit 1 = opacity curve
    pub depth_sort: u32,

    // Effect params forwarded from ParamStore (32 bytes) [352..383]
    pub effect_params: [f32; 8],

    // Obstacle collision (16 bytes) [384..399]
    pub obstacle_enabled: f32,    // 0.0 or 1.0
    pub obstacle_threshold: f32,  // alpha cutoff (default 0.5)
    pub obstacle_mode: u32,       // 0=bounce, 1=stick, 2=flow, 3=contain
    pub obstacle_elasticity: f32, // restitution/friction (default 0.7)
    // 416 bytes above

    // MFCC: 13 coefficients + 3 padding (array<vec4f, 4> on GPU)
    pub mfcc: [f32; 16],
    // Chroma: 12 pitch class energies (array<vec4f, 3> on GPU)
    pub chroma: [f32; 12],

    // Force matrix for particle-life (symbiosis): 8x8 = 64 floats (array<vec4f, 16> on GPU)
    pub force_matrix: [f32; 64],

    // Morph (shape target morphing)
    pub morph_progress: f32,
    pub morph_source: u32,
    pub morph_dest: u32,
    pub morph_flags: u32, // bit 0 = transitioning, bits 1-3 = transition_style

    // Zero-crossing rate (32 bytes: f32 + 12 pad to align vec3f + vec3f pad)
    pub zcr: f32,
    pub _pad_zcr: [f32; 7],
    // Total = 832 bytes
}

/// Obstacle collision mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObstacleMode {
    Bounce = 0,
    Stick = 1,
    Flow = 2,
    Contain = 3,
}

impl ObstacleMode {
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Stick,
            2 => Self::Flow,
            3 => Self::Contain,
            _ => Self::Bounce,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Bounce => "Bounce",
            Self::Stick => "Stick",
            Self::Flow => "Flow Around",
            Self::Contain => "Contain",
        }
    }
}

/// Auxiliary particle data: 16 bytes (1 x vec4f).
/// home.xy = home position (for image decomposition reform), home.z = packed RGBA (bitcast u32→f32),
/// home.w = effect-dependent: sprite_index for sprite effects, gradient magnitude for image decomposition.
/// Stored in a separate storage buffer at compute binding 4.
#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
pub struct ParticleAux {
    pub home: [f32; 4],
}

/// Particle image source: controls where aux data (home positions + colors) come from.
pub enum ParticleImageSource {
    /// Static image (current behavior, no per-frame updates).
    Static,
    /// Pre-decoded video frames — plays back and updates aux per frame.
    #[cfg(feature = "video")]
    Video {
        frames: Vec<DecodedFrame>,
        delays_ms: Vec<u32>,
        current_frame: usize,
        frame_elapsed_ms: f64,
        playing: bool,
        looping: bool,
        speed: f32,
    },
    /// Live webcam feed — frames arrive externally.
    #[cfg(feature = "webcam")]
    Webcam { width: u32, height: u32 },
}

impl ParticleImageSource {
    /// Advance playback by dt seconds. Returns true if the current frame changed.
    pub fn advance(&mut self, dt_secs: f64) -> bool {
        match self {
            #[cfg(feature = "video")]
            ParticleImageSource::Video {
                frames,
                delays_ms,
                current_frame,
                frame_elapsed_ms,
                playing,
                looping,
                speed,
                ..
            } => {
                if !*playing || frames.is_empty() {
                    return false;
                }
                *frame_elapsed_ms += dt_secs * 1000.0 * (*speed as f64);
                let delay = delays_ms.get(*current_frame).copied().unwrap_or(33) as f64;
                if *frame_elapsed_ms >= delay {
                    *frame_elapsed_ms -= delay;
                    let next = *current_frame + 1;
                    if next >= frames.len() {
                        if *looping {
                            *current_frame = 0;
                        } else {
                            *playing = false;
                        }
                    } else {
                        *current_frame = next;
                    }
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Get the current frame's raw RGBA data (for video sources).
    pub fn current_frame_data(&self) -> Option<&DecodedFrame> {
        match self {
            #[cfg(feature = "video")]
            ParticleImageSource::Video {
                frames,
                current_frame,
                ..
            } => frames.get(*current_frame),
            _ => None,
        }
    }

    pub fn frame_count(&self) -> usize {
        match self {
            #[cfg(feature = "video")]
            ParticleImageSource::Video { frames, .. } => frames.len(),
            _ => 0,
        }
    }

    pub fn is_static(&self) -> bool {
        matches!(self, ParticleImageSource::Static)
    }

    pub fn is_video(&self) -> bool {
        #[cfg(feature = "video")]
        if matches!(self, ParticleImageSource::Video { .. }) {
            return true;
        }
        false
    }

    pub fn is_webcam(&self) -> bool {
        #[cfg(feature = "webcam")]
        if matches!(self, ParticleImageSource::Webcam { .. }) {
            return true;
        }
        false
    }

    /// Get video playback speed (1.0 if not video).
    pub fn video_speed(&self) -> Option<f32> {
        #[cfg(feature = "video")]
        if let ParticleImageSource::Video { speed, .. } = self {
            return Some(*speed);
        }
        None
    }

    /// Get current position in seconds (video only).
    pub fn video_position_secs(&self) -> f64 {
        #[cfg(feature = "video")]
        if let ParticleImageSource::Video {
            delays_ms,
            current_frame,
            frame_elapsed_ms,
            ..
        } = self
        {
            let ms: f64 = delays_ms
                .iter()
                .take(*current_frame)
                .map(|d| *d as f64)
                .sum();
            return (ms + *frame_elapsed_ms) / 1000.0;
        }
        0.0
    }

    /// Get total duration in seconds (video only).
    pub fn video_duration_secs(&self) -> f64 {
        #[cfg(feature = "video")]
        if let ParticleImageSource::Video { delays_ms, .. } = self {
            return delays_ms.iter().map(|d| *d as f64).sum::<f64>() / 1000.0;
        }
        0.0
    }

    /// Seek to a specific time position (video only).
    pub fn seek_to_secs(&mut self, target_secs: f64) {
        #[cfg(feature = "video")]
        if let ParticleImageSource::Video {
            delays_ms,
            current_frame,
            frame_elapsed_ms,
            ..
        } = self
        {
            let target_ms = target_secs * 1000.0;
            let mut accumulated = 0.0f64;
            for (i, delay) in delays_ms.iter().enumerate() {
                let d = *delay as f64;
                if accumulated + d > target_ms {
                    *current_frame = i;
                    *frame_elapsed_ms = target_ms - accumulated;
                    return;
                }
                accumulated += d;
            }
            // Past end — clamp to last frame
            if !delays_ms.is_empty() {
                *current_frame = delays_ms.len() - 1;
                *frame_elapsed_ms = 0.0;
            }
        }
    }
}

/// Smooth transition between particle source positions.
pub struct SourceTransition {
    pub from_aux: Vec<ParticleAux>,
    pub to_aux: Vec<ParticleAux>,
    pub progress: f32,
    pub duration_secs: f32,
}

impl SourceTransition {
    /// Interpolate aux data at current progress, returning blended result.
    pub fn interpolated(&self) -> Vec<ParticleAux> {
        let t = self.progress.clamp(0.0, 1.0);
        let len = self.from_aux.len().max(self.to_aux.len());
        let mut result = Vec::with_capacity(len);
        for i in 0..len {
            let from = self.from_aux.get(i).map(|a| a.home).unwrap_or([0.0; 4]);
            let to = self.to_aux.get(i).map(|a| a.home).unwrap_or([0.0; 4]);
            // Lerp xy (home position), keep target's z (packed color) and w (gradient/sprite_index)
            result.push(ParticleAux {
                home: [
                    from[0] * (1.0 - t) + to[0] * t,
                    from[1] * (1.0 - t) + to[1] * t,
                    to[2], // packed RGBA from target
                    to[3], // gradient or sprite_index from target
                ],
            });
        }
        result
    }

    pub fn is_complete(&self) -> bool {
        self.progress >= 1.0
    }
}

/// Particle render uniforms: 48 bytes.
#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
pub struct ParticleRenderUniforms {
    pub resolution: [f32; 2],
    pub time: f32,
    /// 0 = soft circle (default), 1 = sprite texture, 2 = animated sprite
    pub render_mode: u32,
    /// Sprite atlas columns, rows, total frames
    pub sprite_cols: u32,
    pub sprite_rows: u32,
    pub sprite_frames: u32,
    /// Frame index for trail ring buffer head
    pub frame_index: u32,
    /// Trail params
    pub trail_length: u32,
    pub trail_width: f32,
    pub _pad: [f32; 2],
}

/// Sprite atlas definition for textured particles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpriteDef {
    /// Texture file path relative to assets/images/
    pub texture: String,
    #[serde(default = "default_one_u32")]
    pub cols: u32,
    #[serde(default = "default_one_u32")]
    pub rows: u32,
    #[serde(default)]
    pub animated: bool,
    #[serde(default)]
    pub frames: u32,
}

/// Image sampling configuration for image-to-particle decomposition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageSampleDef {
    /// Sampling mode: "grid", "threshold", or "random"
    #[serde(default = "default_sample_mode")]
    pub mode: String,
    /// Brightness threshold (for "threshold" mode)
    #[serde(default = "default_threshold")]
    pub threshold: f32,
    /// Scale factor for mapping image to screen space
    #[serde(default = "default_scale")]
    pub scale: f32,
}

/// Reaction-diffusion configuration for particle effects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReactionDiffusionDef {
    #[serde(default = "default_rd_grid_size")]
    pub grid_size: u32,
    #[serde(default = "default_rd_steps")]
    pub steps_per_frame: u32,
    #[serde(default)]
    pub compute_shader: String,
}

fn default_rd_grid_size() -> u32 {
    512
}
fn default_rd_steps() -> u32 {
    16
}

/// Reaction-diffusion uniforms: 32 bytes.
#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
pub struct RDUniforms {
    pub feed_rate: f32,
    pub kill_rate: f32,
    pub diffuse_a: f32,
    pub diffuse_b: f32,
    pub time: f32,
    pub onset: f32,
    pub drop_radius: f32,
    pub _pad: f32,
}

fn default_sample_mode() -> String {
    "grid".to_string()
}
fn default_threshold() -> f32 {
    0.1
}
fn default_scale() -> f32 {
    1.0
}
fn default_one_u32() -> u32 {
    1
}

/// Morph target definition: specifies a target shape for particle morphing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MorphTargetDef {
    /// Target source: "image:<path>", "geometry:<shape>", or "random"
    pub source: String,
    /// Optional color override
    #[serde(default)]
    pub color: Option<String>,
}

/// .pfx particle definition (JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParticleDef {
    #[serde(default = "default_max_count")]
    pub max_count: u32,
    /// Upper limit after quality scaling. Quality multiplier won't push past this.
    /// If 0 or absent, no per-effect cap is applied.
    #[serde(default)]
    pub max_scaled_count: u32,
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
    /// Sprite texture definition (optional)
    #[serde(default)]
    pub sprite: Option<SpriteDef>,
    /// Image sampling for image-to-particle decomposition (optional)
    #[serde(default)]
    pub image_sample: Option<ImageSampleDef>,
    /// Blend mode: "additive" (default), "alpha", or "wboit"
    #[serde(default = "default_blend")]
    pub blend: String,
    /// Enable 3D curl noise flow field (opt-in)
    #[serde(default)]
    pub flow_field: bool,
    /// Flow field strength (how much the field affects velocity)
    #[serde(default = "default_flow_strength")]
    pub flow_strength: f32,
    /// Flow field sampling scale (spatial frequency)
    #[serde(default = "default_flow_scale")]
    pub flow_scale: f32,
    /// Flow field animation speed (z-axis scroll)
    #[serde(default = "default_flow_speed")]
    pub flow_speed: f32,
    /// Trail length in points (0 = no trails). When > 0, enables trail rendering.
    #[serde(default)]
    pub trail_length: u32,
    /// Trail ribbon width in screen units
    #[serde(default = "default_trail_width")]
    pub trail_width: f32,
    /// Enable spatial hash grid for particle-particle interaction
    #[serde(default)]
    pub interaction: bool,
    /// Maximum spatial hash grid dimension (0 = use default count-based heuristic).
    /// Effects with large interaction radii (e.g. Symbiosis) need coarser grids.
    #[serde(default)]
    pub grid_max: u32,

    // --- Phase 1: Forces ---
    /// Directional wind force [x, y]
    #[serde(default)]
    pub wind: [f32; 2],
    /// Vortex field center position [x, y]
    #[serde(default)]
    pub vortex_center: [f32; 2],
    /// Vortex rotation strength
    #[serde(default)]
    pub vortex_strength: f32,
    /// Vortex falloff radius
    #[serde(default = "default_vortex_radius")]
    pub vortex_radius: f32,
    /// Ground plane y-level for bounce
    #[serde(default = "default_ground_y")]
    pub ground_y: f32,
    /// Ground bounce restitution (0=passthrough, 1=elastic)
    #[serde(default)]
    pub ground_bounce: f32,
    /// FBM noise octaves (0=legacy hash turbulence)
    #[serde(default)]
    pub noise_octaves: u32,
    /// FBM frequency multiplier per octave
    #[serde(default = "default_noise_lacunarity")]
    pub noise_lacunarity: f32,
    /// FBM amplitude decay per octave
    #[serde(default = "default_noise_persistence")]
    pub noise_persistence: f32,
    /// Noise mode: 0=turbulence(abs), 1=curl
    #[serde(default)]
    pub noise_mode: u32,
    /// Noise animation speed
    #[serde(default = "default_noise_speed")]
    pub noise_speed: f32,

    // --- Phase 2: Emitter enhancements ---
    /// Cone emission direction (radians)
    #[serde(default)]
    pub emitter_angle: f32,
    /// Cone emission half-angle spread (0=omnidirectional)
    #[serde(default)]
    pub emitter_spread: f32,
    /// Speed randomness (0-1)
    #[serde(default)]
    pub speed_variance: f32,
    /// Lifetime randomness (0-1)
    #[serde(default)]
    pub life_variance: f32,
    /// Size randomness (0-1)
    #[serde(default)]
    pub size_variance: f32,
    /// Emitter motion inheritance (0-1)
    #[serde(default)]
    pub velocity_inherit: f32,

    // --- Phase 3: Curves + Spin + Color ---
    /// Size over lifetime curve (8 uniformly-spaced samples, 0-1)
    #[serde(default)]
    pub size_curve: Vec<f32>,
    /// Opacity over lifetime curve (8 uniformly-spaced samples, 0-1)
    #[serde(default)]
    pub opacity_curve: Vec<f32>,
    /// Color gradient over lifetime (hex strings: "#RRGGBB" or "#RRGGBBAA")
    #[serde(default)]
    pub color_gradient: Vec<String>,
    /// Particle spin speed (radians/sec)
    #[serde(default)]
    pub spin_speed: f32,

    // --- Phase 4: Sort ---
    /// Enable depth sorting (only meaningful with blend="alpha")
    #[serde(default)]
    pub depth_sort: bool,

    /// Reaction-diffusion configuration (optional)
    #[serde(default)]
    pub reaction_diffusion: Option<ReactionDiffusionDef>,

    /// Render mode: "billboard" (default), "compute" (atomic framebuffer), or "auto"
    #[serde(default = "default_render_mode")]
    pub render_mode: String,

    /// Enable symbiosis (particle-life) force matrix management
    #[serde(default)]
    pub symbiosis: bool,

    /// Enable morph (shape target morphing)
    #[serde(default)]
    pub morph: bool,
    /// Morph target definitions (up to 4)
    #[serde(default)]
    pub morph_targets: Option<Vec<MorphTargetDef>>,
}

fn default_blend() -> String {
    "additive".to_string()
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
fn default_flow_strength() -> f32 {
    1.0
}
fn default_flow_scale() -> f32 {
    1.0
}
fn default_flow_speed() -> f32 {
    0.5
}
fn default_trail_width() -> f32 {
    0.005
}
fn default_vortex_radius() -> f32 {
    1.0
}
fn default_ground_y() -> f32 {
    -2.0
}
fn default_noise_lacunarity() -> f32 {
    2.0
}
fn default_noise_persistence() -> f32 {
    0.5
}
fn default_noise_speed() -> f32 {
    0.5
}
fn default_render_mode() -> String {
    "billboard".to_string()
}

/// Parse a hex color string to packed RGBA u32.
/// Supports "#RRGGBB" (alpha defaults to 0xFF) and "#RRGGBBAA".
pub fn parse_hex_color(s: &str) -> u32 {
    let s = s.trim_start_matches('#');
    let (r, g, b, a) = match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(0);
            let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(0);
            (r, g, b, 0xFF)
        }
        8 => {
            let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(0);
            let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(0);
            let a = u8::from_str_radix(&s[6..8], 16).unwrap_or(0xFF);
            (r, g, b, a)
        }
        _ => (0, 0, 0, 0xFF),
    };
    (r as u32) << 24 | (g as u32) << 16 | (b as u32) << 8 | a as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn particle_size_64() {
        assert_eq!(std::mem::size_of::<Particle>(), 64);
    }

    #[test]
    fn particle_uniforms_size_832() {
        assert_eq!(std::mem::size_of::<ParticleUniforms>(), 832);
    }

    #[test]
    fn particle_render_uniforms_size_48() {
        assert_eq!(std::mem::size_of::<ParticleRenderUniforms>(), 48);
    }

    #[test]
    fn rd_uniforms_size_32() {
        assert_eq!(std::mem::size_of::<RDUniforms>(), 32);
    }

    #[test]
    fn reaction_diffusion_def_defaults() {
        let json = r#"{}"#;
        let def: ReactionDiffusionDef = serde_json::from_str(json).unwrap();
        assert_eq!(def.grid_size, 512);
        assert_eq!(def.steps_per_frame, 16);
        assert!(def.compute_shader.is_empty());
    }

    #[test]
    fn particle_def_serde_defaults() {
        let json = r#"{"emitter":{}}"#;
        let def: ParticleDef = serde_json::from_str(json).unwrap();
        assert_eq!(def.max_count, 10000);
        assert!((def.lifetime - 3.0).abs() < 1e-6);
        assert!((def.initial_speed - 0.3).abs() < 1e-6);
        assert!((def.initial_size - 0.02).abs() < 1e-6);
        assert!((def.drag - 0.98).abs() < 1e-6);
        assert!((def.emit_rate - 100.0).abs() < 1e-6);
        assert_eq!(def.blend, "additive");
        assert!(def.sprite.is_none());
        assert!(def.image_sample.is_none());
        assert!(def.reaction_diffusion.is_none());
        // New fields default to disabled
        assert_eq!(def.wind, [0.0, 0.0]);
        assert_eq!(def.vortex_strength, 0.0);
        assert!((def.vortex_radius - 1.0).abs() < 1e-6);
        assert_eq!(def.ground_bounce, 0.0);
        assert!((def.ground_y - (-2.0)).abs() < 1e-6);
        assert_eq!(def.noise_octaves, 0);
        assert!((def.noise_lacunarity - 2.0).abs() < 1e-6);
        assert!((def.noise_persistence - 0.5).abs() < 1e-6);
        assert_eq!(def.noise_mode, 0);
        assert_eq!(def.emitter_spread, 0.0);
        assert_eq!(def.speed_variance, 0.0);
        assert_eq!(def.velocity_inherit, 0.0);
        assert!(def.size_curve.is_empty());
        assert!(def.opacity_curve.is_empty());
        assert!(def.color_gradient.is_empty());
        assert_eq!(def.spin_speed, 0.0);
        assert!(!def.depth_sort);
        assert_eq!(def.render_mode, "billboard");
    }

    #[test]
    fn particle_def_serde_roundtrip_with_new_fields() {
        let json = r##"{
            "emitter": {},
            "wind": [0.3, 0.0],
            "vortex_strength": 2.0,
            "vortex_radius": 0.8,
            "ground_y": -0.5,
            "ground_bounce": 0.6,
            "noise_octaves": 4,
            "noise_mode": 1,
            "emitter_spread": 0.5,
            "speed_variance": 0.3,
            "size_curve": [1.0, 1.0, 0.8, 0.5, 0.3, 0.1, 0.05, 0.0],
            "color_gradient": ["#FF4400", "#00AAFF"],
            "spin_speed": 3.0,
            "depth_sort": true
        }"##;
        let def: ParticleDef = serde_json::from_str(json).unwrap();
        assert_eq!(def.wind, [0.3, 0.0]);
        assert!((def.vortex_strength - 2.0).abs() < 1e-6);
        assert!((def.vortex_radius - 0.8).abs() < 1e-6);
        assert!((def.ground_y - (-0.5)).abs() < 1e-6);
        assert!((def.ground_bounce - 0.6).abs() < 1e-6);
        assert_eq!(def.noise_octaves, 4);
        assert_eq!(def.noise_mode, 1);
        assert!((def.emitter_spread - 0.5).abs() < 1e-6);
        assert_eq!(def.size_curve.len(), 8);
        assert_eq!(def.color_gradient.len(), 2);
        assert!((def.spin_speed - 3.0).abs() < 1e-6);
        assert!(def.depth_sort);

        // Roundtrip
        let serialized = serde_json::to_string(&def).unwrap();
        let def2: ParticleDef = serde_json::from_str(&serialized).unwrap();
        assert_eq!(def, def2);
    }

    #[test]
    fn parse_hex_color_rgb() {
        assert_eq!(parse_hex_color("#FF0000"), 0xFF0000FF);
        assert_eq!(parse_hex_color("#00FF00"), 0x00FF00FF);
        assert_eq!(parse_hex_color("#0000FF"), 0x0000FFFF);
        assert_eq!(parse_hex_color("#FFFFFF"), 0xFFFFFFFF);
        assert_eq!(parse_hex_color("#000000"), 0x000000FF);
    }

    #[test]
    fn parse_hex_color_rgba() {
        assert_eq!(parse_hex_color("#00FF0080"), 0x00FF0080);
        assert_eq!(parse_hex_color("#FF000000"), 0xFF000000);
        assert_eq!(parse_hex_color("#FFFFFFFF"), 0xFFFFFFFF);
    }

    #[test]
    fn parse_hex_color_no_hash() {
        assert_eq!(parse_hex_color("FF0000"), 0xFF0000FF);
    }

    #[test]
    fn particle_image_source_static_default() {
        let src = ParticleImageSource::Static;
        assert!(src.is_static());
        assert!(!src.is_video());
        assert!(!src.is_webcam());
        assert_eq!(src.frame_count(), 0);
        assert!(src.current_frame_data().is_none());
        assert!((src.video_position_secs() - 0.0).abs() < 1e-10);
        assert!((src.video_duration_secs() - 0.0).abs() < 1e-10);
    }

    #[test]
    fn particle_image_source_static_no_advance() {
        let mut src = ParticleImageSource::Static;
        assert!(!src.advance(0.016));
    }

    #[test]
    fn source_transition_interpolation() {
        let from = vec![ParticleAux {
            home: [0.0, 0.0, 1.0, 0.0],
        }];
        let to = vec![ParticleAux {
            home: [1.0, 1.0, 2.0, 1.0],
        }];
        let trans = SourceTransition {
            from_aux: from,
            to_aux: to,
            progress: 0.5,
            duration_secs: 1.0,
        };
        let result = trans.interpolated();
        assert_eq!(result.len(), 1);
        assert!((result[0].home[0] - 0.5).abs() < 1e-6); // lerp x
        assert!((result[0].home[1] - 0.5).abs() < 1e-6); // lerp y
        assert!((result[0].home[2] - 2.0).abs() < 1e-6); // target color
        assert!((result[0].home[3] - 1.0).abs() < 1e-6); // target sprite idx
    }

    #[test]
    fn source_transition_complete() {
        let trans = SourceTransition {
            from_aux: vec![],
            to_aux: vec![],
            progress: 1.0,
            duration_secs: 0.5,
        };
        assert!(trans.is_complete());

        let trans2 = SourceTransition {
            from_aux: vec![],
            to_aux: vec![],
            progress: 0.5,
            duration_secs: 0.5,
        };
        assert!(!trans2.is_complete());
    }

    #[test]
    fn source_transition_mismatched_lengths() {
        let from = vec![ParticleAux {
            home: [1.0, 2.0, 3.0, 0.0],
        }];
        let to = vec![
            ParticleAux {
                home: [4.0, 5.0, 6.0, 0.0],
            },
            ParticleAux {
                home: [7.0, 8.0, 9.0, 0.0],
            },
        ];
        let trans = SourceTransition {
            from_aux: from,
            to_aux: to,
            progress: 0.0,
            duration_secs: 1.0,
        };
        let result = trans.interpolated();
        assert_eq!(result.len(), 2);
        // First entry: lerp from [1,2] to [4,5] at t=0 → [1,2]
        assert!((result[0].home[0] - 1.0).abs() < 1e-6);
        // Second entry: lerp from [0,0] (missing) to [7,8] at t=0 → [0,0]
        assert!((result[1].home[0] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn obstacle_mode_from_u32() {
        assert_eq!(ObstacleMode::from_u32(0), ObstacleMode::Bounce);
        assert_eq!(ObstacleMode::from_u32(1), ObstacleMode::Stick);
        assert_eq!(ObstacleMode::from_u32(2), ObstacleMode::Flow);
        assert_eq!(ObstacleMode::from_u32(3), ObstacleMode::Contain);
        assert_eq!(ObstacleMode::from_u32(99), ObstacleMode::Bounce); // fallback
    }

    #[test]
    fn obstacle_mode_labels() {
        assert_eq!(ObstacleMode::Bounce.label(), "Bounce");
        assert_eq!(ObstacleMode::Stick.label(), "Stick");
        assert_eq!(ObstacleMode::Flow.label(), "Flow Around");
        assert_eq!(ObstacleMode::Contain.label(), "Contain");
    }
}
