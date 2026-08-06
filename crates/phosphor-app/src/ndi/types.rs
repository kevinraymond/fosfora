use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// Re-export OutputResolution from shared gpu::types module.
pub use crate::gpu::types::OutputResolution;

/// Persisted NDI output configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NdiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_source_name")]
    pub source_name: String,
    #[serde(default)]
    pub resolution: OutputResolution,
    #[serde(default)]
    pub alpha_from_luma: bool,
}

fn default_source_name() -> String {
    "Fosfora".to_string()
}

impl Default for NdiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            source_name: default_source_name(),
            resolution: OutputResolution::default(),
            alpha_from_luma: false,
        }
    }
}

impl NdiConfig {
    pub fn config_path() -> PathBuf {
        crate::paths::config_root().join("ndi.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(config) => {
                    log::info!("Loaded NDI config from {}", path.display());
                    config
                }
                Err(e) => {
                    log::warn!("Failed to parse NDI config: {e}");
                    Self::default()
                }
            },
            Err(_) => {
                log::info!("No NDI config found, using defaults");
                Self::default()
            }
        }
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                log::error!("Failed to create config dir: {e}");
                return;
            }
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    log::error!("Failed to write NDI config: {e}");
                } else {
                    log::debug!("Saved NDI config to {}", path.display());
                }
            }
            Err(e) => log::error!("Failed to serialize NDI config: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ndi_config_defaults() {
        let c = NdiConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.source_name, "Fosfora");
        assert_eq!(c.resolution, OutputResolution::Match);
    }

    #[test]
    fn ndi_config_serde_roundtrip() {
        let c = NdiConfig::default();
        let json = serde_json::to_string(&c).unwrap();
        let c2: NdiConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.source_name, "Fosfora");
        assert!(!c2.enabled);
        assert!(!c2.alpha_from_luma);
    }

    #[test]
    fn ndi_config_alpha_from_luma_roundtrip() {
        let c = NdiConfig {
            alpha_from_luma: true,
            ..Default::default()
        };
        let json = serde_json::to_string(&c).unwrap();
        let c2: NdiConfig = serde_json::from_str(&json).unwrap();
        assert!(c2.alpha_from_luma);
    }

    #[test]
    fn ndi_config_partial_json_defaults() {
        let json = r#"{"source_name": "Custom"}"#;
        let c: NdiConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.source_name, "Custom");
        assert!(!c.enabled);
        assert_eq!(c.resolution, OutputResolution::Match);
    }
}
