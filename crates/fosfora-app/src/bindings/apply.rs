//! Applying a binding's output to app state — the single dispatch.
//!
//! Extracted verbatim from `App::apply_binding_target` so the headless scene
//! renderer (#2027) runs the identical code instead of a copy that drifts.
//! Every leaf is an enum variant parsed once at load (`bindings/target_fields.rs`),
//! so each match below is exhaustive: a new target is a compile error here until
//! it is handled, and a misspelled one never reaches this file (#150).

use crate::bindings::types::{ParticleField, PostFxField, TransportAction, UniformField};
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
    pub pending_triggers: &'a mut Vec<TransportAction>,
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
            if rising {
                ctx.pending_triggers.push(*action);
            }
        }

        BindingTarget::PostFx(field) => apply_postfx(ctx.postprocess, *field, value),

        // Applies to every layer's particle system, as this frame's overlay
        // over the saved def and obstacle settings (#4055).
        BindingTarget::Particle(field) => {
            let v = value.clamp(0.0, 1.0);
            for layer in &mut ctx.layer_stack.layers {
                if let Some(effect) = layer.as_effect_mut() {
                    if let Some(ps) = effect.pass_executor.particle_system.as_mut() {
                        apply_particle(&mut ps.modulation, *field, v);
                    }
                }
            }
        }

        // Direct shader uniform override.
        BindingTarget::Uniform(field) => apply_uniform(ctx.uniforms, *field, value),
    }
}

fn apply_postfx(pp: &mut PostProcessModulation, field: PostFxField, value: f32) {
    let unit = value.clamp(0.0, 1.0);
    match field {
        PostFxField::BloomThreshold => pp.bloom_threshold = Some(value * 1.5),
        PostFxField::BloomIntensity => pp.bloom_intensity = Some(unit),
        PostFxField::Vignette => pp.vignette = Some(unit),
        PostFxField::CaIntensity => pp.ca_intensity = Some(unit),
        PostFxField::GrainIntensity => pp.grain_intensity = Some(unit),
        // Hz, not 0..1 like its neighbors — the bus delivers normalized, the
        // field is a rate.
        PostFxField::GrainRate => pp.grain_rate = Some(unit * 60.0),
    }
}

/// `v` is already clamped to 0..1.
fn apply_particle(
    m: &mut crate::gpu::particle::system::ParticleModulation,
    field: ParticleField,
    v: f32,
) {
    match field {
        ParticleField::EmitRate => m.emit_rate = Some(v * 10000.0),
        ParticleField::BurstOnBeat => m.burst_on_beat = Some((v * 2000.0).round() as u32),
        ParticleField::Lifetime => m.lifetime = Some(0.5 + v * 29.5),
        ParticleField::Speed => m.initial_speed = Some(v * 2.0),
        ParticleField::Size => m.initial_size = Some(0.001 + v * 0.099),
        ParticleField::Drag => m.drag = Some(0.8 + v * 0.2),
        ParticleField::Turbulence => m.turbulence = Some(v * 2.0),
        ParticleField::GravityX => m.gravity_x = Some(-2.0 + v * 4.0),
        ParticleField::GravityY => m.gravity_y = Some(-2.0 + v * 4.0),
        ParticleField::VortexStrength => m.vortex_strength = Some(-5.0 + v * 10.0),
        ParticleField::ObstacleEnabled => m.obstacle_enabled = Some(v > 0.5),
        // Bus outputs are normalized 0..1 (#1792): spread across all 4 modes.
        // The raw-integer OSC path uses from_u32 directly.
        ParticleField::ObstacleMode => {
            m.obstacle_mode = Some(crate::gpu::particle::ObstacleMode::from_normalized(v));
        }
        ParticleField::ObstacleThreshold => m.obstacle_threshold = Some(v),
        ParticleField::ObstacleElasticity => m.obstacle_elasticity = Some(v),
    }
}

fn apply_uniform(u: &mut ShaderUniforms, field: UniformField, value: f32) {
    use UniformField as F;
    let v = value.clamp(0.0, 1.0);
    let slot = match field {
        F::SubBass => &mut u.sub_bass,
        F::Bass => &mut u.bass,
        F::LowMid => &mut u.low_mid,
        F::Mid => &mut u.mid,
        F::UpperMid => &mut u.upper_mid,
        F::Presence => &mut u.presence,
        F::Brilliance => &mut u.brilliance,
        F::Rms => &mut u.rms,
        F::Kick => &mut u.kick,
        F::Centroid => &mut u.centroid,
        F::Flux => &mut u.flux,
        F::Flatness => &mut u.flatness,
        F::Rolloff => &mut u.rolloff,
        F::Bandwidth => &mut u.bandwidth,
        F::Zcr => &mut u.zcr,
        F::Onset => &mut u.onset,
        F::Beat => &mut u.beat,
        F::BeatPhase => &mut u.beat_phase,
        F::Bpm => &mut u.bpm,
        F::BeatStrength => &mut u.beat_strength,
        F::DominantChroma => &mut u.dominant_chroma,
        F::LoudnessM => &mut u.loudness_m,
        F::LoudnessS => &mut u.loudness_s,
        F::LoudnessTrend => &mut u.loudness_trend,
        F::KeyClass => &mut u.key_class,
        F::KeyIsMinor => &mut u.key_is_minor,
        F::KeyConfidence => &mut u.key_confidence,
        F::Downbeat => &mut u.downbeat,
        F::BarPhase => &mut u.bar_phase,
        F::BeatInBar => &mut u.beat_in_bar,
        F::Pan => &mut u.pan,
        F::StereoWidth => &mut u.stereo_width,
        F::StereoCorr => &mut u.stereo_corr,
        F::BandPanSubBass => &mut u.band_pan[0],
        F::BandPanBass => &mut u.band_pan[1],
        F::BandPanLowMid => &mut u.band_pan[2],
        F::BandPanMid => &mut u.band_pan[3],
        F::BandPanUpperMid => &mut u.band_pan[4],
        F::BandPanPresence => &mut u.band_pan[5],
        F::BandPanBrilliance => &mut u.band_pan[6],
        F::SectionNovelty => &mut u.section_novelty,
        F::Buildup => &mut u.buildup,
        F::Drop => &mut u.drop,
        F::PercussiveEnergy => &mut u.percussive_energy,
        F::HarmonicEnergy => &mut u.harmonic_energy,
        F::HarmonicRatio => &mut u.harmonic_ratio,
        F::Pitch => &mut u.pitch,
        F::PitchConfidence => &mut u.pitch_confidence,
        F::Contrast0 => &mut u.contrast_0,
        F::Contrast1 => &mut u.contrast_1,
        F::Contrast2 => &mut u.contrast_2,
        F::Contrast3 => &mut u.contrast_3,
        F::Contrast4 => &mut u.contrast_4,
        F::Contrast5 => &mut u.contrast_5,
        F::ContrastMean => &mut u.contrast_mean,
        F::TimbreFlux => &mut u.timbre_flux,
        F::TempoConfidence => &mut u.tempo_confidence,
        F::BeatLocked => &mut u.beat_locked,
        F::FeedbackDecay => &mut u.feedback_decay,
        // Time is a clock, not a 0..1 feature: not clamped.
        F::Time => {
            u.time = value;
            return;
        }
    };
    *slot = v;
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
    // The def is read in place, not cloned: this runs per bound param per
    // frame, and a clone copied the def's name string every time (#150).
    let Some(def) = store.defs.iter().find(|d| d.name() == param_name) else {
        return;
    };
    let modulated = match *def {
        crate::params::ParamDef::Float { min, max, .. } => {
            ParamValue::Float(min + (max - min) * value.clamp(0.0, 1.0))
        }
        crate::params::ParamDef::Bool { .. } => ParamValue::Bool(value > 0.5),
        _ => return,
    };
    store.set_modulated(param_name, modulated);
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
            &BindingTarget::PostFx(PostFxField::BloomIntensity),
            0.9,
            false,
        );
        apply_binding_target(
            &mut ctx,
            &BindingTarget::PostFx(PostFxField::GrainRate),
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
