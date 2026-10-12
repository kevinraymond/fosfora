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

use crate::effect::format::{PfxEffect, PostProcessModulation};
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
    /// Master post-processing: preset scope, never a layer's (#3147). The
    /// bus writes this frame's overlay, not the saved def (#4055).
    pub postprocess: &'a mut PostProcessModulation,
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

        // Layer fields are laid over the saved ones for this frame, the way
        // params are: the slider, the preset and a morph keep the user's
        // value (#4055).
        BindingTarget::Layer { layer, field } => {
            if let Some(l) = ctx.layer_stack.layers.get_mut(*layer) {
                let m = &mut l.modulation;
                match field {
                    LayerField::Opacity => m.opacity = Some(value.clamp(0.0, 1.0)),
                    LayerField::Blend => {
                        use crate::gpu::layer::BlendMode;
                        // Bus outputs are normalized 0..1 (#1792): spread across
                        // the 10 color modes instead of rounding to 0|1
                        // (Normal|Add). The displacement modes are deliberately
                        // outside the sweep — see BlendMode::from_normalized.
                        // The raw-integer OSC/WS paths use from_u32 directly.
                        m.blend_mode = Some(BlendMode::from_normalized(value));
                    }
                    LayerField::Displace => m.displace_amount = Some(value.clamp(0.0, 1.0)),
                    LayerField::Enabled => m.enabled = Some(value > 0.5),
                }
            }
        }

        BindingTarget::GlobalMasterOpacity => {
            let clamped = value.clamp(0.0, 1.0);
            for layer in &mut ctx.layer_stack.layers {
                layer.modulation.opacity = Some(clamped);
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
            let pp = &mut *ctx.postprocess;
            match rest.as_str() {
                "bloom_threshold" => {
                    pp.bloom_threshold = Some(value * 1.5);
                }
                "bloom_intensity" => {
                    pp.bloom_intensity = Some(value.clamp(0.0, 1.0));
                }
                "vignette" => {
                    pp.vignette = Some(value.clamp(0.0, 1.0));
                }
                "ca_intensity" => {
                    pp.ca_intensity = Some(value.clamp(0.0, 1.0));
                }
                "grain_intensity" => {
                    pp.grain_intensity = Some(value.clamp(0.0, 1.0));
                }
                "grain_rate" => {
                    // Hz, not 0..1 like its neighbors — the bus
                    // delivers normalized, the field is a rate.
                    pp.grain_rate = Some(value.clamp(0.0, 1.0) * 60.0);
                }
                _ => {}
            }
        }

        // Applies to every layer's particle system, as this frame's overlay
        // over the saved def and obstacle settings (#4055).
        BindingTarget::Particle(rest) => {
            let v = value.clamp(0.0, 1.0);
            let rest = rest.as_str();
            for layer in &mut ctx.layer_stack.layers {
                if let Some(effect) = layer.as_effect_mut() {
                    if let Some(ps) = effect.pass_executor.particle_system.as_mut() {
                        let m = &mut ps.modulation;
                        match rest {
                            "emit_rate" => m.emit_rate = Some(v * 10000.0),
                            "burst_on_beat" => {
                                m.burst_on_beat = Some((v * 2000.0).round() as u32);
                            }
                            "lifetime" => m.lifetime = Some(0.5 + v * 29.5),
                            "speed" => m.initial_speed = Some(v * 2.0),
                            "size" => m.initial_size = Some(0.001 + v * 0.099),
                            "drag" => m.drag = Some(0.8 + v * 0.2),
                            "turbulence" => m.turbulence = Some(v * 2.0),
                            "gravity_x" => m.gravity_x = Some(-2.0 + v * 4.0),
                            "gravity_y" => m.gravity_y = Some(-2.0 + v * 4.0),
                            "vortex_strength" => m.vortex_strength = Some(-5.0 + v * 10.0),
                            "obstacle_enabled" => m.obstacle_enabled = Some(v > 0.5),
                            "obstacle_mode" => {
                                // Bus outputs are normalized 0..1 (#1792): spread
                                // across all 4 modes. The raw-integer OSC path
                                // uses from_u32 directly.
                                m.obstacle_mode =
                                    Some(crate::gpu::particle::ObstacleMode::from_normalized(v));
                            }
                            "obstacle_threshold" => m.obstacle_threshold = Some(v),
                            "obstacle_elasticity" => m.obstacle_elasticity = Some(v),
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
                // Tempo trust (#81).
                "tempo_confidence" => ctx.uniforms.tempo_confidence = v,
                "beat_locked" => ctx.uniforms.beat_locked = v,
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
    modulate_param(&mut layer.param_store, param_name, value);
}

/// Lay a binding's normalized output over a param store's base value for this
/// frame. Modulation is not a user edit: the base value and `ParamStore::changed`
/// are left alone, so a bound preset does not read as dirty every frame, a user
/// effect's `.pfx` defaults are never rewritten with modulated values (#3174),
/// and a preset save or a slider still shows the user's own value (#4053).
pub(crate) fn modulate_param(store: &mut crate::params::ParamStore, param_name: &str, value: f32) {
    let Some(def) = store.defs.iter().find(|d| d.name() == param_name).cloned() else {
        return;
    };
    match def {
        crate::params::ParamDef::Float { min, max, .. } => {
            let val = min + (max - min) * value.clamp(0.0, 1.0);
            store.set_modulated(param_name, ParamValue::Float(val));
        }
        crate::params::ParamDef::Bool { .. } => {
            store.set_modulated(param_name, ParamValue::Bool(value > 0.5));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{ParamDef, ParamStore};

    fn store() -> ParamStore {
        let mut s = ParamStore::new();
        s.load_from_defs(&[
            ParamDef::Float {
                name: "speed".into(),
                default: 0.5,
                min: 0.0,
                max: 2.0,
            },
            ParamDef::Bool {
                name: "active".into(),
                default: false,
            },
        ]);
        s.changed = false;
        s
    }

    #[test]
    fn modulation_renders_the_scaled_value_but_is_not_a_user_edit() {
        let mut s = store();
        modulate_param(&mut s, "speed", 0.75);
        modulate_param(&mut s, "active", 1.0);
        // Scaled into the declared range, and what the shader gets.
        assert!(
            matches!(s.effective("speed"), Some(ParamValue::Float(v)) if (*v - 1.5).abs() < 1e-6)
        );
        assert!((s.pack_to_buffer()[0] - 1.5).abs() < 1e-6);
        assert!(matches!(
            s.effective("active"),
            Some(ParamValue::Bool(true))
        ));
        // The base value the preset saves is untouched (#4053).
        assert!(matches!(s.get("speed"), Some(ParamValue::Float(v)) if (*v - 0.5).abs() < 1e-6));
        assert!(matches!(s.get("active"), Some(ParamValue::Bool(false))));
        // A binding runs every frame; if this were a user edit the preset
        // would read dirty forever and the .pfx autosave would fire (#3174).
        assert!(!s.changed, "binding modulation must not set `changed`");
    }

    #[test]
    fn modulation_keeps_a_pending_user_edit_pending() {
        let mut s = store();
        s.set("speed", ParamValue::Float(1.0));
        modulate_param(&mut s, "speed", 0.0);
        assert!(s.changed, "a real edit earlier in the frame still counts");
        assert!(matches!(s.get("speed"), Some(ParamValue::Float(v)) if (*v - 1.0).abs() < 1e-6));
    }

    #[test]
    fn modulation_ignores_unknown_and_unsupported_params() {
        let mut s = store();
        modulate_param(&mut s, "nope", 0.5);
        assert!(s.get("nope").is_none());
        assert!(s.effective("nope").is_none());
        assert!(!s.changed);
    }

    /// A Master post-processing binding writes this frame's overlay, so the
    /// inspector's slider and the preset keep the saved def (#4055).
    #[test]
    fn a_postfx_binding_writes_the_overlay_not_the_saved_def() {
        use crate::bindings::types::BindingTarget;
        let saved = crate::effect::format::PostProcessDef::default();
        let mut stack = crate::gpu::layer::LayerStack::new();
        let mut uniforms: ShaderUniforms = bytemuck::Zeroable::zeroed();
        let mut modulation = PostProcessModulation::default();
        let mut triggers = Vec::new();
        let mut ctx = BindingTargetCtx {
            layer_stack: &mut stack,
            effects: &[],
            uniforms: &mut uniforms,
            postprocess: &mut modulation,
            pending_triggers: &mut triggers,
        };
        apply_binding_target(
            &mut ctx,
            &BindingTarget::PostFx("bloom_intensity".into()),
            0.9,
            false,
        );
        apply_binding_target(
            &mut ctx,
            &BindingTarget::PostFx("grain_rate".into()),
            0.5,
            false,
        );
        assert_eq!(modulation.bloom_intensity, Some(0.9));
        assert_eq!(modulation.grain_rate, Some(30.0));
        assert_eq!(
            modulation.vignette, None,
            "an unbound setting stays unbound"
        );
        let live = saved.with_modulation(&modulation);
        assert!((live.bloom_intensity - 0.9).abs() < 1e-6);
        assert!((live.vignette - saved.vignette).abs() < 1e-6);
        // The def a preset would save never saw the binding.
        assert_eq!(saved, crate::effect::format::PostProcessDef::default());
    }
}
