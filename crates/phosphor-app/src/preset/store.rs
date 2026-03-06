use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::effect::format::PostProcessDef;
use crate::gpu::layer::BlendMode;
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
    /// Obstacle collision image path.
    #[serde(default)]
    pub obstacle_image_path: Option<String>,
    /// Obstacle collision mode (0=bounce, 1=stick, 2=flow).
    #[serde(default)]
    pub obstacle_mode: Option<u32>,
    /// Obstacle alpha threshold.
    #[serde(default)]
    pub obstacle_threshold: Option<f32>,
    /// Obstacle elasticity/restitution.
    #[serde(default)]
    pub obstacle_elasticity: Option<f32>,
    /// True if obstacle source is depth estimation.
    #[serde(default)]
    pub obstacle_depth: Option<bool>,
}

fn default_opacity() -> f32 {
    1.0
}

fn default_true() -> bool {
    true
}

impl Default for BlendMode {
    fn default() -> Self {
        BlendMode::Normal
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub layers: Vec<LayerPreset>,
    #[serde(default)]
    pub active_layer: usize,
    #[serde(default)]
    pub postprocess: PostProcessDef,
}

pub struct PresetStore {
    pub presets: Vec<(String, Preset)>,
    pub current_preset: Option<usize>,
    pub dirty: bool,
    /// Number of built-in presets at the start of the `presets` vec.
    pub builtin_count: usize,
}

impl PresetStore {
    pub fn new() -> Self {
        Self {
            presets: Vec::new(),
            current_preset: None,
            dirty: false,
            builtin_count: 0,
        }
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
                obstacle_image_path: None,
                obstacle_mode: None,
                obstacle_threshold: None,
                obstacle_elasticity: None,
                obstacle_depth: None,
            }],
            active_layer: 0,
            postprocess: PostProcessDef::default(),
        };
        let json = serde_json::to_string(&preset).unwrap();
        let p2: Preset = serde_json::from_str(&json).unwrap();
        assert_eq!(p2.layers.len(), 1);
        assert_eq!(p2.layers[0].effect_name, "Test");
        assert_eq!(p2.layers[0].blend_mode, BlendMode::Add);
        assert!((p2.layers[0].opacity - 0.5).abs() < 1e-6);
        assert!(p2.layers[0].pinned);
    }

    #[test]
    fn layer_preset_obstacle_serde() {
        let json = r#"{
            "effect_name": "Cascade",
            "obstacle_image_path": "/tmp/logo.png",
            "obstacle_mode": 2,
            "obstacle_threshold": 0.3,
            "obstacle_elasticity": 0.8
        }"#;
        let lp: LayerPreset = serde_json::from_str(json).unwrap();
        assert_eq!(lp.obstacle_image_path, Some("/tmp/logo.png".to_string()));
        assert_eq!(lp.obstacle_mode, Some(2));
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
        assert_eq!(
            lp.particle_video_path,
            Some("/tmp/fire.mp4".to_string())
        );
    }

    #[test]
    fn current_name_none_when_no_preset() {
        let s = PresetStore::new();
        assert!(s.current_name().is_none());
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
    fn is_builtin_for_indices() {
        let mut s = PresetStore::new();
        s.builtin_count = 2;
        let empty_preset = Preset {
            layers: vec![],
            active_layer: 0,
            postprocess: PostProcessDef::default(),
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
        };
        s.presets.push(("Crucible".into(), empty_preset));

        let result = s.save("Crucible", vec![], 0, &PostProcessDef::default());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("built-in"));

        // Case-insensitive check
        let result2 = s.save("crucible", vec![], 0, &PostProcessDef::default());
        assert!(result2.is_err());
    }
}
