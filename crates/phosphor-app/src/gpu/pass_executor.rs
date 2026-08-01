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
    /// `viewport`: optional (width, height) to restrict rendering to a sub-region.
    pub fn execute(
        &self,
        encoder: &mut CommandEncoder,
        uniform_buffer: &UniformBuffer,
        queue: &Queue,
        uniforms: &super::ShaderUniforms,
    ) -> &RenderTarget {
        uniform_buffer.update(queue, uniforms);

        // 1. Particle compute dispatch (before fragment passes)
        if let Some(ref ps) = self.particle_system {
            ps.dispatch(encoder, queue);
        }

        // 2. Fragment shader passes
        for pass in &self.passes {
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
            ps.render(encoder, queue, &final_target.view);
        }

        final_target
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
mod tests {
    use super::*;
    use crate::gpu::frame_capture::FrameCapture;
    use crate::gpu::fullscreen_quad::FULLSCREEN_TRIANGLE_VS;
    use crate::gpu::test_gpu::{gpu_guard, test_gpu};

    const FMT: TextureFormat = TextureFormat::Rgba8Unorm;

    // Pass A: a self-feedback accumulator that adds 0.25 to its previous value.
    const FRAG_ACCUM: &str = "@fragment\n\
        fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {\n\
            let prev = feedback(vec2f(0.5, 0.5)).r;\n\
            return vec4f(prev + 0.25, 0.0, 0.0, 1.0);\n\
        }";
    // Pass B: reads pass A's output (input0) and returns its complement.
    const FRAG_INVERT: &str = "@fragment\n\
        fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {\n\
            let a = input0(vec2f(0.5, 0.5)).r;\n\
            return vec4f(1.0 - a, 0.0, 0.0, 1.0);\n\
        }";
    // Passthrough: echo input0 (used to observe a prev-frame input directly).
    const FRAG_ECHO0: &str = "@fragment\n\
        fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {\n\
            return vec4f(input0(vec2f(0.5, 0.5)).r, 0.0, 0.0, 1.0);\n\
        }";
    // Self-feedback accumulator, +0.1 per invocation (for the iterations loop).
    const FRAG_STEP: &str = "@fragment\n\
        fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {\n\
            let prev = feedback(vec2f(0.5, 0.5)).r;\n\
            return vec4f(prev + 0.1, 0.0, 0.0, 1.0);\n\
        }";

    /// A minimal blit pipeline: `textureLoad` the source at the fragment position
    /// and write it out. Lets a probe pull an executor target (which is only
    /// TEXTURE_BINDING) into a FrameCapture texture (which is COPY_SRC).
    fn blit_pipeline(device: &Device) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("probe-blit-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("probe-blit-pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let src = format!(
            "{FULLSCREEN_TRIANGLE_VS}\n\
             @group(0) @binding(0) var src_tex: texture_2d<f32>;\n\
             @fragment fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {{\n\
                 return textureLoad(src_tex, vec2i(i32(pos.x), i32(pos.y)), 0);\n\
             }}"
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("probe-blit"),
            source: wgpu::ShaderSource::Wgsl(src.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("probe-blit-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FMT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        (pipeline, bgl)
    }

    /// Assemble a `PassExecutor` from pre-built pipelines without touching disk —
    /// the same two-phase bind-group build `PassExecutor::new` performs.
    /// One `assemble` pass spec: (name, pipeline, feedback, input srcs, iterations, scale).
    type PassSpec<'a> = (&'a str, ShaderPipeline, bool, Vec<InputSrc>, u32, f32);

    #[allow(clippy::too_many_arguments)]
    fn assemble(
        device: &Device,
        queue: &Queue,
        w: u32,
        h: u32,
        fmt: TextureFormat,
        ubuf: &UniformBuffer,
        placeholder: &PlaceholderTexture,
        audio: &AudioTextures,
        specs: Vec<PassSpec>,
    ) -> PassExecutor {
        let prepared: Vec<PreparedPass> = specs
            .into_iter()
            .map(
                |(name, pipeline, feedback, input_srcs, iterations, scale)| {
                    let target = if feedback {
                        PingPongTarget::new_cleared(device, queue, w, h, fmt, scale)
                    } else {
                        PingPongTarget::new(device, w, h, fmt, scale)
                    };
                    PreparedPass {
                        name: name.to_string(),
                        pipeline,
                        target,
                        has_feedback: feedback,
                        input_srcs,
                        iterations,
                    }
                },
            )
            .collect();
        let bind_groups: Vec<[wgpu::BindGroup; 2]> = {
            let views: Vec<PassView> = prepared
                .iter()
                .map(|p| PassView {
                    layout: &p.pipeline.bind_group_layout,
                    target: &p.target,
                    has_feedback: p.has_feedback,
                    input_srcs: &p.input_srcs,
                })
                .collect();
            (0..views.len())
                .map(|i| build_bind_groups(&views, i, device, ubuf, placeholder, audio, None, None))
                .collect()
        };
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
        PassExecutor {
            passes,
            particle_system: None,
            backdrop: None,
            flip_parity: 0,
        }
    }

    // An input naming a pass that was not declared earlier is a hard error, caught
    // before any shader is loaded (so `shader` never has to resolve on disk).
    #[test]
    #[ignore = "requires a wgpu adapter"]
    fn passgraph_rejects_unknown_input() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let loader = EffectLoader::for_test("");
        let ubuf = UniformBuffer::new(&device);
        let placeholder = PlaceholderTexture::new(&device, &queue, FMT);
        let audio = AudioTextures::new(&device, &queue);

        let passes = vec![PassDef {
            name: "b".into(),
            shader: "unused.wgsl".into(),
            scale: 1.0,
            inputs: vec!["missing".into()],
            prev_inputs: vec![],
            iterations: 1,
            feedback: false,
        }];
        let res = PassExecutor::new(
            &device,
            FMT,
            4,
            4,
            &passes,
            &loader,
            &ubuf,
            &placeholder,
            &audio,
            &queue,
            None,
        );
        let err = res.err().expect("unknown input must be rejected");
        assert!(
            err.contains("missing"),
            "error should name the bad input: {err}"
        );
    }

    // Special-input names (#1482): an unknown `@` name must be rejected with an
    // error that names it, before any shader is loaded.
    #[test]
    #[ignore = "requires a wgpu adapter"]
    fn passgraph_rejects_unknown_special_input() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let loader = EffectLoader::for_test("");
        let ubuf = UniformBuffer::new(&device);
        let placeholder = PlaceholderTexture::new(&device, &queue, FMT);
        let audio = AudioTextures::new(&device, &queue);

        let passes = vec![PassDef {
            name: "p".into(),
            shader: "unused.wgsl".into(),
            scale: 1.0,
            inputs: vec!["@particles.bogus".into()],
            prev_inputs: vec![],
            iterations: 1,
            feedback: false,
        }];
        let err = PassExecutor::new(
            &device,
            FMT,
            4,
            4,
            &passes,
            &loader,
            &ubuf,
            &placeholder,
            &audio,
            &queue,
            None,
        )
        .err()
        .expect("unknown special input must be rejected");
        assert!(
            err.contains("@particles.bogus") && err.contains("special"),
            "error should name the bad special input: {err}"
        );
    }

    // A special input has no previous frame — naming one in `prev_inputs` is an error.
    #[test]
    #[ignore = "requires a wgpu adapter"]
    fn passgraph_rejects_special_prev_input() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let loader = EffectLoader::for_test("");
        let ubuf = UniformBuffer::new(&device);
        let placeholder = PlaceholderTexture::new(&device, &queue, FMT);
        let audio = AudioTextures::new(&device, &queue);

        let passes = vec![PassDef {
            name: "p".into(),
            shader: "unused.wgsl".into(),
            scale: 1.0,
            inputs: vec![],
            prev_inputs: vec![PARTICLE_VELOCITY_INPUT.into()],
            iterations: 1,
            feedback: true,
        }];
        let err = PassExecutor::new(
            &device,
            FMT,
            4,
            4,
            &passes,
            &loader,
            &ubuf,
            &placeholder,
            &audio,
            &queue,
            None,
        )
        .err()
        .expect("special prev_input must be rejected");
        assert!(
            err.contains("previous frame"),
            "error should explain special inputs have no previous frame: {err}"
        );
    }

    // `@particles.velocity` must RESOLVE (not error as an unknown input): with a
    // nonexistent shader path the constructor must get past input resolution and
    // fail at shader loading instead.
    #[test]
    #[ignore = "requires a wgpu adapter"]
    fn passgraph_particle_velocity_resolves() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let loader = EffectLoader::for_test("");
        let ubuf = UniformBuffer::new(&device);
        let placeholder = PlaceholderTexture::new(&device, &queue, FMT);
        let audio = AudioTextures::new(&device, &queue);

        let passes = vec![PassDef {
            name: "p".into(),
            shader: "definitely_missing_shader.wgsl".into(),
            scale: 1.0,
            inputs: vec![PARTICLE_VELOCITY_INPUT.into()],
            prev_inputs: vec![],
            iterations: 1,
            feedback: true,
        }];
        let err = PassExecutor::new(
            &device,
            FMT,
            4,
            4,
            &passes,
            &loader,
            &ubuf,
            &placeholder,
            &audio,
            &queue,
            None,
        )
        .err()
        .expect("missing shader must fail");
        assert!(
            err.contains("Failed to load shader"),
            "@particles.velocity should resolve and the failure move on to shader \
             loading; instead got: {err}"
        );
    }

    // Two-pass probe of the multi-input pass graph (#1481):
    //   pass A  — self-feedback accumulator (prev + 0.25)
    //   pass B  — NON-feedback, reads A as input0, returns 1 - A
    // Frame 1 proves cross-pass sampling works at all (B sees A's 0.25 → 0.75).
    // Frame 2, after a flip, proves the GLOBAL flip parity: A now writes its other
    // ping-pong target (0.50), and B — though its own `current` never moved — must
    // still read A's fresh target (→ 0.50). The pre-fix code, indexing B's bind
    // group by B's own `current`, would read A's stale target and yield ~0.75.
    #[test]
    #[ignore = "requires a wgpu adapter; renders offscreen"]
    fn passgraph_cross_pass_and_parity() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let loader = EffectLoader::for_test("");
        let (w, h) = (4u32, 4u32);

        let ubuf = UniformBuffer::new(&device);
        let placeholder = PlaceholderTexture::new(&device, &queue, FMT);
        let audio = AudioTextures::new(&device, &queue);

        let pipe_a = ShaderPipeline::new(
            &device,
            FMT,
            &loader.prepend_library_with_inputs(FRAG_ACCUM, 0),
            None,
            0,
        )
        .expect("pass A pipeline");
        let pipe_b = ShaderPipeline::new(
            &device,
            FMT,
            &loader.prepend_library_with_inputs(FRAG_INVERT, 1),
            None,
            1,
        )
        .expect("pass B pipeline");

        let mut executor = assemble(
            &device,
            &queue,
            w,
            h,
            FMT,
            &ubuf,
            &placeholder,
            &audio,
            vec![
                ("A", pipe_a, true, vec![], 1, 1.0),
                (
                    "B",
                    pipe_b,
                    false,
                    vec![InputSrc::Pass {
                        pass: 0,
                        prev: false,
                    }],
                    1,
                    1.0,
                ),
            ],
        );

        let mut uniforms = crate::gpu::ShaderUniforms::zeroed();
        uniforms.resolution = [w as f32, h as f32];

        let (blit, blit_bgl) = blit_pipeline(&device);

        let read_final_red = |executor: &PassExecutor| -> f32 {
            let mut fc = FrameCapture::new(&device, w, h, FMT, "probe-cap");
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let final_rt = executor.execute(&mut enc, &ubuf, &queue, &uniforms);
                let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("probe-blit-bg"),
                    layout: &blit_bgl,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&final_rt.view),
                    }],
                });
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("probe-blit-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &fc.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&blit);
                pass.set_bind_group(0, &bg, &[]);
                pass.draw(0..3, 0..1);
            }
            fc.copy_to_staging(&mut enc);
            queue.submit([enc.finish()]);
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            fc.request_map();
            let data = loop {
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                if let Some(d) = fc.take_mapped_data(&device) {
                    break d;
                }
            };
            data[0] as f32 / 255.0
        };

        let b1 = read_final_red(&executor);
        executor.flip();
        let b2 = read_final_red(&executor);

        // Frame 1: B saw A's first value (0.25) → 0.75. Cross-pass sampling works.
        assert!(
            (0.65..0.85).contains(&b1),
            "frame 1: B should read A=0.25 and output ~0.75, got {b1:.3}"
        );
        // Frame 2: global parity picked A's freshly-written target (0.50) → 0.50.
        // A stale read would land near 0.75, so the upper bound is the real guard.
        assert!(
            (0.40..0.60).contains(&b2),
            "frame 2: B should read A's flipped target (0.50) and output ~0.50, \
             got {b2:.3} (a value near 0.75 means the parity fix regressed)"
        );
    }

    /// Execute the graph, then blit `executor.passes[pass_idx]`'s freshly-written
    /// target into a FrameCapture and read back the full RGBA8 bytes. Unlike the
    /// parity test's `read_final_red`, this inspects an arbitrary pass (not just the
    /// last), which the prev-input and Sumi probes need.
    #[allow(clippy::too_many_arguments)]
    fn capture_pass_rgba(
        device: &Device,
        queue: &Queue,
        ubuf: &UniformBuffer,
        blit: &wgpu::RenderPipeline,
        blit_bgl: &wgpu::BindGroupLayout,
        executor: &PassExecutor,
        uniforms: &crate::gpu::ShaderUniforms,
        pass_idx: usize,
        w: u32,
        h: u32,
    ) -> Vec<u8> {
        let mut fc = FrameCapture::new(device, w, h, FMT, "probe-cap");
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let _ = executor.execute(&mut enc, ubuf, queue, uniforms);
            let src = &executor.passes[pass_idx].target.write_target().view;
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("probe-blit-bg"),
                layout: blit_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                }],
            });
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("probe-blit-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &fc.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(blit);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
        fc.copy_to_staging(&mut enc);
        queue.submit([enc.finish()]);
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        fc.request_map();
        loop {
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            if let Some(d) = fc.take_mapped_data(device) {
                break d;
            }
        }
    }

    /// `capture_pass_rgba` reduced to a single pixel's red channel (0..1).
    #[allow(clippy::too_many_arguments)]
    fn render_pass_red(
        device: &Device,
        queue: &Queue,
        ubuf: &UniformBuffer,
        blit: &wgpu::RenderPipeline,
        blit_bgl: &wgpu::BindGroupLayout,
        executor: &PassExecutor,
        uniforms: &crate::gpu::ShaderUniforms,
        pass_idx: usize,
        w: u32,
        h: u32,
    ) -> f32 {
        let data = capture_pass_rgba(
            device, queue, ubuf, blit, blit_bgl, executor, uniforms, pass_idx, w, h,
        );
        data[0] as f32 / 255.0
    }

    // Previous-frame cross-pass input (#1481): pass 0 "reader" samples pass 1 "gen"'s
    // PREVIOUS frame via prev_inputs — a *forward* reference (gen is declared later),
    // legal only for prev inputs. gen is a +0.25 accumulator (0.25, 0.50, 0.75, …).
    // reader echoes gen's prior frame, so it must LAG by one: {0.00, 0.25, 0.50}. Had
    // the bind resolved to gen's current frame, reader would read {0.25, 0.50, 0.75}.
    #[test]
    #[ignore = "requires a wgpu adapter; renders offscreen"]
    fn passgraph_prev_input_reads_previous_frame() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let loader = EffectLoader::for_test("");
        let (w, h) = (4u32, 4u32);

        let ubuf = UniformBuffer::new(&device);
        let placeholder = PlaceholderTexture::new(&device, &queue, FMT);
        let audio = AudioTextures::new(&device, &queue);

        let pipe_reader = ShaderPipeline::new(
            &device,
            FMT,
            &loader.prepend_library_with_inputs(FRAG_ECHO0, 1),
            None,
            1,
        )
        .expect("reader pipeline");
        let pipe_gen = ShaderPipeline::new(
            &device,
            FMT,
            &loader.prepend_library_with_inputs(FRAG_ACCUM, 0),
            None,
            0,
        )
        .expect("gen pipeline");

        // Declaration order: reader (0) then gen (1). reader's only input is gen's
        // previous frame — a forward reference that only prev_inputs allow.
        let mut executor = assemble(
            &device,
            &queue,
            w,
            h,
            FMT,
            &ubuf,
            &placeholder,
            &audio,
            vec![
                (
                    "reader",
                    pipe_reader,
                    false,
                    vec![InputSrc::Pass {
                        pass: 1,
                        prev: true,
                    }],
                    1,
                    1.0,
                ),
                ("gen", pipe_gen, true, vec![], 1, 1.0),
            ],
        );

        let mut uniforms = crate::gpu::ShaderUniforms::zeroed();
        uniforms.resolution = [w as f32, h as f32];
        let (blit, blit_bgl) = blit_pipeline(&device);

        let read = |ex: &PassExecutor| {
            render_pass_red(
                &device, &queue, &ubuf, &blit, &blit_bgl, ex, &uniforms, 0, w, h,
            )
        };

        let f1 = read(&executor);
        executor.flip();
        let f2 = read(&executor);
        executor.flip();
        let f3 = read(&executor);

        assert!(
            f1 < 0.1,
            "frame 1: gen had no prior frame, reader ~0.0, got {f1:.3}"
        );
        assert!(
            (0.15..0.35).contains(&f2),
            "frame 2: reader should lag to gen's frame-1 value (0.25), got {f2:.3} \
             (~0.50 means it read the current frame, not the previous)"
        );
        assert!(
            (0.40..0.60).contains(&f3),
            "frame 3: reader should lag to gen's frame-2 value (0.50), got {f3:.3}"
        );
    }

    // Iterations (#1481): a single feedback pass with `iterations: 5` and a +0.1
    // self-accumulator must run five draws per frame, ping-ponging its own target, and
    // leave the fifth result in targets[flip_parity] (what the reader/warm-start read).
    // Frame 1 from cleared 0 → 0.5; frame 2 warm-starts from 0.5 → 1.0.
    #[test]
    #[ignore = "requires a wgpu adapter; renders offscreen"]
    fn passgraph_iterations_accumulate_within_frame() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let loader = EffectLoader::for_test("");
        let (w, h) = (4u32, 4u32);

        let ubuf = UniformBuffer::new(&device);
        let placeholder = PlaceholderTexture::new(&device, &queue, FMT);
        let audio = AudioTextures::new(&device, &queue);

        let pipe = ShaderPipeline::new(
            &device,
            FMT,
            &loader.prepend_library_with_inputs(FRAG_STEP, 0),
            None,
            0,
        )
        .expect("step pipeline");

        let mut executor = assemble(
            &device,
            &queue,
            w,
            h,
            FMT,
            &ubuf,
            &placeholder,
            &audio,
            vec![("acc", pipe, true, vec![], 5, 1.0)],
        );

        let mut uniforms = crate::gpu::ShaderUniforms::zeroed();
        uniforms.resolution = [w as f32, h as f32];
        let (blit, blit_bgl) = blit_pipeline(&device);
        let read = |ex: &PassExecutor| {
            render_pass_red(
                &device, &queue, &ubuf, &blit, &blit_bgl, ex, &uniforms, 0, w, h,
            )
        };

        let f1 = read(&executor);
        executor.flip();
        let f2 = read(&executor);

        // 5 × 0.1 from a cleared start. A single draw would give 0.1.
        assert!(
            (0.45..0.55).contains(&f1),
            "frame 1: 5 iterations of +0.1 should reach ~0.5, got {f1:.3} \
             (~0.1 means the loop ran once)"
        );
        // Warm-started from frame 1's 0.5, five more steps → ~1.0.
        assert!(
            (0.95..1.01).contains(&f2),
            "frame 2: warm-started 0.5 + 5×0.1 should reach ~1.0, got {f2:.3}"
        );
    }

    // End-to-end probe of the real Sumi stable-fluids graph (#1481) through the actual
    // PassExecutor: divergence(prev velocity) → pressure×24 → velocity(project+advect+
    // forces) → dye. Injects colored onset splats for the first ~20 frames, then coasts
    // with buoyancy for ~140 more. Captures the dye pass early (just after injection) and
    // late, and asserts the ink is present, not blown out, and actually MOVED — a dead
    // sim (static splat) would leave the two frames identical.
    // Run: SUMI_PNG_DIR=/tmp cargo test -p phosphor-app --release -- --ignored sumi_render_previews
    #[test]
    #[ignore = "requires a wgpu adapter; renders offscreen, writes PNGs"]
    fn sumi_render_previews() {
        let out_dir = std::env::var("SUMI_PNG_DIR").ok();
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();

        // Real production preamble: uniform block + libs + injected input bindings.
        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let sdf = include_str!("../../../../assets/shaders/lib/sdf.wgsl");
        let tonemap = include_str!("../../../../assets/shaders/lib/tonemap.wgsl");
        let loader = EffectLoader::for_test(&format!("{noise}\n{palette}\n{sdf}\n{tonemap}"));
        let fmt = TextureFormat::Rgba16Float;
        // 16:9 so the probe reproduces the real window's aspect (a square target hides
        // whether the injection ring fills a wide frame).
        let (w, h) = (480u32, 270u32);

        let ubuf = UniformBuffer::new(&device);
        let placeholder = PlaceholderTexture::new(&device, &queue, fmt);
        let audio = AudioTextures::new(&device, &queue);

        let mk = |shader: &str, count: usize| {
            ShaderPipeline::new(
                &device,
                fmt,
                &loader.prepend_library_with_inputs(shader, count),
                None,
                count,
            )
            .expect("sumi pass pipeline")
        };
        let pipe_div = mk(
            include_str!("../../../../assets/shaders/sumi_divergence.wgsl"),
            1,
        );
        let pipe_pres = mk(
            include_str!("../../../../assets/shaders/sumi_pressure.wgsl"),
            1,
        );
        let pipe_vel = mk(
            include_str!("../../../../assets/shaders/sumi_velocity.wgsl"),
            2,
        );
        let pipe_dye = mk(include_str!("../../../../assets/shaders/sumi_dye.wgsl"), 1);

        // Same wiring as sumi.pfx: passes 0..3 = divergence, pressure, velocity, dye.
        let src = |pass: usize, prev: bool| InputSrc::Pass { pass, prev };
        let mut executor = assemble(
            &device,
            &queue,
            w,
            h,
            fmt,
            &ubuf,
            &placeholder,
            &audio,
            vec![
                ("divergence", pipe_div, false, vec![src(2, true)], 1, 0.5),
                ("pressure", pipe_pres, true, vec![src(0, false)], 24, 0.5),
                (
                    "velocity",
                    pipe_vel,
                    true,
                    vec![src(1, false), src(3, true)],
                    1,
                    0.5,
                ),
                ("dye", pipe_dye, true, vec![src(2, false)], 1, 1.0),
            ],
        );
        let dye_idx = 3;

        let (blit, blit_bgl) = blit_pipeline(&device);

        let mut u = crate::gpu::ShaderUniforms::zeroed();
        u.resolution = [w as f32, h as f32];
        u.delta_time = 1.0 / 60.0;
        // Sumi param defaults (see sumi.pfx), indices 0..9.
        // Sumi param defaults (see sumi.pfx), indices 0..9.
        for (i, v) in [0.5, 0.5, 0.55, 0.45, 0.7, 0.5, 0.6, 0.5, 0.7, 0.55]
            .into_iter()
            .enumerate()
        {
            u.params[i] = v;
        }

        // Luminance stats over an RGBA8 frame: (mean 0..1, centroid uv, coverage) where
        // coverage is the fraction of pixels lit above a small threshold — the "fills the
        // screen" measure.
        let stats = |data: &[u8]| -> (f64, f64, f64, f64) {
            let (mut sum, mut sx, mut sy, mut lit) = (0.0f64, 0.0f64, 0.0f64, 0u32);
            for y in 0..h {
                for x in 0..w {
                    let i = ((y * w + x) * 4) as usize;
                    let l = 0.299 * data[i] as f64
                        + 0.587 * data[i + 1] as f64
                        + 0.114 * data[i + 2] as f64;
                    sum += l;
                    sx += l * x as f64;
                    sy += l * y as f64;
                    if l > 8.0 {
                        lit += 1;
                    }
                }
            }
            let mean = sum / (w * h) as f64 / 255.0;
            let coverage = lit as f64 / (w * h) as f64;
            if sum > 0.0 {
                (mean, sx / sum / w as f64, sy / sum / h as f64, coverage)
            } else {
                (mean, 0.5, 0.5, coverage)
            }
        };

        const FRAMES: u32 = 96;
        const EARLY: u32 = 24;
        const LATE: u32 = 92;
        let mut early: Vec<u8> = Vec::new();
        let mut late: Vec<u8> = Vec::new();

        for f in 0..FRAMES {
            u.time = f as f32 / 60.0;
            u.frame_index = f as f32;
            // Onsets fire periodically (as real music does), so the LATE frame measures
            // the steady-state fill, not a single coasting burst.
            u.onset = if f % 6 == 0 { 1.0 } else { 0.0 };
            u.beat = if f % 12 == 0 { 1.0 } else { 0.0 };
            u.bass = 0.7; // LOUD — buoyancy must stay a drift, not surge the bottom over the top
            u.flux = 0.6; // vorticity confinement
            u.dominant_chroma = 0.0; // C
            // Broad chroma so all twelve ring sites inject; every class stays lit.
            u.chroma = [0.6; 12];

            if f == EARLY || f == LATE {
                let data = capture_pass_rgba(
                    &device, &queue, &ubuf, &blit, &blit_bgl, &executor, &u, dye_idx, w, h,
                );
                if f == EARLY {
                    early = data;
                } else {
                    late = data;
                }
            } else {
                let mut enc = device.create_command_encoder(&Default::default());
                let _ = executor.execute(&mut enc, &ubuf, &queue, &u);
                queue.submit([enc.finish()]);
            }
            executor.flip();
        }

        let (em, _ex, _ey, ec) = stats(&early);
        let (lm, _lx, _ly, lc) = stats(&late);

        if let Some(dir) = out_dir {
            for (name, data) in [("early", &early), ("late", &late)] {
                let path = format!("{dir}/sumi_{name}.png");
                image::RgbaImage::from_raw(w, h, data.clone())
                    .expect("raw->image")
                    .save(&path)
                    .expect("save png");
                eprintln!("wrote {path}");
            }
        }
        // Fraction of near-white pixels — a saturated wash blows this up.
        let hot = late
            .chunks_exact(4)
            .filter(|p| p[0] > 220 && p[1] > 220 && p[2] > 220)
            .count() as f64
            / (w * h) as f64;
        eprintln!("early mean {em:.4} cover {ec:.3}; late mean {lm:.4} cover {lc:.3} hot {hot:.3}");

        // Ink present, and NOT a saturated wash (Kevin's blowout had mean ~0.6 and most of
        // the frame near-white with no fluid detail left).
        assert!(
            lm > 0.004,
            "late frame near-black (mean {lm:.4}) — ink died out"
        );
        assert!(lm < 0.5, "late frame blew out (mean {lm:.4})");
        assert!(
            hot < 0.15,
            "late frame is a saturated wash ({:.0}% near-white) — no fluid detail left",
            hot * 100.0
        );

        // Fills the frame: a good fraction of the 16:9 target is lit at steady state, not
        // a narrow central band. (The pre-fix aspect-squished ring covered ~10%.)
        assert!(
            lc > 0.25,
            "late frame covers only {:.0}% of the frame — ink is too localized",
            lc * 100.0
        );

        // Top/bottom balance under LOUD bass: the lower ring colours must not surge up and
        // dominate the upper half. Compare luminance in the top third vs the bottom third.
        let band_lum = |y0: u32, y1: u32| -> f64 {
            let mut s = 0.0f64;
            for y in y0..y1 {
                for x in 0..w {
                    let i = ((y * w + x) * 4) as usize;
                    s += 0.299 * late[i] as f64
                        + 0.587 * late[i + 1] as f64
                        + 0.114 * late[i + 2] as f64;
                }
            }
            s / ((y1 - y0) * w) as f64
        };
        let top = band_lum(0, h / 3);
        let bottom = band_lum(2 * h / 3, h);
        let ratio = (bottom + 1.0) / (top + 1.0);
        eprintln!("top-third lum {top:.2}, bottom-third lum {bottom:.2}, ratio {ratio:.2}");
        assert!(
            ratio < 3.0,
            "bottom third is {ratio:.1}x the top under loud bass — buoyancy is surging the \
             lower colours over the upper ring"
        );

        // The fluid must be LIVE: early and late differ substantially. A static splat
        // (dead advection/projection) would leave them near-identical.
        let mut sad = 0.0f64;
        for i in (0..early.len()).step_by(4) {
            let el =
                0.299 * early[i] as f64 + 0.587 * early[i + 1] as f64 + 0.114 * early[i + 2] as f64;
            let ll =
                0.299 * late[i] as f64 + 0.587 * late[i + 1] as f64 + 0.114 * late[i + 2] as f64;
            sad += (el - ll).abs();
        }
        let sad = sad / (w * h) as f64 / 255.0;
        assert!(
            sad > 0.01,
            "early and late frames are nearly identical (SAD {sad:.4}) — the fluid isn't moving"
        );
    }

    // End-to-end probe of the Protea Flow Lenia graph through the real PassExecutor:
    // potential(prev mass) → mass(reintegration transport + audio food) → display.
    // Phase 1 plays loud music (rms + onsets + beats) so the ecosystem bootstraps from
    // a cleared field; phase 2 goes silent so starvation must shrink it. Asserts the
    // creatures exist, are localized bodies (not a wash, not a uniform field), keep
    // moving, and that silence starves the mass the music grew.
    // Run: PROTEA_PNG_DIR=/tmp cargo test -p phosphor-app --release -- --ignored protea_render_previews
    #[test]
    #[ignore = "requires a wgpu adapter; renders offscreen, writes PNGs"]
    fn protea_render_previews() {
        let out_dir = std::env::var("PROTEA_PNG_DIR").ok();
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();

        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let sdf = include_str!("../../../../assets/shaders/lib/sdf.wgsl");
        let tonemap = include_str!("../../../../assets/shaders/lib/tonemap.wgsl");
        let loader = EffectLoader::for_test(&format!("{noise}\n{palette}\n{sdf}\n{tonemap}"));
        let fmt = TextureFormat::Rgba16Float;
        let (w, h) = (480u32, 270u32);

        let ubuf = UniformBuffer::new(&device);
        let placeholder = PlaceholderTexture::new(&device, &queue, fmt);
        let audio = AudioTextures::new(&device, &queue);

        let mk = |shader: &str, count: usize| {
            ShaderPipeline::new(
                &device,
                fmt,
                &loader.prepend_library_with_inputs(shader, count),
                None,
                count,
            )
            .expect("protea pass pipeline")
        };
        let pipe_pot = mk(
            include_str!("../../../../assets/shaders/protea_potential.wgsl"),
            1,
        );
        let pipe_mass = mk(
            include_str!("../../../../assets/shaders/protea_mass.wgsl"),
            1,
        );
        let pipe_disp = mk(
            include_str!("../../../../assets/shaders/protea_display.wgsl"),
            2,
        );

        // Same wiring as protea.pfx: passes 0..2 = potential, mass, display.
        let src = |pass: usize, prev: bool| InputSrc::Pass { pass, prev };
        let mut executor = assemble(
            &device,
            &queue,
            w,
            h,
            fmt,
            &ubuf,
            &placeholder,
            &audio,
            vec![
                ("potential", pipe_pot, false, vec![src(1, true)], 1, 0.5),
                ("mass", pipe_mass, true, vec![src(0, false)], 1, 0.5),
                (
                    "display",
                    pipe_disp,
                    true,
                    vec![src(1, false), src(0, false)],
                    1,
                    1.0,
                ),
            ],
        );
        let (mass_idx, disp_idx) = (1usize, 2usize);
        // Pass targets at scale 0.5.
        let (mw, mh) = (w / 2, h / 2);

        let (blit, blit_bgl) = blit_pipeline(&device);

        let mut u = crate::gpu::ShaderUniforms::zeroed();
        u.resolution = [w as f32, h as f32];
        u.delta_time = 1.0 / 60.0;
        // Protea param defaults (see protea.pfx), indices 0..9.
        for (i, v) in [0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.55, 0.55]
            .into_iter()
            .enumerate()
        {
            u.params[i] = v;
        }

        // Mean luminance (0..1) and lit coverage of an RGBA8 frame.
        let stats = |data: &[u8], fw: u32, fh: u32| -> (f64, f64) {
            let (mut sum, mut lit) = (0.0f64, 0u32);
            for p in data.chunks_exact(4) {
                let l = 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
                sum += l;
                if l > 8.0 {
                    lit += 1;
                }
            }
            (
                sum / (fw * fh) as f64 / 255.0,
                lit as f64 / (fw * fh) as f64,
            )
        };
        // Total channel sum of an RGBA8 mass capture — the starvation measure.
        let mass_sum = |data: &[u8]| -> f64 {
            data.chunks_exact(4)
                .map(|p| p[0] as f64 + p[1] as f64 + p[2] as f64)
                .sum()
        };

        // Run the loud phase long enough to reach STEADY STATE, not just first bloom: the
        // original probe stopped at ~4s and passed on the early coral look, but the live
        // failure is the plane-filling uniform speckle that only sets in after ~10s.
        const LOUD: u32 = 780;
        const SILENT: u32 = 240;
        let (mut mid, mut grown, mut starved): (Vec<u8>, Vec<u8>, Vec<u8>) =
            (Vec::new(), Vec::new(), Vec::new());
        let (mut mass_grown, mut mass_starved): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new());

        for f in 0..(LOUD + SILENT) {
            u.time = f as f32 / 60.0;
            u.frame_index = f as f32;
            let loud = f < LOUD;
            u.rms = if loud { 0.30 } else { 0.0 };
            u.bass = if loud { 0.6 } else { 0.0 };
            u.onset = if loud && f % 8 == 0 { 1.0 } else { 0.0 };
            u.beat_strength = if loud { 0.7 } else { 0.0 };
            u.chroma = if loud { [0.5; 12] } else { [0.0; 12] };

            let mut cap: Option<(&mut Vec<u8>, usize, u32, u32)> = None;
            if f == LOUD - 180 {
                cap = Some((&mut mid, disp_idx, w, h));
            } else if f == LOUD - 4 {
                cap = Some((&mut grown, disp_idx, w, h));
            } else if f == LOUD - 3 {
                cap = Some((&mut mass_grown, mass_idx, mw, mh));
            } else if f == LOUD + SILENT - 2 {
                cap = Some((&mut starved, disp_idx, w, h));
            } else if f == LOUD + SILENT - 1 {
                cap = Some((&mut mass_starved, mass_idx, mw, mh));
            }
            if let Some((slot, idx, cw, ch)) = cap {
                *slot = capture_pass_rgba(
                    &device, &queue, &ubuf, &blit, &blit_bgl, &executor, &u, idx, cw, ch,
                );
            } else {
                let mut enc = device.create_command_encoder(&Default::default());
                let _ = executor.execute(&mut enc, &ubuf, &queue, &u);
                queue.submit([enc.finish()]);
            }
            executor.flip();
        }

        if let Some(dir) = out_dir {
            for (name, data, fw, fh) in [
                ("mid", &mid, w, h),
                ("grown", &grown, w, h),
                ("starved", &starved, w, h),
                ("mass", &mass_grown, mw, mh),
            ] {
                let path = format!("{dir}/protea_{name}.png");
                image::RgbaImage::from_raw(fw, fh, data.clone())
                    .expect("raw->image")
                    .save(&path)
                    .expect("save png");
                eprintln!("wrote {path}");
            }
        }

        let (gm, gc) = stats(&grown, w, h);
        let (sm, _sc) = stats(&starved, w, h);
        let mg = mass_sum(&mass_grown);
        let ms = mass_sum(&mass_starved);
        let hot = grown
            .chunks_exact(4)
            .filter(|p| p[0] > 220 && p[1] > 220 && p[2] > 220)
            .count() as f64
            / (w * h) as f64;
        eprintln!(
            "grown mean {gm:.4} cover {gc:.3} hot {hot:.3}; starved mean {sm:.4}; \
             mass grown {mg:.0} → starved {ms:.0} ({:.2}x)",
            ms / mg.max(1.0)
        );

        // The music grew an ecosystem: visible, localized bodies — not black, not a
        // wash, not a uniform field.
        assert!(
            gm > 0.01,
            "grown frame near-black (mean {gm:.4}) — nothing bootstrapped"
        );
        assert!(gm < 0.5, "grown frame blew out (mean {gm:.4})");
        assert!(
            hot < 0.15,
            "grown frame is a saturated wash ({:.0}% near-white)",
            hot * 100.0
        );
        assert!(
            gc > 0.02,
            "grown coverage {gc:.3} — creatures should be present (near 0 = dead ecosystem)"
        );

        // Distinct creatures separated by empty water, NOT a plane-filling uniform speckle.
        // Split the steady-state frame into a 16x9 grid and take each block's mean luma:
        // distinct bodies with water between them leave many DARK blocks and a wide spread
        // of block means; the degenerate fill (live image #2) lights every block near the
        // same value — few/no dark blocks, low spread. This is the metric the original
        // probe lacked, so it green-lit an effect that filled the plane at steady state.
        let block_stats = |data: &[u8]| -> (f64, f64) {
            let (bx, by) = (16u32, 9u32);
            let mut means: Vec<f64> = Vec::with_capacity((bx * by) as usize);
            for byi in 0..by {
                for bxi in 0..bx {
                    let (x0, x1) = (bxi * w / bx, (bxi + 1) * w / bx);
                    let (y0, y1) = (byi * h / by, (byi + 1) * h / by);
                    let mut s = 0.0f64;
                    let mut n = 0u32;
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let i = ((y * w + x) * 4) as usize;
                            s += 0.299 * data[i] as f64
                                + 0.587 * data[i + 1] as f64
                                + 0.114 * data[i + 2] as f64;
                            n += 1;
                        }
                    }
                    means.push(s / n as f64 / 255.0);
                }
            }
            let mean = means.iter().sum::<f64>() / means.len() as f64;
            let var = means.iter().map(|m| (m - mean).powi(2)).sum::<f64>() / means.len() as f64;
            let dark = means.iter().filter(|&&m| m < 0.03).count() as f64 / means.len() as f64;
            (var.sqrt(), dark)
        };
        let (block_std, void_frac) = block_stats(&grown);
        eprintln!("steady-state block_std {block_std:.4} void_frac {void_frac:.3}");
        assert!(
            void_frac > 0.10,
            "steady state filled the plane — only {:.0}% of blocks are empty water \
             (the degenerate uniform fill); distinct creatures need voids between them",
            void_frac * 100.0
        );
        assert!(
            block_std > 0.03,
            "steady state is a uniform field (block_std {block_std:.4}) — no large-scale \
             structure, just even speckle everywhere"
        );

        // Alive: the field keeps moving at steady state (mid vs grown, 56 frames apart).
        let mut sad = 0.0f64;
        for i in (0..mid.len()).step_by(4) {
            let a = 0.299 * mid[i] as f64 + 0.587 * mid[i + 1] as f64 + 0.114 * mid[i + 2] as f64;
            let b =
                0.299 * grown[i] as f64 + 0.587 * grown[i + 1] as f64 + 0.114 * grown[i + 2] as f64;
            sad += (a - b).abs();
        }
        let sad = sad / (w * h) as f64 / 255.0;
        assert!(
            sad > 0.005,
            "mid and grown frames nearly identical (SAD {sad:.4}) — the ecosystem froze"
        );

        // Starvation: 4 seconds of silence must shed a large share of the mass the
        // music grew (the creatures visibly shrink).
        assert!(
            mg > 1000.0,
            "loud-phase mass sum {mg:.0} is tiny — feeding never took hold"
        );
        assert!(
            ms < 0.6 * mg,
            "silence kept {:.0}% of the mass — starvation isn't biting",
            100.0 * ms / mg.max(1.0)
        );
    }

    // Lumen (#1485) end-to-end through the real PassExecutor: scene → 6 radiance
    // cascades (one shared lumen_cascade.wgsl, level from its own size) → display.
    // Synthetic audio grows a lit scene; asserts it isn't black/blown, carries
    // light-and-shadow STRUCTURE (an occluder casts darkness — not a flat wash),
    // brightens on a kick, is coloured by chroma, and keeps moving. Silent low-res
    // proxy — real penumbra/god-ray feel needs a live review.
    // Run: LUMEN_PNG_DIR=/tmp cargo test -p phosphor-app --release -- --ignored lumen_render_previews
    #[test]
    #[ignore = "requires a wgpu adapter; renders offscreen, writes PNGs"]
    fn lumen_render_previews() {
        let out_dir = std::env::var("LUMEN_PNG_DIR").ok();
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();

        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let sdf = include_str!("../../../../assets/shaders/lib/sdf.wgsl");
        let tonemap = include_str!("../../../../assets/shaders/lib/tonemap.wgsl");
        let loader = EffectLoader::for_test(&format!("{noise}\n{palette}\n{sdf}\n{tonemap}"));
        let fmt = TextureFormat::Rgba16Float;
        let (w, h) = (480u32, 270u32);

        let ubuf = UniformBuffer::new(&device);
        let placeholder = PlaceholderTexture::new(&device, &queue, fmt);
        let audio = AudioTextures::new(&device, &queue);

        let mk = |shader: &str, count: usize| {
            ShaderPipeline::new(
                &device,
                fmt,
                &loader.prepend_library_with_inputs(shader, count),
                None,
                count,
            )
            .expect("lumen pass pipeline")
        };
        let scene_src = include_str!("../../../../assets/shaders/lumen_scene.wgsl");
        let casc_src = include_str!("../../../../assets/shaders/lumen_cascade.wgsl");
        let disp_src = include_str!("../../../../assets/shaders/lumen_display.wgsl");

        // Same wiring as lumen.pfx (indices: scene=0, cascade5..0 = 1..6, display=7).
        let src = |pass: usize, prev: bool| InputSrc::Pass { pass, prev };
        let mut executor = assemble(
            &device,
            &queue,
            w,
            h,
            fmt,
            &ubuf,
            &placeholder,
            &audio,
            vec![
                ("scene", mk(scene_src, 0), true, vec![], 1, 0.5),
                // Top cascade: inputs ["scene","scene"] — input1 is a placeholder the
                // shader ignores (level == MAX_LEVEL skips the merge).
                (
                    "cascade5",
                    mk(casc_src, 2),
                    true,
                    vec![src(0, false), src(0, false)],
                    1,
                    0.015625,
                ),
                (
                    "cascade4",
                    mk(casc_src, 2),
                    true,
                    vec![src(0, false), src(1, false)],
                    1,
                    0.03125,
                ),
                (
                    "cascade3",
                    mk(casc_src, 2),
                    true,
                    vec![src(0, false), src(2, false)],
                    1,
                    0.0625,
                ),
                (
                    "cascade2",
                    mk(casc_src, 2),
                    true,
                    vec![src(0, false), src(3, false)],
                    1,
                    0.125,
                ),
                (
                    "cascade1",
                    mk(casc_src, 2),
                    true,
                    vec![src(0, false), src(4, false)],
                    1,
                    0.25,
                ),
                (
                    "cascade0",
                    mk(casc_src, 2),
                    true,
                    vec![src(0, false), src(5, false)],
                    1,
                    0.5,
                ),
                (
                    "display",
                    mk(disp_src, 2),
                    true,
                    vec![src(6, false), src(0, false)],
                    1,
                    1.0,
                ),
            ],
        );
        let (disp_idx, casc0_idx, scene_idx) = (7usize, 6usize, 0usize);

        let (blit, blit_bgl) = blit_pipeline(&device);

        let mut u = crate::gpu::ShaderUniforms::zeroed();
        u.resolution = [w as f32, h as f32];
        u.delta_time = 1.0 / 60.0;
        // Lumen param defaults (see lumen.pfx), indices 0..9.
        for (i, v) in [0.5, 0.5, 0.5, 0.5, 0.5, 0.2, 0.5, 0.5, 0.6, 0.55]
            .into_iter()
            .enumerate()
        {
            u.params[i] = v;
        }

        // Mean luminance (0..1), lit coverage, and near-white fraction of an RGBA8 frame.
        let stats = |data: &[u8], fw: u32, fh: u32| -> (f64, f64, f64) {
            let (mut sum, mut lit, mut hot) = (0.0f64, 0u32, 0u32);
            for p in data.chunks_exact(4) {
                let l = 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
                sum += l;
                if l > 8.0 {
                    lit += 1;
                }
                if p[0] > 235 && p[1] > 235 && p[2] > 235 {
                    hot += 1;
                }
            }
            let n = (fw * fh) as f64;
            (sum / n / 255.0, lit as f64 / n, hot as f64 / n)
        };
        // Colourfulness: mean saturation (max-min)/max over pixels bright enough to read.
        let colorfulness = |data: &[u8]| -> f64 {
            let (mut s, mut n) = (0.0f64, 0u32);
            for p in data.chunks_exact(4) {
                let (r, g, b) = (p[0] as f64, p[1] as f64, p[2] as f64);
                let mx = r.max(g).max(b);
                let mn = r.min(g).min(b);
                if mx > 24.0 {
                    s += (mx - mn) / mx;
                    n += 1;
                }
            }
            if n == 0 { 0.0 } else { s / n as f64 }
        };
        // 16x9 block means → std (large-scale structure) and dark-block fraction (shadow/void).
        let block_stats = |data: &[u8]| -> (f64, f64) {
            let (bx, by) = (16u32, 9u32);
            let mut means: Vec<f64> = Vec::with_capacity((bx * by) as usize);
            for byi in 0..by {
                for bxi in 0..bx {
                    let (x0, x1) = (bxi * w / bx, (bxi + 1) * w / bx);
                    let (y0, y1) = (byi * h / by, (byi + 1) * h / by);
                    let (mut sm, mut n) = (0.0f64, 0u32);
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let i = ((y * w + x) * 4) as usize;
                            sm += 0.299 * data[i] as f64
                                + 0.587 * data[i + 1] as f64
                                + 0.114 * data[i + 2] as f64;
                            n += 1;
                        }
                    }
                    means.push(sm / n as f64 / 255.0);
                }
            }
            let mean = means.iter().sum::<f64>() / means.len() as f64;
            let var = means.iter().map(|m| (m - mean).powi(2)).sum::<f64>() / means.len() as f64;
            let dark = means.iter().filter(|&&m| m < 0.02).count() as f64 / means.len() as f64;
            (var.sqrt(), dark)
        };

        // Onsets/kicks every 16 frames; a spread of chroma so many pitch classes light up.
        const N: u32 = 160;
        let mut prev_f: Vec<u8> = Vec::new();
        let mut lit: Vec<u8> = Vec::new();
        let mut flash: Vec<u8> = Vec::new();
        let mut casc: Vec<u8> = Vec::new();
        let mut scn: Vec<u8> = Vec::new();

        for f in 0..N {
            u.time = f as f32 / 60.0;
            u.frame_index = f as f32;
            let onset = f % 16 == 0;
            u.onset = if onset { 1.0 } else { 0.0 };
            u.kick = if onset { 1.0 } else { 0.0 };
            u.beat_strength = 0.6;
            u.rms = 0.30;
            u.bass = 0.5;
            u.presence = 0.4;
            u.brilliance = 0.35;
            u.flatness = 0.2;
            for c in 0..12usize {
                u.chroma[c] = 0.25 + 0.5 * (((c * 5) % 12) as f32 / 12.0);
            }

            // prev/lit are both between onsets (motion); flash is on an onset (kick response).
            let mut cap: Option<(&mut Vec<u8>, usize, u32, u32)> = None;
            if f == 130 {
                cap = Some((&mut prev_f, disp_idx, w, h));
            } else if f == 136 {
                cap = Some((&mut lit, disp_idx, w, h));
            } else if f == 144 {
                cap = Some((&mut flash, disp_idx, w, h));
            } else if f == 145 {
                cap = Some((&mut casc, casc0_idx, w / 2, h / 2));
            } else if f == 146 {
                cap = Some((&mut scn, scene_idx, w / 2, h / 2));
            }
            if let Some((slot, idx, cw, ch)) = cap {
                *slot = capture_pass_rgba(
                    &device, &queue, &ubuf, &blit, &blit_bgl, &executor, &u, idx, cw, ch,
                );
            } else {
                let mut enc = device.create_command_encoder(&Default::default());
                let _ = executor.execute(&mut enc, &ubuf, &queue, &u);
                queue.submit([enc.finish()]);
            }
            executor.flip();
        }

        if let Some(dir) = out_dir {
            for (name, data, fw, fh) in [
                ("lit", &lit, w, h),
                ("flash", &flash, w, h),
                ("cascade0", &casc, w / 2, h / 2),
                ("scene", &scn, w / 2, h / 2),
            ] {
                let path = format!("{dir}/lumen_{name}.png");
                image::RgbaImage::from_raw(fw, fh, data.clone())
                    .expect("raw->image")
                    .save(&path)
                    .expect("save png");
                eprintln!("wrote {path}");
            }
        }

        let (lm, lc, hot) = stats(&lit, w, h);
        let (fm, _, _) = stats(&flash, w, h);
        let (block_std, void_frac) = block_stats(&lit);
        let cf = colorfulness(&lit);
        let mut sad = 0.0f64;
        for i in (0..lit.len()).step_by(4) {
            let a = 0.299 * prev_f[i] as f64
                + 0.587 * prev_f[i + 1] as f64
                + 0.114 * prev_f[i + 2] as f64;
            let b = 0.299 * lit[i] as f64 + 0.587 * lit[i + 1] as f64 + 0.114 * lit[i + 2] as f64;
            sad += (a - b).abs();
        }
        let sad = sad / (w * h) as f64 / 255.0;
        eprintln!(
            "lit mean {lm:.4} cover {lc:.3} hot {hot:.3}; flash mean {fm:.4}; \
             block_std {block_std:.4} void {void_frac:.3}; colorful {cf:.3}; sad {sad:.4}"
        );

        // Lit, but not a black frame and not a blown-out wash.
        assert!(lm > 0.01, "display near-black (mean {lm:.4}) — nothing lit");
        assert!(lm < 0.6, "display blew out (mean {lm:.4})");
        assert!(
            hot < 0.2,
            "display is a saturated wash ({:.0}% near-white)",
            hot * 100.0
        );
        assert!(lc > 0.02, "almost nothing lit (coverage {lc:.3})");

        // Light AND shadow: large-scale structure with genuinely dark regions — the occluder
        // casts darkness, not a uniform glow filling the frame.
        assert!(
            block_std > 0.02,
            "flat wash (block_std {block_std:.4}) — no light/shadow structure"
        );
        assert!(
            void_frac > 0.02,
            "no dark regions ({:.0}% blocks dark) — the occluder casts no shadow / light leaks everywhere",
            void_frac * 100.0
        );

        // A kick brightens the room (fireflies flash).
        assert!(
            fm > lm * 1.08,
            "onset frame not brighter (flash {fm:.4} vs lit {lm:.4}) — the kick does nothing"
        );

        // Coloured by the music, not grey.
        assert!(
            cf > 0.06,
            "display is grey (colorfulness {cf:.3}) — chroma tints not showing"
        );

        // The swarm keeps moving.
        assert!(
            sad > 0.003,
            "frozen (SAD {sad:.4}) — the scene isn't animating"
        );
    }

    // Every shipped effect's fragment-pass shaders must compile through the
    // production preamble (uniform block + libs + pass-graph input bindings at
    // the exact input count the .pfx declares). Catches a pass shader that
    // references inputN without declaring N inputs, bad WGSL against the shared
    // libs, and param-index typos that reference beyond the uniform block —
    // without launching the app. (Compute sims are not covered: they bind
    // particle buffers this harness doesn't build.)
    // The `@backdrop` special input (#2061): resolved by name, bound via
    // set_backdrop, and actually delivering the layers-beneath composite to the
    // shader. An edge-detect probe over a half-and-half backdrop must light up
    // exactly at the boundary — and render nothing at all when no backdrop is
    // wired (placeholder = uniform = no edges), which is the solo-layer contract.
    // Run: cargo test -p phosphor-app -- --ignored backdrop_input_binds
    #[test]
    #[ignore = "requires a wgpu adapter"]
    fn backdrop_input_binds_and_delivers_content() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let loader = EffectLoader::for_test(&crate::effect::loader::probe_libs());
        let fmt = TextureFormat::Rgba16Float;
        let (w, h) = (64u32, 64u32);

        // Half white / half transparent backdrop, uploaded to a bindable texture.
        let bd_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("probe-backdrop"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut px = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w / 2 {
                let i = ((y * w + x) * 4) as usize;
                px[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &bd_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &px,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let bd_view = bd_tex.create_view(&Default::default());
        let bd_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        const SHADER: &str = r#"
@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let dx = vec2f(2.0 / u.resolution.x, 0.0);
    let e = abs(input0(uv + dx).r - input0(uv - dx).r);
    return vec4f(e, e, e, e);
}
"#;
        let pipe = ShaderPipeline::new(
            &device,
            fmt,
            &loader.prepend_library_with_inputs(SHADER, 1),
            None,
            1,
        )
        .expect("backdrop probe pipeline");

        let ubuf = UniformBuffer::new(&device);
        let placeholder = PlaceholderTexture::new(&device, &queue, fmt);
        let audio = AudioTextures::new(&device, &queue);
        let mut executor = assemble(
            &device,
            &queue,
            w,
            h,
            fmt,
            &ubuf,
            &placeholder,
            &audio,
            vec![("main", pipe, false, vec![InputSrc::Backdrop], 1, 1.0)],
        );
        let (blit, blit_bgl) = blit_pipeline(&device);
        let mut u = crate::gpu::ShaderUniforms::zeroed();
        u.resolution = [w as f32, h as f32];

        // Unwired: placeholder backdrop is uniform — no edges anywhere.
        let dark = capture_pass_rgba(
            &device, &queue, &ubuf, &blit, &blit_bgl, &executor, &u, 0, w, h,
        );
        assert!(
            dark.iter().all(|&b| b <= 2),
            "no backdrop wired must render nothing"
        );

        executor.set_backdrop(
            Some((bd_view, bd_sampler)),
            &device,
            &ubuf,
            &placeholder,
            &audio,
        );
        let lit = capture_pass_rgba(
            &device, &queue, &ubuf, &blit, &blit_bgl, &executor, &u, 0, w, h,
        );
        let col_max = |x: u32| -> u8 {
            (0..h)
                .map(|y| lit[((y * w + x) * 4) as usize])
                .max()
                .unwrap()
        };
        assert!(
            col_max(w / 2 - 1) > 128 || col_max(w / 2) > 128,
            "edge column should light up at the backdrop boundary"
        );
        assert!(
            col_max(4) <= 2 && col_max(w - 4) <= 2,
            "flat regions must stay empty"
        );
    }

    // INV-B bit-identity gate, auto-covering every effect tagged `loop: "phase_locked"`
    // (the plain-CI source lint lives in effect/loader.rs; this is the proof). Per
    // effect: frame A and frame B share one uniform block and must read back
    // byte-identically; frame C changes ONLY the wall-clock uniforms (time /
    // delta_time / frame_index) and must equal A — any drift means motion is not
    // purely phase-derived and exact loop export would be impossible. The same
    // readback doubles as the per-effect alpha probe: transparent background must
    // reach the bytes, with solid coverage present.
    // Run: cargo test -p phosphor-app -- --ignored phase_locked_effects_render
    #[test]
    #[ignore = "requires a wgpu adapter"]
    fn phase_locked_effects_render_bit_identically() {
        use crate::effect::format::LoopMode;

        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let loader = EffectLoader::for_test(&crate::effect::loader::probe_libs());
        let fmt = TextureFormat::Rgba16Float;
        let (w, h) = (320u32, 180u32);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");

        let ubuf = UniformBuffer::new(&device);
        let placeholder = PlaceholderTexture::new(&device, &queue, fmt);
        let audio = AudioTextures::new(&device, &queue);
        let (blit, blit_bgl) = blit_pipeline(&device);

        let mut checked = 0usize;
        for effect in crate::effect::loader::shipped_effects_for_test() {
            if effect.loop_mode != LoopMode::PhaseLocked {
                continue;
            }
            let name = effect.name.clone();
            let passes = effect.normalized_passes();
            assert_eq!(
                passes.len(),
                1,
                "{name}: extend this probe's graph wiring for multi-pass phase-locked effects"
            );
            let src = std::fs::read_to_string(root.join("shaders").join(&passes[0].shader))
                .expect("shader source");
            let pipe = ShaderPipeline::new(
                &device,
                fmt,
                &loader.prepend_library_with_inputs(&src, 0),
                None,
                0,
            )
            .unwrap_or_else(|e| panic!("{name}: pipeline: {e}"));
            let executor = assemble(
                &device,
                &queue,
                w,
                h,
                fmt,
                &ubuf,
                &placeholder,
                &audio,
                vec![("main", pipe, false, vec![], 1, 1.0)],
            );

            // One synthesized uniform block: .pfx param defaults through the
            // production packing, mid-cycle phases, live counters, plausible audio.
            let mut u = crate::gpu::ShaderUniforms::zeroed();
            u.resolution = [w as f32, h as f32];
            let mut store = crate::params::ParamStore::new();
            store.load_from_defs(&effect.inputs);
            u.params = store.pack_to_buffer();
            u.time = 100.0;
            u.delta_time = 1.0 / 60.0;
            u.frame_index = 6000.0;
            u.rms = 0.5;
            u.bass = 0.4;
            u.onset = 0.3;
            u.centroid = 0.45;
            u.flatness = 0.2;
            u.bpm = 0.4;
            u.beat_phase = 0.37;
            u.bar_phase = 0.61;
            u.beat_in_bar = 0.5;
            u.key_class = 5.0 / 11.0;
            u.key_is_minor = 1.0;
            u.bar_index = 5.0;
            u.beat_index = 23.0;
            u.chroma = [0.5; 12];

            let a = capture_pass_rgba(
                &device, &queue, &ubuf, &blit, &blit_bgl, &executor, &u, 0, w, h,
            );
            let b = capture_pass_rgba(
                &device, &queue, &ubuf, &blit, &blit_bgl, &executor, &u, 0, w, h,
            );
            assert_eq!(a, b, "{name}: two renders with identical uniforms differ");

            let mut u2 = u;
            u2.time = 427.3;
            u2.delta_time = 0.004;
            u2.frame_index = 12.0;
            let c = capture_pass_rgba(
                &device, &queue, &ubuf, &blit, &blit_bgl, &executor, &u2, 0, w, h,
            );
            assert_eq!(
                a, c,
                "{name}: wall-clock uniforms moved the picture — INV-B violation"
            );

            let (mut amin, mut amax) = (255u8, 0u8);
            for px in a.chunks_exact(4) {
                amin = amin.min(px[3]);
                amax = amax.max(px[3]);
            }
            assert_eq!(
                amin, 0,
                "{name}: transparent background must reach readback"
            );
            assert!(
                amax > 64,
                "{name}: no solid coverage drawn (max alpha {amax})"
            );
            checked += 1;
        }
        assert!(
            checked >= 4,
            "expected the four overlay effects, probed {checked}"
        );
    }

    // Every overlay_lib.wgsl primitive drawn once over a transparent background,
    // premultiplied, through the production preamble. Asserts variance in RGB AND
    // alpha — a lib regression that flattens coverage (or a preamble change that
    // breaks the prepend) fails here before any overlay effect does.
    // Run: cargo test -p phosphor-app -- --ignored overlay_lib_primitives
    #[test]
    #[ignore = "requires a wgpu adapter"]
    fn overlay_lib_primitives_render_with_alpha() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let loader = EffectLoader::for_test(&crate::effect::loader::probe_libs());
        let fmt = TextureFormat::Rgba16Float;
        let (w, h) = (256u32, 256u32);

        const SHADER: &str = r#"
@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    // One of everything, phase-driven the way a real overlay is.
    let phase = 0.6;
    var cover = 0.0;
    let cell = ovl_cell_id(uv, 8u, 8u);
    let cuv = ovl_cell_uv(uv, 8u, 8u);
    let ch = ovl_cell_hash(cell, 7.0);
    cover = max(cover, ovl_reveal(ch, phase, 0.1) * ovl_rect_stroke(cuv, vec2f(0.5), vec2f(0.35), 0.1));
    let trig = ovl_trigger(phase, ovl_stagger(cell, 7.0, 0.5), 0.05, 0.2, 0.3);
    cover = max(cover, trig * ovl_bracket(uv, vec2f(0.3, 0.3), vec2f(0.2, 0.15), 0.06, 0.02));
    cover = max(cover, ovl_cross(uv, vec2f(0.7, 0.6), 0.12, 0.02, 0.012));
    var seeds = array<vec2f, 8>(
        vec2f(0.1, 0.1), vec2f(0.9, 0.2), vec2f(0.5, 0.9), vec2f(0.0),
        vec2f(0.0), vec2f(0.0), vec2f(0.0), vec2f(0.0),
    );
    cover = max(cover, 0.25 * ovl_flood(uv, seeds, 3u, 0.3, 0.2, 7.0));
    // Instrumentation primitives (epic pass): segment, ring, arc, tick ring.
    cover = max(cover, ovl_segment(uv, vec2f(0.1, 0.8), vec2f(0.45, 0.55), 0.006));
    cover = max(cover, ovl_ring(uv, vec2f(0.5), 0.3, 0.005));
    cover = max(cover, ovl_arc(uv, vec2f(0.5), 0.34, phase, 0.2, 0.01));
    cover = max(cover, ovl_ticks_ring(uv, vec2f(0.5), 0.38, 24.0, 0.03, 0.008));
    let rgb = phosphor_palette(ch, vec3f(0.5), vec3f(0.5), vec3f(1.0), vec3f(0.0, 0.33, 0.67));
    let a = clamp(cover, 0.0, 1.0);
    return vec4f(rgb * a, a); // premultiplied
}
"#;
        let pipe = ShaderPipeline::new(
            &device,
            fmt,
            &loader.prepend_library_with_inputs(SHADER, 0),
            None,
            0,
        )
        .expect("overlay lib probe pipeline");

        let ubuf = UniformBuffer::new(&device);
        let placeholder = PlaceholderTexture::new(&device, &queue, fmt);
        let audio = AudioTextures::new(&device, &queue);
        let executor = assemble(
            &device,
            &queue,
            w,
            h,
            fmt,
            &ubuf,
            &placeholder,
            &audio,
            vec![("overlay", pipe, false, vec![], 1, 1.0)],
        );
        let (blit, blit_bgl) = blit_pipeline(&device);

        let mut u = crate::gpu::ShaderUniforms::zeroed();
        u.resolution = [w as f32, h as f32];
        let data = capture_pass_rgba(
            &device, &queue, &ubuf, &blit, &blit_bgl, &executor, &u, 0, w, h,
        );

        let (mut amin, mut amax) = (255u8, 0u8);
        let (mut rgb_lit, mut solid) = (0usize, 0usize);
        for px in data.chunks_exact(4) {
            amin = amin.min(px[3]);
            amax = amax.max(px[3]);
            if px[0] > 8 || px[1] > 8 || px[2] > 8 {
                rgb_lit += 1;
            }
            if px[3] > 200 {
                solid += 1;
            }
        }
        assert_eq!(amin, 0, "transparent background must reach readback");
        assert!(
            amax > 128,
            "no primitive drew solid coverage (max alpha {amax})"
        );
        assert!(
            rgb_lit as f64 / (w * h) as f64 > 0.01,
            "RGB variance missing — primitives drew nothing"
        );
        // Strokes/brackets/crosses are thin: solid (near-opaque) coverage must be
        // present but sparse. An opaque wash — the alpha-regression failure mode —
        // trips the upper bound.
        let solid_frac = solid as f64 / (w * h) as f64;
        assert!(
            (0.002..0.5).contains(&solid_frac),
            "solid coverage {solid_frac:.3} out of range — no strokes drawn, or an opaque wash"
        );
    }

    #[test]
    #[ignore = "requires a wgpu adapter"]
    fn all_effect_pass_shaders_compile() {
        let _guard = gpu_guard();
        let (device, _queue) = test_gpu();

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let loader = EffectLoader::for_test(&crate::effect::loader::probe_libs());

        let mut checked = 0usize;
        let mut failures: Vec<String> = Vec::new();
        let mut entries: Vec<_> = std::fs::read_dir(root.join("effects"))
            .expect("effects dir")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "pfx"))
            .collect();
        entries.sort();

        for path in entries {
            let json = std::fs::read_to_string(&path).expect("read .pfx");
            let effect: crate::effect::format::PfxEffect = match serde_json::from_str(&json) {
                Ok(e) => e,
                Err(e) => {
                    failures.push(format!("{}: bad JSON: {e}", path.display()));
                    continue;
                }
            };
            for pass in effect.normalized_passes() {
                // Same count the app injects at every compile site (initial + hot-reload):
                // current-frame inputs PLUS previous-frame prev_inputs. A pass whose inputs
                // are all prev_inputs (divergence, protea's potential) declares input0.. and
                // would fail to compile if this dropped prev_inputs — the hot-reload bug.
                let input_count = pass.input_count();
                let src_path = root.join("shaders").join(&pass.shader);
                let src = match std::fs::read_to_string(&src_path) {
                    Ok(s) => s,
                    Err(e) => {
                        failures.push(format!(
                            "{} pass '{}': missing shader {}: {e}",
                            effect.name,
                            pass.name,
                            src_path.display()
                        ));
                        continue;
                    }
                };
                let full = loader.prepend_library_with_inputs(&src, input_count);
                if let Err(e) = ShaderPipeline::new(
                    &device,
                    TextureFormat::Rgba16Float,
                    &full,
                    None,
                    input_count,
                ) {
                    failures.push(format!(
                        "{} pass '{}' ({}): {e}",
                        effect.name, pass.name, pass.shader
                    ));
                }
                checked += 1;
            }
        }

        eprintln!("compiled {checked} pass shaders");
        assert!(
            checked > 40,
            "suspiciously few pass shaders found ({checked})"
        );
        assert!(
            failures.is_empty(),
            "pass shaders failed to compile:\n{}",
            failures.join("\n")
        );
    }

    // End-to-end Chronoflow probe (#1482) through the REAL production pieces: a
    // ParticleSystem with `velocity_field` (compute raster + velocity resolve),
    // the shared chronoflow_velocity.wgsl self-advecting field reading
    // `@particles.velocity`, and phosphor_history.wgsl advecting the trail image
    // — particles composite into the history target each frame exactly as in the
    // app. Particles emit at center under strong +x gravity, so:
    //   1. long-exposure trails accumulate (lit coverage ≫ the snapped frame's),
    //   2. a beat frame collapses history (shutter snap → coverage drops),
    //   3. the lit mass skews rightward (velocity field points the right way),
    //   4. nothing blows out (HDR clamp holds in the feedback loop).
    // Run: CHRONO_PNG_DIR=/tmp cargo test -p phosphor-app --release -- --ignored chronoflow_render_previews
    #[test]
    #[ignore = "requires a wgpu adapter; renders offscreen, writes PNGs"]
    fn chronoflow_render_previews() {
        let out_dir = std::env::var("CHRONO_PNG_DIR").ok();
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();

        let loader = EffectLoader::for_test(&crate::effect::loader::probe_libs());
        let fmt = TextureFormat::Rgba16Float;
        let (w, h) = (480u32, 270u32);

        let ubuf = UniformBuffer::new(&device);
        let placeholder = PlaceholderTexture::new(&device, &queue, fmt);
        let audio = AudioTextures::new(&device, &queue);

        // Particle system: compute raster + velocity field, center emitter,
        // strong rightward gravity for a deterministic drift direction.
        let def: crate::gpu::particle::types::ParticleDef = serde_json::from_str(
            r#"{
                "render_mode": "compute",
                "velocity_field": true,
                "max_count": 4000,
                "emitter": { "shape": "point", "radius": 0.05, "position": [0.0, 0.0] },
                "lifetime": 3.0,
                "initial_speed": 0.12,
                "initial_size": 0.012,
                "size_end": 0.008,
                "gravity": [0.7, 0.0],
                "drag": 0.999,
                "turbulence": 0.0,
                "emit_rate": 1500.0,
                "size_curve": [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
                "opacity_curve": [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]
            }"#,
        )
        .expect("probe ParticleDef");
        let plib = include_str!("../../../../assets/shaders/lib/particle_lib.wgsl");
        let sim = include_str!("../../../../assets/shaders/builtin/particle_sim.wgsl");
        let sim_src = format!("{}\n{plib}\n{sim}", crate::effect::loader::probe_libs());
        let mut ps = crate::gpu::particle::ParticleSystem::new(
            &device,
            &queue,
            fmt,
            &def,
            &sim_src,
            def.interaction,
        );
        // The rasterizer defaults to 1920×1080; resize to the probe target BEFORE
        // the executor binds the velocity texture.
        ps.resize_compute_raster(&device, w, h);

        let mk = |shader: &str, count: usize| {
            ShaderPipeline::new(
                &device,
                fmt,
                &loader.prepend_library_with_inputs(shader, count),
                None,
                count,
            )
            .expect("chronoflow pass pipeline")
        };
        let pipe_vel = mk(
            include_str!("../../../../assets/shaders/chronoflow_velocity.wgsl"),
            1,
        );
        let pipe_hist = mk(
            include_str!("../../../../assets/shaders/phosphor_history.wgsl"),
            1,
        );

        // Same wiring as phosphor.pfx: velocity (½ res, reads @particles.velocity),
        // history (full res, reads velocity; particles composite on top).
        let mut executor = assemble(
            &device,
            &queue,
            w,
            h,
            fmt,
            &ubuf,
            &placeholder,
            &audio,
            vec![
                (
                    "velocity",
                    pipe_vel,
                    true,
                    vec![InputSrc::ParticleVelocity],
                    1,
                    0.5,
                ),
                (
                    "history",
                    pipe_hist,
                    true,
                    vec![InputSrc::Pass {
                        pass: 0,
                        prev: false,
                    }],
                    1,
                    1.0,
                ),
            ],
        );
        executor.set_particle_system(Some(ps), &device, &ubuf, &placeholder, &audio);
        let hist_idx = 1;

        let (blit, blit_bgl) = blit_pipeline(&device);

        let mut u = crate::gpu::ShaderUniforms::zeroed();
        u.resolution = [w as f32, h as f32];
        u.delta_time = 1.0 / 60.0;
        u.rms = 0.4;
        // phosphor_history params: 0 trail_decay (exposure), 1 beat_snap, 2 flow_stretch.
        u.params[0] = 0.9;
        u.params[1] = 0.7;
        u.params[2] = 0.5;

        let stats = |data: &[u8]| -> (f64, f64, f64) {
            // (mean luminance 0..1, lit coverage, right/left mass ratio)
            let (mut sum, mut lit, mut left, mut right) = (0.0f64, 0u32, 0.0f64, 0.0f64);
            for y in 0..h {
                for x in 0..w {
                    let i = ((y * w + x) * 4) as usize;
                    let l = 0.299 * data[i] as f64
                        + 0.587 * data[i + 1] as f64
                        + 0.114 * data[i + 2] as f64;
                    sum += l;
                    if l > 8.0 {
                        lit += 1;
                    }
                    if x < w / 2 {
                        left += l;
                    } else {
                        right += l;
                    }
                }
            }
            (
                sum / (w * h) as f64 / 255.0,
                lit as f64 / (w * h) as f64,
                (right + 1.0) / (left + 1.0),
            )
        };

        const TRAIL_A: u32 = 30;
        const TRAIL_B: u32 = 44;
        const SNAP: u32 = 45;
        let mut cap: Vec<(String, Vec<u8>)> = Vec::new();

        for f in 0..=SNAP {
            let beat = if f == SNAP { 1.0 } else { 0.0 };
            u.time = f as f32 / 60.0;
            u.frame_index = f as f32;
            u.beat = beat;
            u.beat_strength = beat;
            executor
                .particle_system
                .as_mut()
                .expect("probe particle system")
                .update_uniforms(1.0 / 60.0, u.time, [w as f32, h as f32], beat);

            if f == TRAIL_A || f == TRAIL_B || f == SNAP {
                let data = capture_pass_rgba(
                    &device, &queue, &ubuf, &blit, &blit_bgl, &executor, &u, hist_idx, w, h,
                );
                cap.push((format!("f{f}"), data));
            } else {
                let mut enc = device.create_command_encoder(&Default::default());
                let _ = executor.execute(&mut enc, &ubuf, &queue, &u);
                queue.submit([enc.finish()]);
            }
            executor.flip();
        }

        if let Some(dir) = &out_dir {
            for (name, data) in &cap {
                let path = format!("{dir}/chronoflow_{name}.png");
                image::RgbaImage::from_raw(w, h, data.clone())
                    .expect("raw->image")
                    .save(&path)
                    .expect("save png");
                eprintln!("wrote {path}");
            }
        }

        let (mean_a, cov_a, _skew_a) = stats(&cap[0].1);
        let (mean_b, cov_b, skew_b) = stats(&cap[1].1);
        let (mean_s, cov_s, _skew_s) = stats(&cap[2].1);
        eprintln!(
            "trailA mean {mean_a:.4} cov {cov_a:.4}; trailB mean {mean_b:.4} cov {cov_b:.4} \
             skew {skew_b:.2}; snap mean {mean_s:.4} cov {cov_s:.4}"
        );

        // Something rendered at all.
        assert!(
            cov_b > 0.002,
            "trail frame is essentially black (coverage {cov_b:.4})"
        );
        // 1. Long-exposure trails: the steady frame carries far more lit area
        //    and light than the shutter-snapped frame (history collapsed).
        assert!(
            cov_b > cov_s * 1.3,
            "trails should add lit area over a snapped frame: trail cov {cov_b:.4} \
             vs snap cov {cov_s:.4}"
        );
        assert!(
            mean_b > mean_s * 1.2,
            "shutter snap should drop brightness: trail mean {mean_b:.4} vs snap \
             mean {mean_s:.4}"
        );
        // 2. Directionality: +x gravity must skew the lit mass rightward — a
        //    broken velocity field (zero, or wrong sign) leaves it symmetric or
        //    left-heavy.
        assert!(
            skew_b > 1.15,
            "trail mass should skew right under +x gravity, right/left = {skew_b:.2}"
        );
        // 3. Feedback-loop safety: no blowout.
        let hot = cap[1]
            .1
            .chunks_exact(4)
            .filter(|p| p[0] > 220 && p[1] > 220 && p[2] > 220)
            .count() as f64
            / (w * h) as f64;
        assert!(
            hot < 0.2,
            "trail frame is blowing out ({:.0}% near-white)",
            hot * 100.0
        );
    }
}
