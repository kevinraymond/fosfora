use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// Re-export OutputResolution from shared gpu::types module.
pub use crate::gpu::types::OutputResolution;

/// Persisted Syphon output configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyphonConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_server_name")]
    pub server_name: String,
    #[serde(default)]
    pub resolution: OutputResolution,
}

fn default_server_name() -> String {
    "Fosfora".to_string()
}

impl Default for SyphonConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server_name: default_server_name(),
            resolution: OutputResolution::default(),
        }
    }
}

impl SyphonConfig {
    /// The server name to register: the configured name, or the default when
    /// the configured one is empty/whitespace (a nameless Syphon server would
    /// be hard to identify in client server lists).
    pub fn effective_server_name(&self) -> String {
        let trimmed = self.server_name.trim();
        if trimmed.is_empty() {
            default_server_name()
        } else {
            trimmed.to_string()
        }
    }

    pub fn config_path() -> PathBuf {
        crate::paths::config_root().join("syphon.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(config) => {
                    log::info!("Loaded Syphon config from {}", path.display());
                    config
                }
                Err(e) => {
                    log::warn!("Failed to parse Syphon config: {e}");
                    Self::default()
                }
            },
            Err(_) => {
                log::info!("No Syphon config found, using defaults");
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
                    log::error!("Failed to write Syphon config: {e}");
                } else {
                    log::debug!("Saved Syphon config to {}", path.display());
                }
            }
            Err(e) => log::error!("Failed to serialize Syphon config: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syphon_config_defaults() {
        let c = SyphonConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.server_name, "Fosfora");
        assert_eq!(c.resolution, OutputResolution::Match);
    }

    #[test]
    fn syphon_config_serde_roundtrip() {
        let c = SyphonConfig {
            enabled: true,
            server_name: "Custom".to_string(),
            resolution: OutputResolution::Res1080p,
        };
        let json = serde_json::to_string(&c).unwrap();
        let c2: SyphonConfig = serde_json::from_str(&json).unwrap();
        assert!(c2.enabled);
        assert_eq!(c2.server_name, "Custom");
        assert_eq!(c2.resolution, OutputResolution::Res1080p);
    }

    #[test]
    fn syphon_config_partial_json_defaults() {
        let json = r#"{"enabled": true}"#;
        let c: SyphonConfig = serde_json::from_str(json).unwrap();
        assert!(c.enabled);
        assert_eq!(c.server_name, "Fosfora");
        assert_eq!(c.resolution, OutputResolution::Match);
    }

    #[test]
    fn syphon_config_empty_server_name_falls_back() {
        let c = SyphonConfig {
            server_name: "   ".to_string(),
            ..Default::default()
        };
        assert_eq!(c.effective_server_name(), "Fosfora");
        let c = SyphonConfig {
            server_name: " Deck A ".to_string(),
            ..Default::default()
        };
        assert_eq!(c.effective_server_name(), "Deck A");
    }
}
