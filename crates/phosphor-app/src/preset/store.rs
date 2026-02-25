use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::effect::format::PostProcessDef;
use crate::gpu::layer::BlendMode;
use crate::params::ParamValue;

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
}

impl PresetStore {
    pub fn new() -> Self {
        Self {
            presets: Vec::new(),
            current_preset: None,
        }
    }

    pub fn presets_dir() -> PathBuf {
        let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        config_dir.join("phosphor").join("presets")
    }

    pub fn scan(&mut self) {
        let dir = Self::presets_dir();
        self.presets.clear();

        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };

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
            match std::fs::read_to_string(&path) {
                Ok(contents) => match serde_json::from_str::<Preset>(&contents) {
                    Ok(preset) => {
                        self.presets.push((name, preset));
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

        self.presets.sort_by(|a, b| a.0.cmp(&b.0));
        self.current_preset = None;

        log::info!("Scanned {} presets from {}", self.presets.len(), dir.display());
    }

    fn sanitize_name(name: &str) -> String {
        let sanitized: String = name
            .chars()
            .map(|c| if c == '/' || c == '\\' || c == '.' { '_' } else { c })
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
        Ok(idx)
    }

    pub fn load(&self, index: usize) -> Option<&Preset> {
        self.presets.get(index).map(|(_, p)| p)
    }

    pub fn delete(&mut self, index: usize) -> Result<()> {
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
}
