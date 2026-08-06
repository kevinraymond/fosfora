//! Applying a binding's output to app state — the single dispatch.
//!
//! Extracted verbatim from `App::apply_binding_target` so the headless scene
//! renderer (#2027) runs the identical code instead of a copy that drifts.
//! `bindings/catalog.rs` pins the `postfx`/`particle`/`uniform` leaf names by
//! scanning THIS file's match arms — the marker strings
//! (`BindingTarget::PostFx(rest) => {`, …) moved here byte-identical, and the
//! catalog test's `include_str!` points here. If you rename or restructure the
//! arms, that test fails loudly with "marker not found"; retarget it in the
//! same change.

use crate::effect::format::PfxEffect;
use crate::gpu::layer::LayerStack;
use crate::gpu::uniforms::ShaderUniforms;
use crate::params::ParamValue;

/// The disjoint slices of app state a binding can write. Both the live `App`
/// and the headless `SceneRenderer` build one of these from their own fields.
pub(crate) struct BindingTargetCtx<'a> {
    pub layer_stack: &'a mut LayerStack,
    /// Loaded effects, for the legacy indexless target's name check.
    pub effects: &'a [PfxEffect],
    pub uniforms: &'a mut ShaderUniforms,
    /// `scene.transport.*` actions land here; the event loop drains them.
    pub pending_triggers: &'a mut Vec<String>,
}

pub(crate) fn apply_binding_target(
    ctx: &mut BindingTargetCtx<'_>,
    target: &crate::bindings::types::BindingTarget,
    value: f32,
    rising: bool,
) {
    use crate::bindings::types::{BindingTarget, LayerField};
    match target {
        // Unset is a half-made binding; Unknown is one we did not recognise
        // and are carrying verbatim rather than discarding.
        BindingTarget::Unset | BindingTarget::Unknown(_) => {}

        BindingTarget::Param { layer, param, .. } => {
            apply_param_binding(ctx.layer_stack, *layer, param, value);
        }

        // Pre-#1792 form: no index, so it means the ACTIVE layer, and only
        // when that layer really runs the named effect. `*` matched any.
        BindingTarget::LegacyParam { effect, param } => {
            let active = ctx.layer_stack.active_layer;
            let matches = effect == "*"
                || ctx
                    .layer_stack
                    .active()
                    .and_then(|l| l.effect_index())
                    .and_then(|idx| ctx.effects.get(idx))
                    .map(|eff| &eff.name == effect)
                    .unwrap_or(false);
            if matches {
                apply_param_binding(ctx.layer_stack, active, param, value);
            }
        }

        BindingTarget::Layer { layer, field } => {
            if let Some(l) = ctx.layer_stack.layers.get_mut(*layer) {
                match field {
                    LayerField::Opacity => l.opacity = value.clamp(0.0, 1.0),
                    LayerField::Blend => {
                        use crate::gpu::layer::BlendMode;
                        // Bus outputs are normalized 0..1 (#1792): spread across
                        // the 10 color modes instead of rounding to 0|1
                        // (Normal|Add). The displacement modes are deliberately
                        // outside the sweep — see BlendMode::from_normalized.
                        // The raw-integer OSC/WS paths use from_u32 directly.
                        l.blend_mode = BlendMode::from_normalized(value);
                    }
                    LayerField::Displace => l.displace_amount = value.clamp(0.0, 1.0),
                    LayerField::Enabled => l.enabled = value > 0.5,
                }
            }
        }

        BindingTarget::GlobalMasterOpacity => {
            let clamped = value.clamp(0.0, 1.0);
            for layer in &mut ctx.layer_stack.layers {
                layer.opacity = clamped;
            }
        }

        // Edge-triggered (#1791): fire only on the frame the output rises
        // above 0.5, not every frame a source is held high.
        BindingTarget::SceneTransport(action) => {
            if rising && !action.is_empty() {
                ctx.pending_triggers
                    .push(format!("scene.transport.{action}"));
            }
        }

        BindingTarget::PostFx(rest) => {
            if let Some(layer) = ctx.layer_stack.active_mut() {
                let rest = rest.as_str();
                match rest {
                    "bloom_threshold" => {
                        layer.postprocess.bloom_threshold = value * 1.5;
                    }
                    "bloom_intensity" => {
                        layer.postprocess.bloom_intensity = value.clamp(0.0, 1.0);
                    }
                    "vignette" => {
                        layer.postprocess.vignette = value.clamp(0.0, 1.0);
                    }
                    "ca_intensity" => {
                        layer.postprocess.ca_intensity = value.clamp(0.0, 1.0);
                    }
                    "grain_intensity" => {
                        layer.postprocess.grain_intensity = value.clamp(0.0, 1.0);
                    }
                    "grain_rate" => {
                        // Hz, not 0..1 like its neighbours — the bus
                        // delivers normalized, the field is a rate.
                        layer.postprocess.grain_rate = value.clamp(0.0, 1.0) * 60.0;
                    }
                    _ => {}
                }
            }
        }

        // Applies to every layer's particle system.
        BindingTarget::Particle(rest) => {
            let v = value.clamp(0.0, 1.0);
            let rest = rest.as_str();
            for layer in &mut ctx.layer_stack.layers {
                if let Some(effect) = layer.as_effect_mut() {
                    if let Some(ps) = effect.pass_executor.particle_system.as_mut() {
                        match rest {
                            "emit_rate" => {
                                let r = v * 10000.0;
                                ps.emit_rate = r;
                                ps.def.emit_rate = r;
                            }
                            "burst_on_beat" => {
                                let r = (v * 2000.0).round() as u32;
                                ps.burst_on_beat = r;
                                ps.def.burst_on_beat = r;
                            }
                            "lifetime" => ps.def.lifetime = 0.5 + v * 29.5,
                            "speed" => ps.def.initial_speed = v * 2.0,
                            "size" => {
                                ps.def.initial_size = 0.001 + v * 0.099;
                            }
                            "drag" => ps.def.drag = 0.8 + v * 0.2,
                            "turbulence" => ps.def.turbulence = v * 2.0,
                            "gravity_x" => {
                                ps.def.gravity[0] = -2.0 + v * 4.0;
                            }
                            "gravity_y" => {
                                ps.def.gravity[1] = -2.0 + v * 4.0;
                            }
                            "vortex_strength" => {
                                ps.def.vortex_strength = -5.0 + v * 10.0;
                            }
                            // Obstacle state lives on the system itself, not the
                            // def (system.rs), so these write ps.* only (#1793).
                            "obstacle_enabled" => ps.obstacle_enabled = v > 0.5,
                            "obstacle_mode" => {
                                // Bus outputs are normalized 0..1 (#1792): spread
                                // across all 4 modes. The raw-integer OSC path
                                // uses from_u32 directly.
                                ps.obstacle_mode =
                                    crate::gpu::particle::ObstacleMode::from_normalized(v);
                            }
                            "obstacle_threshold" => ps.obstacle_threshold = v,
                            "obstacle_elasticity" => ps.obstacle_elasticity = v,
                            _ => {}
                        }
                    }
                }
            }
        }

        // Direct shader uniform override.
        BindingTarget::Uniform(rest) => {
            let v = value.clamp(0.0, 1.0);
            let rest = rest.as_str();
            match rest {
                "sub_bass" => ctx.uniforms.sub_bass = v,
                "bass" => ctx.uniforms.bass = v,
                "low_mid" => ctx.uniforms.low_mid = v,
                "mid" => ctx.uniforms.mid = v,
                "upper_mid" => ctx.uniforms.upper_mid = v,
                "presence" => ctx.uniforms.presence = v,
                "brilliance" => ctx.uniforms.brilliance = v,
                "rms" => ctx.uniforms.rms = v,
                "kick" => ctx.uniforms.kick = v,
                "centroid" => ctx.uniforms.centroid = v,
                "flux" => ctx.uniforms.flux = v,
                "flatness" => ctx.uniforms.flatness = v,
                "rolloff" => ctx.uniforms.rolloff = v,
                "bandwidth" => ctx.uniforms.bandwidth = v,
                "zcr" => ctx.uniforms.zcr = v,
                "onset" => ctx.uniforms.onset = v,
                "beat" => ctx.uniforms.beat = v,
                "beat_phase" => ctx.uniforms.beat_phase = v,
                "bpm" => ctx.uniforms.bpm = v,
                "beat_strength" => ctx.uniforms.beat_strength = v,
                "dominant_chroma" => ctx.uniforms.dominant_chroma = v,
                // Reserved audio features (batched ABI bump #1505) — allow
                // manual override before their detectors land.
                "loudness_m" => ctx.uniforms.loudness_m = v,
                "loudness_s" => ctx.uniforms.loudness_s = v,
                "loudness_trend" => ctx.uniforms.loudness_trend = v,
                "key_class" => ctx.uniforms.key_class = v,
                "key_is_minor" => ctx.uniforms.key_is_minor = v,
                "key_confidence" => ctx.uniforms.key_confidence = v,
                "downbeat" => ctx.uniforms.downbeat = v,
                "bar_phase" => ctx.uniforms.bar_phase = v,
                "beat_in_bar" => ctx.uniforms.beat_in_bar = v,
                "pan" => ctx.uniforms.pan = v,
                "stereo_width" => ctx.uniforms.stereo_width = v,
                "stereo_corr" => ctx.uniforms.stereo_corr = v,
                // A13b per-band pan (#1801).
                "band_pan_sub_bass" => ctx.uniforms.band_pan[0] = v,
                "band_pan_bass" => ctx.uniforms.band_pan[1] = v,
                "band_pan_low_mid" => ctx.uniforms.band_pan[2] = v,
                "band_pan_mid" => ctx.uniforms.band_pan[3] = v,
                "band_pan_upper_mid" => ctx.uniforms.band_pan[4] = v,
                "band_pan_presence" => ctx.uniforms.band_pan[5] = v,
                "band_pan_brilliance" => ctx.uniforms.band_pan[6] = v,
                "section_novelty" => ctx.uniforms.section_novelty = v,
                "buildup" => ctx.uniforms.buildup = v,
                "drop" => ctx.uniforms.drop = v,
                // Reserved audio features (batched ABI bump #1629, "v3").
                "percussive_energy" => ctx.uniforms.percussive_energy = v,
                "harmonic_energy" => ctx.uniforms.harmonic_energy = v,
                "harmonic_ratio" => ctx.uniforms.harmonic_ratio = v,
                "pitch" => ctx.uniforms.pitch = v,
                "pitch_confidence" => ctx.uniforms.pitch_confidence = v,
                "contrast_0" => ctx.uniforms.contrast_0 = v,
                "contrast_1" => ctx.uniforms.contrast_1 = v,
                "contrast_2" => ctx.uniforms.contrast_2 = v,
                "contrast_3" => ctx.uniforms.contrast_3 = v,
                "contrast_4" => ctx.uniforms.contrast_4 = v,
                "contrast_5" => ctx.uniforms.contrast_5 = v,
                "contrast_mean" => ctx.uniforms.contrast_mean = v,
                "timbre_flux" => ctx.uniforms.timbre_flux = v,
                "feedback_decay" => ctx.uniforms.feedback_decay = v,
                "time" => ctx.uniforms.time = value, // time not clamped
                _ => {}
            }
        }
    }
}

/// Write one normalized value into a layer's param store, scaled to that
/// param's declared range.
pub(crate) fn apply_param_binding(
    layer_stack: &mut LayerStack,
    layer_idx: usize,
    param_name: &str,
    value: f32,
) {
    let Some(layer) = layer_stack.layers.get_mut(layer_idx) else {
        return;
    };
    let Some(def) = layer
        .param_store
        .defs
        .iter()
        .find(|d| d.name() == param_name)
        .cloned()
    else {
        return;
    };
    match def {
        crate::params::ParamDef::Float { min, max, .. } => {
            let val = min + (max - min) * value.clamp(0.0, 1.0);
            layer.param_store.set(param_name, ParamValue::Float(val));
        }
        crate::params::ParamDef::Bool { .. } => {
            layer
                .param_store
                .set(param_name, ParamValue::Bool(value > 0.5));
        }
        _ => {}
    }
}

/// Upgrade the pre-#1792 indexless `param.{effect}.{param}` form now that we
/// know which layer runs that effect. Was a splitn(3) on the raw string, which
/// could not tell a 3-part target from a 4-part one whose param name happened
/// to contain a dot; the parse settles that at load. Shared by the app's
/// preset load and the headless renderer's.
pub(crate) fn upgrade_legacy_targets(
    bus: &mut crate::bindings::bus::BindingBus,
    preset: &crate::preset::Preset,
) {
    for binding in &mut bus.bindings {
        if binding.scope != crate::bindings::types::BindingScope::Preset {
            continue;
        }
        if let crate::bindings::types::BindingTarget::LegacyParam { effect, param } =
            &binding.target
        {
            if let Some(idx) = preset.layers.iter().position(|l| &l.effect_name == effect) {
                binding.target = crate::bindings::types::BindingTarget::Param {
                    layer: idx,
                    effect: effect.clone(),
                    param: param.clone(),
                };
            }
        }
    }
}
