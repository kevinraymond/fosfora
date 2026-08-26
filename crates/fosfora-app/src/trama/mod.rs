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

/// The app-facing façade: registry + executor + UI state + the master chain,
/// owned by `App` the way `shader_editor` is.
///
/// The per-layer chains are NOT here — each lives on its `Layer`
/// (`gpu::layer::LayerChain`), so it travels with its layer and keeps a slot
/// id that reordering cannot disturb. What is here is everything shared:
/// one registry, one executor, one audio view, one modulation resolve per
/// frame across every chain.
pub struct TramaSystem {
    pub canvas_open: bool,
    /// Which chain the canvas is editing. Follows the selected layer; falls
    /// back to the master chain when that layer has none.
    pub active_chain: node::ChainId,
    /// The chain that post-processes the composited frame, upstream of
    /// `PostProcessDef` (which keeps ownership of tonemapping).
    pub master: graph::NodeGraph,
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
        let master = graph::NodeGraph::new_with_output();
        let canvas = ui::canvas::CanvasState::new(&master);
        Self {
            canvas_open: false,
            active_chain: node::ChainId::Master,
            master,
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
    ///
    /// Resolves EVERY chain, not just the one on screen: an oscillator whose
    /// phase went cold while another layer was selected would jump the moment
    /// you looked at it. Same reason orphan nodes resolve.
    pub fn update(
        &mut self,
        layer_stack: &mut crate::gpu::layer::LayerStack,
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
        for layer in &mut layer_stack.layers {
            let Some(chain) = layer.chain.as_deref_mut() else {
                continue;
            };
            for node in chain.graph.params_iter_mut() {
                modulation::resolve_node(node.params, node.mods, dt, &self.audio_view);
            }
        }
        for node in self.master.params_iter_mut() {
            modulation::resolve_node(node.params, node.mods, dt, &self.audio_view);
        }
    }

    /// The executor's feedback parity for this frame. The frame graph pairs a
    /// layer's two ping-pong targets against it when handing the layer's
    /// picture to its chain.
    pub fn parity(&self) -> usize {
        self.executor.parity()
    }

    /// Forget everything the executor holds for a chain whose layer is gone.
    pub fn drop_chain(&mut self, chain: node::ChainId) {
        self.executor.drop_chain(chain);
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.executor.resize(width, height);
    }

    /// `(in_use, total)` pooled targets — the canvas debug line.
    pub fn pool_stats(&self) -> (usize, usize) {
        self.executor.pool_stats()
    }

    /// Live feedback ping-pong pairs — the canvas debug line.
    pub fn feedback_stats(&self) -> usize {
        self.executor.feedback_stats()
    }

    /// Execute the master chain into the caller's `out` target.
    ///
    /// Separate from [`Self::execute_chain`] only because the master graph is
    /// a field here: `&mut self` and `&self.master` are disjoint field borrows
    /// inside the impl, and nothing outside it can spell that.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_master(
        &mut self,
        input: Option<exec::executor::ChainInputSource<'_>>,
        out: &RenderTarget,
        out_generation: u64,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        profiler: crate::gpu::profiler::ProfilerHandle<'_>,
    ) {
        let mut scope = profiler.scope("trama", encoder);
        let chain = node::ChainId::Master;
        let previews_on = self.canvas_open && chain == self.active_chain;
        self.executor.execute(
            chain,
            &self.master,
            &self.registry,
            input,
            out,
            out_generation,
            &self.frame_uniforms,
            previews_on,
            device,
            queue,
            scope.encoder(),
            profiler,
            &mut self.last_error,
        );
    }

    /// Execute one chain into the caller's `out` target.
    ///
    /// Returns nothing on purpose. The output target belongs to the caller
    /// (`gpu::chain_targets`), so `&mut TramaSystem` can be re-borrowed freely
    /// inside a loop that is simultaneously holding shared references to the
    /// targets earlier iterations wrote — which is exactly what the layer
    /// composite loop does.
    ///
    /// Previews (and orphan execution) run for the chain on screen only: no
    /// one can see a thumbnail through a closed window, or for a layer they
    /// are not looking at, and running orphans everywhere would burn GPU
    /// rendering invisible content in every chain at once.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_chain(
        &mut self,
        chain: node::ChainId,
        graph: &graph::NodeGraph,
        input: Option<exec::executor::ChainInputSource<'_>>,
        out: &RenderTarget,
        out_generation: u64,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        profiler: crate::gpu::profiler::ProfilerHandle<'_>,
    ) {
        // Parent timing scope: the executor's per-node scopes nest under it,
        // so the profiler panel shows both the trama total and the split.
        let mut scope = profiler.scope("trama", encoder);
        let previews_on = self.canvas_open && chain == self.active_chain;
        self.executor.execute(
            chain,
            graph,
            &self.registry,
            input,
            out,
            out_generation,
            &self.frame_uniforms,
            previews_on,
            device,
            queue,
            scope.encoder(),
            profiler,
            &mut self.last_error,
        );
    }

    /// Register freshly created preview targets with egui and free the dead
    /// ones. Called from `main.rs` right before the canvas draws, where the
    /// egui renderer lives.
    pub fn register_previews(&mut self, device: &wgpu::Device, renderer: &mut egui_wgpu::Renderer) {
        self.executor.register_previews(device, renderer);
    }
}
