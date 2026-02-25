use serde::{Deserialize, Serialize};

use crate::effect::format::PostProcessDef;
use crate::gpu::pass_executor::PassExecutor;
use crate::gpu::placeholder::PlaceholderTexture;
use crate::gpu::render_target::RenderTarget;
use crate::gpu::uniforms::UniformBuffer;
use crate::gpu::ShaderUniforms;
use crate::params::ParamStore;

/// Blend mode for compositing layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlendMode {
    Normal,
    Add,
    Multiply,
    Screen,
    Overlay,
    SoftLight,
    Difference,
}

impl BlendMode {
    pub const ALL: &[BlendMode] = &[
        BlendMode::Normal,
        BlendMode::Add,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::Difference,
    ];

    pub fn as_u32(&self) -> u32 {
        match self {
            BlendMode::Normal => 0,
            BlendMode::Add => 1,
            BlendMode::Multiply => 2,
            BlendMode::Screen => 3,
            BlendMode::Overlay => 4,
            BlendMode::SoftLight => 5,
            BlendMode::Difference => 6,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Add => "Add",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Overlay => "Overlay",
            BlendMode::SoftLight => "Soft Light",
            BlendMode::Difference => "Difference",
        }
    }
}

/// A single compositing layer. Owns its own rendering pipeline and parameters.
pub struct Layer {
    pub name: String,
    pub effect_index: Option<usize>,
    pub pass_executor: PassExecutor,
    pub uniform_buffer: UniformBuffer,
    pub param_store: ParamStore,
    pub uniforms: ShaderUniforms,
    pub blend_mode: BlendMode,
    pub opacity: f32,
    pub enabled: bool,
    pub locked: bool,
    pub pinned: bool,
    pub shader_sources: Vec<String>,
    pub shader_error: Option<String>,
    pub postprocess: PostProcessDef,
}

impl Layer {
    /// Execute this layer's render passes. Returns the final HDR target.
    pub fn execute(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
    ) -> &RenderTarget {
        self.pass_executor
            .execute(encoder, &self.uniform_buffer, queue, &self.uniforms)
    }

    /// Flip ping-pong targets for next frame.
    pub fn flip(&mut self) {
        self.pass_executor.flip();
    }

    /// Resize all render targets.
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        placeholder: &PlaceholderTexture,
    ) {
        self.pass_executor
            .resize(device, width, height, &self.uniform_buffer, placeholder);
    }
}

/// Lightweight snapshot of layer state for UI rendering (avoids borrow conflicts).
#[derive(Debug, Clone)]
pub struct LayerInfo {
    pub name: String,
    pub effect_index: Option<usize>,
    pub effect_name: Option<String>,
    pub blend_mode: BlendMode,
    pub opacity: f32,
    pub enabled: bool,
    pub locked: bool,
    pub pinned: bool,
    pub has_particles: bool,
    pub shader_error: Option<String>,
}

/// Manages an ordered stack of layers.
pub struct LayerStack {
    pub layers: Vec<Layer>,
    pub active_layer: usize,
}

impl LayerStack {
    pub fn new() -> Self {
        Self {
            layers: Vec::new(),
            active_layer: 0,
        }
    }

    /// Add a new empty layer with default pass executor.
    pub fn add_layer(&mut self, device: &wgpu::Device, name: String, default_executor: PassExecutor) {
        let uniform_buffer = UniformBuffer::new(device);
        self.layers.push(Layer {
            name,
            effect_index: None,
            pass_executor: default_executor,
            uniform_buffer,
            param_store: ParamStore::new(),
            uniforms: ShaderUniforms::zeroed(),
            blend_mode: BlendMode::Normal,
            opacity: 1.0,
            enabled: true,
            locked: false,
            pinned: false,
            shader_sources: Vec::new(),
            shader_error: None,
            postprocess: PostProcessDef::default(),
        });
    }

    /// Remove a layer by index. Adjusts active_layer if needed.
    pub fn remove_layer(&mut self, index: usize) {
        if self.layers.len() <= 1 || index >= self.layers.len() {
            return; // never remove the last layer
        }
        self.layers.remove(index);
        if self.active_layer >= self.layers.len() {
            self.active_layer = self.layers.len() - 1;
        }
    }

    /// Move a layer from `from` to `to` position.
    pub fn move_layer(&mut self, from: usize, to: usize) {
        if from >= self.layers.len() || to >= self.layers.len() || from == to {
            return;
        }
        let layer = self.layers.remove(from);
        self.layers.insert(to, layer);
        // Track active layer through the move
        if self.active_layer == from {
            self.active_layer = to;
        } else if from < to && self.active_layer > from && self.active_layer <= to {
            self.active_layer -= 1;
        } else if from > to && self.active_layer >= to && self.active_layer < from {
            self.active_layer += 1;
        }
    }

    pub fn active(&self) -> Option<&Layer> {
        self.layers.get(self.active_layer)
    }

    pub fn active_mut(&mut self) -> Option<&mut Layer> {
        self.layers.get_mut(self.active_layer)
    }

    /// Collect lightweight snapshots for UI.
    pub fn layer_infos(&self, effects: &[crate::effect::format::PfxEffect]) -> Vec<LayerInfo> {
        self.layers
            .iter()
            .map(|l| LayerInfo {
                name: l.name.clone(),
                effect_index: l.effect_index,
                effect_name: l.effect_index.and_then(|i| effects.get(i)).map(|e| e.name.clone()),
                blend_mode: l.blend_mode,
                opacity: l.opacity,
                enabled: l.enabled,
                locked: l.locked,
                pinned: l.pinned,
                has_particles: l.pass_executor.particle_system.is_some(),
                shader_error: l.shader_error.clone(),
            })
            .collect()
    }

    /// Number of enabled layers.
    pub fn enabled_count(&self) -> usize {
        self.layers.iter().filter(|l| l.enabled).count()
    }
}
