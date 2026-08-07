//! Persisted Ableton Link configuration (`~/.config/fosfora/link.json`).
//!
//! Separate file per the house one-config-per-subsystem pattern. `enabled`
//! defaults to off: joining a Link session announces us on the LAN and (in
//! Lead mode) can retempo everyone else's gear — that must be a deliberate act.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Which direction tempo flows. Both at once is deliberately impossible —
/// chasing a session while committing our own detected tempo back into it is
/// a feedback loop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkMode {
    /// Chase the session: its tempo pins the beat tracker's prior (octave and
    /// centre), while beat *phase* still comes from the audio we hear.
    #[default]
    Follow,
    /// Drive the session: commit Fosfora's detected BPM to Link once it has
    /// held stable — Fosfora as a tempo bridge from a non-Link source.
    Lead,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub mode: LinkMode,
    /// Link quantum in beats (the bar length peers phase-align to).
    #[serde(default = "default_quantum")]
    pub quantum: f64,
    /// Honor/propagate Link transport start/stop (drives timeline follow).
    #[serde(default)]
    pub start_stop_sync: bool,
}

fn default_version() -> u32 {
    1
}
fn default_quantum() -> f64 {
    4.0
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self {
            version: 1,
            enabled: false,
            mode: LinkMode::Follow,
            quantum: default_quantum(),
            start_stop_sync: false,
        }
    }
}

impl LinkConfig {
    pub fn config_path() -> PathBuf {
        crate::paths::config_root().join("link.json")
    }

    pub fn load() -> Self {
        match std::fs::read_to_string(Self::config_path()) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    /// Quantum clamped to something musically sane before it reaches Link.
    pub fn quantum_clamped(&self) -> f64 {
        if self.quantum.is_finite() {
            self.quantum.clamp(1.0, 16.0)
        } else {
            default_quantum()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = LinkConfig::default();
        assert_eq!(c.version, 1);
        assert!(!c.enabled, "joining the LAN session must be deliberate");
        assert_eq!(c.mode, LinkMode::Follow);
        assert_eq!(c.quantum, 4.0);
        assert!(!c.start_stop_sync);
    }

    /// Partial/older JSON deserializes with defaults for missing fields.
    #[test]
    fn partial_json_fills_defaults() {
        let c: LinkConfig = serde_json::from_str(r#"{"enabled": true}"#).unwrap();
        assert!(c.enabled);
        assert_eq!(c.mode, LinkMode::Follow);
        assert_eq!(c.quantum, 4.0);
    }

    #[test]
    fn mode_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&LinkMode::Follow).unwrap(),
            r#""follow""#
        );
        assert_eq!(serde_json::to_string(&LinkMode::Lead).unwrap(), r#""lead""#);
    }

    #[test]
    fn serde_roundtrip() {
        let c = LinkConfig {
            enabled: true,
            mode: LinkMode::Lead,
            quantum: 8.0,
            ..Default::default()
        };
        let json = serde_json::to_string(&c).unwrap();
        let c2: LinkConfig = serde_json::from_str(&json).unwrap();
        assert!(c2.enabled);
        assert_eq!(c2.mode, LinkMode::Lead);
        assert_eq!(c2.quantum, 8.0);
    }

    #[test]
    fn quantum_clamped_rejects_nonsense() {
        let mut c = LinkConfig {
            quantum: f64::NAN,
            ..Default::default()
        };
        assert_eq!(c.quantum_clamped(), 4.0);
        c.quantum = 0.0;
        assert_eq!(c.quantum_clamped(), 1.0);
        c.quantum = 64.0;
        assert_eq!(c.quantum_clamped(), 16.0);
    }
}
