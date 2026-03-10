use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::midi::types::TriggerAction;

/// Inbound message from a WebSocket client.
#[derive(Debug, Clone)]
pub enum WsInMessage {
    /// Set param on active layer (normalized 0-1).
    SetParam { name: String, value: f32 },
    /// Set param on a specific layer (normalized 0-1).
    SetLayerParam {
        layer: usize,
        name: String,
        value: f32,
    },
    /// Load an effect by index on the active layer.
    LoadEffect { index: usize },
    /// Select the active layer.
    SelectLayer { index: usize },
    /// Set layer opacity.
    SetLayerOpacity { layer: usize, value: f32 },
    /// Set layer blend mode (0-6).
    SetLayerBlend { layer: usize, value: u32 },
    /// Set layer enabled.
    SetLayerEnabled { layer: usize, value: bool },
    /// Fire a trigger action.
    Trigger(TriggerAction),
    /// Load a preset by index.
    LoadPreset { index: usize },
    /// Toggle post-processing.
    PostProcessEnabled(bool),
    /// Binding data: source name + field values.
    BindData {
        source: String,
        fields: Vec<(String, f32)>,
    },
    /// Binding schema: source name + field metadata (for auto-discovery).
    #[allow(dead_code)]
    BindSchema {
        source: String,
        fields: Vec<(String, SourceFieldInfo)>,
    },
    /// Preview thumbnail image (JPEG) from a bridge source.
    BindPreview {
        source: String,
        jpeg_data: Vec<u8>,
    },
}

/// Metadata for a WebSocket source field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceFieldInfo {
    #[serde(default)]
    pub min: f32,
    #[serde(default = "default_one")]
    pub max: f32,
    #[serde(default)]
    pub label: String,
}

fn default_one() -> f32 {
    1.0
}

/// Result of WebSystem::update() — mirrors OscFrameResult with extras.
pub struct WebFrameResult {
    pub triggers: Vec<TriggerAction>,
    pub layer_params: Vec<(usize, String, f32)>,
    pub layer_opacity: Vec<(usize, f32)>,
    pub layer_blend: Vec<(usize, u32)>,
    pub layer_enabled: Vec<(usize, bool)>,
    pub postprocess_enabled: Option<bool>,
    pub effect_loads: Vec<usize>,
    pub select_layer: Option<usize>,
    pub preset_loads: Vec<usize>,
}

impl WebFrameResult {
    pub fn empty() -> Self {
        Self {
            triggers: Vec::new(),
            layer_params: Vec::new(),
            layer_opacity: Vec::new(),
            layer_blend: Vec::new(),
            layer_enabled: Vec::new(),
            postprocess_enabled: None,
            effect_loads: Vec::new(),
            select_layer: None,
            preset_loads: Vec::new(),
        }
    }
}

/// Persisted WebSocket server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_port")]
    pub port: u16,
}

fn default_true() -> bool {
    true
}
fn default_port() -> u16 {
    9002
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 9002,
        }
    }
}

impl WebConfig {
    pub fn config_path() -> PathBuf {
        let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        config_dir.join("phosphor").join("web.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(config) => {
                    log::info!("Loaded web config from {}", path.display());
                    config
                }
                Err(e) => {
                    log::warn!("Failed to parse web config: {e}");
                    Self::default()
                }
            },
            Err(_) => {
                log::info!("No web config found, using defaults");
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
                    log::error!("Failed to write web config: {e}");
                } else {
                    log::debug!("Saved web config to {}", path.display());
                }
            }
            Err(e) => log::error!("Failed to serialize web config: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_config_defaults() {
        let c = WebConfig::default();
        assert!(c.enabled);
        assert_eq!(c.port, 9002);
    }

    #[test]
    fn web_config_serde_roundtrip() {
        let c = WebConfig::default();
        let json = serde_json::to_string(&c).unwrap();
        let c2: WebConfig = serde_json::from_str(&json).unwrap();
        assert!(c2.enabled);
        assert_eq!(c2.port, 9002);
    }

    #[test]
    fn web_frame_result_empty() {
        let r = WebFrameResult::empty();
        assert!(r.triggers.is_empty());
        assert!(r.layer_params.is_empty());
        assert!(r.layer_opacity.is_empty());
        assert!(r.layer_blend.is_empty());
        assert!(r.layer_enabled.is_empty());
        assert!(r.postprocess_enabled.is_none());
        assert!(r.effect_loads.is_empty());
        assert!(r.select_layer.is_none());
        assert!(r.preset_loads.is_empty());
    }

    // ---- Additional tests ----

    #[test]
    fn web_config_disabled_roundtrip() {
        let c = WebConfig {
            enabled: false,
            port: 8080,
        };
        let json = serde_json::to_string(&c).unwrap();
        let c2: WebConfig = serde_json::from_str(&json).unwrap();
        assert!(!c2.enabled);
        assert_eq!(c2.port, 8080);
    }

    #[test]
    fn web_config_partial_json_defaults() {
        let json = r#"{"port": 3000}"#;
        let c: WebConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.port, 3000);
        assert!(c.enabled); // default true
    }
}
