use wgpu::{CommandEncoder, Device, Queue, Sampler, TextureFormat, TextureView};

use crate::effect::EffectLoader;
use crate::effect::format::PassDef;

use super::ShaderPipeline;
use super::audio_textures::AudioTextures;
use super::particle::ParticleSystem;
use super::placeholder::PlaceholderTexture;
use super::render_target::{PingPongTarget, RenderTarget};
use super::uniforms::UniformBuffer;

/// Special input name (#1482): the particle system's resolved per-pixel
/// velocity texture. Requires the effect's `particles.velocity_field: true`;
/// without it (or without a particle system) the 1×1 placeholder is bound,
/// which reads as zero velocity.
const PARTICLE_VELOCITY_INPUT: &str = "@particles.velocity";
const BACKDROP_INPUT: &str = "@backdrop";

/// One resolved pass-graph input, in WGSL `input0..` numbering: current-frame
/// inputs first (`PassDef.inputs`), then previous-frame inputs
/// (`PassDef.prev_inputs`), each in declaration order.
#[derive(Clone, Copy)]
enum InputSrc {
    /// Another pass's target: its current frame (`prev: false`) or its
    /// previous frame (`prev: true`).
    Pass { pass: usize, prev: bool },
    /// The particle rasterizer's velocity texture (`@particles.velocity`,
    /// #1482) — (vx, vy, coverage, 0) in NDC units/sec, resolved during
    /// `ParticleSystem::dispatch`, i.e. same-frame for the fragment passes.
    ParticleVelocity,
    /// The composite of every layer BELOW this one (`@backdrop`, #2061) —
    /// the compositor's stable backdrop target, snapshotted by `frame_graph`
    /// right before this layer executes. Premultiplied RGBA; transparent when
    /// nothing is beneath.
    Backdrop,
}

/// A compiled pass: pipeline + render target + bind groups.
struct CompiledPass {
    name: String,
    pipeline: ShaderPipeline,
    /// Ping-pong target for this pass (feedback-capable).
    target: PingPongTarget,
    /// Bind groups indexed by the executor's global flip parity (#1481), not by
    /// this pass's own `target.current`: a non-feedback pass must still read a
    /// feedback input at the right parity.
    bind_groups: [wgpu::BindGroup; 2],
    has_feedback: bool,
    /// Prior passes this pass samples as `input0..inputN-1` (current + prev frame).
    input_srcs: Vec<InputSrc>,
    /// Per-frame draw count. `>1` ping-pongs this pass's own target between draws
    /// (Jacobi/relaxation loops); requires `has_feedback`. `1` = single draw.
    iterations: u32,
}

/// Everything the bind-group builder needs about each pass, borrowed. Lets one
/// builder serve both construction (from freshly prepared passes) and rebuilds
/// (from the live `CompiledPass` list) without a per-pass mutable/immutable
/// aliasing conflict — see `rebuild_all_bind_groups`.
struct PassView<'a> {
    layout: &'a wgpu::BindGroupLayout,
    target: &'a PingPongTarget,
    has_feedback: bool,
    input_srcs: &'a [InputSrc],
}

/// A pass after pipeline + target creation but before its bind groups exist
/// (which need every pass's target to be resolvable). Construction two-phase.
struct PreparedPass {
    name: String,
    pipeline: ShaderPipeline,
    target: PingPongTarget,
    has_feedback: bool,
    input_srcs: Vec<InputSrc>,
    iterations: u32,
}

/// Executes a sequence of render passes for a multi-pass effect.
pub struct PassExecutor {
    passes: Vec<CompiledPass>,
    pub particle_system: Option<ParticleSystem>,
    /// Owned handle to the compositor's `@backdrop` target (TextureView/Sampler
    /// are Arc'd wgpu handles, so this is a cheap clone, not a copy). None until
    /// `layer_builder` wires it; refreshed on resize because the compositor
    /// recreates the texture behind it.
    backdrop: Option<(TextureView, Sampler)>,
    /// Global ping-pong parity. All feedback passes flip in lockstep, so each
    /// feedback pass's `target.current` equals this value; bind groups are indexed
    /// by it so cross-pass reads land on the correct target every frame (#1481).
    flip_parity: usize,
}

impl PassExecutor {
    /// Build a PassExecutor from a list of PassDefs.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &Device,
        hdr_format: TextureFormat,
        width: u32,
        height: u32,
        pass_defs: &[PassDef],
        effect_loader: &EffectLoader,
        uniform_buffer: &UniformBuffer,
        placeholder: &PlaceholderTexture,
        audio: &AudioTextures,
        queue: &Queue,
        pipeline_cache: Option<&wgpu::PipelineCache>,
    ) -> Result<Self, String> {
        // Phase 1: resolve inputs, compile pipelines, create targets.
        let mut prepared: Vec<PreparedPass> = Vec::with_capacity(pass_defs.len());

        for (idx, def) in pass_defs.iter().enumerate() {
            // Resolve inputs into `input0..` order: current-frame inputs first,
            // then previous-frame inputs.
            let mut input_srcs = Vec::with_capacity(def.inputs.len() + def.prev_inputs.len());

            // `inputs`: current-frame output of an EARLIER pass, or a special
            // `@` input. Forward/unknown references are a hard error — that
            // half of the graph is a DAG.
            for name in &def.inputs {
                if name.starts_with('@') {
                    if name == PARTICLE_VELOCITY_INPUT {
                        input_srcs.push(InputSrc::ParticleVelocity);
                        continue;
                    }
                    if name == BACKDROP_INPUT {
                        input_srcs.push(InputSrc::Backdrop);
                        continue;
                    }
                    return Err(format!(
                        "Pass '{}' input '{name}' is not a known special input \
                         (expected '{PARTICLE_VELOCITY_INPUT}' or '{BACKDROP_INPUT}')",
                        def.name
                    ));
                }
                let src = pass_defs[..idx]
                    .iter()
                    .position(|p| &p.name == name)
                    .ok_or_else(|| {
                        format!(
                            "Pass '{}' input '{name}' does not name an earlier pass",
                            def.name
                        )
                    })?;
                input_srcs.push(InputSrc::Pass {
                    pass: src,
                    prev: false,
                });
            }

            // `prev_inputs`: previous-frame output of ANY feedback pass (later refs
            // allowed — previous-frame data has no intra-frame ordering constraint;
            // this is the edge that cuts a solver's velocity→div→pressure→velocity
            // cycle). A non-feedback pass has no distinct previous frame, so require
            // `feedback: true`.
            for name in &def.prev_inputs {
                if name.starts_with('@') {
                    return Err(format!(
                        "Pass '{}' prev_input '{name}': special inputs have no \
                         previous frame — use `inputs` instead",
                        def.name
                    ));
                }
                let src = pass_defs
                    .iter()
                    .position(|p| &p.name == name)
                    .ok_or_else(|| {
                        format!("Pass '{}' prev_input '{name}' names no pass", def.name)
                    })?;
                if !pass_defs[src].feedback {
                    return Err(format!(
                        "Pass '{}' prev_input '{name}' must name a feedback pass",
                        def.name
                    ));
                }
                input_srcs.push(InputSrc::Pass {
                    pass: src,
                    prev: true,
                });
            }

            let input_count = input_srcs.len();
            let source = effect_loader
                .load_effect_source_with_inputs(&def.shader, input_count)
                .map_err(|e| format!("Failed to load shader '{}': {e}", def.shader))?;

            let pipeline =
                ShaderPipeline::new(device, hdr_format, &source, pipeline_cache, input_count)
                    .map_err(|e| format!("Failed to compile shader '{}': {e}", def.shader))?;

            // Clear feedback targets to prevent NaN/garbage from uninitialized GPU memory
            let target = if def.feedback {
                PingPongTarget::new_cleared(device, queue, width, height, hdr_format, def.scale)
            } else {
                PingPongTarget::new(device, width, height, hdr_format, def.scale)
            };

            // Iterations only ping-pong a feedback target; ignore on non-feedback passes.
            let iterations = if def.feedback {
                def.iterations.max(1)
            } else {
                1
            };

            prepared.push(PreparedPass {
                name: def.name.clone(),
                pipeline,
                target,
                has_feedback: def.feedback,
                input_srcs,
                iterations,
            });
        }

        // Phase 2: build every pass's bind groups now that all targets exist.
        let views: Vec<PassView> = prepared
            .iter()
            .map(|p| PassView {
                layout: &p.pipeline.bind_group_layout,
                target: &p.target,
                has_feedback: p.has_feedback,
                input_srcs: &p.input_srcs,
            })
            .collect();
        // The particle system attaches after construction (`set_particle_system`),
        // so any `@particles.velocity` slot starts on the placeholder.
        let bind_groups: Vec<[wgpu::BindGroup; 2]> = (0..views.len())
            .map(|i| {
                build_bind_groups(
                    &views,
                    i,
                    device,
                    uniform_buffer,
                    placeholder,
                    audio,
                    None,
                    None,
                )
            })
            .collect();
        drop(views);

        let passes = prepared
            .into_iter()
            .zip(bind_groups)
            .map(|(p, bg)| CompiledPass {
                name: p.name,
                pipeline: p.pipeline,
                target: p.target,
                bind_groups: bg,
                has_feedback: p.has_feedback,
                input_srcs: p.input_srcs,
                iterations: p.iterations,
            })
            .collect();

        Ok(Self {
            passes,
            particle_system: None,
            backdrop: None,
            flip_parity: 0,
        })
    }

    /// Build a single-pass executor (the common case for backward-compatible effects).
    pub fn single_pass(
        pipeline: ShaderPipeline,
        feedback: PingPongTarget,
        uniform_buffer: &UniformBuffer,
        device: &Device,
        placeholder: &PlaceholderTexture,
        audio: &AudioTextures,
    ) -> Self {
        let bind_groups = {
            let views = [PassView {
                layout: &pipeline.bind_group_layout,
                target: &feedback,
                has_feedback: true, // always enable feedback for single-pass mode
                input_srcs: &[],
            }];
            build_bind_groups(
                &views,
                0,
                device,
                uniform_buffer,
                placeholder,
                audio,
                None,
                None,
            )
        };

        Self {
            passes: vec![CompiledPass {
                name: "main".to_string(),
                pipeline,
                target: feedback,
                bind_groups,
                has_feedback: true,
                input_srcs: Vec::new(),
                iterations: 1,
            }],
            particle_system: None,
            backdrop: None,
            flip_parity: 0,
        }
    }

    /// Does any pass sample `@backdrop`? Drives frame_graph's snapshot step.
    pub fn wants_backdrop(&self) -> bool {
        self.passes
            .iter()
            .any(|p| p.input_srcs.iter().any(|s| matches!(s, InputSrc::Backdrop)))
    }

    /// Store the backdrop handle and rebind any `@backdrop` slots (construction
    /// path — the compositor exists before effects load, so this runs once,
    /// right after `new`/`single_pass`).
    pub fn set_backdrop(
        &mut self,
        backdrop: Option<(TextureView, Sampler)>,
        device: &Device,
        uniform_buffer: &UniformBuffer,
        placeholder: &PlaceholderTexture,
        audio: &AudioTextures,
    ) {
        self.backdrop = backdrop;
        if self.wants_backdrop() {
            self.rebuild_all_bind_groups(device, uniform_buffer, placeholder, audio);
        }
    }

    /// Execute all passes. Returns a reference to the final pass's write target.
    /// The probes' entry point; the app goes through [`Self::execute_profiled`].
    #[cfg(test)]
    pub fn execute(
        &self,
        encoder: &mut CommandEncoder,
        uniform_buffer: &UniformBuffer,
        queue: &Queue,
        uniforms: &super::ShaderUniforms,
    ) -> &RenderTarget {
        self.execute_profiled(
            encoder,
            uniform_buffer,
            queue,
            uniforms,
            super::profiler::ProfilerHandle::none(),
        )
    }

    /// [`Self::execute`] with a timing scope per fragment pass (named after
    /// the pass, all of its iterations inside one scope) and one each for the
    /// particle dispatch and render, so a multi-pass effect's cost can be read
    /// pass by pass rather than only as its layer's total.
    pub fn execute_profiled(
        &self,
        encoder: &mut CommandEncoder,
        uniform_buffer: &UniformBuffer,
        queue: &Queue,
        uniforms: &super::ShaderUniforms,
        profiler: super::profiler::ProfilerHandle<'_>,
    ) -> &RenderTarget {
        uniform_buffer.update(queue, uniforms);

        // 1. Particle compute dispatch (before fragment passes)
        if let Some(ref ps) = self.particle_system {
            let mut scope = profiler.scope("particles-sim", encoder);
            ps.dispatch(scope.encoder(), queue);
        }

        // 2. Fragment shader passes
        for pass in &self.passes {
            let mut scope = profiler.scope(&pass.name, encoder);
            let encoder = scope.encoder();
            // Single-draw passes render into `write_target()` (= targets[flip_parity]
            // for a feedback pass) with the parity-indexed bind group. An iterated
            // (Jacobi) pass ping-pongs its own two targets in-encoder: draw `k` uses
            // bind_group[g] and writes targets[g] (bind_group[g] reads targets[1-g] via
            // feedback(), so consecutive draws chain), with `g` alternating so the FINAL
            // draw lands in targets[flip_parity] — what downstream readers (indexed by
            // flip_parity) and next frame's warm-start expect. Non-feedback inputs stay
            // fixed in targets[0] across the loop, so a stable divergence feeds every
            // pressure iteration.
            let n = pass.iterations.max(1);
            for k in 0..n {
                // (write index, bind-group index). Single draw: write our own
                // `current` target (flip_parity for feedback, 0 for non-feedback) and
                // read with the parity-indexed bind group — a non-feedback pass reading
                // a feedback input must pick the group pointing at that input's
                // current-frame target (#1481). Iterated (feedback only): both indices
                // are `g`, alternating so the FINAL draw lands in targets[flip_parity].
                let (write_idx, bind_idx) = if pass.has_feedback && n > 1 {
                    let g0 = self.flip_parity ^ ((n as usize - 1) & 1);
                    let g = g0 ^ (k as usize & 1);
                    (g, g)
                } else {
                    (pass.target.current, self.flip_parity)
                };
                let write_view = &pass.target.targets[write_idx].view;
                let bind_group = &pass.bind_groups[bind_idx];

                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(&pass.name),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: write_view,
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

                rp.set_pipeline(&pass.pipeline.pipeline);
                rp.set_bind_group(0, bind_group, &[]);
                rp.draw(0..3, 0..1);
            }
        }

        let final_target = self
            .passes
            .last()
            .expect("pipeline always has at least one pass")
            .target
            .write_target();

        // 3. Particle render pass — composites on top of last fragment pass with LoadOp::Load
        if let Some(ref ps) = self.particle_system {
            let mut scope = profiler.scope("particles-render", encoder);
            ps.render(scope.encoder(), queue, &final_target.view);
        }

        final_target
    }

    /// The final pass's two ping-pong targets as `(written this frame, the
    /// other one)`.
    ///
    /// A trama chain that samples this layer binds its texture once, at plan
    /// build, but `execute` hands back `targets[current]` and a feedback pass
    /// flips `current` every frame — so the chain needs a bind group for each
    /// and has to know which is which. Pairing them against the *observed*
    /// current target rather than deriving an offset from the two parity
    /// counters is deliberate: they advance in lockstep (spike #2098), so the
    /// pairing captured at plan build stays correct, and there is no sign
    /// convention to get backwards.
    ///
    /// A last pass without feedback never flips, so both are the same target.
    pub fn final_targets(&self) -> (&RenderTarget, &RenderTarget) {
        let pass = self
            .passes
            .last()
            .expect("pipeline always has at least one pass");
        if pass.has_feedback {
            (pass.target.write_target(), pass.target.read_target())
        } else {
            (pass.target.write_target(), pass.target.write_target())
        }
    }

    /// Flip all feedback-enabled passes for next frame, and advance the global
    /// parity in lockstep so cross-pass reads stay aligned.
    pub fn flip(&mut self) {
        self.flip_parity = 1 - self.flip_parity;
        for pass in &mut self.passes {
            if pass.has_feedback {
                pass.target.flip();
            }
        }
        if let Some(ref mut ps) = self.particle_system {
            ps.flip();
        }
    }

    /// Resize all pass targets (clears feedback targets to prevent NaN from uninitialized GPU memory).
    pub fn resize(
        &mut self,
        device: &Device,
        queue: &Queue,
        width: u32,
        height: u32,
        uniform_buffer: &UniformBuffer,
        placeholder: &PlaceholderTexture,
        audio: &AudioTextures,
    ) {
        // Phase 1: resize every target (recreates the textures behind them).
        for pass in &mut self.passes {
            if pass.has_feedback {
                pass.target.resize_cleared(device, queue, width, height);
            } else {
                pass.target.resize(device, width, height);
            }
        }
        // Resize the compute rasterizer before the bind-group rebuild: its
        // velocity texture is recreated here, and any `@particles.velocity`
        // slot must rebind the new texture, not the dropped one (#1482).
        if let Some(ref mut ps) = self.particle_system {
            ps.resize_compute_raster(device, width, height);
            ps.resize_wboit(device, width, height);
        }

        // Phase 2: rebuild all bind groups against the new targets (a pass may
        // sample another pass's just-recreated target).
        self.rebuild_all_bind_groups(device, uniform_buffer, placeholder, audio);
    }

    /// Rebuild every pass's bind groups from the current targets/layouts. Reads
    /// `&self.passes` to a `Vec<PassView>`, collects the new groups, then assigns
    /// — so cross-pass target references never alias a mutable borrow.
    fn rebuild_all_bind_groups(
        &mut self,
        device: &Device,
        uniform_buffer: &UniformBuffer,
        placeholder: &PlaceholderTexture,
        audio: &AudioTextures,
    ) {
        let new_groups: Vec<[wgpu::BindGroup; 2]> = {
            let particle_velocity = self
                .particle_system
                .as_ref()
                .and_then(|ps| ps.particle_velocity());
            let backdrop = self.backdrop.as_ref().map(|(v, sm)| (v, sm));
            let views: Vec<PassView> = self
                .passes
                .iter()
                .map(|p| PassView {
                    layout: &p.pipeline.bind_group_layout,
                    target: &p.target,
                    has_feedback: p.has_feedback,
                    input_srcs: &p.input_srcs,
                })
                .collect();
            (0..views.len())
                .map(|i| {
                    build_bind_groups(
                        &views,
                        i,
                        device,
                        uniform_buffer,
                        placeholder,
                        audio,
                        particle_velocity,
                        backdrop,
                    )
                })
                .collect()
        };
        for (pass, bg) in self.passes.iter_mut().zip(new_groups) {
            pass.bind_groups = bg;
        }
    }

    /// Install (or clear) the particle system. If any pass samples
    /// `@particles.velocity`, the bind groups are rebuilt so that slot points
    /// at the new system's velocity texture instead of the placeholder (#1482)
    /// — construction can't do it because the particle system only exists
    /// after `PassExecutor::new`.
    pub fn set_particle_system(
        &mut self,
        ps: Option<ParticleSystem>,
        device: &Device,
        uniform_buffer: &UniformBuffer,
        placeholder: &PlaceholderTexture,
        audio: &AudioTextures,
    ) {
        self.particle_system = ps;
        let uses_velocity = self.passes.iter().any(|p| {
            p.input_srcs
                .iter()
                .any(|s| matches!(s, InputSrc::ParticleVelocity))
        });
        if uses_velocity {
            self.rebuild_all_bind_groups(device, uniform_buffer, placeholder, audio);
        }
    }

    /// Try to recompile a specific pass's shader (for hot-reload).
    /// NOTE: This blocks the main thread during compilation. Prefer using
    /// `ShaderCompiler` for background compilation + `swap_pass_pipeline()`.
    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)]
    pub fn recompile_pass(
        &mut self,
        pass_index: usize,
        device: &Device,
        hdr_format: TextureFormat,
        source: &str,
        uniform_buffer: &UniformBuffer,
        placeholder: &PlaceholderTexture,
        audio: &AudioTextures,
        pipeline_cache: Option<&wgpu::PipelineCache>,
    ) -> Result<(), String> {
        if pass_index >= self.passes.len() {
            return Err(format!("Pass index {pass_index} out of range"));
        }
        // recreate_pipeline reuses the existing layout (same input_count), so the
        // rebuilt bind groups stay valid.
        self.passes[pass_index].pipeline.recreate_pipeline(
            device,
            hdr_format,
            source,
            pipeline_cache,
        )?;
        self.rebuild_all_bind_groups(device, uniform_buffer, placeholder, audio);
        Ok(())
    }

    /// Swap in a pre-compiled pipeline for a specific pass (used after background compilation).
    /// Recreates bind groups to match the new pipeline's layout.
    pub fn swap_pass_pipeline(
        &mut self,
        pass_index: usize,
        pipeline: ShaderPipeline,
        device: &Device,
        uniform_buffer: &UniformBuffer,
        placeholder: &PlaceholderTexture,
        audio: &AudioTextures,
    ) -> Result<(), String> {
        if pass_index >= self.passes.len() {
            return Err(format!("Pass index {pass_index} out of range"));
        }
        // Install the new pipeline first so the rebuild reads its layout.
        self.passes[pass_index].pipeline = pipeline;
        self.rebuild_all_bind_groups(device, uniform_buffer, placeholder, audio);
        Ok(())
    }
}

/// Build the `[BindGroup; 2]` (one per global flip parity) for `views[i]`.
///
/// For parity `g`, the group binds: this pass's own previous frame (feedback →
/// the *other* target `targets[1-g]`; non-feedback → the 1x1 placeholder), the
/// three A17 audio textures + sampler, then each declared input pass `P` at
/// `P.targets[P.has_feedback ? g : 0]` — i.e. the target `P` writes this frame.
/// `particle_velocity` fills `@particles.velocity` slots; when `None` (no
/// particle system yet, or no velocity field) the placeholder is bound and
/// reads as zero velocity.
#[allow(clippy::too_many_arguments)]
fn build_bind_groups(
    views: &[PassView],
    i: usize,
    device: &Device,
    uniform_buffer: &UniformBuffer,
    placeholder: &PlaceholderTexture,
    audio: &AudioTextures,
    particle_velocity: Option<(&TextureView, &Sampler)>,
    backdrop: Option<(&TextureView, &Sampler)>,
) -> [wgpu::BindGroup; 2] {
    let view = &views[i];
    let layout = view.layout;
    let waveform_view = &audio.waveform_view;
    let spectrum_view = &audio.spectrum_view;
    let spectrogram_view = &audio.spectrogram_view;
    let audio_sampler = &audio.sampler;

    let make = |g: usize| -> wgpu::BindGroup {
        // Own previous frame.
        let (prev_view, prev_sampler): (&TextureView, &Sampler) = if view.has_feedback {
            let other = &view.target.targets[1 - g];
            (&other.view, &other.sampler)
        } else {
            (&placeholder.view, &placeholder.sampler)
        };

        // Declared inputs → each source pass's target. Current-frame inputs read the
        // target the source writes THIS frame (targets[g] if feedback, else targets[0]);
        // prev-frame inputs read the source feedback pass's OTHER target, targets[1-g],
        // which still holds last frame's output when this pass executes (#1481).
        let input_refs: Vec<(&TextureView, &Sampler)> = view
            .input_srcs
            .iter()
            .map(|&src| match src {
                InputSrc::Pass { pass, prev } => {
                    let sp = &views[pass];
                    let ti = if prev {
                        1 - g
                    } else if sp.has_feedback {
                        g
                    } else {
                        0
                    };
                    let rt = &sp.target.targets[ti];
                    (&rt.view, &rt.sampler)
                }
                InputSrc::ParticleVelocity => {
                    particle_velocity.unwrap_or((&placeholder.view, &placeholder.sampler))
                }
                InputSrc::Backdrop => backdrop.unwrap_or((&placeholder.view, &placeholder.sampler)),
            })
            .collect();

        uniform_buffer.create_bind_group(
            device,
            layout,
            prev_view,
            prev_sampler,
            waveform_view,
            spectrum_view,
            spectrogram_view,
            audio_sampler,
            &input_refs,
        )
    };

    [make(0), make(1)]
}

#[cfg(test)]
mod tests;
