use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// Re-export OutputResolution from shared gpu::types module.
pub use crate::gpu::types::OutputResolution;

/// Persisted Spout output configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpoutConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_sender_name")]
    pub sender_name: String,
    #[serde(default)]
    pub resolution: OutputResolution,
}

fn default_sender_name() -> String {
    "Fosfora".to_string()
}

impl Default for SpoutConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sender_name: default_sender_name(),
            resolution: OutputResolution::default(),
        }
    }
}

impl SpoutConfig {
    /// The sender name to register: the configured name, or the default when
    /// the configured one is empty/whitespace (a nameless Spout sender would
    /// be unselectable in receivers).
    pub fn effective_sender_name(&self) -> String {
        let trimmed = self.sender_name.trim();
        if trimmed.is_empty() {
            default_sender_name()
        } else {
            trimmed.to_string()
        }
    }

    pub fn config_path() -> PathBuf {
        let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        config_dir.join("phosphor").join("spout.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(config) => {
                    log::info!("Loaded Spout config from {}", path.display());
                    config
                }
                Err(e) => {
                    log::warn!("Failed to parse Spout config: {e}");
                    Self::default()
                }
            },
            Err(_) => {
                log::info!("No Spout config found, using defaults");
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
                    log::error!("Failed to write Spout config: {e}");
                } else {
                    log::debug!("Saved Spout config to {}", path.display());
                }
            }
            Err(e) => log::error!("Failed to serialize Spout config: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spout_config_defaults() {
        let c = SpoutConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.sender_name, "Fosfora");
        assert_eq!(c.resolution, OutputResolution::Match);
    }

    #[test]
    fn spout_config_serde_roundtrip() {
        let c = SpoutConfig {
            enabled: true,
            sender_name: "Custom".to_string(),
            resolution: OutputResolution::Res1080p,
        };
        let json = serde_json::to_string(&c).unwrap();
        let c2: SpoutConfig = serde_json::from_str(&json).unwrap();
        assert!(c2.enabled);
        assert_eq!(c2.sender_name, "Custom");
        assert_eq!(c2.resolution, OutputResolution::Res1080p);
    }

    #[test]
    fn spout_config_partial_json_defaults() {
        let json = r#"{"enabled": true}"#;
        let c: SpoutConfig = serde_json::from_str(json).unwrap();
        assert!(c.enabled);
        assert_eq!(c.sender_name, "Fosfora");
        assert_eq!(c.resolution, OutputResolution::Match);
    }

    #[test]
    fn spout_config_empty_sender_name_falls_back() {
        let c = SpoutConfig {
            sender_name: "   ".to_string(),
            ..Default::default()
        };
        assert_eq!(c.effective_sender_name(), "Fosfora");
        let c = SpoutConfig {
            sender_name: " Deck A ".to_string(),
            ..Default::default()
        };
        assert_eq!(c.effective_sender_name(), "Deck A");
    }
}
