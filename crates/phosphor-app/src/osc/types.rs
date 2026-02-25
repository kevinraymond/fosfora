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
    LayerParam { layer: usize, name: String, value: f32 },
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

fn default_version() -> u32 { 1 }
fn default_true() -> bool { true }
fn default_rx_port() -> u16 { 9000 }
fn default_tx_port() -> u16 { 9001 }
fn default_tx_host() -> String { "127.0.0.1".to_string() }
fn default_tx_rate() -> u32 { 30 }

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
