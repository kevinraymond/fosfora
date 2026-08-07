//! Persisted Signal configuration (`~/.config/fosfora/signal.json`).
//!
//! Deliberately separate from `osc.json`: a rig's OSC-learn console (9000/9001)
//! and its analysis consumer are different endpoints with different lifecycles.
//! Events and the curated continuous group are always on — they are the product;
//! only the raw feature bus (bandwidth) and the stem-proxy group are switches.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Continuous-group rate; clamped to 1..=86 at use (events always fire per hop).
    #[serde(default = "default_rate")]
    pub tx_rate_hz: u32,
    /// The raw 83-slot feature bus (~2.5k datagrams/s at 30 Hz) — opt-in.
    #[serde(default)]
    pub feat_bus: bool,
    /// The stem-proxy group (documented estimates, not real separation).
    #[serde(default = "default_true")]
    pub stems: bool,
}

fn default_version() -> u32 {
    1
}
fn default_host() -> String {
    "127.0.0.1".to_string()
}
fn default_port() -> u16 {
    9010
}
fn default_rate() -> u32 {
    30
}
fn default_true() -> bool {
    true
}

impl Default for SignalConfig {
    fn default() -> Self {
        Self {
            version: 1,
            host: default_host(),
            port: default_port(),
            tx_rate_hz: default_rate(),
            feat_bus: false,
            stems: true,
        }
    }
}

impl SignalConfig {
    pub fn config_path() -> PathBuf {
        crate::paths::config_root().join("signal.json")
    }

    pub fn load() -> Self {
        match std::fs::read_to_string(Self::config_path()) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Not yet called anywhere: v1 has no Signal settings panel (that's Workstream
    /// E's workspace) and CLI overrides are deliberately not persisted.
    #[allow(dead_code)]
    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = SignalConfig::default();
        assert_eq!(c.version, 1);
        assert_eq!(c.host, "127.0.0.1");
        assert_eq!(c.port, 9010, "clear of the OSC 9000/9001 pair");
        assert_eq!(c.tx_rate_hz, 30);
        assert!(!c.feat_bus, "feature bus is opt-in");
        assert!(c.stems);
    }

    /// Partial/older JSON deserializes with defaults for missing fields.
    #[test]
    fn partial_json_fills_defaults() {
        let c: SignalConfig = serde_json::from_str(r#"{"port": 9999}"#).unwrap();
        assert_eq!(c.port, 9999);
        assert_eq!(c.host, "127.0.0.1");
        assert!(c.stems);
    }

    #[test]
    fn serde_roundtrip() {
        let c = SignalConfig {
            feat_bus: true,
            tx_rate_hz: 10,
            ..Default::default()
        };
        let json = serde_json::to_string(&c).unwrap();
        let c2: SignalConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.tx_rate_hz, 10);
        assert!(c2.feat_bus);
    }
}
