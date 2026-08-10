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
use super::super::node::{NodeId, NodeKind};
use super::textures::TexturePool;

const UNIFORM_SIZE: u64 = std::mem::size_of::<ShaderUniforms>() as u64;
/// Arena capacity floor — grows at plan build if a graph outgrows it.
const INITIAL_CAPACITY: u32 = 16;

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
}

struct Step {
    node: NodeId,
    kind: StepKind,
    target: TargetSlot,
    uniform_offset: u64,
    /// Indexed by the executor's global `parity` at execute time — the
    /// pass_executor #1481 idiom: a step reading a feedback node's output
    /// binds its *read* buffer, which alternates every frame, so both
    /// variants are prebuilt. Steps with no feedback input carry two clones
    /// of one bind group (wgpu handles are Arc-backed; the clone is free).
    bind_groups: [wgpu::BindGroup; 2],
}

/// One node's thumbnail blit: samples the node's output (per parity, same
/// story as [`Step::bind_groups`]) into its persistent preview target.
struct PreviewBlit {
    node: NodeId,
    bind_groups: [wgpu::BindGroup; 2],
}

struct ExecPlan {
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
}

pub struct TramaExecutor {
    arena: wgpu::Buffer,
    stride: u64,
    capacity: u32,
    output: RenderTarget,
    pool: TexturePool,
    plan: Option<ExecPlan>,
    /// Ping-pong pairs OUTSIDE the plan, keyed by node: contents survive
    /// replans, so a rewire elsewhere in the graph never clears an unrelated
    /// echo. Synced (created/pruned) at plan build; cleared on resize.
    feedback: HashMap<NodeId, PingPongTarget>,
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
            arena: create_arena(device, stride, INITIAL_CAPACITY),
            stride,
            capacity: INITIAL_CAPACITY,
            output: RenderTarget::new(
                device,
                width,
                height,
                GpuContext::hdr_format(),
                1.0,
                "trama-output",
            ),
            pool: TexturePool::new(),
            plan: None,
            feedback: HashMap::new(),
            parity: 0,
            feedback_generation: 0,
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

    /// Output-resolution change: recreate the output target, drop the pool,
    /// force a replan. No-op when the size is unchanged.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }
        self.width = width;
        self.height = height;
        self.output = RenderTarget::new(
            device,
            width,
            height,
            GpuContext::hdr_format(),
            1.0,
            "trama-output",
        );
        self.pool.clear();
        // Echo contents are meaningless at a new size; the next plan build
        // recreates pairs cleared at the new resolution.
        self.feedback.clear();
        self.plan = None;
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
    pub fn preview_tex(&self, node: NodeId) -> Option<egui::TextureId> {
        self.previews.tex_of(node)
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
        graph: &mut NodeGraph,
        registry: &TramaRegistry,
        template: &ShaderUniforms,
        previews_on: bool,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        profiler: crate::gpu::profiler::ProfilerHandle<'_>,
        last_error: &mut Option<String>,
    ) -> &RenderTarget {
        let version = graph.version();
        if self
            .plan
            .as_ref()
            .is_none_or(|p| p.version != version || p.previews_on != previews_on)
        {
            match self.build_plan(graph, registry, device, queue, version, previews_on) {
                Ok(plan) => {
                    self.plan = Some(plan);
                    *last_error = None;
                }
                Err(e) => {
                    // Keep the last-good plan rendering (I4). Stamp BOTH plan
                    // keys so the failed build is not retried every frame;
                    // the next structural edit retries naturally.
                    *last_error = Some(e);
                    match self.plan.as_mut() {
                        Some(p) => {
                            p.version = version;
                            p.previews_on = previews_on;
                        }
                        None => {
                            self.plan = Some(ExecPlan {
                                version,
                                previews_on,
                                steps: Vec::new(),
                                previews: Vec::new(),
                                output_written: false,
                            });
                        }
                    }
                }
            }
        }
        let plan = self.plan.as_ref().expect("plan installed above");

        for step in &plan.steps {
            let node = graph.node(step.node).expect("plan nodes exist in graph");
            let mut u = *template;
            u.params = node.params.pack_to_buffer();
            // Overlay the modulation values resolved in `TramaSystem::update`
            // — this only reads cached state, so the dissolve path's second
            // execute per frame sees identical values (no double-advance).
            super::super::modulation::apply_resolved(&mut u.params, &node.mods);
            queue.write_buffer(&self.arena, step.uniform_offset, bytemuck::bytes_of(&u));
        }

        for step in &plan.steps {
            let view = match step.target {
                TargetSlot::Pool(i) => &self.pool.get(i).view,
                TargetSlot::Output => &self.output.view,
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
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
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
            };
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &step.bind_groups[self.parity], &[]);
            pass.draw(0..3, 0..1);
        }

        if !plan.output_written {
            // Nothing feeds Output: deliberate cleared black, never a stale
            // frame and never a silent fall-back to the layer stack.
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("trama-output-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.output.view,
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

        &self.output
    }

    fn build_plan(
        &mut self,
        graph: &mut NodeGraph,
        registry: &TramaRegistry,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        version: u64,
        previews_on: bool,
    ) -> Result<ExecPlan, String> {
        graph.validate().map_err(|e| e.to_string())?;
        // The execution set: nodes feeding Output — widened to every node
        // (orphans included) when previews are on, per handoff §9.1: orphan
        // subgraphs render only while someone can see their thumbnails.
        let exec_set: Vec<NodeId> = if previews_on {
            graph.topo_order().to_vec()
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
                !matches!(node.kind, NodeKind::Output | NodeKind::Feedback) && !node.bypass
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

        let final_producer = graph
            .input_source(graph.output_node(), 0)
            .and_then(effective);
        // A Feedback node feeding Output has no effect pass; it needs an
        // extra read→Output blit step. If it is inactive (unwired input),
        // Output must show deliberate black, not a stale frame.
        let feedback_feeds_output =
            final_producer.is_some_and(|id| node_is_feedback(id) && is_active_feedback(id));
        let output_written = match final_producer {
            Some(fp) if node_is_feedback(fp) => feedback_feeds_output,
            Some(_) => true,
            None => false,
        };

        // Sync the pair map before bind groups borrow it: prune pairs whose
        // node is gone, create (cleared → first frame reads transparent
        // black) pairs for newly active feedback nodes. `new_cleared`
        // submits its own tiny encoder — replans are rare structural events,
        // so I8's steady-state clause is untouched.
        self.feedback.retain(|id, _| graph.node(*id).is_some());
        let (w, h) = (self.width, self.height);
        for &(fb, _) in &active_feedback {
            let generation = &mut self.feedback_generation;
            self.feedback.entry(fb).or_insert_with(|| {
                *generation += 1;
                PingPongTarget::new_cleared(device, queue, w, h, GpuContext::hdr_format(), 1.0)
            });
        }

        // Preview targets: prune removed nodes always (their egui textures
        // are freed at the next register call); create targets only while
        // previews are on. Both are plan-build-time mutations — the render
        // path below only reads (I8).
        self.previews.prune(|id| graph.node(id).is_some());
        let previewed: Vec<NodeId> = if previews_on {
            step_nodes
                .iter()
                .copied()
                .chain(active_feedback.iter().map(|&(fb, _)| fb))
                .collect()
        } else {
            Vec::new()
        };
        for &id in &previewed {
            self.previews.ensure(device, id);
        }

        let total_steps =
            step_nodes.len() + active_feedback.len() + usize::from(feedback_feeds_output);
        if total_steps as u32 > self.capacity {
            self.capacity = (total_steps as u32).next_power_of_two();
            self.arena = create_arena(device, self.stride, self.capacity);
        }

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

        // Resolve one producer to (view, sampler) at a given parity. An
        // active feedback producer resolves to its READ buffer —
        // `targets[1 - parity]`, written last frame. Everything else is
        // parity-independent.
        let resolve =
            |producer: Option<NodeId>, parity: usize| -> (&wgpu::TextureView, &wgpu::Sampler) {
                let Some(p) = producer else {
                    return (&self.prev_view, &self.prev_sampler);
                };
                if node_is_feedback(p) {
                    if let (true, Some(pair)) = (is_active_feedback(p), self.feedback.get(&p)) {
                        let rt = &pair.targets[1 - parity];
                        return (&rt.view, &rt.sampler);
                    }
                    return (&self.prev_view, &self.prev_sampler);
                }
                match target_of(p) {
                    Some(TargetSlot::Pool(i)) => {
                        let rt = self.pool.get(i);
                        (&rt.view, &rt.sampler)
                    }
                    Some(TargetSlot::Output) => (&self.output.view, &self.output.sampler),
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
                NodeKind::Output | NodeKind::Feedback => unreachable!("filtered above"),
            };
            let effect_idx = registry
                .effects
                .iter()
                .position(|e| &e.id == effect_id)
                .ok_or_else(|| format!("node references unknown effect `{}`", effect_id.0))?;
            let def = &registry.effects[effect_idx];

            let uniform_offset = i as u64 * self.stride;
            let producers: Vec<Option<NodeId>> = (0..def.inputs)
                .map(|pin| graph.input_source(id, pin).and_then(effective))
                .collect();
            // Only an active feedback input makes resolution parity-dependent.
            let parity_dependent = producers.iter().any(|p| p.is_some_and(is_active_feedback));

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
                node: id,
                kind: StepKind::Effect { effect: effect_idx },
                target: targets[i].1,
                uniform_offset,
                bind_groups,
            });
        }

        // Copy steps, appended after all effect passes (see ExecPlan.steps).
        let mut step_index = step_nodes.len();
        for &(fb, producer) in &active_feedback {
            let uniform_offset = step_index as u64 * self.stride;
            step_index += 1;
            let layout = &self.copy_pipeline.bind_group_layout;
            let bg0 = make_bind_group(layout, uniform_offset, &[resolve(Some(producer), 0)]);
            let bind_groups = if is_active_feedback(producer) {
                // Chained feedback: this copy reads another pair's read side.
                [
                    bg0,
                    make_bind_group(layout, uniform_offset, &[resolve(Some(producer), 1)]),
                ]
            } else {
                [bg0.clone(), bg0]
            };
            steps.push(Step {
                node: fb,
                kind: StepKind::Copy,
                target: TargetSlot::FeedbackWrite,
                uniform_offset,
                bind_groups,
            });
        }
        if feedback_feeds_output {
            let fp = final_producer.expect("checked by feedback_feeds_output");
            let uniform_offset = step_index as u64 * self.stride;
            let layout = &self.copy_pipeline.bind_group_layout;
            // Reads the pair's read buffer — parity-dependent by definition.
            steps.push(Step {
                node: fp,
                kind: StepKind::Copy,
                target: TargetSlot::Output,
                uniform_offset,
                bind_groups: [
                    make_bind_group(layout, uniform_offset, &[resolve(Some(fp), 0)]),
                    make_bind_group(layout, uniform_offset, &[resolve(Some(fp), 1)]),
                ],
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
                    node: id,
                    bind_groups: [
                        make_bind_group(layout, 0, &[resolve(Some(id), 0)]),
                        make_bind_group(layout, 0, &[resolve(Some(id), 1)]),
                    ],
                }
            })
            .collect();

        Ok(ExecPlan {
            version,
            previews_on,
            steps,
            previews,
            output_written,
        })
    }

    #[cfg(test)]
    fn plan_version(&self) -> Option<u64> {
        self.plan.as_ref().map(|p| p.version)
    }

    #[cfg(test)]
    fn feedback_generation(&self) -> u64 {
        self.feedback_generation
    }

    #[cfg(test)]
    fn plan_step_count(&self) -> usize {
        self.plan.as_ref().map_or(0, |p| p.steps.len())
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
    use crate::gpu::test_gpu::{gpu_guard, test_gpu};
    use crate::trama::effect::{EffectId, TramaRegistry};
    use crate::trama::graph::NodeGraph;
    use crate::trama::node::NodeKind;

    #[test]
    fn aligned_stride_rounds_448_up_to_alignment() {
        assert_eq!(aligned_stride(448, 256), 512);
        assert_eq!(aligned_stride(448, 64), 448);
        assert_eq!(aligned_stride(448, 32), 448);
        assert_eq!(aligned_stride(256, 256), 256);
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

        let mut template = ShaderUniforms::zeroed();
        template.resolution = [256.0, 144.0];
        let mut last_error = None;

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let _ = exec.execute(
            &mut graph,
            &reg,
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
        let v = exec.plan_version();
        let stats = exec.pool_stats();
        // noise renders to a pooled target; hue renders straight into the
        // output target — one pooled texture total.
        assert_eq!(stats, (1, 1), "pool stats");

        // Second frame with unchanged topology: same plan, no new targets.
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let _ = exec.execute(
            &mut graph,
            &reg,
            &template,
            false,
            &device,
            &queue,
            &mut encoder,
            crate::gpu::profiler::ProfilerHandle::none(),
            &mut last_error,
        );
        queue.submit([encoder.finish()]);
        assert_eq!(exec.plan_version(), v, "plan reused");
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
        let mut template = ShaderUniforms::zeroed();
        template.resolution = [256.0, 144.0];
        let mut last_error = None;

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut first = (None, (0, 0), 0);
        const FRAMES: usize = 10;
        let mut alloc_deltas = [0u64; FRAMES];
        for (frame, delta) in alloc_deltas.iter_mut().enumerate() {
            // Parity flips once per frame, as TramaSystem::update drives it.
            exec.begin_frame();
            let (allocs, ()) = crate::test_alloc::count_allocs(|| {
                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                let _ = exec.execute(
                    &mut graph,
                    &reg,
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
                exec.plan_version(),
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
        assert_eq!(exec.plan_step_count(), 4, "3 effect passes + 1 copy");
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
        let early_floor = alloc_deltas[1..5].iter().min();
        let late_floor = alloc_deltas[FRAMES - 4..].iter().min();
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
        let mut template = ShaderUniforms::zeroed();
        template.resolution = [128.0, 72.0];
        let mut last_error = None;
        let run_frame =
            |exec: &mut TramaExecutor, graph: &mut NodeGraph, last_error: &mut Option<String>| {
                exec.begin_frame();
                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                let _ = exec.execute(
                    graph,
                    &reg,
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
        let v0 = exec.plan_version();
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
        assert_ne!(exec.plan_version(), v0, "structural edit replans");
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
            let _ = exec.execute(
                graph,
                &reg,
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
        assert_eq!(exec.plan_step_count(), 4, "3 effects + 1 copy, no orphan");
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
        assert_eq!(exec.plan_step_count(), 5, "orphan joined");
        assert_eq!(
            exec.preview_stats(),
            5,
            "noise, mix, transform, orphan, feedback"
        );

        // Canvas closes: orphan culled again; targets persist so reopening
        // doesn't recreate textures (stable identity for egui registration).
        run_frame(&mut exec, &mut graph, false, &mut last_error);
        assert!(last_error.is_none(), "{last_error:?}");
        assert_eq!(exec.plan_step_count(), 4);
        assert_eq!(exec.preview_stats(), 5, "targets persist across toggle");

        // Removing a node prunes its preview target.
        graph.remove_node(orphan).unwrap();
        run_frame(&mut exec, &mut graph, false, &mut last_error);
        assert!(last_error.is_none(), "{last_error:?}");
        assert_eq!(exec.preview_stats(), 4, "pruned with its node");
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }

    // Run: cargo test -p fosfora-app -- --ignored trama_executor_black_on_unwired_output
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn trama_executor_black_on_unwired_output() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let reg = registry(&device);
        let mut graph = NodeGraph::new_with_output();
        let placeholder = PlaceholderTexture::new(&device, &queue, GpuContext::hdr_format());
        let audio = AudioTextures::new(&device, &queue);
        let mut exec = TramaExecutor::new(&device, None, &placeholder, &audio, 64, 64);
        let template = ShaderUniforms::zeroed();
        let mut last_error = None;

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let _ = exec.execute(
            &mut graph,
            &reg,
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
