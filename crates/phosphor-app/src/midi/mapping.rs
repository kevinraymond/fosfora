use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::types::{MidiMsgType, TriggerAction};

/// A single MIDI CC/Note → parameter or trigger mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidiMapping {
    pub cc: u8,
    pub channel: u8, // 0 = omni (any channel)
    pub msg_type: MidiMsgType,
    #[serde(default = "default_min")]
    pub min_val: f32,
    #[serde(default = "default_max")]
    pub max_val: f32,
    #[serde(default)]
    pub invert: bool,
}

fn default_min() -> f32 {
    0.0
}
fn default_max() -> f32 {
    1.0
}

impl MidiMapping {
    /// Create a new mapping from a learned message.
    pub fn from_learn(cc: u8, _channel: u8, msg_type: MidiMsgType) -> Self {
        Self {
            cc,
            channel: 0, // omni by default
            msg_type,
            min_val: 0.0,
            max_val: 1.0,
            invert: false,
        }
    }

    /// Scale a raw 0-127 MIDI value to the mapped range.
    pub fn scale(&self, raw: u8) -> f32 {
        let normalized = raw as f32 / 127.0;
        let normalized = if self.invert {
            1.0 - normalized
        } else {
            normalized
        };
        self.min_val + (self.max_val - self.min_val) * normalized
    }

    /// Check if this mapping matches a given CC/channel/type.
    pub fn matches(&self, number: u8, channel: u8, msg_type: MidiMsgType) -> bool {
        self.cc == number
            && self.msg_type == msg_type
            && (self.channel == 0 || self.channel == channel)
    }
}

/// Persisted MIDI configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidiConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub params: HashMap<String, MidiMapping>,
    #[serde(default)]
    pub triggers: HashMap<TriggerAction, MidiMapping>,
    #[serde(default)]
    pub port_name: Option<String>,
}

fn default_version() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

impl Default for MidiConfig {
    fn default() -> Self {
        Self {
            version: 1,
            enabled: true,
            params: HashMap::new(),
            triggers: HashMap::new(),
            port_name: None,
        }
    }
}

impl MidiConfig {
    /// Path to the MIDI config file (~/.config/phosphor/midi.json).
    pub fn config_path() -> PathBuf {
        let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        config_dir.join("phosphor").join("midi.json")
    }

    /// Load config from disk, falling back to default on any error.
    pub fn load() -> Self {
        let path = Self::config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(config) => {
                    log::info!("Loaded MIDI config from {}", path.display());
                    config
                }
                Err(e) => {
                    log::warn!("Failed to parse MIDI config: {e}");
                    Self::default()
                }
            },
            Err(_) => {
                log::info!("No MIDI config found, using defaults");
                Self::default()
            }
        }
    }

    /// Save config to disk.
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
                    log::error!("Failed to write MIDI config: {e}");
                } else {
                    log::debug!("Saved MIDI config to {}", path.display());
                }
            }
            Err(e) => log::error!("Failed to serialize MIDI config: {e}"),
        }
    }

    /// Find which param name is mapped to the given CC/channel/type.
    pub fn find_param(&self, number: u8, channel: u8, msg_type: MidiMsgType) -> Option<&str> {
        for (name, mapping) in &self.params {
            if mapping.matches(number, channel, msg_type) {
                return Some(name.as_str());
            }
        }
        None
    }

    /// Find which trigger action is mapped to the given CC/channel/type.
    pub fn find_trigger(
        &self,
        number: u8,
        channel: u8,
        msg_type: MidiMsgType,
    ) -> Option<TriggerAction> {
        for (action, mapping) in &self.triggers {
            if mapping.matches(number, channel, msg_type) {
                return Some(*action);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn from_learn_sets_omni() {
        let m = MidiMapping::from_learn(42, 5, MidiMsgType::Cc);
        assert_eq!(m.cc, 42);
        assert_eq!(m.channel, 0); // omni
        assert_eq!(m.msg_type, MidiMsgType::Cc);
    }

    #[test]
    fn scale_zero_maps_to_min() {
        let m = MidiMapping {
            cc: 1,
            channel: 0,
            msg_type: MidiMsgType::Cc,
            min_val: 0.0,
            max_val: 1.0,
            invert: false,
        };
        assert!(approx_eq(m.scale(0), 0.0, 1e-6));
    }

    #[test]
    fn scale_127_maps_to_max() {
        let m = MidiMapping {
            cc: 1,
            channel: 0,
            msg_type: MidiMsgType::Cc,
            min_val: 0.0,
            max_val: 1.0,
            invert: false,
        };
        assert!(approx_eq(m.scale(127), 1.0, 1e-6));
    }

    #[test]
    fn scale_midpoint() {
        let m = MidiMapping {
            cc: 1,
            channel: 0,
            msg_type: MidiMsgType::Cc,
            min_val: 0.0,
            max_val: 1.0,
            invert: false,
        };
        // 64/127 ≈ 0.5039
        assert!(approx_eq(m.scale(64), 64.0 / 127.0, 1e-4));
    }

    #[test]
    fn scale_custom_range() {
        let m = MidiMapping {
            cc: 1,
            channel: 0,
            msg_type: MidiMsgType::Cc,
            min_val: 100.0,
            max_val: 200.0,
            invert: false,
        };
        assert!(approx_eq(m.scale(0), 100.0, 1e-6));
        assert!(approx_eq(m.scale(127), 200.0, 1e-6));
    }

    #[test]
    fn scale_inverted() {
        let m = MidiMapping {
            cc: 1,
            channel: 0,
            msg_type: MidiMsgType::Cc,
            min_val: 0.0,
            max_val: 1.0,
            invert: true,
        };
        assert!(approx_eq(m.scale(0), 1.0, 1e-6));
        assert!(approx_eq(m.scale(127), 0.0, 1e-6));
    }

    #[test]
    fn matches_exact() {
        let m = MidiMapping {
            cc: 42,
            channel: 3,
            msg_type: MidiMsgType::Cc,
            min_val: 0.0,
            max_val: 1.0,
            invert: false,
        };
        assert!(m.matches(42, 3, MidiMsgType::Cc));
    }

    #[test]
    fn matches_omni() {
        let m = MidiMapping {
            cc: 42,
            channel: 0,
            msg_type: MidiMsgType::Cc,
            min_val: 0.0,
            max_val: 1.0,
            invert: false,
        };
        assert!(m.matches(42, 1, MidiMsgType::Cc));
        assert!(m.matches(42, 15, MidiMsgType::Cc));
    }

    #[test]
    fn matches_wrong_cc() {
        let m = MidiMapping {
            cc: 42,
            channel: 0,
            msg_type: MidiMsgType::Cc,
            min_val: 0.0,
            max_val: 1.0,
            invert: false,
        };
        assert!(!m.matches(43, 1, MidiMsgType::Cc));
    }

    #[test]
    fn matches_wrong_type() {
        let m = MidiMapping {
            cc: 42,
            channel: 0,
            msg_type: MidiMsgType::Cc,
            min_val: 0.0,
            max_val: 1.0,
            invert: false,
        };
        assert!(!m.matches(42, 1, MidiMsgType::Note));
    }

    #[test]
    fn config_find_param() {
        let mut config = MidiConfig::default();
        config.params.insert(
            "speed".into(),
            MidiMapping::from_learn(10, 1, MidiMsgType::Cc),
        );
        assert_eq!(config.find_param(10, 1, MidiMsgType::Cc), Some("speed"));
        assert_eq!(config.find_param(11, 1, MidiMsgType::Cc), None);
    }

    #[test]
    fn config_find_trigger() {
        let mut config = MidiConfig::default();
        config.triggers.insert(
            TriggerAction::NextEffect,
            MidiMapping::from_learn(20, 1, MidiMsgType::Cc),
        );
        assert_eq!(
            config.find_trigger(20, 1, MidiMsgType::Cc),
            Some(TriggerAction::NextEffect)
        );
        assert_eq!(config.find_trigger(21, 1, MidiMsgType::Cc), None);
    }

    #[test]
    fn config_serde_roundtrip() {
        let mut config = MidiConfig::default();
        config.params.insert(
            "test".into(),
            MidiMapping::from_learn(5, 0, MidiMsgType::Cc),
        );
        let json = serde_json::to_string(&config).unwrap();
        let c2: MidiConfig = serde_json::from_str(&json).unwrap();
        assert!(c2.params.contains_key("test"));
    }

    // ---- Additional tests ----

    #[test]
    fn scale_inverted_custom_range() {
        let m = MidiMapping {
            cc: 1,
            channel: 0,
            msg_type: MidiMsgType::Cc,
            min_val: 10.0,
            max_val: 20.0,
            invert: true,
        };
        assert!(approx_eq(m.scale(0), 20.0, 1e-4)); // inverted: 0 → max
        assert!(approx_eq(m.scale(127), 10.0, 1e-4)); // inverted: 127 → min
    }

    #[test]
    fn scale_zero_range() {
        let m = MidiMapping {
            cc: 1,
            channel: 0,
            msg_type: MidiMsgType::Cc,
            min_val: 5.0,
            max_val: 5.0,
            invert: false,
        };
        assert!(approx_eq(m.scale(0), 5.0, 1e-6));
        assert!(approx_eq(m.scale(127), 5.0, 1e-6));
    }

    #[test]
    fn matches_wrong_channel_non_omni() {
        let m = MidiMapping {
            cc: 42,
            channel: 3,
            msg_type: MidiMsgType::Cc,
            min_val: 0.0,
            max_val: 1.0,
            invert: false,
        };
        assert!(m.matches(42, 3, MidiMsgType::Cc));
        assert!(!m.matches(42, 5, MidiMsgType::Cc)); // non-omni, wrong channel
    }

    #[test]
    fn midi_config_defaults() {
        let c = MidiConfig::default();
        assert_eq!(c.version, 1);
        assert!(c.enabled);
        assert!(c.params.is_empty());
        assert!(c.triggers.is_empty());
        assert!(c.port_name.is_none());
    }
}
