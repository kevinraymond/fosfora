//! trama — the node-graph effect-chain system.
//!
//! Phase-0 survey and design record: `docs/trama/INTEGRATION.md`; running
//! decision log: `docs/trama/DECISIONS.md`. M0 builds the graph model, the
//! manifest registry, the scene-level executor behind the
//! `execute_and_composite` seam, and the canvas.
pub mod audio;
pub mod effect;
pub mod exec;
pub mod graph;
pub mod modulation;
pub mod node;
pub mod ui;

use crate::audio::features::AudioFeatures;
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
    pub canvas: ui::canvas::CanvasState,
    /// Most recent plan-build failure, if any — shown in the canvas window.
    /// The executor keeps rendering its last-good plan meanwhile (I4).
    pub last_error: Option<String>,
    executor: exec::executor::TramaExecutor,
    /// This frame's global uniform template (time/resolution/audio mirror),
    /// captured in `App::update` after the mirror is complete.
    frame_uniforms: ShaderUniforms,
    /// This frame's modulation-source snapshot, advanced in [`Self::update`].
    audio_view: audio::AudioView,
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
        let graph = graph::NodeGraph::new_with_output();
        let canvas = ui::canvas::CanvasState::new(&graph);
        Self {
            mode: RenderMode::default(),
            canvas_open: false,
            graph,
            registry,
            canvas,
            last_error: None,
            executor: exec::executor::TramaExecutor::new(
                device,
                cache,
                placeholder,
                audio,
                width,
                height,
            ),
            frame_uniforms: ShaderUniforms::zeroed(),
            audio_view: audio::AudioView::default(),
        }
    }

    /// Once-per-frame advance, from `App::update`: capture the fully-mirrored
    /// uniform template, fold the audio view, then resolve every node's
    /// modulations (orphans included — phases stay warm across rewires).
    ///
    /// This is the ONLY place modulation state moves. `execute` just reads
    /// the cached resolved values, so the dissolve path's second execute per
    /// frame cannot double-advance oscillators, and the canvas (drawn after
    /// `execute` has borrowed the system) reads the same values for the
    /// inspector's ghost indicators.
    pub fn update(
        &mut self,
        dt: f32,
        template: &ShaderUniforms,
        features: &AudioFeatures,
        mel: &[f32],
    ) {
        // Feedback parity advances here for the same reason modulation does:
        // once per frame, never per execute.
        self.executor.begin_frame();
        self.frame_uniforms = *template;
        self.audio_view.update(dt, features, mel);
        for node in self.graph.params_iter_mut() {
            modulation::resolve_node(node.params, node.mods, dt, &self.audio_view);
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.executor.resize(device, width, height);
    }

    /// `(in_use, total)` pooled targets — the canvas debug line.
    pub fn pool_stats(&self) -> (usize, usize) {
        self.executor.pool_stats()
    }

    /// Live feedback ping-pong pairs — the canvas debug line.
    pub fn feedback_stats(&self) -> usize {
        self.executor.feedback_stats()
    }

    /// Execute the graph and return the Output node's target. Called from
    /// `execute_and_composite` when `mode == Trama`, and from `App::render`
    /// for preview-only execution while patching in Layers mode. Previews
    /// (and orphan execution) follow the canvas: no one can see a thumbnail
    /// through a closed window.
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
            self.canvas_open,
            device,
            queue,
            encoder,
            &mut self.last_error,
        )
    }

    /// Register freshly created preview targets with egui and free the dead
    /// ones. Called from `main.rs` right before the canvas draws, where the
    /// egui renderer lives.
    pub fn register_previews(&mut self, device: &wgpu::Device, renderer: &mut egui_wgpu::Renderer) {
        self.executor.register_previews(device, renderer);
    }
}
