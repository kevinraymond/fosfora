//! Building a `ParticleSystem` from a `.pfx` particle definition.
//!
//! Extracted verbatim from `App::build_particle_system` for the headless scene
//! renderer (#2027): quality scaling, the per-effect scaled cap, and the
//! storage-buffer binding cap must be identical in both, or the same preset
//! spawns different particle counts headless vs live.

use crate::effect::loader::{EffectLoader, assets_dir};
use crate::gpu::context::GpuContext;
use crate::gpu::particle::ParticleSystem;
use crate::settings::ParticleQuality;

pub(crate) fn build_particle_system(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    effect_loader: &EffectLoader,
    particle_quality: ParticleQuality,
    particles: &crate::gpu::particle::types::ParticleDef,
) -> Option<ParticleSystem> {
    let multiplier = particle_quality.multiplier();
    let mut particles = particles.clone();
    let original_count = particles.max_count;
    particles.max_count = (particles.max_count as f32 * multiplier).round() as u32;
    particles.emit_rate *= multiplier;

    // Per-effect cap: don't scale past max_scaled_count if set
    if particles.max_scaled_count > 0 && particles.max_count > particles.max_scaled_count {
        let ratio = particles.max_scaled_count as f32 / particles.max_count as f32;
        particles.max_count = particles.max_scaled_count;
        particles.emit_rate *= ratio;
    }

    // Cap particle count to device storage buffer binding limit.
    // The largest buffer is sorted_particles_buffer = max_particles × 9 × 4 bytes
    // (3×3 tile coverage in compute rasterizer scatter pass).
    let max_binding = device.limits().max_storage_buffer_binding_size as u64;
    let max_from_binding = (max_binding / (9 * 4)) as u32;
    if particles.max_count > max_from_binding {
        log::warn!(
            "Capping particles from {} to {} (storage buffer binding limit {}MB)",
            particles.max_count,
            max_from_binding,
            max_binding / (1024 * 1024),
        );
        particles.max_count = max_from_binding;
        particles.emit_rate = particles.emit_rate.min(max_from_binding as f32);
    }

    if particles.max_count != original_count {
        log::info!(
            "Particle quality {}: {} -> {} particles",
            particle_quality.display_name(),
            original_count,
            particles.max_count,
        );
    }
    let particles = &particles;

    let hdr_format = GpuContext::hdr_format();
    let is_image_emitter = particles.emitter.shape == "image";

    // For image emitters, use the builtin image_scatter compute shader
    let compute_source = if is_image_emitter && particles.compute_shader.is_empty() {
        effect_loader.prepend_compute_libraries(include_str!(
            "../../../../../assets/shaders/builtin/image_scatter.wgsl"
        ))
    } else if particles.compute_shader.is_empty() {
        effect_loader.prepend_compute_libraries(include_str!(
            "../../../../../assets/shaders/builtin/particle_sim.wgsl"
        ))
    } else {
        match effect_loader.load_compute_source(&particles.compute_shader) {
            Ok(src) => src,
            Err(e) => {
                log::error!(
                    "Failed to load compute shader '{}': {e}",
                    particles.compute_shader
                );
                return None;
            }
        }
    };

    let mut ps = ParticleSystem::new(
        device,
        queue,
        hdr_format,
        particles,
        &compute_source,
        particles.interaction,
    );
    log::info!("Particle system created: {} particles", particles.max_count);

    // Load model data for image emitters pointed at a 3D model (#1993). The
    // model is rendered to a frame and sampled by the same code path an image
    // takes, so it lands in the same aux buffer and every media effect gets it
    // for free. Checked before `image` and mutually exclusive with it: a .pfx
    // may carry an image as its fallback, but a model wins when both are set.
    if is_image_emitter && !particles.emitter.model.is_empty() {
        let sample_def =
            particles
                .image_sample
                .clone()
                .unwrap_or(crate::gpu::particle::types::ImageSampleDef {
                    mode: "grid".to_string(),
                    threshold: 0.1,
                    scale: 1.0,
                });
        ps.sample_def = sample_def.clone();
        let model_def = particles.model_sample.clone().unwrap_or_default();
        let model_path = crate::gpu::particle::model_source::resolve_model_path(
            assets_dir(),
            &particles.emitter.model,
        );
        match crate::gpu::particle::model_source::sample_model(
            device,
            queue,
            &model_path,
            &sample_def,
            &model_def,
            particles.max_count,
        ) {
            Ok(aux_data) => {
                ps.upload_aux_data(device, queue, &aux_data);
                ps.store_current_aux(aux_data.clone());
                ps.set_source(crate::gpu::particle::ParticleSource::Model {
                    path: model_path.to_string_lossy().to_string(),
                });
                ps.model_sample = model_def;
                log::info!(
                    "Loaded model '{}': {} particles",
                    particles.emitter.model,
                    aux_data.len()
                );
            }
            Err(e) => {
                log::warn!("Failed to load model '{}': {e}", particles.emitter.model);
            }
        }
    }

    // Load image data for image emitters
    if is_image_emitter && particles.emitter.model.is_empty() && !particles.emitter.image.is_empty()
    {
        let sample_def =
            particles
                .image_sample
                .clone()
                .unwrap_or(crate::gpu::particle::types::ImageSampleDef {
                    mode: "grid".to_string(),
                    threshold: 0.1,
                    scale: 1.0,
                });
        ps.sample_def = sample_def.clone();
        let image_path = assets_dir().join("images").join(&particles.emitter.image);
        match crate::gpu::particle::image_source::sample_image(
            &image_path,
            &sample_def,
            particles.max_count,
        ) {
            Ok(aux_data) => {
                ps.upload_aux_data(device, queue, &aux_data);
                ps.store_current_aux(aux_data.clone());
                ps.set_source(crate::gpu::particle::ParticleSource::Image {
                    path: image_path.to_string_lossy().to_string(),
                });
                log::info!(
                    "Loaded image '{}': {} particles",
                    particles.emitter.image,
                    aux_data.len()
                );
            }
            Err(e) => {
                log::warn!("Failed to load image '{}': {e}", particles.emitter.image);
            }
        }

        // If a video source is specified, set up video playback
        #[cfg(feature = "video")]
        if !particles.emitter.video.is_empty() && particles.emitter.video != "webcam" {
            let video_path = assets_dir().join("videos").join(&particles.emitter.video);
            if video_path.exists() {
                if crate::media::video::ffmpeg_available() {
                    match crate::media::video::probe_video(&video_path) {
                        Ok(meta) => {
                            match crate::media::video::decode_all_frames(&video_path, &meta) {
                                Ok((frames, delays_ms)) => {
                                    let path_str = video_path.to_string_lossy().to_string();
                                    ps.set_video_source(queue, frames, delays_ms, path_str);
                                    log::info!(
                                        "Particle video source: '{}'",
                                        particles.emitter.video
                                    );
                                }
                                Err(e) => log::warn!(
                                    "Failed to decode particle video '{}': {e}",
                                    particles.emitter.video
                                ),
                            }
                        }
                        Err(e) => log::warn!(
                            "Failed to probe particle video '{}': {e}",
                            particles.emitter.video
                        ),
                    }
                }
            }
        }
    }

    // Load sprite texture if defined
    if let Some(ref sprite_def) = particles.sprite {
        let sprite_path = assets_dir().join("images").join(&sprite_def.texture);
        match crate::gpu::particle::sprite::SpriteAtlas::load_with_def(
            device,
            queue,
            &sprite_path,
            sprite_def.cols,
            sprite_def.rows,
            sprite_def.animated,
            sprite_def.frames,
        ) {
            Ok(atlas) => {
                ps.set_sprite(device, atlas);
                log::info!("Loaded sprite atlas: {}", sprite_def.texture);
            }
            Err(e) => {
                log::warn!("Failed to load sprite '{}': {e}", sprite_def.texture);
            }
        }
    }

    // Set up trail rendering if trail_length specified
    if particles.trail_length >= 2 {
        ps.setup_trails(
            device,
            hdr_format,
            particles.trail_length,
            particles.trail_width,
        );
        log::info!(
            "Trail rendering enabled: {} points, width {}",
            particles.trail_length,
            particles.trail_width
        );
    }

    if particles.interaction {
        log::info!("Spatial hash enabled for particle interaction");
    }

    // Morph target loading
    if particles.morph {
        if let Some(ref targets) = particles.morph_targets {
            let assets = assets_dir();
            for (slot, target_def) in targets.iter().take(4).enumerate() {
                match crate::gpu::particle::morph::load_morph_target(
                    target_def,
                    particles.max_count,
                    particles.initial_size,
                    assets,
                    device,
                    queue,
                ) {
                    Ok(data) => {
                        log::info!(
                            "Morph target {}: '{}' ({} particles)",
                            slot,
                            target_def.source,
                            data.len()
                        );
                        if let Some(ref mut morph) = ps.morph_state {
                            morph.load_target(slot as u32, data);
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to load morph target {}: {e}", target_def.source);
                    }
                }
            }
            ps.upload_morph_targets(device, queue);
        }
    }

    Some(ps)
}
