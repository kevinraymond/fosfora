//! The trama scene executor: turns the node graph into ordered wgpu passes.
//!
//! Per-node uniforms live in one arena buffer at static offsets (stride =
//! `ShaderUniforms` rounded up to the device's uniform-offset alignment); each
//! node gets its own bind group against the effect's ABI v3 layout, cached in
//! an [`ExecPlan`] keyed on the graph's structural version. Steady state does
//! zero allocations and creates zero textures (I8): a frame is N uniform
//! writes + N fullscreen passes in topo order.
//!
//! I4 story: invalid states are mostly unreachable (cycles are refused at
//! `connect`, Output exists by construction, broken effect files never enter
//! the registry). If a plan build still fails, the previous plan keeps
//! rendering and the error is surfaced; with no previous plan the output is
//! deliberately cleared black — never a silent fall-back to the layer stack.

use std::collections::HashMap;
use std::num::NonZeroU64;

use crate::gpu::audio_textures::AudioTextures;
use crate::gpu::placeholder::PlaceholderTexture;
use crate::gpu::render_target::{PingPongTarget, RenderTarget};
use crate::gpu::{GpuContext, ShaderPipeline, ShaderUniforms};

use super::super::effect::TramaRegistry;
use super::super::graph::NodeGraph;
use super::super::node::{ChainId, ChainNode, NodeId, NodeKind};
use super::textures::TexturePool;

const UNIFORM_SIZE: u64 = std::mem::size_of::<ShaderUniforms>() as u64;
/// Per-chain arena slot floor — grows at plan build if a graph outgrows it.
const INITIAL_SLOTS_PER_CHAIN: u32 = 16;
/// Chain regions the arena starts with: `MAX_LAYERS` layer chains plus the
/// master chain. Grows the same way `slots_per_chain` does.
const INITIAL_CHAIN_CAPACITY: u32 = crate::bindings::catalog::MAX_LAYERS as u32 + 1;

/// Passthrough fragment for [`StepKind::Copy`] steps. Effect-shaped: built
/// against the standard 1-input ABI layout so copy steps reuse the same
/// bind-group machinery as effect passes, declaring only the binding it
/// reads (a shader may use a subset of its layout). `textureLoad` because
/// source and destination are always the same size — an exact copy, no
/// filtering, no resolution uniform.
const COPY_FS: &str = "
@group(0) @binding(7) var input0_tex: texture_2d<f32>;
@fragment
fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    return textureLoad(input0_tex, vec2i(pos.xy), 0);
}
";

/// Downscale blit into a node's 192×108 `Rgba8Unorm` preview target. The
/// linear→sRGB encode happens HERE because the target is deliberately not an
/// `-srgb` format — see the `preview` module docs for the egui color-space
/// story. HDR input is clamped (the real output gets tonemapped in
/// postprocess; a thumbnail just needs to be legible). Alpha forced to 1 so
/// egui never blends the thumbnail with the node background.
const PREVIEW_FS: &str = "
@group(0) @binding(7) var input0_tex: texture_2d<f32>;
@group(0) @binding(8) var input0_samp: sampler;
fn srgb_encode(c: f32) -> f32 {
    return select(12.92 * c, 1.055 * pow(c, 1.0 / 2.4) - 0.055, c > 0.0031308);
}
@fragment
fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let uv = pos.xy / vec2f(192.0, 108.0);
    let c = clamp(textureSample(input0_tex, input0_samp, uv).rgb, vec3f(0.0), vec3f(1.0));
    return vec4f(srgb_encode(c.r), srgb_encode(c.g), srgb_encode(c.b), 1.0);
}
";

/// How often the round-robin preview blit runs (every Nth frame, D4).
const PREVIEW_CADENCE: u64 = 3;

/// Round `size` up to a multiple of `align` (a power of two).
/// First arena slot owned by the chain at `chain_index`, given the current
/// per-chain reservation. Factored pure — like `textures::select_free` — so
/// the disjointness that keeps two chains from staging uniforms over each
/// other is testable without a GPU.
pub(crate) fn chain_slot_base(chain_index: u32, slots_per_chain: u32) -> u32 {
    chain_index * slots_per_chain
}

pub(crate) fn aligned_stride(size: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    size.div_ceil(align) * align
}

#[derive(Clone, Copy)]
enum TargetSlot {
    Pool(usize),
    Output,
    /// This frame's write buffer of the step's node's feedback pair,
    /// resolved at execute time as `feedback[&node].targets[parity]`.
    FeedbackWrite,
}

#[derive(Clone, Copy)]
enum StepKind {
    /// Fullscreen pass through `registry.effects[i].pipeline`.
    Effect { effect: usize },
    /// Executor-owned passthrough pipeline: a Feedback node's input→write
    /// copy, or its read→Output blit when a Feedback node feeds Output.
    Copy,
    /// The node names an effect the registry does not have — a patch from
    /// someone else's file, or an effect file deleted under a running app.
    /// Clears its target to [`MISSING_EFFECT_COLOR`] and draws nothing, so
    /// the hole is visible in the picture while the rest of the chain runs.
    Missing,
}

struct Step {
    node: ChainNode,
    kind: StepKind,
    target: TargetSlot,
    uniform_offset: u64,
    /// Indexed by the executor's global `parity` at execute time — the
    /// pass_executor #1481 idiom: a step reading a feedback node's output
    /// binds its *read* buffer, which alternates every frame, so both
    /// variants are prebuilt. Steps with no parity-dependent input carry two
    /// clones of one bind group (wgpu handles are Arc-backed; the clone is
    /// free). `None` for a [`StepKind::Missing`] step, which draws nothing.
    bind_groups: Option<[wgpu::BindGroup; 2]>,
}

/// What a node whose effect is missing renders: opaque magenta. Not the
/// signal on its own — the canvas names the node `missing: <id>` — but it
/// keeps the hole from reading as an intentional black.
const MISSING_EFFECT_COLOR: wgpu::Color = wgpu::Color {
    r: 1.0,
    g: 0.0,
    b: 1.0,
    a: 1.0,
};

/// The texture a chain's [`NodeKind::ChainInput`] node samples — the layer's
/// rendered target for a layer chain, the composited frame for the master
/// chain.
///
/// Supplied per feedback parity because a layer that ping-pongs returns a
/// *different* target each frame (`pass_executor` writes
/// `targets[flip_parity]`) while bind groups are built once, at plan time.
/// `per_parity[p]` must be the target the host renders into on frames where
/// the executor's parity is `p`; a source that never alternates passes the
/// same view twice. The layer flip and [`TramaExecutor::begin_frame`] advance
/// in lockstep (spike #2098), which is what makes a fixed pairing possible.
#[derive(Clone, Copy)]
pub struct ChainInputSource<'a> {
    pub per_parity: [(&'a wgpu::TextureView, &'a wgpu::Sampler); 2],
    /// Bumped by the host whenever the *identity* of either target changes —
    /// layer rebuilt, effect swapped, output resized.
    ///
    /// This is part of the plan key, and it has to be: bind groups capture
    /// the views at plan build, and none of those events touches the graph,
    /// so nothing else would trigger a replan. Without it a rebuilt layer
    /// keeps feeding its chain the picture from before the rebuild.
    pub generation: u64,
}

impl<'a> ChainInputSource<'a> {
    /// A source that alternates between two targets: `current` is the one the
    /// host wrote on this frame, where the executor's parity is `parity`.
    ///
    /// The generation names the pair in PARITY order, not `(current, other)`
    /// order. Those two swap roles every frame, so a generation that followed
    /// them moved every frame and replanned the chain every frame — every bind
    /// group rebuilt, against I8, with nothing on screen to show for it. In
    /// parity order it holds still for as long as the lockstep does, and moves
    /// — forcing a re-pair — the moment the lockstep breaks.
    pub fn paired(current: &'a RenderTarget, other: &'a RenderTarget, parity: usize) -> Self {
        let mut targets = [current, current];
        targets[1 - parity] = other;
        Self {
            per_parity: targets.map(|t| (&t.view, &t.sampler)),
            generation: crate::gpu::render_target::pair_id(targets[0], targets[1]),
        }
    }

    /// A source whose target identity never alternates with parity — a media
    /// layer, or the composited frame the master chain reads.
    #[allow(dead_code)] // used by the frame-graph integration in stage C
    pub fn stable(
        view: &'a wgpu::TextureView,
        sampler: &'a wgpu::Sampler,
        generation: u64,
    ) -> Self {
        Self {
            per_parity: [(view, sampler), (view, sampler)],
            generation,
        }
    }
}

/// One node's thumbnail blit: samples the node's output (per parity, same
/// story as [`Step::bind_groups`]) into its persistent preview target.
struct PreviewBlit {
    node: ChainNode,
    bind_groups: [wgpu::BindGroup; 2],
}

struct ExecPlan {
    /// The `TramaRegistry::generation` this plan's effect indices and bind
    /// groups were built against.
    registry_generation: u64,
    /// The `NodeGraph::version()` this plan was built for.
    version: u64,
    /// The second plan key: whether orphans execute and previews blit
    /// (= canvas open). Toggling replans — a rare human-speed event.
    previews_on: bool,
    /// Effect passes in topological order, then all feedback copy steps —
    /// a copy only needs its producer to have run this frame, and nothing
    /// reads a write buffer until next frame, so end-of-frame placement is
    /// universally correct (chained feedbacks included: copies read *read*
    /// buffers, write *write* buffers — always disjoint textures).
    steps: Vec<Step>,
    /// Round-robin thumbnail blits; empty when `previews_on` is false.
    previews: Vec<PreviewBlit>,
    /// False ⇒ nothing feeds Output; `execute` clears the output target.
    output_written: bool,
    /// Which host input this plan was built against: `None` for no input,
    /// `Some(generation)` otherwise. Part of the plan key — see
    /// [`ChainInputSource::generation`].
    chain_input: Option<u64>,
    /// Which output target this plan was built against. Part of the plan key
    /// for the same reason `chain_input` is: the plan-time `resolve` closure
    /// SAMPLES the output target (a preview blit of the final producer does
    /// exactly that), so a recreated target leaves stale bind groups reading a
    /// dropped texture, and nothing about that event touches the graph.
    out_generation: u64,
    /// First arena slot this chain owns. Chains write their uniforms into
    /// disjoint regions: every `queue.write_buffer` in the frame is staged
    /// before any pass runs, so overlapping regions would have the last
    /// chain's values render for all of them.
    base_slot: u32,
}

pub struct TramaExecutor {
    arena: wgpu::Buffer,
    stride: u64,
    /// Arena slots reserved for each chain. Chain regions are fixed rather
    /// than packed, because a step's bind group embeds its absolute offset:
    /// repacking would silently invalidate every other chain's bind groups.
    /// Growth is a rare structural event and drops all plans (see
    /// [`Self::ensure_slots`]).
    slots_per_chain: u32,
    /// How many chain regions the arena currently holds.
    chain_capacity: u32,
    pool: TexturePool,
    /// One plan per chain, each keyed on its own graph version.
    plans: HashMap<ChainId, ExecPlan>,
    /// Ping-pong pairs OUTSIDE the plan, keyed by [`ChainNode`]: contents
    /// survive replans, so a rewire elsewhere in the graph never clears an
    /// unrelated echo. Keyed by the pair and not a bare `NodeId` because
    /// every chain numbers its nodes from zero — see [`ChainNode`]. Synced
    /// (created/pruned) at plan build; cleared on resize.
    feedback: HashMap<ChainNode, PingPongTarget>,
    /// Global feedback parity (#1481): copy steps write `targets[parity]`,
    /// consumers read `targets[1 - parity]`. Advances ONLY in
    /// [`Self::begin_frame`] — the dissolve path executes twice per frame.
    /// `PingPongTarget::current` is deliberately unused here; one global
    /// parity is the whole point of the idiom.
    parity: usize,
    /// Bumped whenever a pair is (re)created — steady-state tests assert it
    /// holds still while echoes survive replans.
    #[allow(dead_code)] // read from tests
    feedback_generation: u64,
    /// Successful plan builds, ever. The direct observable for "did the plan
    /// key do its job": tests assert it moves when a key term moves and holds
    /// still when nothing did (which is also I8's steady state).
    #[allow(dead_code)] // read from tests
    plans_built: u64,
    copy_pipeline: ShaderPipeline,
    preview_pipeline: ShaderPipeline,
    /// Persistent thumbnail targets, outside the plan for the same reason as
    /// `feedback`: stable texture identity across replans (egui registers a
    /// texture once).
    previews: super::preview::PreviewSet,
    /// Frames seen (advanced in `begin_frame`); gates the preview cadence.
    frame_index: u64,
    /// Which planned preview blits this cadence tick, round-robin.
    preview_cursor: usize,
    // Stable resource handles, cloned once (wgpu handles are ref-counted; the
    // AudioTextures views are fixed-size and never recreated, so bind groups
    // built against them stay valid).
    prev_view: wgpu::TextureView,
    prev_sampler: wgpu::Sampler,
    waveform: wgpu::TextureView,
    spectrum: wgpu::TextureView,
    spectrogram: wgpu::TextureView,
    audio_sampler: wgpu::Sampler,
    width: u32,
    height: u32,
}

impl TramaExecutor {
    pub fn new(
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        placeholder: &PlaceholderTexture,
        audio: &AudioTextures,
        width: u32,
        height: u32,
    ) -> Self {
        let stride = aligned_stride(
            UNIFORM_SIZE,
            u64::from(device.limits().min_uniform_buffer_offset_alignment),
        );
        Self {
            arena: create_arena(
                device,
                stride,
                INITIAL_SLOTS_PER_CHAIN * INITIAL_CHAIN_CAPACITY,
            ),
            stride,
            slots_per_chain: INITIAL_SLOTS_PER_CHAIN,
            chain_capacity: INITIAL_CHAIN_CAPACITY,
            pool: TexturePool::new(),
            plans: HashMap::new(),
            feedback: HashMap::new(),
            parity: 0,
            feedback_generation: 0,
            plans_built: 0,
            // Baked-in constant shaders: failure here is a programming bug,
            // not an authoring error, so I4's keep-last-good doesn't apply.
            copy_pipeline: ShaderPipeline::new(device, GpuContext::hdr_format(), COPY_FS, cache, 1)
                .expect("built-in trama copy shader compiles"),
            preview_pipeline: ShaderPipeline::new(
                device,
                wgpu::TextureFormat::Rgba8Unorm,
                PREVIEW_FS,
                cache,
                1,
            )
            .expect("built-in trama preview shader compiles"),
            previews: super::preview::PreviewSet::default(),
            frame_index: 0,
            preview_cursor: 0,
            prev_view: placeholder.view.clone(),
            prev_sampler: placeholder.sampler.clone(),
            waveform: audio.waveform_view.clone(),
            spectrum: audio.spectrum_view.clone(),
            spectrogram: audio.spectrogram_view.clone(),
            audio_sampler: audio.sampler.clone(),
            width,
            height,
        }
    }

    /// Output-resolution change: drop the pool, force a replan. No-op when the
    /// size is unchanged. The output targets are the caller's now
    /// (`gpu::chain_targets`), and it drops its own on the same event.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }
        self.width = width;
        self.height = height;
        self.pool.clear();
        // Echo contents are meaningless at a new size; the next plan build
        // recreates pairs cleared at the new resolution.
        self.feedback.clear();
        self.plans.clear();
    }

    /// Grow the arena so every chain region holds `needed` slots. Bind groups
    /// embed absolute offsets, so a resize renumbers every region and all
    /// plans must go — a rare structural event, never steady state (I8).
    fn ensure_slots(&mut self, device: &wgpu::Device, chain: ChainId, needed: u32) {
        let chains = (chain.index() + 1).max(self.chain_capacity);
        if needed <= self.slots_per_chain && chains == self.chain_capacity {
            return;
        }
        self.slots_per_chain = needed.max(self.slots_per_chain).next_power_of_two();
        self.chain_capacity = chains;
        self.arena = create_arena(
            device,
            self.stride,
            self.slots_per_chain * self.chain_capacity,
        );
        self.plans.clear();
    }

    /// Once-per-frame advance, driven from `TramaSystem::update` — never
    /// from `execute`, which the dissolve path runs twice per frame. With
    /// parity fixed for the whole frame, the second execute copies the same
    /// input into the same write buffer (last write wins, deterministic) and
    /// consumers read the same delayed frame twice; likewise the preview
    /// cursor holds still, so a double execute re-blits the same thumbnail.
    pub fn begin_frame(&mut self) {
        self.parity ^= 1;
        self.frame_index = self.frame_index.wrapping_add(1);
        if self.frame_index.is_multiple_of(PREVIEW_CADENCE) {
            self.preview_cursor = self.preview_cursor.wrapping_add(1);
        }
    }

    /// This frame's feedback parity. The frame graph needs it to pair a
    /// layer's two ping-pong targets with the executor's, since a chain's
    /// bind groups are built once and the layer's target alternates.
    pub fn parity(&self) -> usize {
        self.parity
    }

    /// `(in_use, total)` pooled targets — the canvas debug line.
    pub fn pool_stats(&self) -> (usize, usize) {
        self.pool.stats()
    }

    /// Live feedback ping-pong pairs — the canvas debug line.
    pub fn feedback_stats(&self) -> usize {
        self.feedback.len()
    }

    /// Persistent preview targets — the canvas debug line.
    pub fn preview_stats(&self) -> usize {
        self.previews.count()
    }

    /// The egui texture for a node's thumbnail, once registered.
    pub fn preview_tex(&self, node: ChainNode) -> Option<egui::TextureId> {
        self.previews.tex_of(node)
    }

    /// Forget everything belonging to `chain` — its layer was removed.
    pub fn drop_chain(&mut self, chain: ChainId) {
        self.plans.remove(&chain);
        self.feedback.retain(|id, _| id.chain != chain);
        self.previews.drop_chain(chain);
    }

    /// Register new preview targets with egui, free dead ones. Called from
    /// the frame loop (needs the egui renderer); GPU tests skip it.
    pub fn register_previews(&mut self, device: &wgpu::Device, renderer: &mut egui_wgpu::Renderer) {
        self.previews.register(device, renderer);
    }

    /// Execute the graph for this frame and return the Output node's target.
    ///
    /// Replans when the graph's structural version moved (that is what makes
    /// "rewire updates the output next frame" true); otherwise the frame is
    /// pure uniform writes + passes.
    #[allow(clippy::too_many_arguments)]
    pub fn execute(
        &mut self,
        chain: ChainId,
        graph: &NodeGraph,
        registry: &TramaRegistry,
        input: Option<ChainInputSource<'_>>,
        out: &RenderTarget,
        out_generation: u64,
        template: &ShaderUniforms,
        previews_on: bool,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        profiler: crate::gpu::profiler::ProfilerHandle<'_>,
        last_error: &mut Option<String>,
    ) {
        let version = graph.version();
        if self.plans.get(&chain).is_none_or(|p| {
            p.version != version
                || p.registry_generation != registry.generation
                || p.previews_on != previews_on
                || p.chain_input != input.map(|i| i.generation)
                || p.out_generation != out_generation
        }) {
            match self.build_plan(
                chain,
                graph,
                registry,
                input,
                out,
                out_generation,
                device,
                queue,
                version,
                previews_on,
            ) {
                Ok(plan) => {
                    self.plans.insert(chain, plan);
                    self.plans_built += 1;
                    *last_error = None;
                }
                Err(e) => {
                    // Keep the last-good plan rendering (I4). Stamp BOTH plan
                    // keys so the failed build is not retried every frame;
                    // the next structural edit retries naturally.
                    *last_error = Some(e);
                    let base_slot = chain_slot_base(chain.index(), self.slots_per_chain);
                    match self.plans.get_mut(&chain) {
                        Some(p) => {
                            p.version = version;
                            p.registry_generation = registry.generation;
                            p.previews_on = previews_on;
                            p.chain_input = input.map(|i| i.generation);
                            p.out_generation = out_generation;
                        }
                        None => {
                            self.plans.insert(
                                chain,
                                ExecPlan {
                                    registry_generation: registry.generation,
                                    version,
                                    previews_on,
                                    chain_input: input.map(|i| i.generation),
                                    out_generation,
                                    steps: Vec::new(),
                                    previews: Vec::new(),
                                    output_written: false,
                                    base_slot,
                                },
                            );
                        }
                    }
                }
            }
        }
        let plan = self.plans.get(&chain).expect("plan installed above");
        // Every step's uniform offset was baked against this region at plan
        // build. If the arena grew without dropping plans, the regions have
        // renumbered underneath them and chains would stage over each other
        // again — silently, and only once two chains are populated.
        debug_assert_eq!(
            plan.base_slot,
            chain_slot_base(chain.index(), self.slots_per_chain),
            "stale arena region: growing the arena must clear every plan"
        );

        for step in &plan.steps {
            let node = graph
                .node(step.node.node)
                .expect("plan nodes exist in graph");
            let mut u = *template;
            u.params = node.params.pack_to_buffer();
            // Overlay the modulation values resolved in `TramaSystem::update`
            // — this only reads cached state, so the dissolve path's second
            // execute per frame sees identical values (no double-advance).
            super::super::modulation::apply_resolved(&mut u.params, &node.mods);
            // A rate parameter's slot carries its running integral, not its
            // value — advanced in `TramaSystem::update`, only read here.
            super::super::modulation::apply_phases(&mut u.params, &node.params, &node.phases);
            queue.write_buffer(&self.arena, step.uniform_offset, bytemuck::bytes_of(&u));
        }

        for step in &plan.steps {
            let view = match step.target {
                TargetSlot::Pool(i) => &self.pool.get(i).view,
                TargetSlot::Output => &out.view,
                TargetSlot::FeedbackWrite => {
                    &self
                        .feedback
                        .get(&step.node)
                        .expect("plan only emits FeedbackWrite for synced pairs")
                        .targets[self.parity]
                        .view
                }
            };
            // Per-node timing scope (feature `profiling`), labeled by effect
            // id. Declared before `pass` so the pass ends before the scope's
            // end-timestamp lands on the encoder.
            let label = match step.kind {
                StepKind::Effect { effect } => registry.effects[effect].id.0.as_str(),
                StepKind::Copy => "feedback-copy",
                StepKind::Missing => "missing-effect",
            };
            let clear = match step.kind {
                StepKind::Missing => MISSING_EFFECT_COLOR,
                StepKind::Effect { .. } | StepKind::Copy => wgpu::Color::TRANSPARENT,
            };
            let mut step_scope = profiler.scope(label, encoder);
            let mut pass = step_scope
                .encoder()
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("trama-node-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(clear),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
            let pipeline = match step.kind {
                StepKind::Effect { effect } => &registry.effects[effect].pipeline.pipeline,
                StepKind::Copy => &self.copy_pipeline.pipeline,
                // The clear above is the whole step.
                StepKind::Missing => continue,
            };
            let bind_groups = step
                .bind_groups
                .as_ref()
                .expect("every step that draws was planned with bind groups");
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_groups[self.parity], &[]);
            pass.draw(0..3, 0..1);
        }

        if !plan.output_written {
            // Nothing feeds Output: deliberate cleared black, never a stale
            // frame and never a silent fall-back to the layer stack.
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("trama-output-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &out.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }

        // One thumbnail every PREVIEW_CADENCE frames, round-robin (D4) —
        // amortized well under a pass. The cursor moved in `begin_frame`, so
        // a dissolve's double execute re-blits the same node harmlessly.
        if plan.previews_on
            && self.frame_index.is_multiple_of(PREVIEW_CADENCE)
            && !plan.previews.is_empty()
        {
            let blit = &plan.previews[self.preview_cursor % plan.previews.len()];
            if let Some(view) = self.previews.view_of(blit.node) {
                let mut blit_scope = profiler.scope("preview-blit", encoder);
                let mut pass =
                    blit_scope
                        .encoder()
                        .begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("trama-preview-blit"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                        });
                pass.set_pipeline(&self.preview_pipeline.pipeline);
                pass.set_bind_group(0, &blit.bind_groups[self.parity], &[]);
                pass.draw(0..3, 0..1);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_plan(
        &mut self,
        chain: ChainId,
        graph: &NodeGraph,
        registry: &TramaRegistry,
        input: Option<ChainInputSource<'_>>,
        out: &RenderTarget,
        out_generation: u64,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        version: u64,
        previews_on: bool,
    ) -> Result<ExecPlan, String> {
        graph.validate().map_err(|e| e.to_string())?;
        let key = move |node: NodeId| ChainNode::new(chain, node);
        // The execution set: nodes feeding Output — widened to every node
        // (orphans included) when previews are on, per handoff §9.1: orphan
        // subgraphs render only while someone can see their thumbnails.
        let exec_set: Vec<NodeId> = if previews_on {
            graph.topo_order()
        } else {
            graph.live_set()
        };

        // The nodes that run an *effect* pass: not the Output, not bypassed,
        // not Feedback (feedback nodes get copy steps, not passes).
        let step_nodes: Vec<NodeId> = exec_set
            .iter()
            .copied()
            .filter(|&id| {
                let node = graph.node(id).expect("exec-set nodes exist");
                !matches!(
                    node.kind,
                    // ChainInput runs no pass for the same reason Feedback
                    // does not: its texture comes from outside the graph.
                    NodeKind::Output | NodeKind::Feedback | NodeKind::ChainInput
                ) && !node.bypass
            })
            .collect();

        let node_is_feedback = |id: NodeId| {
            graph
                .node(id)
                .is_some_and(|n| matches!(n.kind, NodeKind::Feedback))
        };

        // Bypass aliasing resolves at plan time: the "effective producer" of a
        // pin follows bypassed effects through their input 0 until it lands on
        // a running node (or nothing — placeholder input / cleared output).
        let effective = |mut id: NodeId| -> Option<NodeId> {
            loop {
                let node = graph.node(id)?;
                if !node.bypass {
                    return Some(id);
                }
                match node.kind {
                    NodeKind::Effect { .. } => id = graph.input_source(id, 0)?,
                    // A bypassed source (or Output, unreachable here) yields
                    // nothing. A bypassed FEEDBACK also lands here, and must:
                    // aliasing across the delay edge would re-close the cycle
                    // combinationally (I9) — a node could end up sampling its
                    // own render target in its own pass. Bypass = kill the
                    // echo; consumers read stable black.
                    _ => return None,
                }
            }
        };

        // Feedback nodes that copy this frame: executing, not bypassed,
        // input wired to a running producer. An unwired feedback node emits
        // no copy step and its consumers read the placeholder — with parity
        // still flipping, an un-copied pair would alternate two different
        // stale frames at frame rate (a visible strobe); stable black wins.
        // The pair itself is retained, so rewiring resumes from the stale
        // image rather than restarting the echo from scratch.
        let active_feedback: Vec<(NodeId, NodeId)> = exec_set
            .iter()
            .copied()
            .filter(|&id| {
                let node = graph.node(id).expect("exec-set nodes exist");
                matches!(node.kind, NodeKind::Feedback) && !node.bypass
            })
            .filter_map(|id| {
                graph
                    .input_source(id, 0)
                    .and_then(effective)
                    .map(|producer| (id, producer))
            })
            .collect();
        let is_active_feedback = |id: NodeId| active_feedback.iter().any(|&(f, _)| f == id);

        let node_is_chain_input = |id: NodeId| {
            graph
                .node(id)
                .is_some_and(|n| matches!(n.kind, NodeKind::ChainInput))
        };
        // A ChainInput is only a texture when the host actually handed one
        // in. A master chain on a frame with no layers, or a chain whose
        // host passed None, resolves to the 1x1 placeholder like any other
        // unwired input.
        let chain_input_live = input.is_some();

        let final_producer = graph
            .input_source(graph.output_node(), 0)
            .and_then(effective);
        // A Feedback node feeding Output has no effect pass; it needs an
        // extra read→Output blit step. If it is inactive (unwired input),
        // Output must show deliberate black, not a stale frame.
        let feedback_feeds_output =
            final_producer.is_some_and(|id| node_is_feedback(id) && is_active_feedback(id));
        // Same story for a ChainInput wired straight to Output: no pass runs
        // for it, so the host's texture needs an explicit blit. This is the
        // identity chain — the picture passes through untouched.
        let chain_input_feeds_output =
            final_producer.is_some_and(|id| node_is_chain_input(id) && chain_input_live);
        let output_written = match final_producer {
            Some(fp) if node_is_feedback(fp) => feedback_feeds_output,
            Some(fp) if node_is_chain_input(fp) => chain_input_feeds_output,
            Some(_) => true,
            None => false,
        };

        // Sync the pair map before bind groups borrow it: prune pairs whose
        // node is gone, create (cleared → first frame reads transparent
        // black) pairs for newly active feedback nodes. `new_cleared`
        // submits its own tiny encoder — replans are rare structural events,
        // so I8's steady-state clause is untouched.
        // Scoped to THIS chain: an unscoped retain would drop every other
        // chain's pairs, because this graph has never heard of their nodes.
        self.feedback
            .retain(|id, _| id.chain != chain || graph.node(id.node).is_some());
        let (w, h) = (self.width, self.height);
        for &(fb, _) in &active_feedback {
            let generation = &mut self.feedback_generation;
            self.feedback.entry(key(fb)).or_insert_with(|| {
                *generation += 1;
                PingPongTarget::new_cleared(device, queue, w, h, GpuContext::hdr_format(), 1.0)
            });
        }

        // Preview targets: prune removed nodes always (their egui textures
        // are freed at the next register call); create targets only while
        // previews are on. Both are plan-build-time mutations — the render
        // path below only reads (I8).
        self.previews.prune(chain, |id| graph.node(id).is_some());
        let previewed: Vec<NodeId> = if previews_on {
            step_nodes
                .iter()
                .copied()
                .chain(active_feedback.iter().map(|&(fb, _)| fb))
                // The chain input gets a thumbnail too, though it runs no
                // pass: seeing what the layer handed in is most of the value
                // of putting the node on the canvas at all.
                .chain(graph.chain_input().filter(|_| chain_input_live))
                .collect()
        } else {
            Vec::new()
        };
        for &id in &previewed {
            self.previews.ensure(device, key(id));
        }

        let total_steps = step_nodes.len()
            + active_feedback.len()
            + usize::from(feedback_feeds_output)
            + usize::from(chain_input_feeds_output);
        self.ensure_slots(device, chain, total_steps as u32);
        let base_slot = chain_slot_base(chain.index(), self.slots_per_chain);
        // Captured by value: the pool acquisitions below need `&mut self`, so
        // this closure must not hold a borrow of it.
        let stride = self.stride;
        let slot_offset = move |slot: usize| (u64::from(base_slot) + slot as u64) * stride;

        // Pass 1: assign targets so pass 2 can resolve inputs to views.
        self.pool.release_all();
        let mut targets: Vec<(NodeId, TargetSlot)> = Vec::with_capacity(step_nodes.len());
        for &id in &step_nodes {
            let slot = if final_producer == Some(id) {
                TargetSlot::Output
            } else {
                TargetSlot::Pool(self.pool.acquire(
                    device,
                    self.width,
                    self.height,
                    GpuContext::hdr_format(),
                ))
            };
            targets.push((id, slot));
        }
        let target_of = |id: NodeId| -> Option<TargetSlot> {
            targets.iter().find(|(n, _)| *n == id).map(|(_, s)| *s)
        };

        // Does `resolve` give this producer a different texture per parity?
        // Every consumer asks this one question, so that a new kind of
        // parity-dependent producer cannot be added to `resolve` and forgotten
        // at a call site — which is how the chain input was.
        let reads_per_parity = |p: NodeId| is_active_feedback(p) || node_is_chain_input(p);

        // Resolve one producer to (view, sampler) at a given parity. An
        // active feedback producer resolves to its READ buffer —
        // `targets[1 - parity]`, written last frame. A ChainInput resolves to
        // the host's texture for that parity. Everything else is
        // parity-independent.
        let resolve = |producer: Option<NodeId>,
                       parity: usize|
         -> (&wgpu::TextureView, &wgpu::Sampler) {
            let Some(p) = producer else {
                return (&self.prev_view, &self.prev_sampler);
            };
            if node_is_feedback(p) {
                if let (true, Some(pair)) = (is_active_feedback(p), self.feedback.get(&key(p))) {
                    let rt = &pair.targets[1 - parity];
                    return (&rt.view, &rt.sampler);
                }
                return (&self.prev_view, &self.prev_sampler);
            }
            if node_is_chain_input(p) {
                // The host's texture for THIS parity. A layer that ping-pongs
                // returns a different target each frame (`pass_executor`
                // writes `targets[flip_parity]`), so the host supplies both
                // and a bind group is prebuilt for each — the same
                // `[BindGroup; 2]` idiom feedback already uses.
                return match input {
                    Some(src) => src.per_parity[parity],
                    None => (&self.prev_view, &self.prev_sampler),
                };
            }
            match target_of(p) {
                Some(TargetSlot::Pool(i)) => {
                    let rt = self.pool.get(i);
                    (&rt.view, &rt.sampler)
                }
                Some(TargetSlot::Output) => (&out.view, &out.sampler),
                // Unwired (or a dead bypass chain): 1x1 black.
                Some(TargetSlot::FeedbackWrite) | None => (&self.prev_view, &self.prev_sampler),
            }
        };

        // Bind-group builder shared by effect and copy steps. Binding 0 is an
        // arena slice at a static offset — the reason
        // `UniformBuffer::create_bind_group` (which binds its own whole
        // buffer) is not reusable here.
        let make_bind_group = |layout: &wgpu::BindGroupLayout,
                               uniform_offset: u64,
                               inputs: &[(&wgpu::TextureView, &wgpu::Sampler)]|
         -> wgpu::BindGroup {
            let mut entries = vec![
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.arena,
                        offset: uniform_offset,
                        size: Some(NonZeroU64::new(UNIFORM_SIZE).expect("nonzero")),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.prev_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.prev_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.waveform),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&self.spectrum),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&self.spectrogram),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(&self.audio_sampler),
                },
            ];
            for (pin, &(view, sampler)) in inputs.iter().enumerate() {
                let b = 7 + 2 * pin as u32;
                entries.push(wgpu::BindGroupEntry {
                    binding: b,
                    resource: wgpu::BindingResource::TextureView(view),
                });
                entries.push(wgpu::BindGroupEntry {
                    binding: b + 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                });
            }
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("trama-node-bind-group"),
                layout,
                entries: &entries,
            })
        };

        // Pass 2: effect steps in topo order.
        let mut steps = Vec::with_capacity(total_steps);
        for (i, &id) in step_nodes.iter().enumerate() {
            let node = graph.node(id).expect("live nodes exist");
            let effect_id = match &node.kind {
                NodeKind::Source { effect } | NodeKind::Effect { effect } => effect,
                NodeKind::Output | NodeKind::Feedback | NodeKind::ChainInput => {
                    unreachable!("filtered above")
                }
            };
            let uniform_offset = slot_offset(i);
            // An effect the registry does not have is a placeholder step, not
            // a failed plan: failing here froze the WHOLE chain on its
            // last-good plan and stopped every other node updating.
            let Some(effect_idx) = registry.effects.iter().position(|e| &e.id == effect_id) else {
                steps.push(Step {
                    node: key(id),
                    kind: StepKind::Missing,
                    target: targets[i].1,
                    uniform_offset,
                    bind_groups: None,
                });
                continue;
            };
            let def = &registry.effects[effect_idx];

            let producers: Vec<Option<NodeId>> = (0..def.inputs)
                .map(|pin| graph.input_source(id, pin).and_then(effective))
                .collect();
            // Two kinds of producer resolve differently per parity: an active
            // feedback node (its read buffer alternates) and the chain input
            // (a layer whose last pass has feedback WRITES alternate targets).
            // Leaving the second one out handed such a step the parity-0 bind
            // group twice: every other frame it read the picture the layer had
            // rendered the frame before, and on the first frame ever it read a
            // target nothing had written yet.
            let parity_dependent = producers.iter().any(|p| p.is_some_and(reads_per_parity));

            let inputs0: Vec<_> = producers.iter().map(|&p| resolve(p, 0)).collect();
            let bg0 = make_bind_group(&def.pipeline.bind_group_layout, uniform_offset, &inputs0);
            let bind_groups = if parity_dependent {
                let inputs1: Vec<_> = producers.iter().map(|&p| resolve(p, 1)).collect();
                let bg1 =
                    make_bind_group(&def.pipeline.bind_group_layout, uniform_offset, &inputs1);
                [bg0, bg1]
            } else {
                [bg0.clone(), bg0]
            };

            steps.push(Step {
                node: key(id),
                kind: StepKind::Effect { effect: effect_idx },
                target: targets[i].1,
                uniform_offset,
                bind_groups: Some(bind_groups),
            });
        }

        // Copy steps, appended after all effect passes (see ExecPlan.steps).
        let mut step_index = step_nodes.len();
        for &(fb, producer) in &active_feedback {
            let uniform_offset = slot_offset(step_index);
            step_index += 1;
            let layout = &self.copy_pipeline.bind_group_layout;
            let bg0 = make_bind_group(layout, uniform_offset, &[resolve(Some(producer), 0)]);
            let bind_groups = if reads_per_parity(producer) {
                // Chained feedback reads another pair's read side; a feedback
                // node fed straight from the chain input reads whichever
                // target the host wrote this frame.
                [
                    bg0,
                    make_bind_group(layout, uniform_offset, &[resolve(Some(producer), 1)]),
                ]
            } else {
                [bg0.clone(), bg0]
            };
            steps.push(Step {
                node: key(fb),
                kind: StepKind::Copy,
                target: TargetSlot::FeedbackWrite,
                uniform_offset,
                bind_groups: Some(bind_groups),
            });
        }
        if chain_input_feeds_output {
            // The identity chain: host texture straight through to Output.
            let fp = final_producer.expect("checked by chain_input_feeds_output");
            let uniform_offset = slot_offset(step_index);
            step_index += 1;
            let layout = &self.copy_pipeline.bind_group_layout;
            steps.push(Step {
                node: key(fp),
                kind: StepKind::Copy,
                target: TargetSlot::Output,
                uniform_offset,
                bind_groups: Some([
                    make_bind_group(layout, uniform_offset, &[resolve(Some(fp), 0)]),
                    make_bind_group(layout, uniform_offset, &[resolve(Some(fp), 1)]),
                ]),
            });
        }
        if feedback_feeds_output {
            let fp = final_producer.expect("checked by feedback_feeds_output");
            let uniform_offset = slot_offset(step_index);
            let layout = &self.copy_pipeline.bind_group_layout;
            // Reads the pair's read buffer — parity-dependent by definition.
            steps.push(Step {
                node: key(fp),
                kind: StepKind::Copy,
                target: TargetSlot::Output,
                uniform_offset,
                bind_groups: Some([
                    make_bind_group(layout, uniform_offset, &[resolve(Some(fp), 0)]),
                    make_bind_group(layout, uniform_offset, &[resolve(Some(fp), 1)]),
                ]),
            });
        }

        // Thumbnail blits: each previewed node's own output, resolved with
        // the same per-parity rule as any consumer (a feedback node previews
        // its read buffer — the delayed frame it presents to the graph).
        // Binding 0 must be a valid arena slice per the layout; the preview
        // shader never reads it, so offset 0 serves every blit.
        let previews = previewed
            .iter()
            .map(|&id| {
                let layout = &self.preview_pipeline.bind_group_layout;
                PreviewBlit {
                    node: key(id),
                    bind_groups: [
                        make_bind_group(layout, 0, &[resolve(Some(id), 0)]),
                        make_bind_group(layout, 0, &[resolve(Some(id), 1)]),
                    ],
                }
            })
            .collect();

        Ok(ExecPlan {
            registry_generation: registry.generation,
            version,
            previews_on,
            chain_input: input.map(|i| i.generation),
            out_generation,
            steps,
            previews,
            output_written,
            base_slot,
        })
    }

    #[cfg(test)]
    fn plan_version(&self, chain: ChainId) -> Option<u64> {
        self.plans.get(&chain).map(|p| p.version)
    }

    #[cfg(test)]
    pub(crate) fn plans_built(&self) -> u64 {
        self.plans_built
    }

    #[cfg(test)]
    fn feedback_generation(&self) -> u64 {
        self.feedback_generation
    }

    #[cfg(test)]
    fn plan_step_count(&self, chain: ChainId) -> usize {
        self.plans.get(&chain).map_or(0, |p| p.steps.len())
    }

    /// Arena byte range chain `c` writes into — the guard against two chains
    /// staging their uniforms over each other.
    #[cfg(test)]
    fn slot_span(&self, chain: ChainId) -> (u64, u64) {
        let base = u64::from(chain_slot_base(chain.index(), self.slots_per_chain)) * self.stride;
        (base, base + u64::from(self.slots_per_chain) * self.stride)
    }

    /// Every arena byte offset chain `c`'s steps write this frame.
    #[cfg(test)]
    fn step_offsets(&self, chain: ChainId) -> Vec<u64> {
        self.plans
            .get(&chain)
            .map(|p| p.steps.iter().map(|s| s.uniform_offset).collect())
            .unwrap_or_default()
    }

    /// Ping-pong pairs belonging to one chain — the H1 isolation guard.
    #[cfg(test)]
    fn feedback_count(&self, chain: ChainId) -> usize {
        self.feedback.keys().filter(|k| k.chain == chain).count()
    }

    /// Preview targets belonging to one chain — the H1 isolation guard.
    #[cfg(test)]
    fn preview_count(&self, chain: ChainId) -> usize {
        self.previews.count_in(chain)
    }
}

fn create_arena(device: &wgpu::Device, stride: u64, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("trama-uniform-arena"),
        size: stride * u64::from(capacity),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::loader::{EffectLoader, probe_libs};
    use crate::gpu::test_gpu::snapshot;
    use crate::gpu::test_gpu::{gpu_guard, test_gpu};
    use crate::trama::effect::{EffectId, TramaRegistry};
    use crate::trama::graph::NodeGraph;
    use crate::trama::node::NodeKind;

    /// The chain the pre-existing single-graph tests run on. Their behavior
    /// must not depend on which one it is.
    const TEST_CHAIN: ChainId = ChainId::Master;

    #[test]
    fn aligned_stride_rounds_448_up_to_alignment() {
        assert_eq!(aligned_stride(448, 256), 512);
        assert_eq!(aligned_stride(448, 64), 448);
        assert_eq!(aligned_stride(448, 32), 448);
        assert_eq!(aligned_stride(256, 256), 256);
    }

    /// Every chain writes its uniforms into its own arena region. Without
    /// this, two chains stage over each other: `queue.write_buffer` calls all
    /// land before any pass runs at submit, so the last writer's values would
    /// render for every chain, not just its own.
    #[test]
    fn chain_slot_regions_never_overlap() {
        const SLOTS: u32 = 16;
        let chains = [
            ChainId::Layer(0),
            ChainId::Layer(1),
            ChainId::Layer(7),
            ChainId::Master,
        ];
        let mut spans: Vec<(u32, u32)> = chains
            .iter()
            .map(|c| {
                let base = chain_slot_base(c.index(), SLOTS);
                (base, base + SLOTS)
            })
            .collect();
        spans.sort_unstable();
        for w in spans.windows(2) {
            assert!(
                w[0].1 <= w[1].0,
                "chain regions {:?} and {:?} overlap",
                w[0],
                w[1]
            );
        }
    }

    /// Master sits above every layer slot, so adding a layer never renumbers
    /// it — a renumber would silently invalidate its cached bind groups,
    /// which embed absolute arena offsets.
    #[test]
    fn master_chain_indexes_above_every_layer() {
        let max = crate::bindings::catalog::MAX_LAYERS as u32;
        assert_eq!(ChainId::Master.index(), max);
        for n in 0..max {
            assert!(ChainId::Layer(n as u8).index() < ChainId::Master.index());
        }
    }

    fn effects_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/trama/effects")
    }

    fn registry(device: &wgpu::Device) -> TramaRegistry {
        let loader = EffectLoader::for_test(&probe_libs());
        TramaRegistry::load(device, None, &loader, &effects_dir())
    }

    // Run: cargo test -p fosfora-app -- --ignored trama_registry_builds_pipelines_for_builtins
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn trama_registry_builds_pipelines_for_builtins() {
        let _guard = gpu_guard();
        let (device, _queue) = test_gpu();
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let reg = registry(&device);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
        assert!(
            reg.errors.is_empty(),
            "registry load errors: {:?}",
            reg.errors
        );
        let ids: Vec<&str> = reg.effects.iter().map(|e| e.id.0.as_str()).collect();
        assert_eq!(
            ids,
            ["hue_drift", "mix", "noise_field", "transform"],
            "sorted by id"
        );
    }

    // Run: cargo test -p fosfora-app -- --ignored trama_executor_renders_noise_hue_output_chain
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn trama_executor_renders_noise_hue_output_chain() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let reg = registry(&device);
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);

        let noise = reg.get(&EffectId("noise_field".into())).unwrap();
        let hue = reg.get(&EffectId("hue_drift".into())).unwrap();
        let (noise_id, noise_params) = (noise.id.clone(), noise.params.clone());
        let (hue_id, hue_params) = (hue.id.clone(), hue.params.clone());

        let mut graph = NodeGraph::new_with_output();
        let n = graph.add_node(NodeKind::Source { effect: noise_id }, 0, &noise_params);
        let h = graph.add_node(NodeKind::Effect { effect: hue_id }, 1, &hue_params);
        let out = graph.output_node();
        graph.connect(n, h, 0).unwrap();
        graph.connect(h, out, 0).unwrap();

        // Modulate hue_drift.speed from Bass and resolve once, as
        // App::update does — the executes below then exercise the
        // resolved-value overlay under the validation scope.
        use crate::trama::modulation::{ModMode, ModSource, Modulation, resolve_node};
        graph
            .set_modulation(
                h,
                "speed",
                Some(Modulation {
                    source: ModSource::Audio(crate::trama::audio::AudioFeature::Bass),
                    amount: 0.6,
                    mode: ModMode::Add,
                    smoothing: 0.0,
                }),
            )
            .unwrap();
        let mut view = crate::trama::audio::AudioView::default();
        let features = crate::audio::features::AudioFeatures {
            sub_bass: 1.0,
            bass: 1.0,
            ..Default::default()
        };
        view.update(0.016, &features, &[]);
        for node in graph.params_iter_mut() {
            resolve_node(node.params, node.mods, 0.016, &view);
        }
        let hue_mod = &graph.node(h).unwrap().mods[0];
        assert!(hue_mod.state.slot.is_some(), "speed must resolve to a slot");
        // Bass signal is 1.0 here, so Add lands at base + 0.6·span (clamped).
        let expected = hue_params
            .iter()
            .find_map(|d| match d {
                crate::params::ParamDef::Float {
                    name,
                    default,
                    min,
                    max,
                } if name == "speed" => Some((default + 0.6 * (max - min)).clamp(*min, *max)),
                _ => None,
            })
            .expect("hue_drift has a Float speed param");
        assert!(
            (hue_mod.state.resolved - expected).abs() < 1e-5,
            "resolved {} != expected {expected}",
            hue_mod.state.resolved
        );

        let placeholder = PlaceholderTexture::new(&device, &queue, GpuContext::hdr_format());
        let audio = AudioTextures::new(&device, &queue);
        let mut exec = TramaExecutor::new(&device, None, &placeholder, &audio, 256, 144);
        // The output target is the caller's now; the executor renders into it.
        let out = RenderTarget::new(
            &device,
            256,
            144,
            GpuContext::hdr_format(),
            1.0,
            "test-chain-output",
        );

        let mut template = ShaderUniforms::zeroed();
        template.resolution = [256.0, 144.0];
        let mut last_error = None;

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        exec.execute(
            TEST_CHAIN,
            &graph,
            &reg,
            None,
            &out,
            1,
            &template,
            false,
            &device,
            &queue,
            &mut encoder,
            crate::gpu::profiler::ProfilerHandle::none(),
            &mut last_error,
        );
        queue.submit([encoder.finish()]);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
        assert!(last_error.is_none(), "{last_error:?}");
        let v = exec.plan_version(TEST_CHAIN);
        let stats = exec.pool_stats();
        // noise renders to a pooled target; hue renders straight into the
        // output target — one pooled texture total.
        assert_eq!(stats, (1, 1), "pool stats");

        // Second frame with unchanged topology: same plan, no new targets.
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        exec.execute(
            TEST_CHAIN,
            &graph,
            &reg,
            None,
            &out,
            1,
            &template,
            false,
            &device,
            &queue,
            &mut encoder,
            crate::gpu::profiler::ProfilerHandle::none(),
            &mut last_error,
        );
        queue.submit([encoder.finish()]);
        assert_eq!(exec.plan_version(TEST_CHAIN), v, "plan reused");
        assert_eq!(exec.pool_stats(), stats, "no new pool targets");
    }

    /// Build the classic M2 motion-echo patch from the real registry:
    /// `noise → mix ← feedback(transform(mix)) → out`. Returns the feedback
    /// node's id.
    fn build_motion_echo(reg: &TramaRegistry, graph: &mut NodeGraph) -> crate::trama::node::NodeId {
        let noise = reg.get(&EffectId("noise_field".into())).unwrap();
        let mix = reg.get(&EffectId("mix".into())).unwrap();
        let transform = reg.get(&EffectId("transform".into())).unwrap();
        let n = graph.add_node(
            NodeKind::Source {
                effect: noise.id.clone(),
            },
            0,
            &noise.params.clone(),
        );
        let m = graph.add_node(
            NodeKind::Effect {
                effect: mix.id.clone(),
            },
            2,
            &mix.params.clone(),
        );
        let t = graph.add_node(
            NodeKind::Effect {
                effect: transform.id.clone(),
            },
            1,
            &transform.params.clone(),
        );
        let f = graph.add_node(NodeKind::Feedback, 1, &[]);
        let out = graph.output_node();
        graph.connect(n, m, 0).unwrap();
        graph.connect(m, t, 0).unwrap();
        graph.connect(t, f, 0).unwrap();
        graph.connect(f, m, 1).unwrap();
        graph.connect(m, out, 0).unwrap();
        f
    }

    /// Compare two snapshots, reporting the first difference rather than
    /// dumping both buffers — a full 64x64 Rgba16Float pair is 64 KB of
    /// noise in a panic message.
    fn assert_same_image(got: &[u8], want: &[u8], msg: &str) {
        assert_eq!(got.len(), want.len(), "{msg}: snapshot sizes differ");
        if got == want {
            return;
        }
        let at = got
            .iter()
            .zip(want)
            .position(|(a, b)| a != b)
            .expect("lengths match and buffers differ");
        let end = (at + 8).min(got.len());
        panic!(
            "{msg}\n  first difference at byte {at}\n    got:  {:?}\n    want: {:?}",
            &got[at..end],
            &want[at..end]
        );
    }

    /// A `ChainInput` wired straight to Output is the identity chain: the
    /// host's picture reaches the screen untouched. It runs no effect pass,
    /// so without an explicit blit step Output would clear to black and a
    /// layer carrying a trivial chain would simply vanish — which is what
    /// makes this the load-bearing test for stage B.
    ///
    /// It also pins the other half of the contract: handed no host input, the
    /// same graph must fall back to deliberate black rather than to whatever
    /// happened to be sitting in the target.
    // Run: cargo test -p fosfora-app -- --ignored trama_chain_input_identity_passes_host_picture
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn trama_chain_input_identity_passes_host_picture() {
        const DIM: u32 = 64;
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let reg = registry(&device);
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);

        let mut graph = NodeGraph::new_with_output();
        let ci = graph.add_node(NodeKind::ChainInput, 0, &[]);
        let out = graph.output_node();
        graph.connect(ci, out, 0).unwrap();
        graph.validate().unwrap();
        assert_eq!(graph.chain_input(), Some(ci));

        let placeholder = PlaceholderTexture::new(&device, &queue, GpuContext::hdr_format());
        let audio = AudioTextures::new(&device, &queue);
        let mut exec = TramaExecutor::new(&device, None, &placeholder, &audio, DIM, DIM);
        // The output target is the caller's now; the executor renders into it.
        let out = RenderTarget::new(
            &device,
            DIM,
            DIM,
            GpuContext::hdr_format(),
            1.0,
            "test-chain-output",
        );
        let mut template = ShaderUniforms::zeroed();
        template.resolution = [DIM as f32, DIM as f32];
        let mut last_error = None;

        // Stand-in for a layer's rendered output: a target cleared to a
        // distinctive color, so "passed through" is falsifiable.
        let host = RenderTarget::new(
            &device,
            DIM,
            DIM,
            GpuContext::hdr_format(),
            1.0,
            "test-host-layer",
        );
        let mut enc = device.create_command_encoder(&Default::default());
        enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("host-fill"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &host.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.25,
                        g: 0.5,
                        b: 0.75,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        queue.submit([enc.finish()]);
        let host_bytes = snapshot(&device, &queue, &host.view, DIM);
        assert!(
            host_bytes.iter().any(|&b| b != 0),
            "the host picture must not be black, or this test proves nothing"
        );

        device.push_error_scope(wgpu::ErrorFilter::Validation);

        // Fed: the host picture reaches Output untouched.
        exec.begin_frame();
        let produced = {
            let mut encoder = device.create_command_encoder(&Default::default());
            exec.execute(
                TEST_CHAIN,
                &graph,
                &reg,
                Some(ChainInputSource::stable(&host.view, &host.sampler, 1)),
                &out,
                1,
                &template,
                false,
                &device,
                &queue,
                &mut encoder,
                crate::gpu::profiler::ProfilerHandle::none(),
                &mut last_error,
            );
            // The blit has to reach the GPU before the copy-to-buffer that
            // reads it; `encoder` is independent of the borrow on `exec`.
            queue.submit([encoder.finish()]);
            snapshot(&device, &queue, &out.view, DIM)
        };
        assert!(last_error.is_none(), "{last_error:?}");
        assert_eq!(
            exec.plan_step_count(TEST_CHAIN),
            1,
            "one blit step: ChainInput runs no effect pass, but must still \
             reach Output"
        );
        assert_same_image(
            &produced,
            &host_bytes,
            "identity chain must pass the host picture through unchanged",
        );

        // A REBUILT host at the same graph version. Nothing about the graph
        // changed, so only the input generation can force the replan — and
        // the cached bind group still points at the old texture until it
        // does. Drop the `chain_input` term from the plan key and this reads
        // back `host_bytes`, the picture from before the rebuild.
        let host2 = RenderTarget::new(
            &device,
            DIM,
            DIM,
            GpuContext::hdr_format(),
            1.0,
            "test-host-layer-rebuilt",
        );
        let mut enc = device.create_command_encoder(&Default::default());
        enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("host2-fill"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &host2.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.9,
                        g: 0.1,
                        b: 0.2,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        queue.submit([enc.finish()]);
        let host2_bytes = snapshot(&device, &queue, &host2.view, DIM);
        assert_ne!(
            host2_bytes, host_bytes,
            "the two host pictures must differ, or the replan is untestable"
        );

        exec.begin_frame();
        let after_rebuild = {
            let mut encoder = device.create_command_encoder(&Default::default());
            exec.execute(
                TEST_CHAIN,
                &graph,
                &reg,
                Some(ChainInputSource::stable(&host2.view, &host2.sampler, 2)),
                &out,
                1,
                &template,
                false,
                &device,
                &queue,
                &mut encoder,
                crate::gpu::profiler::ProfilerHandle::none(),
                &mut last_error,
            );
            queue.submit([encoder.finish()]);
            snapshot(&device, &queue, &out.view, DIM)
        };
        assert_same_image(
            &after_rebuild,
            &host2_bytes,
            "a rebuilt host texture must replan; the chain is still showing \
             the picture from before the rebuild",
        );

        // Unfed: deliberate black, never a stale frame.
        exec.begin_frame();
        let blank = {
            let mut encoder = device.create_command_encoder(&Default::default());
            exec.execute(
                TEST_CHAIN,
                &graph,
                &reg,
                None,
                &out,
                1,
                &template,
                false,
                &device,
                &queue,
                &mut encoder,
                crate::gpu::profiler::ProfilerHandle::none(),
                &mut last_error,
            );
            queue.submit([encoder.finish()]);
            snapshot(&device, &queue, &out.view, DIM)
        };
        assert_eq!(
            exec.plan_step_count(TEST_CHAIN),
            0,
            "nothing to blit without a host texture"
        );
        assert!(
            blank.iter().all(|&b| b == 0),
            "an unfed chain input must clear to black, not keep the last frame"
        );

        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }

    /// H1: two chains number their nodes from zero, so the same `NodeId`
    /// names a different node in each. Every piece of executor state that
    /// outlives a plan must be keyed by `(chain, node)`.
    ///
    /// The bug this guards is not hypothetical — it is what the code did
    /// before the re-key. `build_plan` ran
    /// `feedback.retain(|id, _| graph.node(*id).is_some())` and
    /// `previews.prune(|id| graph.node(id).is_some())` against *only the
    /// chain being planned*, so planning layer B tore down layer A's echo
    /// buffers and thumbnails on the grounds that B's graph had never heard
    /// of them. To watch it fail, drop the `id.chain != chain` guard from
    /// either call: this asserts A keeps its pair across B's build.
    // Run: cargo test -p fosfora-app -- --ignored trama_chains_do_not_share_node_keyed_state
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn trama_chains_do_not_share_node_keyed_state() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let reg = registry(&device);
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);

        // The two chains must be structurally DIFFERENT, or this test cannot
        // fail: an unscoped `graph.node(id.node)` check would still find a
        // node at chain A's id inside an identically-shaped chain B, and the
        // prune would have nothing to delete. A is the 5-node motion echo; B
        // is a bare noise → out. A's feedback node id does not exist in B.
        let (a, b) = (ChainId::Layer(0), ChainId::Layer(3));
        let mut graph_a = NodeGraph::new_with_output();
        let fb_a = build_motion_echo(&reg, &mut graph_a);

        let mut graph_b = NodeGraph::new_with_output();
        let noise = reg.get(&EffectId("noise_field".into())).unwrap();
        let n_b = graph_b.add_node(
            NodeKind::Source {
                effect: noise.id.clone(),
            },
            0,
            &noise.params.clone(),
        );
        let out_b = graph_b.output_node();
        graph_b.connect(n_b, out_b, 0).unwrap();

        assert!(
            graph_b.node(fb_a).is_none(),
            "chain B must NOT contain a node at A's feedback id, or the \
             unscoped-prune bug stays invisible here"
        );
        assert_eq!(
            graph_a.output_node(),
            graph_b.output_node(),
            "both chains still number from zero — the collision is real"
        );

        let placeholder = PlaceholderTexture::new(&device, &queue, GpuContext::hdr_format());
        let audio = AudioTextures::new(&device, &queue);
        let mut exec = TramaExecutor::new(&device, None, &placeholder, &audio, 256, 144);
        // The output target is the caller's now; the executor renders into it.
        let out = RenderTarget::new(
            &device,
            256,
            144,
            GpuContext::hdr_format(),
            1.0,
            "test-chain-output",
        );
        let mut template = ShaderUniforms::zeroed();
        template.resolution = [256.0, 144.0];
        let mut last_error = None;

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut run = |exec: &mut TramaExecutor, chain, graph: &mut NodeGraph| {
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            exec.execute(
                chain,
                graph,
                &reg,
                None,
                &out,
                1,
                &template,
                true, // previews on, so preview targets get created too
                &device,
                &queue,
                &mut encoder,
                crate::gpu::profiler::ProfilerHandle::none(),
                &mut last_error,
            );
            queue.submit([encoder.finish()]);
        };

        exec.begin_frame();
        run(&mut exec, a, &mut graph_a);
        assert_eq!(exec.feedback_count(a), 1, "chain A has its echo pair");
        let previews_a = exec.preview_count(a);
        assert!(previews_a > 1, "chain A created preview targets");

        // Planning B must not disturb A. Both assertions fail if either
        // prune drops its `id.chain != chain` guard.
        run(&mut exec, b, &mut graph_b);
        assert_eq!(
            exec.feedback_count(a),
            1,
            "chain A's echo pair survived chain B's plan build"
        );
        assert_eq!(
            exec.preview_count(a),
            previews_a,
            "chain A's thumbnails survived chain B's plan build"
        );
        assert_eq!(exec.feedback_count(b), 0, "B has no feedback node");
        assert_eq!(exec.preview_count(b), 1, "B previews its one source");
        assert_eq!(
            exec.preview_stats(),
            previews_a + 1,
            "the two chains' thumbnails coexist"
        );

        // And their uniforms land in disjoint arena regions.
        let (a_lo, a_hi) = exec.slot_span(a);
        let (b_lo, b_hi) = exec.slot_span(b);
        assert!(a_hi <= b_lo || b_hi <= a_lo, "chain arena regions overlap");
        for off in exec.step_offsets(a) {
            assert!(
                (a_lo..a_hi).contains(&off),
                "chain A step at {off} is outside its region {a_lo}..{a_hi}"
            );
        }
        for off in exec.step_offsets(b) {
            assert!(
                (b_lo..b_hi).contains(&off),
                "chain B step at {off} is outside its region {b_lo}..{b_hi}"
            );
        }

        // Dropping a chain takes only its own resources with it.
        exec.drop_chain(a);
        assert_eq!(exec.feedback_count(a), 0, "A's pair went with A");
        assert_eq!(exec.preview_count(a), 0, "A's thumbnails went with A");
        assert_eq!(exec.preview_count(b), 1, "B's thumbnails did not");

        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }

    // Run: cargo test -p fosfora-app -- --ignored trama_feedback_motion_echo_steady_state
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn trama_feedback_motion_echo_steady_state() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let reg = registry(&device);
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);
        let mut graph = NodeGraph::new_with_output();
        build_motion_echo(&reg, &mut graph);

        let placeholder = PlaceholderTexture::new(&device, &queue, GpuContext::hdr_format());
        let audio = AudioTextures::new(&device, &queue);
        let mut exec = TramaExecutor::new(&device, None, &placeholder, &audio, 256, 144);
        // The output target is the caller's now; the executor renders into it.
        let out = RenderTarget::new(
            &device,
            256,
            144,
            GpuContext::hdr_format(),
            1.0,
            "test-chain-output",
        );
        let mut template = ShaderUniforms::zeroed();
        template.resolution = [256.0, 144.0];
        let mut last_error = None;

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut first = (None, (0, 0), 0);
        // Long enough that each half sees wgpu's LOW allocation regime at
        // least once: its per-frame count wanders between ~93 and ~109 for
        // stretches of several frames, and with ten frames and four-frame
        // windows this test failed four runs in ten on unchanged code
        // (`[451, 107, 109, 96, 93, 96, 105, 107, 107, 107]` — no accumulation
        // anywhere, the late window simply never saw a 93).
        const FRAMES: usize = 60;
        let mut alloc_deltas = [0u64; FRAMES];
        for (frame, delta) in alloc_deltas.iter_mut().enumerate() {
            // Parity flips once per frame, as TramaSystem::update drives it.
            exec.begin_frame();
            let (allocs, ()) = crate::test_alloc::count_allocs(|| {
                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                exec.execute(
                    TEST_CHAIN,
                    &graph,
                    &reg,
                    None,
                    &out,
                    1,
                    &template,
                    false,
                    &device,
                    &queue,
                    &mut encoder,
                    crate::gpu::profiler::ProfilerHandle::none(),
                    &mut last_error,
                );
                queue.submit([encoder.finish()]);
            });
            *delta = allocs;
            assert!(last_error.is_none(), "frame {frame}: {last_error:?}");
            let state = (
                exec.plan_version(TEST_CHAIN),
                exec.pool_stats(),
                exec.feedback_generation(),
            );
            if frame == 0 {
                first = state;
            } else {
                assert_eq!(state, first, "steady state moved on frame {frame}");
            }
        }
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");

        // noise + transform pooled; mix writes Output; one copy step for the
        // feedback node = 4 steps total, one ping-pong pair.
        assert_eq!(exec.pool_stats(), (2, 2), "pool stats");
        assert_eq!(
            exec.plan_step_count(TEST_CHAIN),
            4,
            "3 effect passes + 1 copy"
        );
        assert_eq!(exec.feedback_stats(), 1, "one ping-pong pair");
        assert_eq!(exec.feedback_generation(), 1, "pair created exactly once");

        // I8's heap half at the GPU boundary: wgpu's own command encoding
        // allocates every frame (a ~93-alloc floor with sporadic internal
        // spikes), so neither zero nor exact constancy is attainable around
        // `execute`. What IS assertable: the per-frame allocation FLOOR must
        // not RISE between early and late steady-state frames — that catches
        // accumulation-type regressions (a Vec pushed per frame, a growing
        // map) which surface as an upward drift; a lower late floor is spike
        // noise in our favor. Constant per-frame costs in trama-owned CPU
        // code are held to a hard zero by
        // `steady_state_frame_cpu_work_allocates_nothing`.
        let early_floor = alloc_deltas[1..FRAMES / 2].iter().min();
        let late_floor = alloc_deltas[FRAMES / 2..].iter().min();
        assert!(
            late_floor <= early_floor,
            "steady-state allocation floor rose: {alloc_deltas:?}"
        );
    }

    // Run: cargo test -p fosfora-app -- --ignored trama_feedback_state_survives_unrelated_rewire
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn trama_feedback_state_survives_unrelated_rewire() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let reg = registry(&device);
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);
        let mut graph = NodeGraph::new_with_output();
        let f = build_motion_echo(&reg, &mut graph);

        let placeholder = PlaceholderTexture::new(&device, &queue, GpuContext::hdr_format());
        let audio = AudioTextures::new(&device, &queue);
        let mut exec = TramaExecutor::new(&device, None, &placeholder, &audio, 128, 72);
        // The output target is the caller's now; the executor renders into it.
        let out = RenderTarget::new(
            &device,
            128,
            72,
            GpuContext::hdr_format(),
            1.0,
            "test-chain-output",
        );
        let mut template = ShaderUniforms::zeroed();
        template.resolution = [128.0, 72.0];
        let mut last_error = None;
        let run_frame =
            |exec: &mut TramaExecutor, graph: &mut NodeGraph, last_error: &mut Option<String>| {
                exec.begin_frame();
                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                exec.execute(
                    TEST_CHAIN,
                    graph,
                    &reg,
                    None,
                    &out,
                    1,
                    &template,
                    false,
                    &device,
                    &queue,
                    &mut encoder,
                    crate::gpu::profiler::ProfilerHandle::none(),
                    last_error,
                );
                queue.submit([encoder.finish()]);
            };

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        run_frame(&mut exec, &mut graph, &mut last_error);
        assert!(last_error.is_none(), "{last_error:?}");
        let v0 = exec.plan_version(TEST_CHAIN);
        assert_eq!(exec.feedback_generation(), 1);

        // A structural edit elsewhere in the graph replans — but must not
        // recreate the pair (the echo's contents survive the rewire).
        let noise = reg.get(&EffectId("noise_field".into())).unwrap();
        let orphan = graph.add_node(
            NodeKind::Source {
                effect: noise.id.clone(),
            },
            0,
            &noise.params.clone(),
        );
        run_frame(&mut exec, &mut graph, &mut last_error);
        assert!(last_error.is_none(), "{last_error:?}");
        assert_ne!(exec.plan_version(TEST_CHAIN), v0, "structural edit replans");
        assert_eq!(exec.feedback_generation(), 1, "pair survives the replan");
        assert_eq!(exec.feedback_stats(), 1);
        let _ = orphan;

        // Removing the feedback node prunes its pair; the graph keeps
        // rendering (mix's echo pin falls back to the placeholder).
        graph.remove_node(f).unwrap();
        run_frame(&mut exec, &mut graph, &mut last_error);
        assert!(last_error.is_none(), "{last_error:?}");
        assert_eq!(exec.feedback_stats(), 0, "pair pruned with its node");
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }

    // Run: cargo test -p fosfora-app -- --ignored trama_previews_toggle_replans_and_runs_orphans
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn trama_previews_toggle_replans_and_runs_orphans() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let reg = registry(&device);
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);
        let mut graph = NodeGraph::new_with_output();
        build_motion_echo(&reg, &mut graph);
        // An orphan source: never reaches Output, renders only for previews.
        let noise = reg.get(&EffectId("noise_field".into())).unwrap();
        let orphan = graph.add_node(
            NodeKind::Source {
                effect: noise.id.clone(),
            },
            0,
            &noise.params.clone(),
        );

        let placeholder = PlaceholderTexture::new(&device, &queue, GpuContext::hdr_format());
        let audio = AudioTextures::new(&device, &queue);
        let mut exec = TramaExecutor::new(&device, None, &placeholder, &audio, 128, 72);
        // The output target is the caller's now; the executor renders into it.
        let out = RenderTarget::new(
            &device,
            128,
            72,
            GpuContext::hdr_format(),
            1.0,
            "test-chain-output",
        );
        let mut template = ShaderUniforms::zeroed();
        template.resolution = [128.0, 72.0];
        let mut last_error = None;
        let run_frame = |exec: &mut TramaExecutor,
                         graph: &mut NodeGraph,
                         previews_on: bool,
                         last_error: &mut Option<String>| {
            exec.begin_frame();
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            exec.execute(
                TEST_CHAIN,
                graph,
                &reg,
                None,
                &out,
                1,
                &template,
                previews_on,
                &device,
                &queue,
                &mut encoder,
                crate::gpu::profiler::ProfilerHandle::none(),
                &mut *last_error,
            );
            queue.submit([encoder.finish()]);
        };

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        // Canvas closed: the orphan is culled, no preview targets exist.
        run_frame(&mut exec, &mut graph, false, &mut last_error);
        assert!(last_error.is_none(), "{last_error:?}");
        assert_eq!(
            exec.plan_step_count(TEST_CHAIN),
            4,
            "3 effects + 1 copy, no orphan"
        );
        assert_eq!(exec.preview_stats(), 0);
        let version = graph.version();

        // Canvas opens: same graph version, but the plan re-keys — the
        // orphan joins the step set and every executing node (feedback
        // included) gets a preview target. Run past a cadence tick so the
        // round-robin blit itself passes validation.
        for _ in 0..4 {
            run_frame(&mut exec, &mut graph, true, &mut last_error);
        }
        assert!(last_error.is_none(), "{last_error:?}");
        assert_eq!(graph.version(), version, "toggle is not a graph edit");
        assert_eq!(exec.plan_step_count(TEST_CHAIN), 5, "orphan joined");
        assert_eq!(
            exec.preview_stats(),
            5,
            "noise, mix, transform, orphan, feedback"
        );

        // Canvas closes: orphan culled again; targets persist so reopening
        // doesn't recreate textures (stable identity for egui registration).
        run_frame(&mut exec, &mut graph, false, &mut last_error);
        assert!(last_error.is_none(), "{last_error:?}");
        assert_eq!(exec.plan_step_count(TEST_CHAIN), 4);
        assert_eq!(exec.preview_stats(), 5, "targets persist across toggle");

        // Removing a node prunes its preview target.
        graph.remove_node(orphan).unwrap();
        run_frame(&mut exec, &mut graph, false, &mut last_error);
        assert!(last_error.is_none(), "{last_error:?}");
        assert_eq!(exec.preview_stats(), 4, "pruned with its node");
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }

    // Run: cargo test -p fosfora-app -- --ignored trama_output_target_identity_is_a_plan_key
    //
    // The output target moved out of the executor and into the caller, which
    // means its identity can now change under a cached plan. That matters
    // because `build_plan`'s `resolve` closure SAMPLES the output target: a
    // feedback node whose input is the final producer binds it in a copy step,
    // and every preview blit of the final producer binds it too. Nothing about
    // recreating a target touches the graph, so `out_generation` is the only
    // thing that can force the replan — exactly the role `chain_input` plays
    // for the host input, and the same bug that shipped in stage B's first
    // draft.
    //
    // Delete `|| p.out_generation != out_generation` from the plan key and the
    // third assertion here goes red.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn trama_output_target_identity_is_a_plan_key() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let reg = registry(&device);
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);

        // noise → mix ← feedback(mix), mix → Output. `mix` is the final
        // producer, so its target IS the output target, and the feedback copy
        // step samples it — a real render path, not just a thumbnail.
        let noise = reg.get(&EffectId("noise_field".into())).unwrap();
        let mix = reg.get(&EffectId("mix".into())).unwrap();
        let (noise_id, noise_params) = (noise.id.clone(), noise.params.clone());
        let (mix_id, mix_params) = (mix.id.clone(), mix.params.clone());

        let mut graph = NodeGraph::new_with_output();
        let n = graph.add_node(NodeKind::Source { effect: noise_id }, 0, &noise_params);
        let m = graph.add_node(NodeKind::Effect { effect: mix_id }, 2, &mix_params);
        let f = graph.add_node(NodeKind::Feedback, 1, &[]);
        let out_node = graph.output_node();
        graph.connect(n, m, 0).unwrap();
        graph.connect(m, f, 0).unwrap();
        graph.connect(f, m, 1).unwrap();
        graph.connect(m, out_node, 0).unwrap();

        let placeholder = PlaceholderTexture::new(&device, &queue, GpuContext::hdr_format());
        let audio = AudioTextures::new(&device, &queue);
        let mut exec = TramaExecutor::new(&device, None, &placeholder, &audio, 64, 64);
        let make_target =
            |label| RenderTarget::new(&device, 64, 64, GpuContext::hdr_format(), 1.0, label);
        let out_a = make_target("test-chain-output-a");
        let out_b = make_target("test-chain-output-b");

        let mut template = ShaderUniforms::zeroed();
        template.resolution = [64.0, 64.0];
        let mut last_error = None;
        let mut run = |exec: &mut TramaExecutor, target: &RenderTarget, generation: u64| {
            exec.begin_frame();
            let mut encoder = device.create_command_encoder(&Default::default());
            exec.execute(
                TEST_CHAIN,
                &graph,
                &reg,
                None,
                target,
                generation,
                &template,
                false,
                &device,
                &queue,
                &mut encoder,
                crate::gpu::profiler::ProfilerHandle::none(),
                &mut last_error,
            );
            queue.submit([encoder.finish()]);
        };

        run(&mut exec, &out_a, 1);
        assert_eq!(exec.plans_built(), 1, "first frame plans");

        // Steady state: nothing moved, so nothing replans. Without this the
        // test would pass against an executor that rebuilt every frame, which
        // is the failure mode `out_generation` could easily introduce (I8).
        for _ in 0..3 {
            run(&mut exec, &out_a, 1);
        }
        assert_eq!(exec.plans_built(), 1, "steady state must not replan");

        // A different target under the same graph version. The graph did not
        // change and neither did the host input, so only `out_generation` can
        // catch this.
        run(&mut exec, &out_b, 2);
        assert_eq!(
            exec.plans_built(),
            2,
            "a recreated output target must replan, or the feedback copy step \
             keeps sampling the target that was dropped"
        );
        assert!(last_error.is_none(), "{last_error:?}");
    }

    /// One frame of `graph` on a fresh 64x64 output; returns the raw
    /// Rgba16Float bytes and leaves the plan error in `last_error`.
    #[allow(clippy::too_many_arguments)]
    fn render_once(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        exec: &mut TramaExecutor,
        reg: &TramaRegistry,
        graph: &NodeGraph,
        out: &RenderTarget,
        last_error: &mut Option<String>,
    ) -> Vec<u8> {
        let mut template = ShaderUniforms::zeroed();
        template.resolution = [64.0, 64.0];
        exec.begin_frame();
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        exec.execute(
            TEST_CHAIN,
            graph,
            reg,
            None,
            out,
            1,
            &template,
            false,
            device,
            queue,
            &mut encoder,
            crate::gpu::profiler::ProfilerHandle::none(),
            last_error,
        );
        queue.submit([encoder.finish()]);
        snapshot(device, queue, &out.view, 64)
    }

    // Run: cargo test -p fosfora-app -- --ignored trama_missing_effect_is_a_placeholder
    //
    // A node whose effect is not installed — a patch loaded from someone
    // else's file, or an effect file deleted under a running app — must not
    // fail the plan. It did: `build_plan` returned Err on the first unknown
    // id, so the WHOLE chain froze on its last-good plan (or cleared to black
    // if it had none) and every other node stopped updating. The node now runs
    // a placeholder step that clears its target to magenta, and everything
    // around it keeps rendering. Magenta is not the signal on its own (the
    // canvas names the node "missing: <id>"); it is what makes the hole
    // visible in the picture rather than silently black.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn trama_missing_effect_is_a_placeholder() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let reg = registry(&device);
        let placeholder = PlaceholderTexture::new(&device, &queue, GpuContext::hdr_format());
        let audio = AudioTextures::new(&device, &queue);
        let mut exec = TramaExecutor::new(&device, None, &placeholder, &audio, 64, 64);
        let out = RenderTarget::new(
            &device,
            64,
            64,
            GpuContext::hdr_format(),
            1.0,
            "test-chain-output",
        );
        let mut last_error = None;
        let ghost_kind = || NodeKind::Effect {
            effect: EffectId("no_such_effect".into()),
        };
        // 1.0 and 0.0 as f16, little-endian: opaque magenta.
        const MAGENTA: [u8; 8] = [0x00, 0x3c, 0x00, 0x00, 0x00, 0x3c, 0x00, 0x3c];

        device.push_error_scope(wgpu::ErrorFilter::Validation);

        // The missing node alone, straight into Output.
        let mut graph = NodeGraph::new_with_output();
        let ghost = graph.add_node(ghost_kind(), 1, &[]);
        let out_node = graph.output_node();
        graph.connect(ghost, out_node, 0).unwrap();
        let shot = render_once(
            &device,
            &queue,
            &mut exec,
            &reg,
            &graph,
            &out,
            &mut last_error,
        );
        assert!(
            last_error.is_none(),
            "a missing effect is not a plan error: {last_error:?}"
        );
        assert!(
            shot.chunks_exact(8).all(|px| px == MAGENTA),
            "the placeholder fills its target with magenta, got {:?}",
            &shot[..8]
        );

        // The missing node as ONE input of a mix: the rest of the chain keeps
        // running, so the picture is neither the placeholder nor black.
        let mut graph = NodeGraph::new_with_output();
        let noise = reg.get(&EffectId("noise_field".into())).unwrap();
        let mix = reg.get(&EffectId("mix".into())).unwrap();
        let n = graph.add_node(
            NodeKind::Source {
                effect: noise.id.clone(),
            },
            0,
            &noise.params,
        );
        let ghost = graph.add_node(ghost_kind(), 1, &[]);
        let m = graph.add_node(
            NodeKind::Effect {
                effect: mix.id.clone(),
            },
            2,
            &mix.params,
        );
        let out_node = graph.output_node();
        graph.connect(n, m, 0).unwrap();
        graph.connect(ghost, m, 1).unwrap();
        graph.connect(m, out_node, 0).unwrap();
        let before = exec.plans_built();
        let shot = render_once(
            &device,
            &queue,
            &mut exec,
            &reg,
            &graph,
            &out,
            &mut last_error,
        );
        assert!(last_error.is_none(), "{last_error:?}");
        assert_eq!(
            exec.plans_built(),
            before + 1,
            "the new topology was planned"
        );
        assert!(
            shot.chunks_exact(8).any(|px| px != MAGENTA),
            "the noise side of the mix must still be rendering"
        );
        assert!(
            shot.chunks_exact(8)
                .all(|px| px[..2] != [0, 0] && px[4..6] != [0, 0]),
            "and the placeholder side must be in the blend: red and blue everywhere"
        );

        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }

    // Run: cargo test -p fosfora-app -- --ignored trama_hot_reload_swaps_live_and_never_blanks
    //
    // Handoff §12, end to end against real files in a scratch directory:
    // an edit swaps in live and replans exactly once; a file that stops
    // compiling leaves the LAST-GOOD picture on screen (I4 — a typo must never
    // blank the output), flags the effect, and clears the flag when fixed; a
    // changed manifest reaches the live node; a deleted file turns its node
    // into a placeholder and restoring the file brings the picture back.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn trama_hot_reload_swaps_live_and_never_blanks() {
        use crate::trama::effect::Reloaded;

        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let dir = std::env::temp_dir().join(format!("fosfora-trama-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for file in ["noise_field.wgsl", "hue_drift.wgsl"] {
            std::fs::copy(effects_dir().join(file), dir.join(file)).unwrap();
        }
        let hue_path = dir.join("hue_drift.wgsl");
        let original = std::fs::read_to_string(&hue_path).unwrap();
        let body = "return vec4f(fosfora_hue_shift(c.rgb, param(0u) + param(1u)), c.a);";
        assert!(
            original.contains(body),
            "hue_drift.wgsl moved on; update this test"
        );
        let inverted = original.replace(body, "return vec4f(vec3f(1.0) - c.rgb, c.a);");
        let broken = inverted.replace("let c = input0(uv);", "let c = input0(uv)");
        assert!(
            broken != inverted,
            "hue_drift.wgsl moved on; update this test"
        );

        let loader = EffectLoader::for_test(&probe_libs());
        let mut reg = TramaRegistry::load(&device, None, &loader, &dir);
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);
        let hue_id = EffectId("hue_drift".into());

        let placeholder = PlaceholderTexture::new(&device, &queue, GpuContext::hdr_format());
        let audio = AudioTextures::new(&device, &queue);
        let mut exec = TramaExecutor::new(&device, None, &placeholder, &audio, 64, 64);
        let out = RenderTarget::new(&device, 64, 64, GpuContext::hdr_format(), 1.0, "test-out");
        let mut last_error = None;
        let mut graph = NodeGraph::new_with_output();
        let noise = reg.get(&EffectId("noise_field".into())).unwrap();
        let n = graph.add_node(
            NodeKind::Source {
                effect: noise.id.clone(),
            },
            0,
            &noise.params.clone(),
        );
        let hue = reg.get(&hue_id).unwrap();
        let h = graph.add_node(
            NodeKind::Effect {
                effect: hue.id.clone(),
            },
            1,
            &hue.params.clone(),
        );
        graph
            .params_mut(h)
            .unwrap()
            .params
            .set("shift", crate::params::ParamValue::Float(0.4));
        let out_node = graph.output_node();
        graph.connect(n, h, 0).unwrap();
        graph.connect(h, out_node, 0).unwrap();

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        macro_rules! render {
            () => {
                render_once(
                    &device,
                    &queue,
                    &mut exec,
                    &reg,
                    &graph,
                    &out,
                    &mut last_error,
                )
            };
        }
        let before = render!();
        assert_eq!(exec.plans_built(), 1);

        // 1. An edit swaps in live.
        std::fs::write(&hue_path, &inverted).unwrap();
        assert_eq!(
            reg.reload_file(&device, None, &loader, &hue_path),
            Reloaded::Swapped {
                id: hue_id.clone(),
                manifest_changed: false
            }
        );
        let swapped = render!();
        render!();
        assert!(swapped != before, "the edited shader is the one rendering");
        assert_eq!(exec.plans_built(), 2, "one replan for one reload");

        // 2. A typo: last good keeps rendering, the effect is flagged.
        std::fs::write(&hue_path, &broken).unwrap();
        assert_eq!(
            reg.reload_file(&device, None, &loader, &hue_path),
            Reloaded::Failed(hue_id.clone())
        );
        assert!(
            render!() == swapped,
            "a file that does not compile never blanks the output"
        );
        assert_eq!(exec.plans_built(), 2, "and costs no replan");
        assert!(last_error.is_none(), "{last_error:?}");
        assert!(
            reg.get(&hue_id).unwrap().error.is_some(),
            "every instance gets its badge"
        );
        assert_eq!(reg.errors.len(), 1);

        // 3. Fixed: the flag clears.
        std::fs::write(&hue_path, &inverted).unwrap();
        reg.reload_file(&device, None, &loader, &hue_path);
        assert!(reg.get(&hue_id).unwrap().error.is_none());
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);
        assert!(
            render!() == swapped,
            "the fixed file renders what it did before the typo"
        );

        // 4. The manifest grows a parameter; the live node gets it, and keeps
        // the value it already had.
        let grown = inverted.replace(
            r#"{ "type": "Float", "name": "speed","#,
            r#"{ "type": "Float", "name": "extra", "default": 0.75, "min": 0.0, "max": 1.0 },
    { "type": "Float", "name": "speed","#,
        );
        assert!(
            grown != inverted,
            "hue_drift's manifest moved on; update this test"
        );
        std::fs::write(&hue_path, &grown).unwrap();
        assert_eq!(
            reg.reload_file(&device, None, &loader, &hue_path),
            Reloaded::Swapped {
                id: hue_id.clone(),
                manifest_changed: true
            }
        );
        let def = reg.get(&hue_id).unwrap();
        assert!(
            graph
                .sync_manifest(&hue_id, def.inputs, &def.params)
                .is_empty()
        );
        let node = graph.node(h).unwrap();
        assert!(
            matches!(node.params.get("extra"), Some(crate::params::ParamValue::Float(v)) if *v == 0.75)
        );
        assert!(
            matches!(node.params.get("shift"), Some(crate::params::ParamValue::Float(v)) if *v == 0.4)
        );
        render!();

        // 5. The file is deleted: a placeholder, not a frozen chain. Put it
        // back and the picture returns.
        std::fs::remove_file(&hue_path).unwrap();
        assert_eq!(
            reg.reload_file(&device, None, &loader, &hue_path),
            Reloaded::Removed(hue_id.clone())
        );
        let gone = render!();
        assert!(last_error.is_none(), "{last_error:?}");
        assert!(
            gone.chunks_exact(8)
                .all(|px| px[..2] == [0x00, 0x3c] && px[2..4] == [0, 0]),
            "the node renders the magenta placeholder"
        );
        std::fs::write(&hue_path, &inverted).unwrap();
        reg.reload_file(&device, None, &loader, &hue_path);
        assert!(render!() == swapped, "and comes back when its file does");

        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // Run: cargo test -p fosfora-app -- --ignored trama_registry_generation_is_a_plan_key
    //
    // A step caches a positional index into `registry.effects` and bind groups
    // built against that effect's bind-group layout. A hot reload changes both
    // without touching the graph, so the registry's generation has to be in
    // the plan key — the same bug shape as the output target's identity and
    // the chain input's, third time. Drop the key term and the second count
    // below stays at 1.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn trama_registry_generation_is_a_plan_key() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let mut reg = registry(&device);
        let placeholder = PlaceholderTexture::new(&device, &queue, GpuContext::hdr_format());
        let audio = AudioTextures::new(&device, &queue);
        let mut exec = TramaExecutor::new(&device, None, &placeholder, &audio, 64, 64);
        let out = RenderTarget::new(
            &device,
            64,
            64,
            GpuContext::hdr_format(),
            1.0,
            "test-chain-output",
        );
        let mut last_error = None;
        let mut graph = NodeGraph::new_with_output();
        let noise = reg.get(&EffectId("noise_field".into())).unwrap();
        let n = graph.add_node(
            NodeKind::Source {
                effect: noise.id.clone(),
            },
            0,
            &noise.params.clone(),
        );
        let out_node = graph.output_node();
        graph.connect(n, out_node, 0).unwrap();

        for _ in 0..3 {
            render_once(
                &device,
                &queue,
                &mut exec,
                &reg,
                &graph,
                &out,
                &mut last_error,
            );
        }
        assert_eq!(exec.plans_built(), 1, "steady state: one plan");

        reg.generation += 1;
        for _ in 0..3 {
            render_once(
                &device,
                &queue,
                &mut exec,
                &reg,
                &graph,
                &out,
                &mut last_error,
            );
        }
        assert_eq!(
            exec.plans_built(),
            2,
            "a reloaded registry replans, exactly once"
        );
    }

    // Run: cargo test -p fosfora-app -- --ignored trama_executor_black_on_unwired_output
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn trama_executor_black_on_unwired_output() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let reg = registry(&device);
        let graph = NodeGraph::new_with_output();
        let placeholder = PlaceholderTexture::new(&device, &queue, GpuContext::hdr_format());
        let audio = AudioTextures::new(&device, &queue);
        let mut exec = TramaExecutor::new(&device, None, &placeholder, &audio, 64, 64);
        // The output target is the caller's now; the executor renders into it.
        let out = RenderTarget::new(
            &device,
            64,
            64,
            GpuContext::hdr_format(),
            1.0,
            "test-chain-output",
        );
        let template = ShaderUniforms::zeroed();
        let mut last_error = None;

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        exec.execute(
            TEST_CHAIN,
            &graph,
            &reg,
            None,
            &out,
            1,
            &template,
            false,
            &device,
            &queue,
            &mut encoder,
            crate::gpu::profiler::ProfilerHandle::none(),
            &mut last_error,
        );
        queue.submit([encoder.finish()]);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
        assert!(last_error.is_none(), "{last_error:?}");
    }

    /// SPIKE (#2088, M2): mechanical feasibility of hosting a `.pfx` effect
    /// as a trama Source. Builds a real shipped effect (Aurora) into a
    /// `Layer` through `layer_builder` — the one production path — executes
    /// it, and binds its output target as a trama effect's `input0`, exactly
    /// as a wrapped-Source step would. What this proves: format/usage
    /// compatibility (both sides are Rgba16Float RENDER_ATTACHMENT |
    /// TEXTURE_BINDING), the flip()-per-frame cadence matching trama's
    /// `begin_frame`, and validation-clean cross-system sampling. The
    /// architectural findings (parity-alternating target identity needs the
    /// per-parity bind-group machinery trama already has; params share
    /// ParamStore/16-slot packing; cost = a full ping-pong pair per wrapped
    /// pass) live in docs/trama/DECISIONS.md.
    // Run: cargo test -p fosfora-app -- --ignored trama_spike_pfx_layer_feeds_trama_input
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn trama_spike_pfx_layer_feeds_trama_input() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        if !std::path::Path::new("assets/effects").is_dir() {
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            std::env::set_current_dir(&repo).unwrap();
        }

        // The .pfx side, assembled the way LoopSession does it.
        let mut loader = EffectLoader::new();
        loader.scan_effects_directory();
        let idx = loader
            .effects
            .iter()
            .position(|e| e.name == "Aurora")
            .expect("Aurora ships");
        let effect = loader.effects[idx].clone();
        let placeholder = PlaceholderTexture::new(&device, &queue, GpuContext::hdr_format());
        let audio = AudioTextures::new(&device, &queue);
        let ctx = crate::gpu::layer_builder::LayerBuildCtx {
            device: &device,
            queue: &queue,
            pipeline_cache: None,
            width: 128,
            height: 72,
            placeholder: &placeholder,
            audio_textures: &audio,
            particle_quality: Default::default(),
            backdrop: None,
        };
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut layer =
            crate::gpu::layer_builder::new_default_layer(&ctx, "spike".into()).expect("layer");
        let ps = crate::gpu::layer_builder::prepare_particles(&ctx, &mut loader, &effect);
        crate::gpu::layer_builder::load_effect_into_layer(
            &ctx, &loader, &mut layer, 0, &effect, idx, ps,
        )
        .expect("Aurora loads");
        layer.param_store.load_from_defs(&effect.inputs);

        // The trama side: hue_drift's real pipeline, its input0 bound to the
        // LAYER's output — the exact bind a wrapped-Source step would build.
        let reg = registry(&device);
        let hue = reg.get(&EffectId("hue_drift".into())).unwrap();
        let stride = aligned_stride(
            UNIFORM_SIZE,
            u64::from(device.limits().min_uniform_buffer_offset_alignment),
        );
        let arena = create_arena(&device, stride, 1);
        queue.write_buffer(&arena, 0, bytemuck::bytes_of(&ShaderUniforms::zeroed()));
        let scratch = RenderTarget::new(
            &device,
            128,
            72,
            GpuContext::hdr_format(),
            1.0,
            "spike-scratch",
        );

        // Two frames: layer flip() once per frame, mirroring trama's
        // begin_frame — the returned target alternates sides, so the bind
        // group is rebuilt per frame here (a real wrap prebuilds both
        // parities, the executor's existing [BindGroup; 2] idiom).
        for frame in 0..2 {
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            let pfx_out = layer.execute(&mut encoder, &queue);
            assert_eq!(
                pfx_out.format,
                GpuContext::hdr_format(),
                "same internal format both sides"
            );
            let entries = [
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &arena,
                        offset: 0,
                        size: Some(NonZeroU64::new(UNIFORM_SIZE).expect("nonzero")),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&placeholder.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&placeholder.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&audio.waveform_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&audio.spectrum_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&audio.spectrogram_view),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(&audio.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&pfx_out.view),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(&pfx_out.sampler),
                },
            ];
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("spike-pfx-into-trama"),
                layout: &hue.pipeline.bind_group_layout,
                entries: &entries,
            });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("spike-trama-consumer"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &scratch.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&hue.pipeline.pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            queue.submit([encoder.finish()]);
            layer.flip();
            let _ = frame;
        }
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }
}
