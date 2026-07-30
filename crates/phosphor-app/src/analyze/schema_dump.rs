//! `--dump-schema [--out <path>]`: what this build can be told to do, as JSON.
//!
//! The scene generator (#2027 part b) has to author presets, bindings and cue
//! lists against 48 effects, 228 param names and ~80 binding sources. Every one
//! of those names is a string that fails *quietly* when wrong — an unknown param
//! key is dropped, an unknown source never fires, an unhandled target loads and
//! does nothing. A generator working from a hand-written list of names would
//! drift from the app the first time an effect gained a param.
//!
//! So the app states its own vocabulary. Everything here is read from the same
//! place the running app reads it:
//!
//! * effects from the real `.pfx` scan, not a checked-in copy;
//! * sources by calling the real collectors and taking the keys they emit, so a
//!   renamed feature shows up as a missing source rather than a stale entry;
//! * targets, limits and enums from [`crate::bindings::catalog`] and the enums'
//!   own `ALL` arrays.
//!
//! No GPU, no window, no audio device — same early-exit path as `--analyze`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use bytemuck::Zeroable;
use serde::Serialize;

use crate::audio::AudioFeatures;
use crate::audio::analyzer::SPECTROGRAM_MELS;
use crate::bindings::catalog;
use crate::bindings::transforms::CURVE_TYPES;
use crate::effect::format::{AudioMapping, PfxEffect};
use crate::gpu::layer::BlendMode;
use crate::params::ParamDef;

/// Bumped when the shape below changes in a way a pinned generator would care
/// about. Separate from [`crate::analyze::report::ANALYSIS_VERSION`]: the two
/// files move independently.
pub const CAPABILITIES_VERSION: u32 = 1;

#[derive(Serialize)]
pub struct Capabilities {
    pub capabilities_version: u32,
    /// The analysis-report version this build emits, so a generator can check
    /// both halves of its input against one file.
    pub analysis_version: u32,
    pub app_version: &'static str,
    pub effects: Vec<EffectCapability>,
    /// Every binding source key this build can produce, sorted. Audio only —
    /// MIDI, OSC and WebSocket keys depend on connected hardware and live input,
    /// so they cannot be enumerated offline.
    pub sources: Vec<String>,
    pub targets: Targets,
    pub limits: Limits,
    pub enums: Enums,
    pub encoding: Encoding,
}

#[derive(Serialize)]
pub struct EffectCapability {
    pub name: String,
    pub description: String,
    /// `shader`, `particle` or `feedback` — auto-detected when the `.pfx` omits it.
    pub effect_type: String,
    /// Hidden effects load but are not offered in the picker; a generated preset
    /// should not name one.
    pub hidden: bool,
    pub inputs: Vec<ParamDef>,
    /// Prose, display-only in the app: the `feature` side is not validated
    /// against the source list and the `target` side is human text, not a
    /// binding target. Useful to a generator as a hint about intent, useless as
    /// a source of names.
    pub audio_mappings: Vec<AudioMapping>,
    pub pass_count: usize,
    pub has_particles: bool,
}

#[derive(Serialize)]
pub struct Targets {
    /// The dotted forms a target string may take, in the order
    /// `parse_target` discriminates them.
    pub grammar: Vec<&'static str>,
    pub postfx: Vec<&'static str>,
    pub particle: Vec<&'static str>,
    pub uniform: Vec<&'static str>,
    pub layer_fields: Vec<&'static str>,
    pub scene_transport: Vec<&'static str>,
    pub global: Vec<&'static str>,
}

#[derive(Serialize)]
pub struct Limits {
    /// A preset with more layers than this loads with the extras silently
    /// dropped.
    pub max_layers: usize,
    pub bindings_file_version: u32,
    pub scene_set_version: u32,
}

#[derive(Serialize)]
pub struct Enums {
    pub blend_modes: Vec<String>,
    pub transform_types: Vec<&'static str>,
    pub curve_types: Vec<&'static str>,
    pub transitions: Vec<&'static str>,
    pub advance_modes: Vec<&'static str>,
    pub binding_scopes: Vec<&'static str>,
}

/// On-disk quirks a generator has to get exactly right, stated rather than
/// inferred.
#[derive(Serialize)]
pub struct Encoding {
    /// `ParamValue` is an externally tagged enum: the value is wrapped in a
    /// single-key object named after the variant.
    pub param_value: Vec<&'static str>,
    /// `BindingTarget` serializes as the flat dotted string, not as a struct.
    pub binding_target: &'static str,
    /// Under `Timer`, a cue with no `hold_secs` never advances.
    pub timer_requires_hold_secs: bool,
    /// A bindings sidecar is only applied when every binding is preset-scoped.
    pub sidecar_scope: &'static str,
}

/// Serialize an enum through serde and unquote it, so the dump carries the wire
/// spelling rather than the Rust identifier. The two differ: `EffectType` is
/// `rename_all = "lowercase"`, and `BlendMode::Overlay` carries a `SoftLight`
/// read alias.
fn wire_name<T: Serialize + std::fmt::Debug>(value: &T) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|e| panic!("{value:?} does not serialize: {e}"))
        .trim_matches('"')
        .to_string()
}

/// Every audio source key this build emits.
///
/// Calls the real collectors over zeroed inputs and takes their keys. The values
/// are irrelevant — the point is that the key *set* comes from the code that
/// produces it at runtime, so it cannot describe a source that does not exist.
pub fn source_keys() -> Vec<String> {
    use crate::bindings::sources;

    let features = AudioFeatures::zeroed();
    let mut keys: Vec<String> = sources::collect_audio(&features).into_keys().collect();
    keys.extend(sources::collect_mel_bands(&vec![0.0; SPECTROGRAM_MELS]).into_keys());
    keys.extend(sources::collect_dmfcc_bands(&[0.0; 13]).into_keys());
    keys.sort();
    keys.dedup();
    keys
}

fn effects(loader_effects: &[PfxEffect]) -> Vec<EffectCapability> {
    loader_effects
        .iter()
        .map(|e| EffectCapability {
            name: e.name.clone(),
            description: e.description.clone(),
            effect_type: wire_name(&e.effect_type()),
            hidden: e.hidden,
            inputs: e.inputs.clone(),
            audio_mappings: e.audio_mappings.clone(),
            pass_count: e.passes.len(),
            has_particles: e.particles.is_some(),
        })
        .collect()
}

pub fn build(loader_effects: &[PfxEffect]) -> Capabilities {
    Capabilities {
        capabilities_version: CAPABILITIES_VERSION,
        analysis_version: crate::analyze::report::ANALYSIS_VERSION,
        app_version: env!("CARGO_PKG_VERSION"),
        effects: effects(loader_effects),
        sources: source_keys(),
        targets: Targets {
            grammar: vec![
                "param.{layer}.{effect}.{param}",
                "param.{effect}.{param}",
                "layer.{layer}.{field}",
                "postfx.{field}",
                "particle.{field}",
                "uniform.{field}",
                "scene.transport.{action}",
                "global.master_opacity",
            ],
            postfx: catalog::POSTFX_TARGETS.to_vec(),
            particle: catalog::PARTICLE_TARGETS.to_vec(),
            uniform: catalog::UNIFORM_TARGETS.to_vec(),
            layer_fields: catalog::layer_fields().to_vec(),
            scene_transport: catalog::SCENE_TRANSPORT_ACTIONS.to_vec(),
            global: vec!["global.master_opacity"],
        },
        limits: Limits {
            max_layers: catalog::MAX_LAYERS,
            bindings_file_version: 1,
            scene_set_version: 1,
        },
        enums: Enums {
            // Through serde, so the wire spelling is what a file needs, not the
            // Rust identifier.
            blend_modes: BlendMode::ALL.iter().map(wire_name).collect(),
            transform_types: vec![
                "remap", "smooth", "invert", "quantize", "deadzone", "curve", "gate", "scale",
                "offset", "clamp",
            ],
            curve_types: CURVE_TYPES.to_vec(),
            transitions: vec!["Cut", "Dissolve", "ParamMorph"],
            advance_modes: vec!["Manual", "Timer", "BeatSync"],
            binding_scopes: vec!["Preset", "Global"],
        },
        encoding: Encoding {
            param_value: vec!["Float", "Color", "Bool", "Point2D"],
            binding_target: "flat dotted string, e.g. \"param.0.aurora.curtain_speed\"",
            timer_requires_hold_secs: true,
            sidecar_scope: "Preset",
        },
    }
}

/// `--dump-schema [--out <path>]` entry point. Returns the path written.
pub fn run(out: Option<&Path>) -> Result<PathBuf> {
    let mut loader = crate::effect::loader::EffectLoader::new();
    // Returns nothing and warn-logs its failures: a missing assets/effects, or a
    // `.pfx` that fails to parse, leaves the list short rather than erroring.
    // Silence there would be a dump that says this build has no effects, which a
    // generator would read as "every effect name is invalid".
    loader.scan_effects_directory();
    if loader.effects.is_empty() {
        bail!(
            "no .pfx effects found — run from the repo root, or beside a build with an \
             assets/ directory. Writing a schema with no effects would be worse than failing."
        );
    }

    let caps = build(&loader.effects);
    let out_path = out.map_or_else(|| PathBuf::from("capabilities.json"), Path::to_path_buf);
    let json = serde_json::to_string_pretty(&caps).context("serializing capabilities")?;
    std::fs::write(&out_path, json).with_context(|| format!("writing {}", out_path.display()))?;
    Ok(out_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The source list is the generator's entire vocabulary for the left-hand
    /// side of a binding, so an empty or truncated one is worse than useless —
    /// it would make every binding look invalid. Pin the families rather than an
    /// exact count, which would churn on every new DSP feature.
    #[test]
    fn source_keys_cover_every_family() {
        let keys = source_keys();

        for expected in [
            "audio.rms",
            "audio.kick",
            "audio.percussive_energy",
            "audio.harmonic_ratio",
            "audio.band.0",
            "audio.band.6",
            "audio.mfcc.0",
            "audio.chroma.0",
            "audio.chroma.11",
            "audio.mel.0",
            "audio.dmfcc.0",
            "audio.dmfcc.12",
            "audio.contrast_mean",
            "audio.key_hue",
        ] {
            assert!(
                keys.iter().any(|k| k == expected),
                "{expected} missing from the dumped source list"
            );
        }

        assert_eq!(
            keys.iter().filter(|k| k.starts_with("audio.mel.")).count(),
            SPECTROGRAM_MELS,
            "mel family should have one key per band"
        );
        assert!(
            keys.windows(2).all(|w| w[0] < w[1]),
            "keys must be sorted and unique"
        );
    }

    /// `BlendMode::Overlay` carries a `SoftLight` alias, so the Rust identifier
    /// and the wire name are not interchangeable in general. Dump the wire name.
    #[test]
    fn blend_modes_are_wire_names() {
        let caps = build(&[]);
        assert_eq!(caps.enums.blend_modes.len(), BlendMode::ALL.len());
        assert_eq!(caps.enums.blend_modes[0], "Normal");
        assert!(caps.enums.blend_modes.iter().any(|m| m == "Overlay"));
        assert!(
            !caps.enums.blend_modes.iter().any(|m| m == "SoftLight"),
            "SoftLight is a read alias, not the name to write"
        );
    }

    /// Every transform type named in the dump must actually deserialize into a
    /// `TransformDef`. Catches the dump drifting from the enum's serde renames.
    #[test]
    fn transform_types_deserialize() {
        use crate::bindings::types::TransformDef;

        let caps = build(&[]);
        for ty in &caps.enums.transform_types {
            // Supply every field any variant needs; serde ignores the extras.
            let json = format!(
                r#"{{"type":"{ty}","in_lo":0.0,"in_hi":1.0,"out_lo":0.0,"out_hi":1.0,
                     "factor":0.5,"steps":4,"lo":0.0,"hi":1.0,
                     "curve_type":"linear","threshold":0.5,"value":0.0}}"#
            );
            serde_json::from_str::<TransformDef>(&json)
                .unwrap_or_else(|e| panic!("transform type {ty:?} does not deserialize: {e}"));
        }
    }

    /// Curve names come from `CURVE_TYPES`, which exists because the picker and
    /// `apply_curve` drifted apart once already. Keep the dump honest about it.
    #[test]
    fn curve_types_are_all_handled() {
        use crate::bindings::transforms::apply_chain;
        use crate::bindings::types::{BindingRuntime, TransformDef};

        let caps = build(&[]);
        for curve in &caps.enums.curve_types {
            let mut rt = BindingRuntime::new();
            let chain = [TransformDef::Curve {
                curve_type: (*curve).to_string(),
            }];
            // Not 0.5: that is a fixed point of smoothstep, so `ease_in_out`
            // legitimately returns its input there and would read as unhandled.
            // 0.25 separates every curve in the list from identity.
            const PROBE: f32 = 0.25;
            let out = apply_chain(PROBE, &chain, &mut rt);
            if *curve != "linear" {
                assert!(
                    (out - PROBE).abs() > 1e-6,
                    "curve {curve:?} evaluated to identity at {PROBE} — it is offered but \
                     unhandled by apply_curve"
                );
            } else {
                assert!((out - PROBE).abs() < 1e-6, "linear must be identity");
            }
        }
    }
}
