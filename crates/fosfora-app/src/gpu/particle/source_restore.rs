//! Restoring a preset's image / 3D-model particle source onto a live
//! `ParticleSystem` — shared between `App::apply_preset_immediately` and the
//! headless scene renderer (#2027).
//!
//! Only the synchronous arms live here. Video restore (decode-heavy, feature
//! `video`) and webcam (capture hardware) stay App-only: the headless renderer
//! disables those layers with a warning instead.

use wgpu::{Device, Queue};

use crate::gpu::particle::{ParticleSource, ParticleSystem};

/// Load a 3D model as the particle source (or as a morph target, when the
/// effect is a morph — same split the picker uses). `pose`/`light` are the
/// preset's saved sampling state; absent means front-on / light off, matching
/// the presets written before those fields existed.
///
/// The already-loaded check compares the WHOLE source, not one path field —
/// a live video no longer satisfies "the model is already loaded" (#2011).
pub(crate) fn restore_model_source(
    device: &Device,
    queue: &Queue,
    ps: &mut ParticleSystem,
    model_path: &str,
    pose: Option<[f32; 4]>,
    light: Option<[f32; 5]>,
    layer_idx: usize,
) {
    let path = std::path::PathBuf::from(model_path);
    if !path.exists() {
        log::warn!("Particle model '{model_path}' not found for layer {layer_idx}");
        return;
    }
    let already_loaded = matches!(
        &ps.source,
        ParticleSource::Model { path } if path == model_path
    );
    if already_loaded {
        return;
    }
    let pose = pose.unwrap_or([0.0, 0.0, 1.0, 0.25]);
    // Absent light block = a v1.28.0 preset, which meant "off".
    let light = light.unwrap_or([0.0; 5]);
    let model_def = crate::gpu::particle::types::ModelSampleDef {
        yaw_degrees: pose[0],
        pitch_degrees: pose[1],
        scale: pose[2],
        ambient: pose[3],
        light_mix: light[0],
        light_x: light[1],
        light_y: light[2],
        light_z: light[3],
        ray_strength: light[4],
    };
    // A morph effect takes the model as a TARGET; anything else takes it as
    // the source.
    let outcome = if ps.morph_state.is_some() {
        ps.apply_model_morph_target(device, queue, &path, &model_def, None)
            .map(|(slot, _)| format!("morph slot {slot}"))
    } else {
        ps.apply_model_source(device, queue, &path, &model_def)
            .map(|_| "source".to_string())
    };
    match outcome {
        Ok(where_) => {
            log::info!("Restored particle model for layer {layer_idx}: {model_path} ({where_})");
        }
        Err(e) => log::warn!("Failed to restore particle model for layer {layer_idx}: {e}"),
    }
}

/// Load a still image as the particle source. Same whole-source
/// already-loaded rule as the model arm.
pub(crate) fn restore_image_source(
    device: &Device,
    queue: &Queue,
    ps: &mut ParticleSystem,
    img_path: &str,
    layer_idx: usize,
) {
    let path = std::path::PathBuf::from(img_path);
    if !path.exists() {
        log::warn!("Particle image '{img_path}' not found for layer {layer_idx}");
        return;
    }
    let already_loaded = matches!(
        &ps.source,
        ParticleSource::Image { path } if path == img_path
    );
    if already_loaded {
        return;
    }
    match crate::gpu::particle::image_source::sample_image(&path, &ps.sample_def, ps.max_particles)
    {
        Ok(aux_data) => {
            ps.upload_aux_data(device, queue, &aux_data);
            ps.store_current_aux(aux_data);
            ps.set_source(ParticleSource::Image {
                path: img_path.to_string(),
            });
            log::info!("Restored particle image source for layer {layer_idx}: {img_path}");
        }
        Err(e) => log::warn!("Failed to restore particle image for layer {layer_idx}: {e}"),
    }
}
