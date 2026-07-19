use serde::{Deserialize, Serialize};

use crate::audio::{StructureConfig, TempoConfig};
use crate::ui::theme::ThemeMode;

/// How the 7 frequency bands are scaled (A1 #1452).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BandScale {
    /// All seven bands share one dB domain (−60..0) with an equal-loudness tilt, so the
    /// adaptive normalizer and detectors see a single comparable family. (Default.)
    #[default]
    Db,
    /// Pre-A1 behavior: the low four bands are linear RMS and the high three are dB(−80..0)
    /// — two families with very different dynamics. Kept for presets tuned to the old feel.
    Legacy,
}

impl BandScale {
    pub const ALL: &[BandScale] = &[BandScale::Db, BandScale::Legacy];

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Db => "Unified dB",
            Self::Legacy => "Legacy",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ParticleQuality {
    Low,
    Medium,
    #[default]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsConfig {
    pub version: u32,
    pub theme: ThemeMode,
    #[serde(default)]
    pub audio_device: Option<String>,
    #[serde(default)]
    pub band_scale: BandScale,
    #[serde(default)]
    pub particle_quality: ParticleQuality,
    #[serde(default)]
    pub webcam_device: Option<u32>,
    #[serde(default)]
    pub use_ffmpeg_webcam: bool,
    /// A18 structure-detector tuning (#1510). `#[serde(default)]` so older settings files
    /// without this key load with the built-in defaults.
    #[serde(default)]
    pub structure_tuning: StructureConfig,
    /// A7 tempo prior (#1458). `#[serde(default)]` so older settings files without this key
    /// load with the built-in defaults (the pre-A7 hardcoded 150 BPM / sigma 1.0).
    #[serde(default)]
    pub tempo: TempoConfig,
    /// A9 (#1460): reopen the capture device automatically when the watchdog confirms it died.
    ///
    /// `default = "default_true"`, not the bare `#[serde(default)]` every field above uses:
    /// `bool`'s `Default` is `false`, which would silently ship this off for every settings
    /// file written before #1460 — a default the user never chose and no test would catch.
    #[serde(default = "default_true")]
    pub auto_reconnect: bool,
    /// Effect names pinned to the FAVORITES row of the Effects browser.
    /// Names, not indices — the library re-scans and reorders; names survive it.
    #[serde(default)]
    pub favorite_effects: Vec<String>,
}

/// Serde default for [`SettingsConfig::auto_reconnect`] — see the note on that field.
fn default_true() -> bool {
    true
}

impl Default for SettingsConfig {
    fn default() -> Self {
        Self {
            version: 1,
            theme: ThemeMode::Dark,
            audio_device: None,
            band_scale: BandScale::default(),
            particle_quality: ParticleQuality::default(),
            webcam_device: None,
            use_ffmpeg_webcam: false,
            structure_tuning: StructureConfig::default(),
            tempo: TempoConfig::default(),
            auto_reconnect: true,
            favorite_effects: Vec::new(),
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
        let c = SettingsConfig {
            audio_device: Some("hw:0".to_string()),
            ..Default::default()
        };
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
    fn favorite_effects_default_from_missing_field() {
        // Settings files written before favorites existed must load empty, not error.
        let json = r#"{"version":1,"theme":"Dark"}"#;
        let c: SettingsConfig = serde_json::from_str(json).unwrap();
        assert!(c.favorite_effects.is_empty());
    }

    #[test]
    fn favorite_effects_roundtrip() {
        let c = SettingsConfig {
            favorite_effects: vec!["Beam".to_string(), "Lattice Clouds".to_string()],
            ..Default::default()
        };
        let json = serde_json::to_string(&c).unwrap();
        let c2: SettingsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.favorite_effects, c.favorite_effects);
    }

    #[test]
    fn settings_config_all_themes_roundtrip() {
        for mode in ThemeMode::ALL {
            let c = SettingsConfig {
                theme: *mode,
                ..Default::default()
            };
            let json = serde_json::to_string(&c).unwrap();
            let c2: SettingsConfig = serde_json::from_str(&json).unwrap();
            assert_eq!(c2.theme, *mode);
        }
    }

    #[test]
    fn settings_config_non_default_theme_persists() {
        let c = SettingsConfig {
            theme: ThemeMode::HighContrast,
            ..Default::default()
        };
        let json = serde_json::to_string(&c).unwrap();
        let c2: SettingsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.theme, ThemeMode::HighContrast);
    }

    #[test]
    fn structure_tuning_defaults_when_missing() {
        // Older settings.json (pre-#1510) has no structure_tuning key → built-in defaults.
        let json = r#"{"version":1,"theme":"Dark"}"#;
        let c: SettingsConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.structure_tuning, StructureConfig::default());
    }

    #[test]
    fn structure_tuning_roundtrips() {
        let mut c = SettingsConfig::default();
        c.structure_tuning.drop_loud_jump = 0.15;
        c.structure_tuning.buildup_bias = -3.0;
        let json = serde_json::to_string(&c).unwrap();
        let c2: SettingsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.structure_tuning, c.structure_tuning);
    }

    #[test]
    fn auto_reconnect_defaults_on_when_missing() {
        // A settings.json written before #1460 has no key. It must load as ON — a bare
        // #[serde(default)] would give bool::default() == false and silently disable it.
        let json = r#"{"version":1,"theme":"Dark"}"#;
        let c: SettingsConfig = serde_json::from_str(json).unwrap();
        assert!(c.auto_reconnect);
    }

    #[test]
    fn auto_reconnect_off_roundtrips() {
        let c = SettingsConfig {
            auto_reconnect: false,
            ..Default::default()
        };
        let json = serde_json::to_string(&c).unwrap();
        let c2: SettingsConfig = serde_json::from_str(&json).unwrap();
        assert!(
            !c2.auto_reconnect,
            "an explicit opt-out must survive a reload"
        );
    }

    #[test]
    fn settings_config_old_cvd_theme_falls_back_to_default() {
        // Users with old CVD theme names in settings.json should fall back to Dark
        let json = r#"{"version":1,"theme":"Deuteranopia"}"#;
        let c: SettingsConfig = serde_json::from_str(json).unwrap_or_default();
        assert_eq!(c.theme, ThemeMode::Dark);
    }
}
