//! `--validate <dir>`: reject a generated scene before the app loads it.
//!
//! Every layer of this config format fails *quietly*. An unknown param key is
//! dropped, a param out of range is applied unclamped, a missing media path is
//! skipped by a bare `if path.exists()` with no `else`, an over-tall preset
//! loses its extra layers, a `Timer` cue with no `hold_secs` never advances, and
//! — the worst of them — a binding target that does not parse becomes
//! `BindingTarget::Unknown(raw)`, which round-trips byte-identically, loads with
//! no warning, appears in the UI, and does nothing. A human authoring by hand
//! notices the knob is dead and fixes the spelling. A generator does not.
//!
//! So this runs the real types: [`Preset`], [`BindingsFile`] and [`SceneSet`]
//! deserialize through the same serde impls the app uses, targets go through the
//! same `parse_target`, and leaf names are checked against
//! [`crate::bindings::catalog`], which is itself pinned to `app.rs`. "Would the
//! app accept this" is answered by the app's own code rather than by a
//! reimplementation that can drift — `scripts/capture/check_demos.py` is the
//! cautionary example: it rejects two targets the app accepts
//! (`layer.*.displace`, `postfx.grain_rate`), waves four whole target categories
//! through unchecked, and accepts two source keys that the test suite explicitly
//! asserts are never emitted.
//!
//! Layout expected in `<dir>`:
//!
//! ```text
//! <Name>.json             a Preset
//! <Name>.bindings.json    its sidecar (optional)
//! _scene.json             a SceneSet naming those presets (optional)
//! ```
//!
//! Exit 0 clean, 1 with problems, 2 on usage error.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::bindings::catalog;
use crate::bindings::persistence::BindingsFile;
use crate::bindings::types::{BindingScope, BindingTarget, TransformDef};
use crate::effect::format::PfxEffect;
use crate::params::{ParamDef, ParamValue};
use crate::preset::store::Preset;
use crate::scene::types::{AdvanceMode, SceneSet};

/// One thing wrong with the scene, phrased so a generator repair pass can act on
/// it: what is wrong, where, and what would have been valid.
pub struct Problem {
    pub where_: String,
    pub what: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.where_, self.what)
    }
}

#[derive(Default)]
pub struct Report {
    pub problems: Vec<Problem>,
    /// Things worth saying that are not faults — chiefly the `@REPO@`/`@WORK@`
    /// placeholders in `scripts/capture/demos`, which are substituted before the
    /// app ever sees them. Reported but not counted, so the exit code stays
    /// meaningful when validating a template directory.
    pub notes: Vec<Problem>,
    pub presets_checked: usize,
    pub bindings_checked: usize,
    pub cues_checked: usize,
}

impl Report {
    fn bad(&mut self, where_: impl Into<String>, what: impl Into<String>) {
        self.problems.push(Problem {
            where_: where_.into(),
            what: what.into(),
        });
    }

    fn note(&mut self, where_: impl Into<String>, what: impl Into<String>) {
        self.notes.push(Problem {
            where_: where_.into(),
            what: what.into(),
        });
    }

    pub fn is_clean(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Effects indexed by name, with their params indexed by name.
struct EffectTable {
    by_name: std::collections::HashMap<String, std::collections::HashMap<String, ParamDef>>,
}

impl EffectTable {
    fn build(effects: &[PfxEffect]) -> Self {
        let by_name = effects
            .iter()
            .map(|e| {
                let params = e
                    .inputs
                    .iter()
                    .map(|p| (p.name().to_string(), p.clone()))
                    .collect();
                (e.name.clone(), params)
            })
            .collect();
        Self { by_name }
    }

    fn param(&self, effect: &str, param: &str) -> Option<&ParamDef> {
        self.by_name.get(effect)?.get(param)
    }

    fn has_effect(&self, effect: &str) -> bool {
        self.by_name.contains_key(effect)
    }

    fn known_effects(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.by_name.keys().map(String::as_str).collect();
        v.sort_unstable();
        v
    }
}

/// `parse_target` splits on `.` and discriminates by segment count, so any dot
/// inside an effect or param name silently re-parses as a different variant.
/// `bindings/types.rs` claims this is "guarded by a test over every .pfx" — it
/// was not, until here.
fn check_no_dots(effects: &[PfxEffect], report: &mut Report) {
    for e in effects {
        if e.name.contains('.') {
            report.bad(
                format!("effect '{}'", e.name),
                "effect name contains a dot; param targets for it would re-parse as a \
                 different target variant",
            );
        }
        for p in &e.inputs {
            if p.name().contains('.') {
                report.bad(
                    format!("effect '{}' param '{}'", e.name, p.name()),
                    "param name contains a dot; its target would re-parse as a different \
                     target variant",
                );
            }
        }
    }
}

/// Type tag and range of one param value against the effect's declaration.
fn check_param_value(where_: &str, def: &ParamDef, value: &ParamValue, report: &mut Report) {
    let tag = |v: &ParamValue| match v {
        ParamValue::Float(_) => "Float",
        ParamValue::Color(_) => "Color",
        ParamValue::Bool(_) => "Bool",
        ParamValue::Point2D(_) => "Point2D",
    };
    let want = match def {
        ParamDef::Float { .. } => "Float",
        ParamDef::Color { .. } => "Color",
        ParamDef::Bool { .. } => "Bool",
        ParamDef::Point2D { .. } => "Point2D",
    };
    if tag(value) != want {
        report.bad(
            where_,
            format!(
                "value is {} but the effect declares {want}; ParamStore keys on the declared \
                 type, so this is dropped on load",
                tag(value)
            ),
        );
        return;
    }
    // ParamStore::set does not clamp, so an out-of-range value is applied as
    // given rather than corrected.
    match (def, value) {
        (ParamDef::Float { min, max, .. }, ParamValue::Float(v)) => {
            if v < min || v > max {
                report.bad(
                    where_,
                    format!("{v} is outside the declared range {min}..={max} (applied unclamped)"),
                );
            }
        }
        (ParamDef::Point2D { min, max, .. }, ParamValue::Point2D(v)) => {
            for (i, axis) in ["x", "y"].iter().enumerate() {
                if v[i] < min[i] || v[i] > max[i] {
                    report.bad(
                        where_,
                        format!(
                            "{axis} = {} is outside the declared range {}..={}",
                            v[i], min[i], max[i]
                        ),
                    );
                }
            }
        }
        _ => {}
    }
}

fn check_preset(name: &str, preset: &Preset, effects: &EffectTable, report: &mut Report) {
    if preset.layers.is_empty() {
        report.bad(format!("preset '{name}'"), "has no layers");
        return;
    }
    if preset.layers.len() > catalog::MAX_LAYERS {
        report.bad(
            format!("preset '{name}'"),
            format!(
                "{} layers, but add_layer stops at {} — the extra layers load silently \
                 dropped, with no warning",
                preset.layers.len(),
                catalog::MAX_LAYERS
            ),
        );
    }
    if preset.active_layer >= preset.layers.len() {
        report.bad(
            format!("preset '{name}'"),
            format!(
                "active_layer is {} but there are {} layers",
                preset.active_layer,
                preset.layers.len()
            ),
        );
    }

    for (i, layer) in preset.layers.iter().enumerate() {
        let where_ = format!("preset '{name}' layer {i}");

        // An empty effect_name is not a mistake: it is how a *media* layer is
        // spelled. The app's own predicate for "this layer has no source at all"
        // is empty name AND no media AND no webcam (app.rs), so use the same one
        // rather than flagging every media layer in every preset.
        let is_media_layer = layer.media_path.is_some() || layer.webcam_device.is_some();
        if layer.effect_name.is_empty() {
            if !is_media_layer {
                report.bad(
                    &where_,
                    "effect_name is empty and there is no media_path or webcam_device — \
                     the layer has no source and renders nothing",
                );
            }
        } else if !effects.has_effect(&layer.effect_name) {
            report.bad(
                &where_,
                format!(
                    "no effect named '{}' — the layer loads as the default shader. \
                     Known effects: {}",
                    layer.effect_name,
                    effects.known_effects().join(", ")
                ),
            );
            continue;
        }

        // Params only mean anything against a known effect; a media layer has
        // none to check against.
        for (param, value) in layer
            .params
            .iter()
            .filter(|_| effects.has_effect(&layer.effect_name))
        {
            match effects.param(&layer.effect_name, param) {
                Some(def) => {
                    check_param_value(&format!("{where_} param '{param}'"), def, value, report);
                }
                None => report.bad(
                    &where_,
                    format!(
                        "'{}' has no param '{param}' — unknown keys are dropped with no log",
                        layer.effect_name
                    ),
                ),
            }
        }

        // Asset paths are guarded by a bare `if path.exists()` with no else, so a
        // wrong path is a layer that renders nothing.
        for (field, path) in [
            ("media_path", &layer.media_path),
            ("particle_video_path", &layer.particle_video_path),
            ("particle_image_path", &layer.particle_image_path),
            ("particle_model_path", &layer.particle_model_path),
            ("splat_scene_path", &layer.splat_scene_path),
            ("obstacle_image_path", &layer.obstacle_image_path),
        ] {
            let Some(p) = path else { continue };
            // `scripts/capture/demos` keeps its asset paths as @REPO@/@WORK@
            // templates that capture_advanced.sh substitutes at install time. A
            // path that still holds one is a template, not a broken path — say so
            // without failing, so this stays usable as a smoke test against that
            // directory.
            if p.contains("@REPO@") || p.contains("@WORK@") {
                report.note(
                    &where_,
                    format!("{field} '{p}' is an unsubstituted template placeholder"),
                );
            } else if !Path::new(p).exists() {
                report.bad(
                    &where_,
                    format!("{field} '{p}' does not exist; the load is skipped silently"),
                );
            }
        }
    }
}

fn check_bindings(
    file_name: &str,
    file: &BindingsFile,
    preset: Option<&Preset>,
    effects: &EffectTable,
    sources: &BTreeSet<String>,
    report: &mut Report,
) {
    if file.version != 1 {
        report.bad(
            file_name.to_string(),
            format!("version {} — the loader expects 1", file.version),
        );
    }

    let mut seen_ids: BTreeSet<&str> = BTreeSet::new();
    for b in &file.bindings {
        let where_ = format!("{file_name} binding '{}'", b.name);

        if !seen_ids.insert(&b.id) {
            report.bad(&where_, format!("duplicate id '{}'", b.id));
        }
        if b.scope != BindingScope::Preset {
            report.bad(
                &where_,
                "scope is Global; save_preset_bindings only writes Preset-scoped bindings \
                 into a sidecar, so this one would not be applied with the preset",
            );
        }
        if !sources.contains(&b.source) {
            report.bad(
                &where_,
                format!(
                    "source '{}' is not a key this build emits — the binding never fires",
                    b.source
                ),
            );
        }

        check_target(&where_, &b.target, preset, effects, report);

        for t in &b.transforms {
            if let TransformDef::Curve { curve_type } = t
                && !crate::bindings::transforms::CURVE_TYPES.contains(&curve_type.as_str())
            {
                report.bad(
                    &where_,
                    format!(
                        "curve_type '{curve_type}' is not handled; apply_curve falls through to \
                         identity. Valid: {}",
                        crate::bindings::transforms::CURVE_TYPES.join(", ")
                    ),
                );
            }
        }
    }
}

fn check_target(
    where_: &str,
    target: &BindingTarget,
    preset: Option<&Preset>,
    effects: &EffectTable,
    report: &mut Report,
) {
    let in_catalog = |leaf: &str, list: &[&str], kind: &str, report: &mut Report| {
        if !list.contains(&leaf) {
            report.bad(
                where_,
                format!(
                    "{kind} target '{leaf}' is not handled by apply_binding_target — it parses, \
                     loads, and does nothing. Valid: {}",
                    list.join(", ")
                ),
            );
        }
    };

    match target {
        // The whole reason this validator exists.
        BindingTarget::Unknown(raw) => report.bad(
            where_,
            format!(
                "target '{raw}' does not match any target form. parse_target is infallible, so \
                 this loads as Unknown and is silently ignored rather than rejected"
            ),
        ),
        BindingTarget::Unset => report.bad(where_, "target is empty (a half-made binding)"),

        BindingTarget::Param {
            layer,
            effect,
            param,
        } => {
            if let Some(preset) = preset {
                match preset.layers.get(*layer) {
                    None => report.bad(
                        where_,
                        format!(
                            "names layer {layer}, but the preset has {}",
                            preset.layers.len()
                        ),
                    ),
                    Some(l) if &l.effect_name != effect => report.bad(
                        where_,
                        format!(
                            "says layer {layer} runs '{effect}', but the preset puts \
                             '{}' there",
                            l.effect_name
                        ),
                    ),
                    Some(_) => {}
                }
            }
            match effects.param(effect, param) {
                None if effects.has_effect(effect) => {
                    report.bad(where_, format!("'{effect}' has no param '{param}'"));
                }
                None => report.bad(where_, format!("no effect named '{effect}'")),
                // Only Float and Bool are drivable: the bus carries one f32, and
                // apply_param_binding scales it into the declared range.
                Some(def) if !matches!(def, ParamDef::Float { .. } | ParamDef::Bool { .. }) => {
                    report.bad(
                        where_,
                        format!(
                            "'{param}' is {}; only Float and Bool params can be bound",
                            match def {
                                ParamDef::Color { .. } => "Color",
                                ParamDef::Point2D { .. } => "Point2D",
                                _ => unreachable!("Float and Bool matched above"),
                            }
                        ),
                    );
                }
                Some(_) => {}
            }
        }

        BindingTarget::LegacyParam { effect, param } => report.bad(
            where_,
            format!(
                "'param.{effect}.{param}' is the pre-#1792 form, which applies only to the \
                 ACTIVE layer. Write 'param.{{layer}}.{effect}.{param}' instead"
            ),
        ),

        BindingTarget::Layer { layer, field } => {
            if let Some(preset) = preset
                && *layer >= preset.layers.len()
            {
                report.bad(
                    where_,
                    format!(
                        "names layer {layer}, but the preset has {}",
                        preset.layers.len()
                    ),
                );
            }
            // LayerField only exists for names parse_target accepted, so the
            // field itself cannot be wrong here — assert the catalog agrees.
            debug_assert!(catalog::layer_fields().contains(&field.as_str()));
        }

        BindingTarget::PostFx(leaf) => in_catalog(leaf, catalog::POSTFX_TARGETS, "postfx", report),
        BindingTarget::Particle(leaf) => {
            in_catalog(leaf, catalog::PARTICLE_TARGETS, "particle", report);
        }
        BindingTarget::Uniform(leaf) => {
            in_catalog(leaf, catalog::UNIFORM_TARGETS, "uniform", report);
        }
        BindingTarget::SceneTransport(action) => in_catalog(
            action,
            catalog::SCENE_TRANSPORT_ACTIONS,
            "scene.transport",
            report,
        ),
        BindingTarget::GlobalMasterOpacity => {}
    }
}

fn check_scene(
    file_name: &str,
    scene: &SceneSet,
    preset_names: &BTreeSet<String>,
    report: &mut Report,
) {
    if scene.name.trim().is_empty() {
        report.bad(file_name, "scene name is empty");
    }
    if scene.cues.is_empty() {
        report.bad(file_name, "scene has no cues");
    }

    let timer = matches!(scene.advance_mode, AdvanceMode::Timer);
    for (i, cue) in scene.cues.iter().enumerate() {
        let where_ = format!("{file_name} cue {i} ('{}')", cue.preset_name);

        if !preset_names.contains(&cue.preset_name) {
            report.bad(
                &where_,
                format!(
                    "no preset file named '{}.json' beside the scene; the cue loads nothing",
                    cue.preset_name
                ),
            );
        }
        // The one that costs a whole show: under Timer, timeline.rs only arms a
        // timer when hold_secs is Some. A None cue holds forever.
        if timer && cue.hold_secs.is_none() {
            report.bad(
                &where_,
                "advance_mode is Timer but this cue has no hold_secs — the timeline stops \
                 here permanently, with nothing logged",
            );
        }
        if let Some(h) = cue.hold_secs
            && h <= 0.0
        {
            report.bad(&where_, format!("hold_secs is {h}; must be positive"));
        }
    }

    // Adjacent cues that put the same effect on the same layer index hit the
    // already-loaded morph-safe skip, which carries particle and obstacle state
    // across what should be a cut.
    for (i, pair) in scene.cues.windows(2).enumerate() {
        if pair[0].preset_name == pair[1].preset_name {
            report.bad(
                format!("{file_name} cues {i}-{}", i + 1),
                format!(
                    "both name preset '{}'; the second is a no-op transition",
                    pair[0].preset_name
                ),
            );
        }
    }
}

/// Read and check every preset, sidecar and scene in `dir`.
pub fn check_dir(dir: &Path, effects: &[PfxEffect], sources: &BTreeSet<String>) -> Result<Report> {
    let mut report = Report::default();
    let table = EffectTable::build(effects);
    check_no_dots(effects, &mut report);

    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    entries.sort();

    // A `<Name>.bindings.json` also ends in `.json`, so classify by the
    // compound extension first — the same trap PresetStore::scan hits when it
    // tries to deserialize sidecars as presets.
    let is_sidecar = |p: &Path| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".bindings.json"))
    };
    let is_scene = |p: &Path| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('_'))
    };

    let mut presets: Vec<(String, Preset)> = Vec::new();
    for path in entries.iter().filter(|p| !is_sidecar(p) && !is_scene(p)) {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        match serde_json::from_str::<Preset>(&text) {
            Ok(preset) => {
                check_preset(&name, &preset, &table, &mut report);
                report.presets_checked += 1;
                presets.push((name, preset));
            }
            Err(e) => report.bad(format!("preset '{name}'"), format!("does not parse: {e}")),
        }
    }

    let preset_names: BTreeSet<String> = presets.iter().map(|(n, _)| n.clone()).collect();

    for path in entries.iter().filter(|p| is_sidecar(p)) {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let stem = file_name.trim_end_matches(".bindings.json");
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        match serde_json::from_str::<BindingsFile>(&text) {
            Ok(file) => {
                let preset = presets.iter().find(|(n, _)| n == stem).map(|(_, p)| p);
                if preset.is_none() {
                    report.bad(
                        file_name,
                        format!("no preset '{stem}.json' beside it, so it is never loaded"),
                    );
                }
                report.bindings_checked += file.bindings.len();
                check_bindings(file_name, &file, preset, &table, sources, &mut report);
            }
            Err(e) => report.bad(file_name, format!("does not parse: {e}")),
        }
    }

    for path in entries.iter().filter(|p| is_scene(p)) {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        match serde_json::from_str::<SceneSet>(&text) {
            Ok(scene) => {
                report.cues_checked += scene.cues.len();
                check_scene(file_name, &scene, &preset_names, &mut report);
            }
            Err(e) => report.bad(file_name, format!("does not parse: {e}")),
        }
    }

    Ok(report)
}

/// `--validate <dir>` entry point. Returns the report; the caller sets the exit code.
pub fn run(dir: &Path) -> Result<Report> {
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }

    let mut loader = crate::effect::loader::EffectLoader::new();
    loader.scan_effects_directory();
    if loader.effects.is_empty() {
        bail!(
            "no .pfx effects found — run from the repo root, or beside a build with an \
             assets/ directory. With no effect table every effect name would look invalid."
        );
    }

    let sources: BTreeSet<String> = super::schema_dump::source_keys().into_iter().collect();
    check_dir(dir, &loader.effects, &sources)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effects() -> Vec<PfxEffect> {
        crate::effect::loader::shipped_effects_for_test()
    }

    fn sources() -> BTreeSet<String> {
        super::super::schema_dump::source_keys()
            .into_iter()
            .collect()
    }

    fn write(dir: &Path, name: &str, json: &str) {
        std::fs::write(dir.join(name), json).unwrap();
    }

    /// A minimal preset using a real effect and a real param, so only the thing
    /// under test is wrong.
    fn good_preset() -> String {
        r#"{"layers":[{"effect_name":"Aurora","params":{}}],"active_layer":0}"#.to_string()
    }

    fn run_on(files: &[(&str, &str)]) -> Report {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in files {
            write(dir.path(), name, body);
        }
        check_dir(dir.path(), &effects(), &sources()).unwrap()
    }

    fn assert_flags(files: &[(&str, &str)], needle: &str) {
        let report = run_on(files);
        assert!(
            report
                .problems
                .iter()
                .any(|p| p.to_string().contains(needle)),
            "expected a problem mentioning {needle:?}, got: {:#?}",
            report
                .problems
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_good_scene_passes() {
        let report = run_on(&[
            ("Look A.json", &good_preset()),
            (
                "Look A.bindings.json",
                r#"{"version":1,"bindings":[{"id":"b_000","name":"kick to speed",
                   "enabled":true,"scope":"Preset","source":"audio.percussive_energy",
                   "target":"postfx.bloom_intensity","transforms":[]}]}"#,
            ),
            (
                "_scene.json",
                r#"{"version":1,"name":"S","cues":[{"preset_name":"Look A","hold_secs":8.0}],
                   "advance_mode":"Timer"}"#,
            ),
        ]);
        assert!(
            report.is_clean(),
            "expected clean, got: {:#?}",
            report
                .problems
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(report.presets_checked, 1);
        assert_eq!(report.bindings_checked, 1);
        assert_eq!(report.cues_checked, 1);
    }

    /// The headline case: a target that parses to Unknown and would load clean.
    #[test]
    fn unknown_target_is_rejected() {
        assert_flags(
            &[
                ("P.json", &good_preset()),
                (
                    "P.bindings.json",
                    r#"{"version":1,"bindings":[{"id":"b_000","name":"x","enabled":true,
                       "scope":"Preset","source":"audio.kick","target":"parm.0.Aurora.speed",
                       "transforms":[]}]}"#,
                ),
            ],
            "parse_target is infallible",
        );
    }

    #[test]
    fn unhandled_leaf_name_is_rejected() {
        assert_flags(
            &[
                ("P.json", &good_preset()),
                (
                    "P.bindings.json",
                    r#"{"version":1,"bindings":[{"id":"b_000","name":"x","enabled":true,
                       "scope":"Preset","source":"audio.kick","target":"uniform.nonsense",
                       "transforms":[]}]}"#,
                ),
            ],
            "is not handled by apply_binding_target",
        );
    }

    /// `check_demos.py` waves `uniform.*` through, so this is a case the existing
    /// harness accepts and this validator must not.
    #[test]
    fn a_source_key_the_app_never_emits_is_rejected() {
        assert_flags(
            &[
                ("P.json", &good_preset()),
                (
                    "P.bindings.json",
                    r#"{"version":1,"bindings":[{"id":"b_000","name":"x","enabled":true,
                       "scope":"Preset","source":"audio.mel.64","target":"postfx.vignette",
                       "transforms":[]}]}"#,
                ),
            ],
            "never fires",
        );
    }

    #[test]
    fn timer_without_hold_secs_is_rejected() {
        assert_flags(
            &[
                ("P.json", &good_preset()),
                (
                    "_scene.json",
                    r#"{"version":1,"name":"S","cues":[{"preset_name":"P"}],
                       "advance_mode":"Timer"}"#,
                ),
            ],
            "the timeline stops here permanently",
        );
    }

    /// Manual mode has no such requirement, so the check must not fire there —
    /// otherwise it would reject every hand-authored manual scene.
    #[test]
    fn manual_without_hold_secs_is_fine() {
        let report = run_on(&[
            ("P.json", &good_preset()),
            (
                "_scene.json",
                r#"{"version":1,"name":"S","cues":[{"preset_name":"P"}],"advance_mode":"Manual"}"#,
            ),
        ]);
        assert!(
            report.is_clean(),
            "manual scenes need no hold_secs: {:#?}",
            report
                .problems
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn over_tall_preset_is_rejected() {
        let layers = (0..=catalog::MAX_LAYERS)
            .map(|_| r#"{"effect_name":"Aurora","params":{}}"#)
            .collect::<Vec<_>>()
            .join(",");
        assert_flags(
            &[(
                "Tall.json",
                &format!(r#"{{"layers":[{layers}],"active_layer":0}}"#),
            )],
            "silently dropped",
        );
    }

    #[test]
    fn global_scoped_binding_in_a_sidecar_is_rejected() {
        assert_flags(
            &[
                ("P.json", &good_preset()),
                (
                    "P.bindings.json",
                    r#"{"version":1,"bindings":[{"id":"b_000","name":"x","enabled":true,
                       "scope":"Global","source":"audio.kick","target":"postfx.vignette",
                       "transforms":[]}]}"#,
                ),
            ],
            "would not be applied with the preset",
        );
    }

    #[test]
    fn unhandled_curve_type_is_rejected() {
        assert_flags(
            &[
                ("P.json", &good_preset()),
                (
                    "P.bindings.json",
                    r#"{"version":1,"bindings":[{"id":"b_000","name":"x","enabled":true,
                       "scope":"Preset","source":"audio.kick","target":"postfx.vignette",
                       "transforms":[{"type":"curve","curve_type":"ease_in_quad"}]}]}"#,
                ),
            ],
            "falls through to identity",
        );
    }

    #[test]
    fn cue_naming_a_missing_preset_is_rejected() {
        assert_flags(
            &[
                ("P.json", &good_preset()),
                (
                    "_scene.json",
                    r#"{"version":1,"name":"S","cues":[{"preset_name":"Nope","hold_secs":4.0}],
                       "advance_mode":"Timer"}"#,
                ),
            ],
            "the cue loads nothing",
        );
    }

    #[test]
    fn unknown_effect_and_unknown_param_are_rejected() {
        assert_flags(
            &[(
                "P.json",
                r#"{"layers":[{"effect_name":"Nonexistent","params":{}}],"active_layer":0}"#,
            )],
            "no effect named 'Nonexistent'",
        );
        assert_flags(
            &[(
                "P.json",
                r#"{"layers":[{"effect_name":"Aurora","params":{"nope":{"Float":0.5}}}],
                   "active_layer":0}"#,
            )],
            "has no param 'nope'",
        );
    }

    #[test]
    fn missing_media_path_is_rejected() {
        assert_flags(
            &[(
                "P.json",
                r#"{"layers":[{"effect_name":"Aurora","params":{},
                   "media_path":"/nope/definitely-not-here.mp4"}],"active_layer":0}"#,
            )],
            "does not exist",
        );
    }

    /// The invariant `bindings/types.rs` claims a test guards. It holds today;
    /// this is what makes it stay true.
    #[test]
    fn no_shipped_effect_or_param_name_contains_a_dot() {
        let mut report = Report::default();
        check_no_dots(&effects(), &mut report);
        assert!(
            report.is_clean(),
            "a dot in an effect or param name breaks parse_target's segment counting: {:#?}",
            report
                .problems
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
        );
    }
}
