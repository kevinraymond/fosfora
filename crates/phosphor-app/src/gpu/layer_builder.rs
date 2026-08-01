//! Building a layer's GPU contents from a `.pfx` effect — the production path,
//! shared between the live app and the headless scene renderer (#2027).
//!
//! Extracted from `App::load_effect_on_layer`, which mixed this core with
//! App-only concerns (splat background loads, shader-editor auto-open, webcam
//! cleanup, active-layer UI sync). Those stayed in the App wrapper; everything
//! that decides what the layer *renders* is here, because a headless copy of it
//! would drift exactly like every other duplicated grammar in this repo has.

use wgpu::{Device, PipelineCache, Queue};

use crate::effect::format::PfxEffect;
use crate::effect::loader::{EffectLoader, assets_dir};
use crate::gpu::audio_textures::AudioTextures;
use crate::gpu::context::GpuContext;
use crate::gpu::layer::{EffectLayer, Layer, LayerContent};
use crate::gpu::particle::ParticleSystem;
use crate::gpu::pass_executor::PassExecutor;
use crate::gpu::placeholder::PlaceholderTexture;
use crate::gpu::render_target::PingPongTarget;
use crate::gpu::{ShaderPipeline, ShaderUniforms, UniformBuffer};
use crate::settings::ParticleQuality;

/// Everything layer construction needs from the environment. Both `App` and
/// the headless renderer build one from their own fields; `width`/`height` are
/// the render dimensions (the app passes its surface size).
pub(crate) struct LayerBuildCtx<'a> {
    pub device: &'a Device,
    pub queue: &'a Queue,
    pub pipeline_cache: Option<&'a PipelineCache>,
    pub width: u32,
    pub height: u32,
    pub placeholder: &'a PlaceholderTexture,
    pub audio_textures: &'a AudioTextures,
    pub particle_quality: ParticleQuality,
    /// The compositor's `@backdrop` target (view, sampler) — wired into every
    /// executor at build so backdrop-reactive effects (#2061) resolve it.
    pub backdrop: Option<(&'a wgpu::TextureView, &'a wgpu::Sampler)>,
}

/// Read default.wgsl from assets dir, falling back to embedded copy.
pub(crate) fn read_default_shader() -> String {
    let path = assets_dir().join("shaders/default.wgsl");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|_| include_str!("../../../../assets/shaders/default.wgsl").to_string())
}

/// Update the spatial-hash grid dims for an interaction effect, then build its
/// particle system. One helper because the two must happen in this order and
/// both callers (app, headless) would otherwise each carry the pairing.
pub(crate) fn prepare_particles(
    ctx: &LayerBuildCtx<'_>,
    effect_loader: &mut EffectLoader,
    effect: &PfxEffect,
) -> Option<ParticleSystem> {
    let pd = effect.particles.as_ref()?;
    if pd.interaction {
        use crate::gpu::particle::spatial_hash::grid_dims;
        effect_loader.grid_dims = grid_dims(pd.max_count, pd.grid_max);
    }
    crate::gpu::particle::build::build_particle_system(
        ctx.device,
        ctx.queue,
        effect_loader,
        ctx.particle_quality,
        pd,
    )
}

/// A fresh default-shader effect layer (the core of `App::add_layer`).
pub(crate) fn new_default_layer(ctx: &LayerBuildCtx<'_>, name: String) -> Option<Layer> {
    let hdr_format = GpuContext::hdr_format();
    let source = read_default_shader();
    let uniform_buffer = UniformBuffer::new(ctx.device);
    let feedback = PingPongTarget::new_cleared(
        ctx.device, ctx.queue, ctx.width, ctx.height, hdr_format, 1.0,
    );
    let pipeline =
        ShaderPipeline::new(ctx.device, hdr_format, &source, ctx.pipeline_cache, 0).ok()?;
    let pass_executor = PassExecutor::single_pass(
        pipeline,
        feedback,
        &uniform_buffer,
        ctx.device,
        ctx.placeholder,
        ctx.audio_textures,
    );
    Some(Layer::new_effect(
        name,
        EffectLayer {
            pass_executor,
            uniform_buffer,
            uniforms: ShaderUniforms::zeroed(),
            effect_index: None,
            shader_sources: vec![source],
            shader_error: None,
            pending_rebuild: false,
        },
        crate::params::ParamStore::new(),
    ))
}

/// Load `effect` into `layer`: pass graph, particle system, params, per-effect
/// postprocess. `particle_system` comes from [`prepare_particles`] — built by
/// the caller so it can inspect it first (the app kicks splat loads off it).
///
/// On failure the GPU state stays on the previous effect (a broken shader
/// cannot render) but the CPU side moves to the new one — `shader_error`,
/// `effect_index`, `pending_rebuild` and the param defs — so panels and
/// hot-reload agree about what the layer is *trying* to be (#1855). The error
/// string is returned for the caller's own reporting (the app auto-opens the
/// shader editor with it).
pub(crate) fn load_effect_into_layer(
    ctx: &LayerBuildCtx<'_>,
    effect_loader: &EffectLoader,
    layer: &mut Layer,
    layer_idx: usize,
    effect: &PfxEffect,
    effect_index: usize,
    particle_system: Option<ParticleSystem>,
) -> Result<(), String> {
    let hdr_format = GpuContext::hdr_format();
    let passes = effect.normalized_passes();
    if passes.is_empty() {
        let msg = format!("Effect '{}' has no shader or passes defined", effect.name);
        log::error!("{msg}");
        return Err(msg);
    }

    // If layer is currently Media, convert to Effect first
    if layer.is_media() {
        let uniform_buffer = UniformBuffer::new(ctx.device);
        let feedback = PingPongTarget::new_cleared(
            ctx.device, ctx.queue, ctx.width, ctx.height, hdr_format, 1.0,
        );
        // Temporary pipeline — will be replaced by executor_result below
        let default_source = read_default_shader();
        if let Ok(pipeline) = ShaderPipeline::new(
            ctx.device,
            hdr_format,
            &default_source,
            ctx.pipeline_cache,
            0,
        ) {
            let pass_executor = PassExecutor::single_pass(
                pipeline,
                feedback,
                &uniform_buffer,
                ctx.device,
                ctx.placeholder,
                ctx.audio_textures,
            );
            layer.content = LayerContent::Effect(Box::new(EffectLayer {
                pass_executor,
                uniform_buffer,
                uniforms: ShaderUniforms::zeroed(),
                effect_index: None,
                shader_sources: vec![],
                shader_error: None,
                pending_rebuild: false,
            }));
        }
    }

    // Need the layer's uniform buffer reference for PassExecutor::new.
    let LayerContent::Effect(ref eff) = layer.content else {
        return Err("layer is not an effect layer".to_string());
    };
    let executor_result = PassExecutor::new(
        ctx.device,
        hdr_format,
        ctx.width,
        ctx.height,
        &passes,
        effect_loader,
        &eff.uniform_buffer,
        ctx.placeholder,
        ctx.audio_textures,
        ctx.queue,
        ctx.pipeline_cache,
    );

    match executor_result {
        Ok(mut executor) => {
            let LayerContent::Effect(ref mut e) = layer.content else {
                return Err("layer is not an effect layer".to_string());
            };
            executor.set_particle_system(
                particle_system,
                ctx.device,
                &e.uniform_buffer,
                ctx.placeholder,
                ctx.audio_textures,
            );
            executor.set_backdrop(
                ctx.backdrop.map(|(v, sm)| (v.clone(), sm.clone())),
                ctx.device,
                &e.uniform_buffer,
                ctx.placeholder,
                ctx.audio_textures,
            );
            e.pass_executor = executor;
            layer.param_store.load_from_defs(&effect.inputs);
            let LayerContent::Effect(ref mut e) = layer.content else {
                unreachable!("checked above");
            };
            e.shader_error = None;
            e.pending_rebuild = false;
            e.effect_index = Some(effect_index);
            // Apply per-effect postprocess overrides
            layer.postprocess = effect.postprocess.clone().unwrap_or_default();
            // Track shader sources for hot-reload
            let LayerContent::Effect(ref mut e) = layer.content else {
                unreachable!("checked above");
            };
            e.shader_sources = passes
                .iter()
                .filter_map(|p| {
                    effect_loader
                        .load_effect_source_with_inputs(&p.shader, p.input_count())
                        .ok()
                })
                .collect();
            log::info!(
                "Layer {}: loaded effect '{}' ({} pass{})",
                layer_idx,
                effect.name,
                passes.len(),
                if passes.len() == 1 { "" } else { "es" }
            );
            Ok(())
        }
        Err(e) => {
            log::error!("Failed to load effect '{}': {e}", effect.name);
            // The GPU state stays on the previous effect — a broken shader
            // cannot render. Everything CPU-side moves to the new effect so
            // the panel, the grid and "Edit Shader" agree with each other,
            // and `pending_rebuild` records that the two disagree, so shader
            // hot-reload retries the whole load instead of patching a
            // pipeline into the previous effect's executor (#1855).
            if let LayerContent::Effect(ref mut eff) = layer.content {
                eff.shader_error = Some(format!("Load error: {e}"));
                eff.effect_index = Some(effect_index);
                eff.pending_rebuild = true;
            }
            layer.param_store.load_from_defs(&effect.inputs);
            Err(e)
        }
    }
}
