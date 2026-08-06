//! Per-frame derived state for every effect layer, in one place.
//!
//! Extracted verbatim from `App::update`'s layer loop for the headless scene
//! renderer (#2027). This is the block no offscreen probe exercises — uniform
//! propagation, particle audio, obstacle video/model re-raster, water/fluid
//! reconciliation, splat/helix CPU drivers, volumetric gating, symbiosis and
//! morph state. Skipping any piece of it produces frames that render without
//! error and are silently wrong, which is exactly what a judging pipeline must
//! not do — so both the app and the renderer call this one function.

use wgpu::{Device, Queue};

use crate::audio::AudioFeatures;
use crate::gpu::context::GpuContext;
use crate::gpu::layer::{Layer, LayerContent};
use crate::gpu::uniforms::ShaderUniforms;
use crate::gpu::volumetric::VolumetricParams;

/// Push this frame's global uniforms + audio into every effect layer and run
/// each particle system's CPU-side per-frame work. `global` is the fully
/// mirrored uniform template (time/dt/resolution/features already set).
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_effect_layers(
    layers: &mut [Layer],
    global: &ShaderUniforms,
    audio: &AudioFeatures,
    dt: f32,
    device: &Device,
    queue: &Queue,
    active_layer_idx: usize,
    vol_enabled: bool,
    vol_params: VolumetricParams,
) {
    let vol_hdr = GpuContext::hdr_format();
    for (layer_idx, layer) in layers.iter_mut().enumerate() {
        if let LayerContent::Effect(ref mut e) = layer.content {
            e.uniforms = *global;
            e.uniforms.params = layer.param_store.pack_to_buffer();

            // Update particle systems
            if let Some(ref mut ps) = e.pass_executor.particle_system {
                ps.update_uniforms(dt, global.time, global.resolution, global.beat);
                // Forward first 8 effect params to compute shader
                let p = e.uniforms.params;
                ps.uniforms.effect_params = [p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7]];
                // Advance obstacle video playback
                if ps.obstacle_source == "video" {
                    ps.advance_obstacle_video(device, queue, dt as f64);
                }
                // Pack obstacle collision uniforms
                ps.uniforms.obstacle_enabled = if ps.obstacle_enabled { 1.0 } else { 0.0 };
                ps.uniforms.obstacle_threshold = ps.obstacle_threshold;
                ps.uniforms.obstacle_mode = ps.obstacle_mode as u32;
                ps.uniforms.obstacle_elasticity = ps.obstacle_elasticity;
                ps.uniforms.obstacle_fit = ps.obstacle_fit as u32;
                let audio = *audio;
                ps.update_audio(&audio);
                // Re-raster the 3D-model obstacle depth for this frame
                // (#1851). Submitted here in update(), before the particle
                // compute pass samples the obstacle in render().
                if ps.obstacle_source == "model" {
                    ps.render_obstacle_model(device, queue, &audio, dt);
                }
                // Reconcile the obstacle water sim (#1851) before dispatch.
                ps.sync_water(device, queue);
                // Reconcile the obstacle fluid sim (#1939) after water, so the
                // solver can fold pooled water into its solid mask.
                ps.sync_fluid(device, queue);
                // Splat (#1800): camera params ride slots 8–11 and roundness
                // slot 12 (only 0–7 reach the sim); advance the CPU
                // orbit/envelope driver with this frame's dt + audio (no-op
                // for non-splat).
                ps.splat_ui_params = [p[8], p[9], p[10], p[11], p[12]];
                ps.update_splat_driver();

                // Helix (#1802): the ribbon has no sim shader, so its twelve
                // performance knobs ride slots 0–11 and are applied CPU-side.
                // They live in `inputs` rather than the contextual panel
                // precisely so they reach the binding bus — a panel's state
                // never does.
                if ps.helix_enabled {
                    let ui: [f32; crate::gpu::helix::HELIX_PARAM_NAMES.len()] = [
                        p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7], p[8], p[9], p[10], p[11],
                    ];
                    ps.helix_params.apply_ui_params(&ui);
                }

                // Volumetric mode (R3): apply the global toggle to the active
                // particle layer only (V1); lazily build the renderer on enable.
                let vol_on = vol_enabled && layer_idx == active_layer_idx;
                ps.volumetric_enabled = vol_on;
                if vol_on {
                    ps.init_volumetric(device, vol_hdr);
                    ps.volumetric_params = vol_params;
                }

                // Symbiosis force matrix management
                if let Some(ref mut sym) = ps.symbiosis_state {
                    // param(0) = num_species (0-1 maps to 2-8)
                    let ns = (p[0] * 6.0 + 2.0).round() as u32;
                    sym.set_num_species(ns);
                    // param(6) = preset (0-1 maps to preset index)
                    let preset_idx = (p[6]
                        * (crate::gpu::particle::symbiosis::SymbiosisPreset::count() as f32 - 0.01))
                        as usize;
                    sym.set_preset(preset_idx);
                    sym.update(dt, &audio);
                    ps.uniforms.force_matrix = sym.active_matrix();
                }

                // Morph state management
                if let Some(ref mut morph) = ps.morph_state {
                    morph.update(dt, audio.beat, audio.dominant_chroma);
                }
            }
        }
    }
}
