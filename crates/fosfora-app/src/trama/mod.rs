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
pub mod persist;
pub mod ser;
pub mod ui;

use crate::audio::features::AudioFeatures;
use crate::effect::loader::EffectLoader;
use crate::gpu::ShaderUniforms;
use crate::gpu::audio_textures::AudioTextures;
use crate::gpu::placeholder::PlaceholderTexture;
use crate::gpu::render_target::RenderTarget;

/// Which chain the canvas is pointed at. A choice, not a chain id: "the
/// selected layer" keeps following the layer panel, and the chain it names is
/// resolved once per frame in `App::update`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CanvasTarget {
    #[default]
    SelectedLayer,
    Master,
}

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
    /// The canvas tab: the selected layer's chain, or the master chain.
    pub canvas_target: CanvasTarget,
    /// Which chain the canvas is editing — `canvas_target` resolved against
    /// this frame's layer selection. The master chain when there is no layer
    /// to select.
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
    /// The Export / Import file dialogs, and imports waiting to be applied.
    pub io: persist::ChainIo,
    /// Has the chain on the canvas been edited since it was last looked at?
    edit_watch: persist::EditWatch,
    frames_since_edit_poll: u32,
    edited: bool,
}

/// How often the open canvas is checked for edits, in frames. An edit only
/// has to light the "unsaved" marker, so half a second at 60 fps is prompt
/// enough and keeps a document capture out of the per-frame path.
const EDIT_POLL_FRAMES: u32 = 30;

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
        let canvas = ui::canvas::CanvasState::default();
        Self {
            canvas_open: false,
            canvas_target: CanvasTarget::default(),
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
            io: persist::ChainIo::default(),
            edit_watch: persist::EditWatch::default(),
            frames_since_edit_poll: 0,
            edited: false,
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

        // A `.fio.json` picked in the import dialog, on whichever frame the
        // dialog thread finished. It names the chain that asked, which may
        // have gone away while the dialog was open.
        for imported in self.io.drain() {
            let outcome = imported
                .result
                .and_then(|doc| persist::load_into(layer_stack, self, imported.chain, &doc));
            match outcome {
                Ok(notes) => {
                    for note in &notes {
                        log::warn!("trama: import: {note}");
                    }
                    // Loaded over whatever was there: that IS an edit.
                    self.edited = true;
                    self.canvas.status = (!notes.is_empty())
                        .then(|| format!("imported, with {} repair(s) — see the log", notes.len()));
                }
                Err(e) => {
                    log::error!("trama: import failed: {e}");
                    self.canvas.status = Some(format!("import failed: {e}"));
                }
            }
        }

        // Chains are only edited through the canvas, so only look while it is
        // open, and not every frame.
        self.frames_since_edit_poll += 1;
        if self.canvas_open && self.frames_since_edit_poll >= EDIT_POLL_FRAMES {
            self.frames_since_edit_poll = 0;
            let chain = self.active_chain;
            let graph = match chain {
                node::ChainId::Master => Some(&self.master),
                node::ChainId::Layer(_) => layer_stack
                    .layers
                    .iter()
                    .filter_map(|l| l.chain.as_deref())
                    .find(|c| c.id == chain)
                    .map(|c| &c.graph),
            };
            if let Some(graph) = graph {
                let doc = ser::ChainDoc::capture(graph, |n| self.canvas.position(chain, n));
                self.edited |= self.edit_watch.observe(chain, doc);
            }
        }
    }

    /// Effect files changed on disk (or, with `all`, the shared shader library
    /// did): reload them, and bring every live node of a changed effect in
    /// line with its new manifest — in every layer's chain and the master.
    ///
    /// Compiles on the calling thread. A fullscreen effect is a few tens of
    /// milliseconds, paid once per save of a file being edited live; the layer
    /// side compiles off-thread because its shaders are far heavier.
    pub fn reload_effects(
        &mut self,
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        loader: &EffectLoader,
        paths: &[std::path::PathBuf],
        all: bool,
        layer_stack: &mut crate::gpu::layer::LayerStack,
    ) {
        let mut outcomes: Vec<effect::Reloaded> = paths
            .iter()
            .map(|p| self.registry.reload_file(device, cache, loader, p))
            .collect();
        if all {
            outcomes.extend(self.registry.reload_all(
                device,
                cache,
                loader,
                &effect::trama_effects_dir(),
            ));
        }
        for outcome in outcomes {
            let effect::Reloaded::Swapped {
                id,
                manifest_changed: true,
            } = outcome
            else {
                continue;
            };
            let Some(def) = self.registry.get(&id) else {
                continue;
            };
            let (inputs, params) = (def.inputs, def.params.clone());
            let mut sync = |chain: node::ChainId, graph: &mut graph::NodeGraph| {
                let version = graph.version();
                let notes = graph.sync_manifest(&id, inputs, &params);
                for note in &notes {
                    log::warn!("trama: reload: {note}");
                }
                // Pins moved, so wires may have gone: the canvas view holds its
                // own copy of the wire set and has to be rebuilt from the graph,
                // where its nodes are.
                if graph.version() != version {
                    let layout = graph
                        .nodes()
                        .iter()
                        .filter_map(|n| Some((n.id, self.canvas.position(chain, n.id)?)))
                        .collect();
                    self.canvas.replace_chain(chain, layout);
                }
            };
            for layer in &mut layer_stack.layers {
                if let Some(chain) = layer.chain.as_deref_mut() {
                    sync(chain.id, &mut chain.graph);
                }
            }
            sync(node::ChainId::Master, &mut self.master);
        }
    }

    /// Was a chain edited since this was last asked? For the preset's
    /// "unsaved" marker.
    pub fn take_edited(&mut self) -> bool {
        std::mem::take(&mut self.edited)
    }

    /// The executor's feedback parity for this frame. The frame graph pairs a
    /// layer's two ping-pong targets against it when handing the layer's
    /// picture to its chain.
    pub fn parity(&self) -> usize {
        self.executor.parity()
    }

    /// Forget everything held for a chain whose layer is gone: the executor's
    /// feedback pairs and thumbnails, and the canvas's node positions. Slots
    /// are reused, so a view left behind would reappear under the next chain
    /// to land on that slot.
    pub fn drop_chain(&mut self, chain: node::ChainId) {
        self.executor.drop_chain(chain);
        self.canvas.drop_chain(chain);
    }

    /// A chain's graph was REPLACED (a preset or a `.fio.json` was loaded into
    /// it): forget everything cached for the slot — plan, bind groups, echo
    /// buffers, thumbnails, and its canvas view, the master's included — and
    /// remember the layout the new graph was saved with.
    pub fn reset_chain(&mut self, chain: node::ChainId, layout: Vec<(node::NodeId, [f32; 2])>) {
        self.executor.drop_chain(chain);
        self.canvas.replace_chain(chain, layout);
        self.edit_watch.forget();
    }

    /// Does the master chain need an output target this frame? Yes while it
    /// reaches its Output, and also while it is the chain on the canvas: a
    /// patch being built has to keep its thumbnails running before the last
    /// wire lands, the same rule `run_layer_chain` applies to a layer's.
    pub fn master_live(&self) -> bool {
        self.master.contributes() || self.master_on_screen()
    }

    /// Is the canvas open on the master chain?
    pub fn master_on_screen(&self) -> bool {
        self.canvas_open && self.active_chain == node::ChainId::Master
    }

    /// How many plans the executor has built — proof that a chain actually
    /// ran, for probes whose expected picture is "unchanged".
    #[cfg(test)]
    pub(crate) fn plans_built(&self) -> u64 {
        self.executor.plans_built()
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
