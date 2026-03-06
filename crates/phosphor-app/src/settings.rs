use serde::{Deserialize, Serialize};

use crate::ui::theme::ThemeMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParticleQuality {
    Low,
    Medium,
    High,
    Ultra,
    Max,
}

impl ParticleQuality {
    pub const ALL: &[ParticleQuality] = &[
        ParticleQuality::Low,
        ParticleQuality::Medium,
        ParticleQuality::High,
        ParticleQuality::Ultra,
        ParticleQuality::Max,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Low => "Low (0.25x)",
            Self::Medium => "Medium (0.5x)",
            Self::High => "High (1x)",
            Self::Ultra => "Ultra (2x)",
            Self::Max => "Max (4x)",
        }
    }

    pub fn multiplier(self) -> f32 {
        match self {
            Self::Low => 0.25,
            Self::Medium => 0.5,
            Self::High => 1.0,
            Self::Ultra => 2.0,
            Self::Max => 4.0,
        }
    }
}

impl Default for ParticleQuality {
    fn default() -> Self {
        Self::High
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsConfig {
    pub version: u32,
    pub theme: ThemeMode,
    #[serde(default)]
    pub audio_device: Option<String>,
    #[serde(default)]
    pub particle_quality: ParticleQuality,
}

impl Default for SettingsConfig {
    fn default() -> Self {
        Self {
            version: 1,
            theme: ThemeMode::Dark,
            audio_device: None,
            particle_quality: ParticleQuality::default(),
        }
    }
}

impl SettingsConfig {
    pub fn load() -> Self {
        let Some(config_dir) = dirs::config_dir() else {
            return Self::default();
        };
        let path = config_dir.join("phosphor").join("settings.json");
        match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let Some(config_dir) = dirs::config_dir() else {
            return;
        };
        let dir = config_dir.join("phosphor");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("settings.json");
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_config_defaults() {
        let c = SettingsConfig::default();
        assert_eq!(c.version, 1);
        assert_eq!(c.theme, ThemeMode::Dark);
        assert!(c.audio_device.is_none());
    }

    #[test]
    fn settings_config_serde_roundtrip() {
        let c = SettingsConfig::default();
        let json = serde_json::to_string(&c).unwrap();
        let c2: SettingsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.version, 1);
        assert_eq!(c2.theme, ThemeMode::Dark);
    }

    #[test]
    fn settings_config_with_audio_device() {
        let mut c = SettingsConfig::default();
        c.audio_device = Some("hw:0".to_string());
        let json = serde_json::to_string(&c).unwrap();
        let c2: SettingsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.audio_device, Some("hw:0".to_string()));
    }

    // ---- Additional tests ----

    #[test]
    fn particle_quality_serde_roundtrip() {
        for &q in ParticleQuality::ALL {
            let json = serde_json::to_string(&q).unwrap();
            let q2: ParticleQuality = serde_json::from_str(&json).unwrap();
            assert_eq!(q, q2);
        }
    }

    #[test]
    fn particle_quality_default_from_missing_field() {
        let json = r#"{"version":1,"theme":"Dark"}"#;
        let c: SettingsConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.particle_quality, ParticleQuality::High);
    }

    #[test]
    fn settings_config_all_themes_roundtrip() {
        for mode in ThemeMode::ALL {
            let c = SettingsConfig {
                version: 1,
                theme: *mode,
                audio_device: None,
                particle_quality: ParticleQuality::default(),
            };
            let json = serde_json::to_string(&c).unwrap();
            let c2: SettingsConfig = serde_json::from_str(&json).unwrap();
            assert_eq!(c2.theme, *mode);
        }
    }

    #[test]
    fn settings_config_non_default_theme_persists() {
        let c = SettingsConfig {
            version: 1,
            theme: ThemeMode::HighContrast,
            audio_device: None,
            particle_quality: ParticleQuality::default(),
        };
        let json = serde_json::to_string(&c).unwrap();
        let c2: SettingsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.theme, ThemeMode::HighContrast);
    }

    #[test]
    fn settings_config_old_cvd_theme_falls_back_to_default() {
        // Users with old CVD theme names in settings.json should fall back to Dark
        let json = r#"{"version":1,"theme":"Deuteranopia"}"#;
        let c: SettingsConfig = serde_json::from_str(json).unwrap_or_default();
        assert_eq!(c.theme, ThemeMode::Dark);
    }
}
