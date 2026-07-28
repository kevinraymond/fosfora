use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::effect::format::PostProcessDef;
use crate::gpu::helix::HelixParams;
use crate::gpu::lattice::LatticeParams;
use crate::gpu::layer::BlendMode;
use crate::gpu::particle::types::{
    default_drag, default_emit_rate, default_initial_size, default_initial_speed, default_lifetime,
};
use crate::gpu::volumetric::VolumetricParams;
use crate::params::ParamValue;

// Embedded built-in presets
const BUILTIN_CRUCIBLE: &str = include_str!("../../../../assets/presets/Crucible.json");
const BUILTIN_SPECTRAL_EYE: &str = include_str!("../../../../assets/presets/Spectral Eye.json");

/// Built-in preset names in display order.
const BUILTIN_PRESETS: &[(&str, &str)] = &[
    ("Crucible", BUILTIN_CRUCIBLE),
    ("Spectral Eye", BUILTIN_SPECTRAL_EYE),
];

/// Per-layer state saved in a preset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerPreset {
    pub effect_name: String,
    #[serde(default)]
    pub params: HashMap<String, ParamValue>,
    #[serde(default)]
    pub blend_mode: BlendMode,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    /// Warp strength for the displacement blend modes (#1478). Absent in
    /// pre-Rift presets, which restore at the default.
    #[serde(default = "default_displace_amount")]
    pub displace_amount: f32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub custom_name: Option<String>,
    #[serde(default)]
    pub media_path: Option<String>,
    #[serde(default)]
    pub media_speed: Option<f32>,
    #[serde(default)]
    pub media_looping: Option<bool>,
    #[serde(default)]
    pub webcam_device: Option<String>,
    /// Absolute path to video file used as particle image source.
    #[serde(default)]
    pub particle_video_path: Option<String>,
    #[serde(default)]
    pub particle_video_speed: Option<f32>,
    #[serde(default)]
    pub particle_video_looping: Option<bool>,
    /// True if particle source is webcam.
    #[serde(default)]
    pub particle_webcam: Option<bool>,
    /// Absolute path to static image used as particle source.
    #[serde(default)]
    pub particle_image_path: Option<String>,
    /// Absolute path to the 3D model used as particle source (#1993). Mutually
    /// exclusive with `particle_image_path` — a model is rendered to a frame rather
    /// than read from one.
    #[serde(default)]
    pub particle_model_path: Option<String>,
    /// Pose the model was sampled at: `[yaw_degrees, pitch_degrees, scale, ambient]`.
    /// Without it a restored model snaps back to a front-on default view, which is a
    /// different picture from the one that was saved.
    #[serde(default)]
    pub particle_model_pose: Option<[f32; 4]>,
    /// Absolute path to the Gaussian-splat scene loaded on this layer
    /// (#1800). Restored via a BACKGROUND load — files can be ~1.5 GB.
    #[serde(default)]
    pub splat_scene_path: Option<String>,
    /// Obstacle collision image path.
    #[serde(default)]
    pub obstacle_image_path: Option<String>,
    /// Obstacle collision mode (0=bounce, 1=stick, 2=flow, 3=contain).
    #[serde(default)]
    pub obstacle_mode: Option<u32>,
    /// Obstacle fit mode (0=stretch, 1=contain/"Fit", 2=cover/"Fill", #1790).
    /// `None` (pre-fit presets) restores as Cover — the aspect-correct default.
    #[serde(default)]
    pub obstacle_fit: Option<u32>,
    /// Obstacle alpha threshold.
    #[serde(default)]
    pub obstacle_threshold: Option<f32>,
    /// Obstacle elasticity/restitution.
    #[serde(default)]
    pub obstacle_elasticity: Option<f32>,
    /// True if obstacle source is depth estimation.
    #[serde(default)]
    pub obstacle_depth: Option<bool>,
    /// True if obstacle source is a 3D model (#1851); the model file lives in
    /// `obstacle_image_path` and is re-loaded + depth-rasterized on restore.
    #[serde(default)]
    pub obstacle_model: Option<bool>,
    /// Live Lattice (3D CA) tunables captured from the contextual panel; `None`
    /// for non-lattice effects and old presets. Includes the embedded `render`
    /// camera/palette look, so a Lattice effect round-trips fully.
    #[serde(default)]
    pub lattice: Option<LatticeParams>,
    /// Live Helix (swept audio-history ribbon) tunables captured from the
    /// contextual panel; `None` for non-Helix effects and old presets. Includes
    /// the embedded `render` look, so a Helix effect round-trips fully.
    #[serde(default)]
    pub helix: Option<HelixParams>,
    /// Live particle-sim knobs edited in the right panel (emit/lifetime/etc.).
    /// `None` for old presets; restored over the `.pfx` defaults on load.
    #[serde(default)]
    pub particle_sim: Option<ParticleSimPreset>,
}

/// The six particle-sim knobs exposed by the contextual particle panel, grouped
/// so they round-trip through the preset independent of the effect's `.pfx`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ParticleSimPreset {
    pub emit_rate: f32,
    pub burst_on_beat: u32,
    pub lifetime: f32,
    pub initial_speed: f32,
    pub initial_size: f32,
    pub drag: f32,
    /// Runtime "Trail length" slider override. `None` (the default, and the value
    /// for any preset saved before this field existed) means "leave the effect's
    /// `.pfx` trail length alone" so trails aren't silently disabled on load.
    #[serde(default)]
    pub trail_length: Option<u32>,
}

impl Default for ParticleSimPreset {
    fn default() -> Self {
        // Mirror ParticleDef's serde defaults so a partial/hand-edited block
        // never yields a dead emitter.
        Self {
            emit_rate: default_emit_rate(),
            burst_on_beat: 0,
            lifetime: default_lifetime(),
            initial_speed: default_initial_speed(),
            initial_size: default_initial_size(),
            drag: default_drag(),
            trail_length: None,
        }
    }
}

/// Global Volumetric (R3) state, persisted at preset scope like `postprocess`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VolumetricPreset {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub params: VolumetricParams,
}

fn default_opacity() -> f32 {
    1.0
}

fn default_displace_amount() -> f32 {
    crate::gpu::layer::DEFAULT_DISPLACE_AMOUNT
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub layers: Vec<LayerPreset>,
    #[serde(default)]
    pub active_layer: usize,
    #[serde(default)]
    pub postprocess: PostProcessDef,
    /// Global Volumetric (R3) mode state — a global like `postprocess`, not a
    /// per-layer property. `None` for old presets.
    #[serde(default)]
    pub volumetric: Option<VolumetricPreset>,
}

#[derive(Default)]
pub struct PresetStore {
    pub presets: Vec<(String, Preset)>,
    pub current_preset: Option<usize>,
    pub dirty: bool,
    /// Number of built-in presets at the start of the `presets` vec.
    pub builtin_count: usize,
}

impl PresetStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true if the preset at `index` is a built-in preset.
    pub fn is_builtin(&self, index: usize) -> bool {
        index < self.builtin_count
    }

    /// Mark the current preset as dirty (modified since last save/load).
    pub fn mark_dirty(&mut self) {
        if self.current_preset.is_some() {
            self.dirty = true;
        }
    }

    /// Get the name of the currently loaded preset, if any.
    pub fn current_name(&self) -> Option<&str> {
        self.current_preset
            .and_then(|i| self.presets.get(i))
            .map(|(name, _)| name.as_str())
    }

    pub fn presets_dir() -> PathBuf {
        let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        config_dir.join("phosphor").join("presets")
    }

    /// Returns the set of built-in preset names (lowercase) for shadow detection.
    fn builtin_names() -> Vec<String> {
        BUILTIN_PRESETS
            .iter()
            .map(|(name, _)| name.to_lowercase())
            .collect()
    }

    pub fn scan(&mut self) {
        self.presets.clear();
        self.builtin_count = 0;

        // 1. Parse embedded built-in presets
        for &(name, json) in BUILTIN_PRESETS {
            match serde_json::from_str::<Preset>(json) {
                Ok(preset) => {
                    self.presets.push((name.to_string(), preset));
                    self.builtin_count += 1;
                }
                Err(e) => {
                    log::error!("Failed to parse built-in preset '{}': {e}", name);
                }
            }
        }

        // 2. Load user presets from disk, skip names that shadow built-ins
        let dir = Self::presets_dir();
        let builtin_names = Self::builtin_names();

        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => {
                log::info!(
                    "Scanned {} presets ({} built-in, 0 user)",
                    self.presets.len(),
                    self.builtin_count
                );
                self.current_preset = None;
                self.dirty = false;
                return;
            }
        };

        let mut user_presets: Vec<(String, Preset)> = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            // Skip user presets that shadow built-in names
            if builtin_names.contains(&name.to_lowercase()) {
                log::info!("Skipping user preset '{}' (shadows built-in)", name);
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(contents) => match serde_json::from_str::<Preset>(&contents) {
                    Ok(preset) => {
                        user_presets.push((name, preset));
                    }
                    Err(e) => {
                        log::warn!("Failed to parse preset {}: {e}", path.display());
                    }
                },
                Err(e) => {
                    log::warn!("Failed to read preset {}: {e}", path.display());
                }
            }
        }

        // Sort user presets alphabetically
        user_presets.sort_by(|a, b| a.0.cmp(&b.0));
        self.presets.extend(user_presets);

        self.current_preset = None;
        self.dirty = false;

        let user_count = self.presets.len() - self.builtin_count;
        log::info!(
            "Scanned {} presets ({} built-in, {} user) from {}",
            self.presets.len(),
            self.builtin_count,
            user_count,
            dir.display()
        );
    }

    fn sanitize_name(name: &str) -> String {
        let sanitized: String = name
            .chars()
            .map(|c| {
                if c == '/' || c == '\\' || c == '.' {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        let trimmed = sanitized.trim();
        if trimmed.len() > 64 {
            trimmed[..64].to_string()
        } else {
            trimmed.to_string()
        }
    }

    pub fn save(
        &mut self,
        name: &str,
        layers: Vec<LayerPreset>,
        active_layer: usize,
        postprocess: &PostProcessDef,
        volumetric: Option<VolumetricPreset>,
    ) -> Result<usize> {
        let name = Self::sanitize_name(name);
        if name.is_empty() {
            anyhow::bail!("Preset name cannot be empty");
        }

        // Reject saving with a built-in name
        let builtin_names = Self::builtin_names();
        if builtin_names.contains(&name.to_lowercase()) {
            anyhow::bail!("Cannot overwrite built-in preset '{}'", name);
        }

        let dir = Self::presets_dir();
        std::fs::create_dir_all(&dir)?;

        let preset = Preset {
            layers,
            active_layer,
            postprocess: postprocess.clone(),
            volumetric,
        };

        let path = dir.join(format!("{name}.json"));
        let json = serde_json::to_string_pretty(&preset)?;
        std::fs::write(&path, json)?;
        log::info!("Saved preset '{}' to {}", name, path.display());

        self.scan();

        let idx = self
            .presets
            .iter()
            .position(|(n, _)| n == &name)
            .unwrap_or(0);
        self.current_preset = Some(idx);
        self.dirty = false;
        Ok(idx)
    }

    pub fn load(&self, index: usize) -> Option<&Preset> {
        self.presets.get(index).map(|(_, p)| p)
    }

    pub fn delete(&mut self, index: usize) -> Result<()> {
        // Reject deleting built-in presets
        if self.is_builtin(index) {
            anyhow::bail!("Cannot delete built-in preset");
        }

        let (name, _) = self
            .presets
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Invalid preset index"))?;

        let path = Self::presets_dir().join(format!("{name}.json"));
        if path.exists() {
            std::fs::remove_file(&path)?;
            log::info!("Deleted preset '{}'", name);
        }

        self.scan();
        Ok(())
    }

    /// Copy a preset to disk as a user preset with the given name.
    /// Returns the index of the new preset after re-scan.
    pub fn copy_preset(&mut self, source_index: usize, new_name: &str) -> Result<usize> {
        let new_name = Self::sanitize_name(new_name);
        if new_name.is_empty() {
            anyhow::bail!("Copy name cannot be empty");
        }

        // Reject copying to a built-in name
        let builtin_names = Self::builtin_names();
        if builtin_names.contains(&new_name.to_lowercase()) {
            anyhow::bail!("Cannot overwrite built-in preset '{}'", new_name);
        }

        let preset = self
            .presets
            .get(source_index)
            .ok_or_else(|| anyhow::anyhow!("Invalid source preset index"))?
            .1
            .clone();

        let dir = Self::presets_dir();
        std::fs::create_dir_all(&dir)?;

        let path = dir.join(format!("{new_name}.json"));
        let json = serde_json::to_string_pretty(&preset)?;
        std::fs::write(&path, json)?;
        log::info!("Copied preset to '{}'", new_name);

        self.scan();

        let idx = self
            .presets
            .iter()
            .position(|(n, _)| n == &new_name)
            .unwrap_or(0);
        self.current_preset = Some(idx);
        self.dirty = false;
        Ok(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_name_strips_slashes() {
        assert_eq!(PresetStore::sanitize_name("a/b\\c"), "a_b_c");
    }

    #[test]
    fn sanitize_name_strips_dots() {
        assert_eq!(PresetStore::sanitize_name("my.preset"), "my_preset");
    }

    #[test]
    fn sanitize_name_trims_whitespace() {
        assert_eq!(PresetStore::sanitize_name("  hello  "), "hello");
    }

    #[test]
    fn sanitize_name_max_64_chars() {
        let long = "a".repeat(100);
        assert_eq!(PresetStore::sanitize_name(&long).len(), 64);
    }

    #[test]
    fn preset_store_new_empty() {
        let s = PresetStore::new();
        assert!(s.presets.is_empty());
        assert!(s.current_preset.is_none());
        assert!(!s.dirty);
        assert_eq!(s.builtin_count, 0);
    }

    #[test]
    fn mark_dirty_without_current_preset() {
        let mut s = PresetStore::new();
        s.mark_dirty();
        assert!(!s.dirty); // no current preset, so dirty stays false
    }

    #[test]
    fn mark_dirty_with_current_preset() {
        let mut s = PresetStore::new();
        s.current_preset = Some(0);
        s.mark_dirty();
        assert!(s.dirty);
    }

    #[test]
    fn current_name_returns_correct() {
        let mut s = PresetStore::new();
        let preset = Preset {
            layers: vec![],
            active_layer: 0,
            postprocess: PostProcessDef::default(),
            volumetric: None,
        };
        s.presets.push(("Test Preset".into(), preset));
        s.current_preset = Some(0);
        assert_eq!(s.current_name(), Some("Test Preset"));
    }

    // ---- Additional tests ----

    #[test]
    fn sanitize_name_all_bad_chars() {
        assert_eq!(PresetStore::sanitize_name("/.\\."), "____");
    }

    #[test]
    fn sanitize_name_whitespace_only() {
        assert_eq!(PresetStore::sanitize_name("   "), "");
    }

    #[test]
    fn sanitize_name_exact_64_chars() {
        let s = "a".repeat(64);
        assert_eq!(PresetStore::sanitize_name(&s).len(), 64);
    }

    #[test]
    fn sanitize_name_65_truncated() {
        let s = "b".repeat(65);
        assert_eq!(PresetStore::sanitize_name(&s).len(), 64);
    }

    #[test]
    fn layer_preset_minimal_json_defaults() {
        let json = r#"{"effect_name": "Aurora"}"#;
        let lp: LayerPreset = serde_json::from_str(json).unwrap();
        assert_eq!(lp.effect_name, "Aurora");
        assert!(lp.params.is_empty());
        assert_eq!(lp.blend_mode, BlendMode::Normal);
        assert!((lp.opacity - 1.0).abs() < 1e-6);
        // Pre-Rift presets carry no displace_amount (#1478) — they must land on
        // the default rather than 0.0, or a saved Displace layer would restore
        // with the warp switched off.
        assert!(
            (lp.displace_amount - crate::gpu::layer::DEFAULT_DISPLACE_AMOUNT).abs() < 1e-6,
            "got {}",
            lp.displace_amount
        );
        assert!(lp.enabled);
        assert!(!lp.locked);
        assert!(!lp.pinned);
        assert!(lp.custom_name.is_none());
        assert!(lp.media_path.is_none());
        // New particle source fields default to None
        assert!(lp.particle_video_path.is_none());
        assert!(lp.particle_video_speed.is_none());
        assert!(lp.particle_video_looping.is_none());
        assert!(lp.particle_webcam.is_none());
    }

    #[test]
    fn layer_preset_particle_source_serde() {
        let json = r#"{
            "effect_name": "Raster",
            "particle_video_path": "/tmp/test.mp4",
            "particle_video_speed": 1.5,
            "particle_video_looping": false,
            "particle_webcam": null
        }"#;
        let lp: LayerPreset = serde_json::from_str(json).unwrap();
        assert_eq!(lp.particle_video_path.as_deref(), Some("/tmp/test.mp4"));
        assert!((lp.particle_video_speed.unwrap() - 1.5).abs() < 1e-6);
        assert_eq!(lp.particle_video_looping, Some(false));
        assert!(lp.particle_webcam.is_none());

        // Roundtrip
        let serialized = serde_json::to_string(&lp).unwrap();
        let lp2: LayerPreset = serde_json::from_str(&serialized).unwrap();
        assert_eq!(lp2.particle_video_path, lp.particle_video_path);
        assert_eq!(lp2.particle_video_speed, lp.particle_video_speed);
        assert_eq!(lp2.particle_video_looping, lp.particle_video_looping);
    }

    #[test]
    fn layer_preset_splat_scene_path_serde() {
        // Round-trip + back-compat: pre-Splat presets deserialize with None.
        let json = r#"{
            "effect_name": "Splat",
            "splat_scene_path": "/home/user/scenes/studio.ply"
        }"#;
        let lp: LayerPreset = serde_json::from_str(json).unwrap();
        assert_eq!(
            lp.splat_scene_path.as_deref(),
            Some("/home/user/scenes/studio.ply")
        );
        let lp2: LayerPreset = serde_json::from_str(&serde_json::to_string(&lp).unwrap()).unwrap();
        assert_eq!(lp2.splat_scene_path, lp.splat_scene_path);

        let old: LayerPreset = serde_json::from_str(r#"{"effect_name": "Splat"}"#).unwrap();
        assert!(old.splat_scene_path.is_none());
    }

    #[test]
    fn layer_preset_particle_webcam_serde() {
        let json = r#"{
            "effect_name": "Raster",
            "particle_webcam": true
        }"#;
        let lp: LayerPreset = serde_json::from_str(json).unwrap();
        assert_eq!(lp.particle_webcam, Some(true));
        assert!(lp.particle_video_path.is_none());
    }

    #[test]
    fn preset_serde_roundtrip() {
        let preset = Preset {
            layers: vec![LayerPreset {
                effect_name: "Test".into(),
                params: HashMap::new(),
                blend_mode: BlendMode::Add,
                opacity: 0.5,
                displace_amount: 0.75,
                enabled: true,
                locked: false,
                pinned: true,
                custom_name: Some("My Layer".into()),
                media_path: None,
                media_speed: None,
                media_looping: None,
                webcam_device: None,
                particle_video_path: None,
                particle_video_speed: None,
                particle_video_looping: None,
                particle_webcam: None,
                particle_image_path: None,
                particle_model_path: None,
                particle_model_pose: None,
                splat_scene_path: None,
                obstacle_image_path: None,
                obstacle_mode: None,
                obstacle_fit: None,
                obstacle_threshold: None,
                obstacle_elasticity: None,
                obstacle_depth: None,
                obstacle_model: None,
                lattice: None,
                helix: None,
                particle_sim: None,
            }],
            active_layer: 0,
            postprocess: PostProcessDef::default(),
            volumetric: None,
        };
        let json = serde_json::to_string(&preset).unwrap();
        let p2: Preset = serde_json::from_str(&json).unwrap();
        assert_eq!(p2.layers.len(), 1);
        assert_eq!(p2.layers[0].effect_name, "Test");
        assert_eq!(p2.layers[0].blend_mode, BlendMode::Add);
        assert!((p2.layers[0].opacity - 0.5).abs() < 1e-6);
        assert!((p2.layers[0].displace_amount - 0.75).abs() < 1e-6);
        assert!(p2.layers[0].pinned);
    }

    #[test]
    fn layer_preset_obstacle_serde() {
        let json = r#"{
            "effect_name": "Cascade",
            "obstacle_image_path": "/tmp/logo.png",
            "obstacle_mode": 2,
            "obstacle_fit": 1,
            "obstacle_threshold": 0.3,
            "obstacle_elasticity": 0.8
        }"#;
        let lp: LayerPreset = serde_json::from_str(json).unwrap();
        assert_eq!(lp.obstacle_image_path, Some("/tmp/logo.png".to_string()));
        assert_eq!(lp.obstacle_mode, Some(2));
        assert_eq!(lp.obstacle_fit, Some(1));
        assert!((lp.obstacle_threshold.unwrap() - 0.3).abs() < 1e-6);
        assert!((lp.obstacle_elasticity.unwrap() - 0.8).abs() < 1e-6);
    }

    #[test]
    fn layer_preset_obstacle_backward_compat() {
        // Old preset without obstacle fields should parse fine
        let json = r#"{
            "effect_name": "Flux",
            "opacity": 0.8
        }"#;
        let lp: LayerPreset = serde_json::from_str(json).unwrap();
        assert!(lp.obstacle_image_path.is_none());
        assert!(lp.obstacle_mode.is_none());
        // Pre-#1790 preset: no fit field -> restore keeps the Cover default.
        assert!(lp.obstacle_fit.is_none());
        assert!(lp.obstacle_threshold.is_none());
        assert!(lp.obstacle_elasticity.is_none());
        assert!(lp.obstacle_depth.is_none());
    }

    #[test]
    fn layer_preset_obstacle_depth_serde() {
        let json = r#"{
            "effect_name": "Cascade",
            "obstacle_depth": true,
            "obstacle_mode": 0,
            "obstacle_threshold": 0.5,
            "obstacle_elasticity": 0.7
        }"#;
        let lp: LayerPreset = serde_json::from_str(json).unwrap();
        assert_eq!(lp.obstacle_depth, Some(true));
        assert!(lp.obstacle_image_path.is_none());
        // Roundtrip
        let serialized = serde_json::to_string(&lp).unwrap();
        let lp2: LayerPreset = serde_json::from_str(&serialized).unwrap();
        assert_eq!(lp2.obstacle_depth, Some(true));
    }

    #[test]
    fn layer_preset_particle_model_serde() {
        // A model particle source (#1993) stores its path plus the pose it was
        // sampled at, so a restored preset shows the same picture rather than
        // snapping back to a front-on default view.
        let json = r#"{
            "effect_name": "Pegboard",
            "particle_model_path": "/home/user/models/skull.glb",
            "particle_model_pose": [25.0, 15.0, 1.2, 0.3]
        }"#;
        let lp: LayerPreset = serde_json::from_str(json).unwrap();
        assert_eq!(
            lp.particle_model_path.as_deref(),
            Some("/home/user/models/skull.glb")
        );
        assert_eq!(lp.particle_model_pose, Some([25.0, 15.0, 1.2, 0.3]));

        let lp2: LayerPreset = serde_json::from_str(&serde_json::to_string(&lp).unwrap()).unwrap();
        assert_eq!(lp2.particle_model_path, lp.particle_model_path);
        assert_eq!(lp2.particle_model_pose, lp.particle_model_pose);

        // Every preset written before #1993 must still parse, with no model.
        let old: LayerPreset = serde_json::from_str(
            r#"{"effect_name":"Pegboard","particle_image_path":"/img/a.png"}"#,
        )
        .unwrap();
        assert!(old.particle_model_path.is_none());
        assert!(old.particle_model_pose.is_none());
    }

    #[test]
    fn layer_preset_obstacle_model_serde() {
        // A 3D-model obstacle stores its file in obstacle_image_path plus the
        // obstacle_model discriminator, so restore re-loads it via the model
        // path instead of image::open (#1851).
        let json = r#"{
            "effect_name": "Tide",
            "obstacle_model": true,
            "obstacle_image_path": "/home/user/models/skull.glb",
            "obstacle_mode": 3,
            "obstacle_threshold": 0.4
        }"#;
        let lp: LayerPreset = serde_json::from_str(json).unwrap();
        assert_eq!(lp.obstacle_model, Some(true));
        assert_eq!(
            lp.obstacle_image_path.as_deref(),
            Some("/home/user/models/skull.glb")
        );
        // Roundtrip
        let serialized = serde_json::to_string(&lp).unwrap();
        let lp2: LayerPreset = serde_json::from_str(&serialized).unwrap();
        assert_eq!(lp2.obstacle_model, Some(true));
        assert_eq!(lp2.obstacle_image_path, lp.obstacle_image_path);
        // Old presets without the field default to None.
        let old: LayerPreset = serde_json::from_str(r#"{"effect_name":"Tide"}"#).unwrap();
        assert!(old.obstacle_model.is_none());
    }

    #[test]
    fn layer_preset_particle_image_serde() {
        let json = r#"{
            "effect_name": "Raster",
            "particle_image_path": "/home/user/images/skull.png"
        }"#;
        let lp: LayerPreset = serde_json::from_str(json).unwrap();
        assert_eq!(
            lp.particle_image_path,
            Some("/home/user/images/skull.png".to_string())
        );
        // Roundtrip
        let serialized = serde_json::to_string(&lp).unwrap();
        let lp2: LayerPreset = serde_json::from_str(&serialized).unwrap();
        assert_eq!(lp2.particle_image_path, lp.particle_image_path);
    }

    #[test]
    fn layer_preset_particle_image_backward_compat() {
        // Old preset without particle_image_path should parse with None
        let json = r#"{
            "effect_name": "Cascade",
            "particle_video_path": "/tmp/fire.mp4"
        }"#;
        let lp: LayerPreset = serde_json::from_str(json).unwrap();
        assert!(lp.particle_image_path.is_none());
        assert_eq!(lp.particle_video_path, Some("/tmp/fire.mp4".to_string()));
    }

    #[test]
    fn current_name_none_when_no_preset() {
        let s = PresetStore::new();
        assert!(s.current_name().is_none());
    }

    // ---- Lattice / particle-sim / volumetric persistence (#1721) ----

    #[test]
    fn layer_preset_lattice_particle_sim_backward_compat() {
        // Old preset without the new blocks parses with None.
        let json = r#"{ "effect_name": "Flux" }"#;
        let lp: LayerPreset = serde_json::from_str(json).unwrap();
        assert!(lp.lattice.is_none());
        assert!(lp.particle_sim.is_none());
    }

    #[test]
    fn layer_preset_lattice_serde_roundtrip() {
        let lat = LatticeParams {
            grid_res: 128,
            max_age: 42,
            render: VolumetricParams {
                cam_distance: 4.5,
                palette_hue: 0.25,
                ..Default::default()
            },
            ..Default::default()
        };
        let lp = LayerPreset {
            effect_name: "Clouds".into(),
            params: HashMap::new(),
            blend_mode: BlendMode::Normal,
            opacity: 1.0,
            displace_amount: crate::gpu::layer::DEFAULT_DISPLACE_AMOUNT,
            enabled: true,
            locked: false,
            pinned: false,
            custom_name: None,
            media_path: None,
            media_speed: None,
            media_looping: None,
            webcam_device: None,
            particle_video_path: None,
            particle_video_speed: None,
            particle_video_looping: None,
            particle_webcam: None,
            particle_image_path: None,
            particle_model_path: None,
            particle_model_pose: None,
            splat_scene_path: None,
            obstacle_image_path: None,
            obstacle_mode: None,
            obstacle_fit: None,
            obstacle_threshold: None,
            obstacle_elasticity: None,
            obstacle_depth: None,
            obstacle_model: None,
            lattice: Some(lat),
            helix: None,
            particle_sim: None,
        };
        let json = serde_json::to_string(&lp).unwrap();
        let lp2: LayerPreset = serde_json::from_str(&json).unwrap();
        let got = lp2.lattice.expect("lattice survives round-trip");
        assert_eq!(got.grid_res, 128);
        assert_eq!(got.max_age, 42);
        // The embedded `render` look nests correctly.
        assert!((got.render.cam_distance - 4.5).abs() < 1e-6);
        assert!((got.render.palette_hue - 0.25).abs() < 1e-6);
    }

    #[test]
    fn particle_sim_preset_serde_and_defaults() {
        // Full round-trip.
        let sim = ParticleSimPreset {
            emit_rate: 250.0,
            burst_on_beat: 3,
            lifetime: 1.25,
            initial_speed: 0.7,
            initial_size: 0.03,
            drag: 0.9,
            trail_length: Some(12),
        };
        let json = serde_json::to_string(&sim).unwrap();
        let sim2: ParticleSimPreset = serde_json::from_str(&json).unwrap();
        assert!((sim2.emit_rate - 250.0).abs() < 1e-6);
        assert_eq!(sim2.burst_on_beat, 3);
        assert!((sim2.drag - 0.9).abs() < 1e-6);
        assert_eq!(sim2.trail_length, Some(12));

        // Partial block fills the rest from ParticleDef's serde defaults (proves
        // container #[serde(default)] — no dead emitter from a partial JSON). A
        // block predating the trail_length field parses to None = "keep .pfx".
        let partial: ParticleSimPreset = serde_json::from_str(r#"{ "lifetime": 5.0 }"#).unwrap();
        assert!((partial.lifetime - 5.0).abs() < 1e-6);
        assert!((partial.emit_rate - default_emit_rate()).abs() < 1e-6);
        assert!((partial.initial_speed - default_initial_speed()).abs() < 1e-6);
        assert!((partial.drag - default_drag()).abs() < 1e-6);
        assert_eq!(partial.trail_length, None);
    }

    #[test]
    fn preset_volumetric_backward_compat_and_roundtrip() {
        // Old preset without the top-level volumetric block parses with None.
        let json = r#"{ "layers": [], "active_layer": 0 }"#;
        let p: Preset = serde_json::from_str(json).unwrap();
        assert!(p.volumetric.is_none());

        // Round-trip a populated volumetric block.
        let params = VolumetricParams {
            absorption: 0.5,
            cam_distance: 6.0,
            ..Default::default()
        };
        let preset = Preset {
            layers: vec![],
            active_layer: 0,
            postprocess: PostProcessDef::default(),
            volumetric: Some(VolumetricPreset {
                enabled: true,
                params,
            }),
        };
        let json = serde_json::to_string(&preset).unwrap();
        let p2: Preset = serde_json::from_str(&json).unwrap();
        let vol = p2.volumetric.expect("volumetric survives round-trip");
        assert!(vol.enabled);
        assert!((vol.params.absorption - 0.5).abs() < 1e-6);
        assert!((vol.params.cam_distance - 6.0).abs() < 1e-6);
    }

    #[test]
    fn volumetric_params_partial_serde_fills_defaults() {
        // A partial params block fills the rest from Default (container serde(default)).
        let params: VolumetricParams = serde_json::from_str(r#"{ "absorption": 0.5 }"#).unwrap();
        assert!((params.absorption - 0.5).abs() < 1e-6);
        let def = VolumetricParams::default();
        assert_eq!(params.march_steps, def.march_steps);
        assert!((params.cam_distance - def.cam_distance).abs() < 1e-6);
    }

    // ---- Built-in preset tests ----

    #[test]
    fn builtin_presets_parse() {
        // Verify all embedded built-in presets parse correctly
        for &(name, json) in BUILTIN_PRESETS {
            let preset: Preset = serde_json::from_str(json)
                .unwrap_or_else(|e| panic!("Built-in preset '{}' failed to parse: {}", name, e));
            assert!(
                !preset.layers.is_empty(),
                "Built-in preset '{}' has no layers",
                name
            );
        }
    }

    #[test]
    fn builtin_presets_reference_effects_that_still_ship() {
        // Crucible layer 7 and Spectral Eye layer 2 shipped for months pointing at
        // "Swarm", deleted in a42c6cb. Nothing caught it: apply_preset_immediately
        // only warned, and the layer kept whatever effect was loaded before it.
        //
        // CARGO_MANIFEST_DIR, not assets_dir(): that resolves CWD-relative, and
        // `cargo test` runs with CWD = crates/phosphor-app, which has no assets/.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/effects");
        let shipped: std::collections::HashSet<String> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "pfx"))
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .filter_map(|j| serde_json::from_str::<crate::effect::format::PfxEffect>(&j).ok())
            .map(|p| p.name)
            .collect();
        assert!(
            !shipped.is_empty(),
            "no .pfx files found in {}",
            dir.display()
        );

        for &(preset_name, json) in BUILTIN_PRESETS {
            let preset: Preset = serde_json::from_str(json).unwrap();
            for (i, l) in preset.layers.iter().enumerate() {
                if l.effect_name.is_empty() || l.media_path.is_some() || l.webcam_device.is_some() {
                    continue;
                }
                assert!(
                    shipped.contains(&l.effect_name),
                    "built-in preset '{preset_name}' layer {i} wants effect '{}', \
                     which no longer ships",
                    l.effect_name
                );
            }
        }
    }

    #[test]
    fn builtin_preset_params_are_in_range() {
        // The other half of the Swarm fallout: ParamStore::set does NOT clamp to the
        // declared range (params/store.rs), so a stale value reaches the shader out of
        // bounds. Crucible's trail_decay was 0.98, above Array's max of 0.96.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/effects");
        let effects: Vec<crate::effect::format::PfxEffect> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "pfx"))
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .filter_map(|j| serde_json::from_str(&j).ok())
            .collect();

        for &(preset_name, json) in BUILTIN_PRESETS {
            let preset: Preset = serde_json::from_str(json).unwrap();
            for (i, l) in preset.layers.iter().enumerate() {
                let Some(eff) = effects.iter().find(|e| e.name == l.effect_name) else {
                    continue;
                };
                for (name, value) in &l.params {
                    let Some(def) = eff.inputs.iter().find(|d| d.name() == name) else {
                        panic!(
                            "built-in preset '{preset_name}' layer {i} sets '{name}', \
                             which '{}' does not have",
                            eff.name
                        );
                    };
                    if let (crate::params::ParamDef::Float { min, max, .. }, ParamValue::Float(v)) =
                        (def, value)
                    {
                        assert!(
                            *v >= *min && *v <= *max,
                            "built-in preset '{preset_name}' layer {i}: {name} = {v} \
                             is outside {}'s declared range {min}..{max}",
                            eff.name
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn is_builtin_for_indices() {
        let mut s = PresetStore::new();
        s.builtin_count = 2;
        let empty_preset = Preset {
            layers: vec![],
            active_layer: 0,
            postprocess: PostProcessDef::default(),
            volumetric: None,
        };
        s.presets.push(("Crucible".into(), empty_preset.clone()));
        s.presets
            .push(("Spectral Eye".into(), empty_preset.clone()));
        s.presets.push(("User Preset".into(), empty_preset));

        assert!(s.is_builtin(0));
        assert!(s.is_builtin(1));
        assert!(!s.is_builtin(2));
        assert!(!s.is_builtin(99));
    }

    #[test]
    fn delete_builtin_returns_error() {
        let mut s = PresetStore::new();
        s.builtin_count = 1;
        let empty_preset = Preset {
            layers: vec![],
            active_layer: 0,
            postprocess: PostProcessDef::default(),
            volumetric: None,
        };
        s.presets.push(("Crucible".into(), empty_preset));

        let result = s.delete(0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("built-in"));
    }

    #[test]
    fn save_builtin_name_fails() {
        let mut s = PresetStore::new();
        s.builtin_count = 1;
        let empty_preset = Preset {
            layers: vec![],
            active_layer: 0,
            postprocess: PostProcessDef::default(),
            volumetric: None,
        };
        s.presets.push(("Crucible".into(), empty_preset));

        let result = s.save("Crucible", vec![], 0, &PostProcessDef::default(), None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("built-in"));

        // Case-insensitive check
        let result2 = s.save("crucible", vec![], 0, &PostProcessDef::default(), None);
        assert!(result2.is_err());
    }
}
