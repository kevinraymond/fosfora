use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::midi::types::TriggerAction;

/// Parsed inbound OSC message.
#[derive(Debug, Clone)]
pub enum OscInMessage {
    /// Set param on active layer: /phosphor/param/{name}
    Param { name: String, value: f32 },
    /// Set param on specific layer: /phosphor/layer/{n}/param/{name}
    LayerParam {
        layer: usize,
        name: String,
        value: f32,
    },
    /// Fire a trigger: /phosphor/trigger/{action_name}
    Trigger(TriggerAction),
    /// Set layer opacity: /phosphor/layer/{n}/opacity
    LayerOpacity { layer: usize, value: f32 },
    /// Set layer blend mode: /phosphor/layer/{n}/blend
    LayerBlend { layer: usize, value: u32 },
    /// Set layer enabled: /phosphor/layer/{n}/enabled
    LayerEnabled { layer: usize, value: bool },
    /// Toggle post-processing: /phosphor/postprocess/enabled
    PostProcessEnabled(bool),
    /// Jump to a specific cue: /phosphor/scene/goto_cue
    SceneGotoCue(usize),
    /// Load scene by index: /phosphor/scene/load (int arg)
    SceneLoadIndex(usize),
    /// Load scene by name: /phosphor/scene/load (string arg)
    SceneLoadName(String),
    /// Set loop mode: /phosphor/scene/loop_mode
    SceneLoopMode(bool),
    /// Set advance mode: /phosphor/scene/advance_mode (0=Manual, 1=Timer, 2=BeatSync)
    SceneAdvanceMode(u8),
    /// Unrecognized address (captured for learn mode)
    Raw { address: String, value: f32 },
}

/// What we're learning an OSC mapping for.
#[derive(Debug, Clone, PartialEq)]
pub enum OscLearnTarget {
    Param(String),
    Trigger(TriggerAction),
}

/// A single OSC address → parameter or trigger mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OscMapping {
    pub address: String,
}

/// Persisted OSC configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OscConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_rx_port")]
    pub rx_port: u16,
    #[serde(default = "default_tx_port")]
    pub tx_port: u16,
    #[serde(default = "default_tx_host")]
    pub tx_host: String,
    #[serde(default)]
    pub tx_enabled: bool,
    #[serde(default = "default_tx_rate")]
    pub tx_rate_hz: u32,
    #[serde(default)]
    pub params: HashMap<String, OscMapping>,
    #[serde(default)]
    pub triggers: HashMap<TriggerAction, OscMapping>,
}

fn default_version() -> u32 {
    1
}
fn default_true() -> bool {
    true
}
fn default_rx_port() -> u16 {
    9000
}
fn default_tx_port() -> u16 {
    9001
}
fn default_tx_host() -> String {
    "127.0.0.1".to_string()
}
fn default_tx_rate() -> u32 {
    30
}

impl Default for OscConfig {
    fn default() -> Self {
        Self {
            version: 1,
            enabled: true,
            rx_port: 9000,
            tx_port: 9001,
            tx_host: "127.0.0.1".to_string(),
            tx_enabled: false,
            tx_rate_hz: 30,
            params: HashMap::new(),
            triggers: HashMap::new(),
        }
    }
}

impl OscConfig {
    pub fn config_path() -> PathBuf {
        let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        config_dir.join("phosphor").join("osc.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(config) => {
                    log::info!("Loaded OSC config from {}", path.display());
                    config
                }
                Err(e) => {
                    log::warn!("Failed to parse OSC config: {e}");
                    Self::default()
                }
            },
            Err(_) => {
                log::info!("No OSC config found, using defaults");
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
                    log::error!("Failed to write OSC config: {e}");
                } else {
                    log::debug!("Saved OSC config to {}", path.display());
                }
            }
            Err(e) => log::error!("Failed to serialize OSC config: {e}"),
        }
    }

    /// Find which param name is mapped to the given OSC address.
    pub fn find_param(&self, address: &str) -> Option<&str> {
        for (name, mapping) in &self.params {
            if mapping.address == address {
                return Some(name.as_str());
            }
        }
        None
    }

    /// Find which trigger action is mapped to the given OSC address.
    pub fn find_trigger(&self, address: &str) -> Option<TriggerAction> {
        for (action, mapping) in &self.triggers {
            if mapping.address == address {
                return Some(*action);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc_config_defaults() {
        let c = OscConfig::default();
        assert_eq!(c.rx_port, 9000);
        assert_eq!(c.tx_port, 9001);
        assert!(!c.tx_enabled);
        assert!(c.enabled);
        assert_eq!(c.tx_rate_hz, 30);
    }

    #[test]
    fn osc_config_serde_roundtrip() {
        let c = OscConfig::default();
        let json = serde_json::to_string(&c).unwrap();
        let c2: OscConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.rx_port, 9000);
        assert_eq!(c2.tx_port, 9001);
    }

    #[test]
    fn find_param_existing() {
        let mut c = OscConfig::default();
        c.params.insert(
            "speed".into(),
            OscMapping {
                address: "/custom/speed".into(),
            },
        );
        assert_eq!(c.find_param("/custom/speed"), Some("speed"));
    }

    #[test]
    fn find_param_missing() {
        let c = OscConfig::default();
        assert_eq!(c.find_param("/nonexistent"), None);
    }

    #[test]
    fn find_trigger() {
        let mut c = OscConfig::default();
        c.triggers.insert(
            TriggerAction::NextEffect,
            OscMapping {
                address: "/pad/1".into(),
            },
        );
        assert_eq!(c.find_trigger("/pad/1"), Some(TriggerAction::NextEffect));
        assert_eq!(c.find_trigger("/pad/2"), None);
    }

    // ---- Additional tests ----

    #[test]
    fn osc_config_partial_json_defaults() {
        let json = r#"{"rx_port": 8000}"#;
        let c: OscConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.rx_port, 8000);
        assert_eq!(c.tx_port, 9001); // default
        assert!(c.enabled); // default true
        assert!(!c.tx_enabled); // default false
        assert_eq!(c.tx_rate_hz, 30); // default
    }

    #[test]
    fn osc_learn_target_param_equality() {
        let a = OscLearnTarget::Param("speed".into());
        let b = OscLearnTarget::Param("speed".into());
        let c = OscLearnTarget::Param("other".into());
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn osc_learn_target_trigger_equality() {
        let a = OscLearnTarget::Trigger(TriggerAction::NextEffect);
        let b = OscLearnTarget::Trigger(TriggerAction::NextEffect);
        let c = OscLearnTarget::Trigger(TriggerAction::PrevEffect);
        assert_eq!(a, b);
        assert_ne!(a, c);
        // Param != Trigger
        assert_ne!(
            OscLearnTarget::Param("x".into()),
            OscLearnTarget::Trigger(TriggerAction::NextEffect)
        );
    }
}
