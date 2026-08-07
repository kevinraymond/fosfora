//! trama — the node-graph effect-chain system.
//!
//! Phase-0 survey and design record: `docs/trama/INTEGRATION.md`; running
//! decision log: `docs/trama/DECISIONS.md`. M0 builds the graph model, the
//! manifest registry, the scene-level executor behind the
//! `execute_and_composite` seam, and the canvas.
#![allow(dead_code)] // M0 lands in stages; removed when the canvas commit wires everything up.

pub mod effect;
pub mod exec;
pub mod graph;
pub mod node;

use crate::effect::loader::EffectLoader;
use crate::gpu::ShaderUniforms;
use crate::gpu::audio_textures::AudioTextures;
use crate::gpu::placeholder::PlaceholderTexture;
use crate::gpu::render_target::RenderTarget;

/// Which pipeline produces the frame: the 8-layer stack or the trama graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderMode {
    #[default]
    Layers,
    Trama,
}

/// The app-facing façade: graph + registry + executor + UI state, owned by
/// `App` the way `shader_editor` is. `execute_and_composite` consumes it in
/// `Trama` mode; `mode` defaults to `Layers`, so constructing the system is
/// behaviorally inert until the canvas flips the switch.
pub struct TramaSystem {
    pub mode: RenderMode,
    pub canvas_open: bool,
    pub graph: graph::NodeGraph,
    pub registry: effect::TramaRegistry,
    /// Most recent plan-build failure, if any — shown in the canvas window.
    /// The executor keeps rendering its last-good plan meanwhile (I4).
    pub last_error: Option<String>,
    executor: exec::executor::TramaExecutor,
    /// This frame's global uniform template (time/resolution/audio mirror),
    /// captured in `App::update` after the mirror is complete.
    frame_uniforms: ShaderUniforms,
}

impl TramaSystem {
    pub fn new(
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        loader: &EffectLoader,
        placeholder: &PlaceholderTexture,
        audio: &AudioTextures,
        width: u32,
        height: u32,
    ) -> Self {
        let registry =
            effect::TramaRegistry::load(device, cache, loader, &effect::trama_effects_dir());
        Self {
            mode: RenderMode::default(),
            canvas_open: false,
            graph: graph::NodeGraph::new_with_output(),
            registry,
            last_error: None,
            executor: exec::executor::TramaExecutor::new(device, placeholder, audio, width, height),
            frame_uniforms: ShaderUniforms::zeroed(),
        }
    }

    /// Capture this frame's fully-mirrored global uniforms; the executor
    /// copies them per node and overwrites only `params`.
    pub fn set_frame_uniforms(&mut self, template: &ShaderUniforms) {
        self.frame_uniforms = *template;
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.executor.resize(device, width, height);
    }

    /// `(in_use, total)` pooled targets — the canvas debug line.
    pub fn pool_stats(&self) -> (usize, usize) {
        self.executor.pool_stats()
    }

    /// Execute the graph and return the Output node's target. Called from
    /// `execute_and_composite` when `mode == Trama`.
    pub(crate) fn execute(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
    ) -> &RenderTarget {
        self.executor.execute(
            &mut self.graph,
            &self.registry,
            &self.frame_uniforms,
            device,
            queue,
            encoder,
            &mut self.last_error,
        )
    }
}
