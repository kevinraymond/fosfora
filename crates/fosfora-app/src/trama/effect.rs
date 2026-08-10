//! Trama effect files: manifest parsing, naga validation, and the registry.
//!
//! An effect is one `.wgsl` file under `assets/trama/effects/` that opens with
//! a `/*! trama { … } */` JSON manifest. Params use the typed [`ParamDef`]
//! wire format verbatim — declaration order is packing order, exactly like
//! [`crate::params::ParamStore::pack_to_buffer`] — so there is no index field
//! to keep consistent by hand.
//!
//! Load path per file (invariant I5: validate before pipeline): parse
//! manifest → prepend the ABI v3 preamble via the untouched `EffectLoader` →
//! naga front-end validation of exactly what will compile → only then
//! `ShaderPipeline::new` (whose error scope stays as the backend-level
//! backstop). A file that fails at any step lands in `errors` and is skipped;
//! the app keeps running (I4).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::effect::loader::EffectLoader;
use crate::gpu::fullscreen_quad::FULLSCREEN_TRIANGLE_VS;
use crate::gpu::{GpuContext, ShaderPipeline};
use crate::params::ParamDef;

/// The manifest `id` of an effect; also its file stem and registry key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EffectId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffectKind {
    /// Generates content; declares `inputs: 0`.
    Source,
    /// Transforms content; declares `inputs: 1` or `2`.
    Effect,
}

/// The JSON block at the top of a trama effect file.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TramaManifest {
    pub name: String,
    pub id: String,
    pub kind: EffectKind,
    pub inputs: u8,
    #[serde(default)]
    pub params: Vec<ParamDef>,
}

/// Scalar slots available to one node's params — the ABI v3 ceiling
/// (`params: array<vec4f, 4>`, packed by `ParamStore::pack_to_buffer`).
pub const PARAM_SLOT_CAP: usize = 16;

const MANIFEST_HEADER: &str = "/*! trama";

#[derive(Debug, thiserror::Error)]
pub enum EffectLoadError {
    #[error("missing `/*! trama` manifest header at the top of the file")]
    MissingManifest,
    #[error("manifest comment never closes (`*/` not found)")]
    UnterminatedManifest,
    #[error("manifest JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("kind/inputs mismatch: {0}")]
    KindInputs(String),
    #[error("duplicate param name `{0}`")]
    DuplicateParam(String),
    #[error("params need {needed} scalar slots; the ABI caps a node at {cap}")]
    ParamOverflow { needed: usize, cap: usize },
    #[error("manifest id `{id}` does not match file stem `{stem}`")]
    IdMismatch { id: String, stem: String },
    #[error("WGSL validation:\n{0}")]
    Naga(String),
    #[error("pipeline: {0}")]
    Pipeline(String),
    #[error("read: {0}")]
    Io(#[from] std::io::Error),
}

/// Parse and semantically check the manifest block. Pure — no filesystem, no
/// GPU — so every reject rule tests in plain CI.
pub fn parse_effect_file(source: &str) -> Result<TramaManifest, EffectLoadError> {
    let s = source.trim_start_matches('\u{feff}').trim_start();
    let Some(rest) = s.strip_prefix(MANIFEST_HEADER) else {
        return Err(EffectLoadError::MissingManifest);
    };
    let Some(end) = rest.find("*/") else {
        return Err(EffectLoadError::UnterminatedManifest);
    };
    let manifest: TramaManifest = serde_json::from_str(&rest[..end])?;
    check_manifest(&manifest)?;
    Ok(manifest)
}

fn check_manifest(m: &TramaManifest) -> Result<(), EffectLoadError> {
    match m.kind {
        EffectKind::Source if m.inputs != 0 => {
            return Err(EffectLoadError::KindInputs(format!(
                "a source declares inputs: {}, must be 0",
                m.inputs
            )));
        }
        EffectKind::Effect if !(1..=2).contains(&m.inputs) => {
            return Err(EffectLoadError::KindInputs(format!(
                "an effect declares inputs: {}, must be 1 or 2",
                m.inputs
            )));
        }
        _ => {}
    }
    let mut names = HashSet::new();
    for def in &m.params {
        if !names.insert(def.name()) {
            return Err(EffectLoadError::DuplicateParam(def.name().to_string()));
        }
    }
    let needed: usize = m
        .params
        .iter()
        .map(|d| d.default_value().float_count())
        .sum();
    if needed > PARAM_SLOT_CAP {
        return Err(EffectLoadError::ParamOverflow {
            needed,
            cap: PARAM_SLOT_CAP,
        });
    }
    Ok(())
}

/// Registry lookup stays unambiguous because the id IS the file stem.
fn check_id_matches(manifest: &TramaManifest, stem: &str) -> Result<(), EffectLoadError> {
    if manifest.id != stem {
        return Err(EffectLoadError::IdMismatch {
            id: manifest.id.clone(),
            stem: stem.to_string(),
        });
    }
    Ok(())
}

/// naga front-end validation of a complete shader module (I5). `full_source`
/// must be exactly what the pipeline will compile — vertex shader included —
/// so a pass here means `create_shader_module` cannot fail on syntax or types.
pub fn validate_wgsl(full_source: &str) -> Result<(), EffectLoadError> {
    let module = naga::front::wgsl::parse_str(full_source)
        .map_err(|e| EffectLoadError::Naga(e.emit_to_string(full_source)))?;
    // Baseline WebGPU capabilities: anything the strictest portable target
    // forbids should fail at load, not on some other machine.
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| EffectLoadError::Naga(e.emit_to_string(full_source)))?;
    Ok(())
}

/// A loaded, validated, pipeline-ready effect.
pub struct EffectDef {
    pub id: EffectId,
    pub name: String,
    pub kind: EffectKind,
    pub inputs: u8,
    pub params: Vec<ParamDef>,
    pub pipeline: ShaderPipeline,
}

pub struct TramaRegistry {
    /// Sorted by id — stable palette order in the canvas.
    pub effects: Vec<EffectDef>,
    /// `(file name, error)` per failed file; surfaced in the canvas window.
    pub errors: Vec<(String, String)>,
}

/// Production effect directory. A separate root from `assets/effects/` (which
/// holds `.pfx` under a different schema and is already hot-reload-watched).
pub fn trama_effects_dir() -> PathBuf {
    crate::effect::loader::assets_dir().join("trama/effects")
}

impl TramaRegistry {
    pub fn load(
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        loader: &EffectLoader,
        dir: &Path,
    ) -> Self {
        let mut effects = Vec::new();
        let mut errors = Vec::new();
        let mut paths: Vec<PathBuf> = match std::fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|ext| ext == "wgsl"))
                .collect(),
            Err(e) => {
                errors.push((dir.display().to_string(), format!("read dir: {e}")));
                Vec::new()
            }
        };
        paths.sort();
        for path in paths {
            let file = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            match load_one(device, cache, loader, &path) {
                Ok(def) => effects.push(def),
                Err(e) => {
                    log::warn!("trama: skipping {file}: {e}");
                    errors.push((file, e.to_string()));
                }
            }
        }
        effects.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        Self { effects, errors }
    }

    pub fn get(&self, id: &EffectId) -> Option<&EffectDef> {
        self.effects.iter().find(|e| &e.id == id)
    }
}

fn load_one(
    device: &wgpu::Device,
    cache: Option<&wgpu::PipelineCache>,
    loader: &EffectLoader,
    path: &Path,
) -> Result<EffectDef, EffectLoadError> {
    let source = std::fs::read_to_string(path)?;
    let manifest = parse_effect_file(&source)?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    check_id_matches(&manifest, &stem)?;
    // The whole file — manifest comment included — is what gets preambled:
    // WGSL comments are harmless, the own-struct check is comment-aware
    // (#1855), and diagnostics keep the on-disk line numbers.
    let fragment = loader.prepend_library_with_inputs(&source, usize::from(manifest.inputs));
    let full = format!("{FULLSCREEN_TRIANGLE_VS}\n{fragment}");
    validate_wgsl(&full)?;
    let pipeline = ShaderPipeline::new(
        device,
        GpuContext::hdr_format(),
        &fragment,
        cache,
        usize::from(manifest.inputs),
    )
    .map_err(|e| EffectLoadError::Pipeline(e.to_string()))?;
    Ok(EffectDef {
        id: EffectId(manifest.id),
        name: manifest.name,
        kind: manifest.kind,
        inputs: manifest.inputs,
        params: manifest.params,
        pipeline,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::loader::probe_libs;

    const NOISE_FIELD: &str = include_str!("../../../../assets/trama/effects/noise_field.wgsl");
    const HUE_DRIFT: &str = include_str!("../../../../assets/trama/effects/hue_drift.wgsl");
    const MIX: &str = include_str!("../../../../assets/trama/effects/mix.wgsl");
    const TRANSFORM: &str = include_str!("../../../../assets/trama/effects/transform.wgsl");

    fn manifest(json: &str) -> Result<TramaManifest, EffectLoadError> {
        parse_effect_file(&format!(
            "/*! trama\n{json}\n*/\n@fragment fn fs_main() {{}}"
        ))
    }

    #[test]
    fn manifest_parses_typed_paramdefs() {
        let m = manifest(
            r#"{ "name": "X", "id": "x", "kind": "effect", "inputs": 1,
                 "params": [
                   { "type": "Float", "name": "shift", "default": 0.1, "min": 0.0, "max": 1.0 },
                   { "type": "Color", "name": "tint", "default": [1.0, 0.5, 0.0, 1.0] },
                   { "type": "Bool", "name": "invert", "default": false }
                 ] }"#,
        )
        .unwrap();
        assert_eq!(m.name, "X");
        assert_eq!(m.kind, EffectKind::Effect);
        assert_eq!(m.inputs, 1);
        assert_eq!(m.params.len(), 3);
        assert_eq!(m.params[0].name(), "shift");
        assert!(matches!(m.params[1], ParamDef::Color { .. }));
        assert!(matches!(m.params[2], ParamDef::Bool { .. }));
    }

    #[test]
    fn manifest_rejects_missing_header() {
        let err = parse_effect_file("// just a shader\n@fragment fn fs_main() {}").unwrap_err();
        assert!(matches!(err, EffectLoadError::MissingManifest));
    }

    #[test]
    fn manifest_rejects_unterminated_comment() {
        let err = parse_effect_file("/*! trama\n{ \"name\": \"X\" }").unwrap_err();
        assert!(matches!(err, EffectLoadError::UnterminatedManifest));
    }

    #[test]
    fn manifest_rejects_bad_json() {
        let err = manifest("{ not json").unwrap_err();
        assert!(matches!(err, EffectLoadError::Json(_)));
    }

    #[test]
    fn manifest_rejects_unknown_field() {
        let err =
            manifest(r#"{ "name": "X", "id": "x", "kind": "source", "inputs": 0, "index": 3 }"#)
                .unwrap_err();
        assert!(matches!(err, EffectLoadError::Json(_)));
    }

    #[test]
    fn manifest_rejects_source_with_inputs() {
        let err =
            manifest(r#"{ "name": "X", "id": "x", "kind": "source", "inputs": 1 }"#).unwrap_err();
        assert!(matches!(err, EffectLoadError::KindInputs(_)));
    }

    #[test]
    fn manifest_rejects_effect_input_arity() {
        for inputs in ["0", "3"] {
            let err = manifest(&format!(
                r#"{{ "name": "X", "id": "x", "kind": "effect", "inputs": {inputs} }}"#
            ))
            .unwrap_err();
            assert!(
                matches!(err, EffectLoadError::KindInputs(_)),
                "inputs = {inputs}"
            );
        }
    }

    #[test]
    fn manifest_rejects_duplicate_param_names() {
        let err = manifest(
            r#"{ "name": "X", "id": "x", "kind": "source", "inputs": 0,
                 "params": [
                   { "type": "Float", "name": "a", "default": 0.0, "min": 0.0, "max": 1.0 },
                   { "type": "Bool", "name": "a", "default": true }
                 ] }"#,
        )
        .unwrap_err();
        assert!(matches!(err, EffectLoadError::DuplicateParam(name) if name == "a"));
    }

    #[test]
    fn manifest_rejects_param_slot_overflow() {
        // 5 colors = 20 floats > the 16-slot ABI cap.
        let params: Vec<String> = (0..5)
            .map(|i| format!(r#"{{ "type": "Color", "name": "c{i}", "default": [0,0,0,1] }}"#))
            .collect();
        let err = manifest(&format!(
            r#"{{ "name": "X", "id": "x", "kind": "source", "inputs": 0, "params": [{}] }}"#,
            params.join(",")
        ))
        .unwrap_err();
        assert!(matches!(
            err,
            EffectLoadError::ParamOverflow {
                needed: 20,
                cap: PARAM_SLOT_CAP
            }
        ));
    }

    #[test]
    fn manifest_id_must_match_file_stem() {
        let m = manifest(r#"{ "name": "X", "id": "x", "kind": "source", "inputs": 0 }"#).unwrap();
        assert!(check_id_matches(&m, "x").is_ok());
        assert!(matches!(
            check_id_matches(&m, "y").unwrap_err(),
            EffectLoadError::IdMismatch { .. }
        ));
    }

    #[test]
    fn builtin_noise_field_parses_from_assets() {
        let m = parse_effect_file(NOISE_FIELD).unwrap();
        assert_eq!(m.id, "noise_field");
        assert_eq!(m.kind, EffectKind::Source);
        assert_eq!(m.inputs, 0);
        assert_eq!(m.params.len(), 4);
    }

    #[test]
    fn builtin_hue_drift_parses_from_assets() {
        let m = parse_effect_file(HUE_DRIFT).unwrap();
        assert_eq!(m.id, "hue_drift");
        assert_eq!(m.kind, EffectKind::Effect);
        assert_eq!(m.inputs, 1);
        assert_eq!(m.params.len(), 2);
    }

    #[test]
    fn builtin_mix_and_transform_parse_from_assets() {
        let m = parse_effect_file(MIX).unwrap();
        assert_eq!(m.id, "mix");
        assert_eq!(m.kind, EffectKind::Effect);
        assert_eq!(m.inputs, 2);
        assert_eq!(m.params.len(), 1);
        let m = parse_effect_file(TRANSFORM).unwrap();
        assert_eq!(m.id, "transform");
        assert_eq!(m.kind, EffectKind::Effect);
        assert_eq!(m.inputs, 1);
        assert_eq!(m.params.len(), 4);
    }

    #[test]
    fn builtin_effects_pass_naga_validation() {
        // The production compile source, byte for byte, but validated on the
        // CPU — no GPU adapter needed, so this runs in plain CI.
        let loader = EffectLoader::for_test(&probe_libs());
        for (name, source) in [
            ("noise_field", NOISE_FIELD),
            ("hue_drift", HUE_DRIFT),
            ("mix", MIX),
            ("transform", TRANSFORM),
        ] {
            let m = parse_effect_file(source).unwrap();
            let fragment = loader.prepend_library_with_inputs(source, usize::from(m.inputs));
            let full = format!("{FULLSCREEN_TRIANGLE_VS}\n{fragment}");
            if let Err(e) = validate_wgsl(&full) {
                panic!("{name} failed naga validation: {e}");
            }
        }
    }

    #[test]
    fn naga_rejects_invalid_wgsl_before_pipeline() {
        let err = validate_wgsl("@fragment fn fs_main() -> f32 { return 1; }").unwrap_err();
        assert!(matches!(err, EffectLoadError::Naga(_)));
    }
}
