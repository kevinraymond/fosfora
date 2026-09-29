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
    BindPreview { source: String, jpeg_data: Vec<u8> },
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
    /// Last effect load this frame. Loads are coalesced: each one compiles
    /// shaders synchronously, and only the last would stay on screen.
    pub effect_load: Option<usize>,
    pub select_layer: Option<usize>,
    /// Last preset load this frame (coalesced like `effect_load`).
    pub preset_load: Option<usize>,
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
            effect_load: None,
            select_layer: None,
            preset_load: None,
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
    /// Accept connections from other devices (a phone, a bridge in Docker on
    /// Linux). Off: the server listens on 127.0.0.1 only, so nothing on the
    /// venue network can drive the show.
    #[serde(default)]
    pub lan: bool,
    /// What another device must present to connect: `?key=` on the network
    /// link (the page passes it on to its WebSocket), `--key` / `FOSFORA_KEY`
    /// for a bridge. This computer's own connections need none. Created on
    /// first start and kept across restarts, so a bookmarked link or a Docker
    /// env var keeps working; replacing it disconnects every holder of the old
    /// one. Empty refuses every other device.
    #[serde(default)]
    pub access_key: String,
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
            lan: false,
            access_key: String::new(),
        }
    }
}

/// A fresh access key: 128 random bits as 32 hex digits, or empty (which
/// refuses every other device) if the OS has no randomness to give.
pub fn new_access_key() -> String {
    let mut bytes = [0u8; 16];
    match getrandom::fill(&mut bytes) {
        Ok(()) => bytes.iter().fold(String::with_capacity(32), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        }),
        Err(e) => {
            log::error!("No access key for the web remote (other devices are refused): {e}");
            String::new()
        }
    }
}

impl WebConfig {
    pub fn config_path() -> PathBuf {
        crate::paths::config_root().join("web.json")
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
        assert!(r.effect_load.is_none());
        assert!(r.select_layer.is_none());
        assert!(r.preset_load.is_none());
    }

    // ---- Additional tests ----

    #[test]
    fn web_config_disabled_roundtrip() {
        let c = WebConfig {
            enabled: false,
            port: 8080,
            lan: true,
            access_key: "k".into(),
        };
        let json = serde_json::to_string(&c).unwrap();
        let c2: WebConfig = serde_json::from_str(&json).unwrap();
        assert!(!c2.enabled);
        assert_eq!(c2.port, 8080);
        assert!(c2.lan);
        assert_eq!(c2.access_key, "k");
    }

    #[test]
    fn access_keys_are_random_hex() {
        let (a, b) = (new_access_key(), new_access_key());
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()), "{a}");
        assert_ne!(a, b);
    }

    /// A web.json saved before LAN access was a setting has no `lan` key; it
    /// loads as loopback-only (#43).
    #[test]
    fn web_config_without_lan_is_loopback_only() {
        let c: WebConfig = serde_json::from_str(r#"{"enabled":true,"port":9002}"#).unwrap();
        assert!(!c.lan);
        assert!(!WebConfig::default().lan);
    }

    #[test]
    fn web_config_partial_json_defaults() {
        let json = r#"{"port": 3000}"#;
        let c: WebConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.port, 3000);
        assert!(c.enabled); // default true
    }
}
