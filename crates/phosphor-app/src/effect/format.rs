use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::gpu::particle::types::ParticleDef;
use crate::params::ParamDef;

/// Visual classification of an effect for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffectType {
    /// Fragment shader only, no particles.
    Shader,
    /// Compute particles + background shader.
    Particle,
    /// Particles + accumulated-state feedback (trails, RD, N-body, etc.).
    Feedback,
}

/// A render pass definition within a multi-pass effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PassDef {
    pub name: String,
    pub shader: String,
    #[serde(default = "default_scale")]
    pub scale: f32,
    /// Names of earlier passes whose **current-frame** outputs this pass samples as
    /// `input0..` (in declared order). Forward/unknown references are a hard error.
    #[serde(default)]
    pub inputs: Vec<String>,
    /// Names of feedback passes whose **previous-frame** outputs this pass samples,
    /// appended after `inputs` in the `input0..` numbering. Unlike `inputs`, these
    /// may name a later-declared pass (previous-frame data has no intra-frame ordering
    /// constraint) — the edge that cuts a solver's velocity→divergence→pressure→velocity
    /// cycle (#1481). Each named pass must have `feedback: true`.
    #[serde(default)]
    pub prev_inputs: Vec<String>,
    /// Number of times to run this pass per frame, ping-ponging its own target between
    /// runs (Jacobi/relaxation loops). Only meaningful for `feedback: true` passes;
    /// defaults to 1 (single draw, legacy behavior).
    #[serde(default = "default_one")]
    pub iterations: u32,
    /// Whether this pass reads its own previous frame (ping-pong feedback).
    /// Defaults to true (matches legacy single-shader behavior); set false to disable.
    #[serde(default = "default_true")]
    pub feedback: bool,
}

impl PassDef {
    /// Number of `input0..` bindings this pass declares: current-frame `inputs` plus
    /// previous-frame `prev_inputs`. This MUST match what `PassExecutor::new` resolves and
    /// inject, so every shader-compile site (initial build, hot-reload) uses it — counting
    /// only `inputs` drops the bindings for a pass whose inputs are all previous-frame
    /// (e.g. a divergence/potential pass), making `input0_tex` undefined on recompile.
    pub fn input_count(&self) -> usize {
        self.inputs.len() + self.prev_inputs.len()
    }
}

fn default_scale() -> f32 {
    1.0
}

fn default_one() -> u32 {
    1
}

/// Per-effect post-processing overrides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PostProcessDef {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_bloom_threshold")]
    pub bloom_threshold: f32,
    #[serde(default = "default_bloom_intensity")]
    pub bloom_intensity: f32,
    #[serde(default = "default_vignette")]
    pub vignette: f32,
    #[serde(default = "default_half")]
    pub ca_intensity: f32,
    #[serde(default = "default_half")]
    pub grain_intensity: f32,
    #[serde(default = "default_true")]
    pub bloom_enabled: bool,
    #[serde(default = "default_true")]
    pub ca_enabled: bool,
    #[serde(default = "default_true")]
    pub vignette_enabled: bool,
    #[serde(default = "default_true")]
    pub grain_enabled: bool,
    /// Tonemap operator: "aces" (default, Phosphor house look) or "linear"
    /// (raw passthrough clamp, matching SuperSplat for the Splat effect).
    #[serde(default = "default_tonemap")]
    pub tonemap: String,
}

fn default_true() -> bool {
    true
}

fn default_tonemap() -> String {
    "aces".to_string()
}

fn default_bloom_threshold() -> f32 {
    0.8
}

fn default_bloom_intensity() -> f32 {
    0.35
}

fn default_vignette() -> f32 {
    0.3
}

fn default_half() -> f32 {
    0.5
}

impl Default for PostProcessDef {
    fn default() -> Self {
        Self {
            enabled: true,
            bloom_threshold: 0.8,
            bloom_intensity: 0.35,
            vignette: 0.3,
            ca_intensity: 0.5,
            grain_intensity: 0.5,
            bloom_enabled: true,
            ca_enabled: true,
            vignette_enabled: true,
            grain_enabled: true,
            tonemap: "aces".to_string(),
        }
    }
}

/// Describes which audio feature drives which visual aspect of an effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioMapping {
    pub feature: String,
    pub target: String,
}

/// A .pfx effect definition (JSON format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PfxEffect {
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    /// Single-pass shader (backward compatible). Ignored if `passes` is non-empty.
    #[serde(default)]
    pub shader: String,
    #[serde(default)]
    pub inputs: Vec<ParamDef>,
    /// Multi-pass pipeline definition. If empty, `shader` field is used as a single pass.
    #[serde(default)]
    pub passes: Vec<PassDef>,
    /// Per-effect post-processing overrides.
    #[serde(default)]
    pub postprocess: Option<PostProcessDef>,
    /// GPU particle system definition.
    #[serde(default)]
    pub particles: Option<ParticleDef>,
    /// Audio feature → visual target mappings (read-only display in UI).
    #[serde(default)]
    pub audio_mappings: Vec<AudioMapping>,
    /// If true, effect is hidden from UI (not shown in effects panel or next/prev cycling).
    #[serde(default)]
    pub hidden: bool,
    /// Explicit effect type override (shader/particle/feedback).
    /// If absent, auto-detected: no particles → Shader, particles → Particle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_type: Option<EffectType>,
    /// Path to the .pfx file on disk (not serialized).
    #[serde(skip)]
    pub source_path: Option<PathBuf>,
}

impl PfxEffect {
    /// Returns the effect type: explicit if set, otherwise auto-detected.
    pub fn effect_type(&self) -> EffectType {
        if let Some(et) = self.effect_type {
            return et;
        }
        if self.particles.is_some() {
            EffectType::Particle
        } else {
            EffectType::Shader
        }
    }

    /// Normalize: if `passes` is empty but `shader` is set, create a single-pass definition.
    /// Single-pass effects get feedback enabled by default (matches legacy behavior).
    pub fn normalized_passes(&self) -> Vec<PassDef> {
        if !self.passes.is_empty() {
            return self.passes.clone();
        }
        if !self.shader.is_empty() {
            vec![PassDef {
                name: "main".to_string(),
                shader: self.shader.clone(),
                scale: 1.0,
                inputs: vec![],
                prev_inputs: vec![],
                iterations: 1,
                feedback: true,
            }]
        } else {
            vec![]
        }
    }

    /// Compare two PfxEffect versions and identify what changed.
    pub fn diff(&self, other: &PfxEffect) -> PfxDiff {
        PfxDiff {
            metadata_changed: self.name != other.name
                || self.author != other.author
                || self.description != other.description
                || self.hidden != other.hidden
                || self.audio_mappings != other.audio_mappings,
            inputs_changed: self.inputs != other.inputs,
            postprocess_changed: self.postprocess != other.postprocess,
            passes_changed: self.normalized_passes() != other.normalized_passes(),
            particles_changed: self.particles != other.particles,
        }
    }
}

/// Describes what changed between two versions of a PfxEffect.
#[derive(Debug, Clone, PartialEq)]
pub struct PfxDiff {
    pub metadata_changed: bool,
    pub inputs_changed: bool,
    pub postprocess_changed: bool,
    pub passes_changed: bool,
    pub particles_changed: bool,
}

impl PfxDiff {
    /// Returns true if nothing changed.
    pub fn is_empty(&self) -> bool {
        !self.metadata_changed
            && !self.inputs_changed
            && !self.postprocess_changed
            && !self.passes_changed
            && !self.particles_changed
    }

    /// Returns true if changes require a full GPU rebuild (new pipelines).
    pub fn needs_rebuild(&self) -> bool {
        self.passes_changed || self.particles_changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn normalized_passes_from_single_shader() {
        let effect = PfxEffect {
            name: "test".into(),
            author: String::new(),
            description: String::new(),
            shader: "test.wgsl".into(),
            inputs: vec![],
            passes: vec![],
            postprocess: None,
            particles: None,
            audio_mappings: vec![],
            hidden: false,
            effect_type: None,
            source_path: None,
        };
        let passes = effect.normalized_passes();
        assert_eq!(passes.len(), 1);
        assert_eq!(passes[0].shader, "test.wgsl");
        assert_eq!(passes[0].name, "main");
        assert!(passes[0].feedback);
        assert!(approx_eq(passes[0].scale, 1.0, 1e-6));
    }

    #[test]
    fn normalized_passes_empty_when_no_shader() {
        let effect = PfxEffect {
            name: "test".into(),
            author: String::new(),
            description: String::new(),
            shader: String::new(),
            inputs: vec![],
            passes: vec![],
            postprocess: None,
            particles: None,
            audio_mappings: vec![],
            hidden: false,
            effect_type: None,
            source_path: None,
        };
        assert!(effect.normalized_passes().is_empty());
    }

    #[test]
    fn normalized_passes_from_passes_array() {
        let pass = PassDef {
            name: "p1".into(),
            shader: "a.wgsl".into(),
            scale: 0.5,
            inputs: vec![],
            prev_inputs: vec![],
            iterations: 1,
            feedback: false,
        };
        let effect = PfxEffect {
            name: "test".into(),
            author: String::new(),
            description: String::new(),
            shader: "ignored.wgsl".into(),
            inputs: vec![],
            passes: vec![pass],
            postprocess: None,
            particles: None,
            audio_mappings: vec![],
            hidden: false,
            effect_type: None,
            source_path: None,
        };
        let passes = effect.normalized_passes();
        assert_eq!(passes.len(), 1);
        assert_eq!(passes[0].shader, "a.wgsl");
        assert!(!passes[0].feedback);
    }

    #[test]
    fn input_count_includes_prev_inputs() {
        // Regression: a pass whose inputs are all previous-frame (a divergence/potential
        // pass — inputs empty, prev_inputs set) still declares input0.. bindings. Counting
        // only `inputs` dropped them and left `input0_tex` undefined when the app recompiled
        // the shader on hot-reload. input_count() must include prev_inputs.
        let pass = PassDef {
            name: "potential".into(),
            shader: "protea_potential.wgsl".into(),
            scale: 0.5,
            inputs: vec![],
            prev_inputs: vec!["mass".into()],
            iterations: 1,
            feedback: false,
        };
        assert_eq!(pass.input_count(), 1);

        let two = PassDef {
            inputs: vec!["velocity".into()],
            prev_inputs: vec!["dye".into()],
            ..pass.clone()
        };
        assert_eq!(two.input_count(), 2);
    }

    #[test]
    fn postprocess_def_defaults() {
        let pp = PostProcessDef::default();
        assert!(approx_eq(pp.bloom_threshold, 0.8, 1e-6));
        assert!(approx_eq(pp.bloom_intensity, 0.35, 1e-6));
        assert!(approx_eq(pp.vignette, 0.3, 1e-6));
        assert!(approx_eq(pp.ca_intensity, 0.5, 1e-6));
        assert!(approx_eq(pp.grain_intensity, 0.5, 1e-6));
        assert!(pp.enabled);
        assert!(pp.bloom_enabled);
    }

    #[test]
    fn pfx_effect_serde_minimal() {
        let json = r#"{"name":"test","shader":"t.wgsl"}"#;
        let effect: PfxEffect = serde_json::from_str(json).unwrap();
        assert_eq!(effect.name, "test");
        assert_eq!(effect.shader, "t.wgsl");
        assert!(effect.passes.is_empty());
        assert!(effect.particles.is_none());
    }

    #[test]
    fn pfx_effect_serde_with_passes() {
        let json = r#"{"name":"multi","shader":"","passes":[{"name":"p1","shader":"a.wgsl"},{"name":"p2","shader":"b.wgsl","feedback":false}]}"#;
        let effect: PfxEffect = serde_json::from_str(json).unwrap();
        assert_eq!(effect.passes.len(), 2);
        assert!(effect.passes[0].feedback);
        assert!(!effect.passes[1].feedback);
    }

    #[test]
    fn pass_def_serde_defaults() {
        let json = r#"{"name":"test","shader":"t.wgsl"}"#;
        let pass: PassDef = serde_json::from_str(json).unwrap();
        assert!(approx_eq(pass.scale, 1.0, 1e-6));
        assert!(pass.feedback);
        assert!(pass.inputs.is_empty());
        assert!(pass.prev_inputs.is_empty());
        assert_eq!(pass.iterations, 1);
    }

    #[test]
    fn pass_def_serde_prev_inputs_and_iterations() {
        let json = r#"{"name":"pressure","shader":"p.wgsl","feedback":true,
            "inputs":["divergence"],"prev_inputs":["velocity"],"iterations":24}"#;
        let pass: PassDef = serde_json::from_str(json).unwrap();
        assert_eq!(pass.inputs, vec!["divergence".to_string()]);
        assert_eq!(pass.prev_inputs, vec!["velocity".to_string()]);
        assert_eq!(pass.iterations, 24);
        // A pass differing only in iterations is still a passes-change (forces rebuild).
        let mut other = pass.clone();
        other.iterations = 12;
        assert_ne!(pass, other);
    }

    #[test]
    fn feedback_defaults_true_for_legacy_and_passes() {
        // Legacy single-shader effect: normalized pass gets feedback=true.
        let legacy: PfxEffect =
            serde_json::from_str(r#"{"name":"legacy","shader":"x.wgsl"}"#).unwrap();
        let legacy_passes = legacy.normalized_passes();
        assert_eq!(legacy_passes.len(), 1);
        assert!(legacy_passes[0].feedback);

        // Equivalent passes-array effect omitting `feedback`: same result.
        let multi: PfxEffect = serde_json::from_str(
            r#"{"name":"multi","passes":[{"name":"main","shader":"x.wgsl"}]}"#,
        )
        .unwrap();
        let multi_passes = multi.normalized_passes();
        assert_eq!(multi_passes.len(), 1);
        assert!(multi_passes[0].feedback);
    }

    fn make_effect(name: &str, shader: &str) -> PfxEffect {
        PfxEffect {
            name: name.into(),
            author: String::new(),
            description: String::new(),
            shader: shader.into(),
            inputs: vec![],
            passes: vec![],
            postprocess: None,
            particles: None,
            audio_mappings: vec![],
            hidden: false,
            effect_type: None,
            source_path: None,
        }
    }

    #[test]
    fn diff_identical_effects_is_empty() {
        let a = make_effect("test", "t.wgsl");
        let b = make_effect("test", "t.wgsl");
        let diff = a.diff(&b);
        assert!(diff.is_empty());
        assert!(!diff.needs_rebuild());
    }

    #[test]
    fn diff_detects_metadata_change() {
        let a = make_effect("test", "t.wgsl");
        let mut b = make_effect("test", "t.wgsl");
        b.description = "changed".into();
        let diff = a.diff(&b);
        assert!(diff.metadata_changed);
        assert!(!diff.inputs_changed);
        assert!(!diff.passes_changed);
        assert!(!diff.needs_rebuild());
    }

    #[test]
    fn diff_detects_inputs_change() {
        let a = make_effect("test", "t.wgsl");
        let mut b = make_effect("test", "t.wgsl");
        b.inputs = vec![ParamDef::Float {
            name: "speed".into(),
            default: 0.5,
            min: 0.0,
            max: 1.0,
        }];
        let diff = a.diff(&b);
        assert!(!diff.metadata_changed);
        assert!(diff.inputs_changed);
        assert!(!diff.needs_rebuild());
    }

    #[test]
    fn diff_detects_postprocess_change() {
        let a = make_effect("test", "t.wgsl");
        let mut b = make_effect("test", "t.wgsl");
        b.postprocess = Some(PostProcessDef {
            bloom_threshold: 0.5,
            ..PostProcessDef::default()
        });
        let diff = a.diff(&b);
        assert!(diff.postprocess_changed);
        assert!(!diff.needs_rebuild());
    }

    #[test]
    fn diff_detects_passes_change() {
        let a = make_effect("test", "a.wgsl");
        let b = make_effect("test", "b.wgsl");
        let diff = a.diff(&b);
        assert!(diff.passes_changed);
        assert!(diff.needs_rebuild());
    }

    #[test]
    fn diff_detects_particles_change() {
        let a = make_effect("test", "t.wgsl");
        let mut b = make_effect("test", "t.wgsl");
        b.particles = Some(serde_json::from_str(r#"{"max_count": 1000}"#).unwrap());
        let diff = a.diff(&b);
        assert!(diff.particles_changed);
        assert!(diff.needs_rebuild());
    }

    #[test]
    fn diff_multiple_changes() {
        let a = make_effect("test", "a.wgsl");
        let mut b = make_effect("renamed", "b.wgsl");
        b.inputs = vec![ParamDef::Bool {
            name: "flag".into(),
            default: false,
        }];
        let diff = a.diff(&b);
        assert!(diff.metadata_changed);
        assert!(diff.inputs_changed);
        assert!(diff.passes_changed);
        assert!(diff.needs_rebuild());
        assert!(!diff.is_empty());
    }
}
