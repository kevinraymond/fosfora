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

/// What the alpha channel of the final composite carries — on screen, into every
/// output sink (NDI/Spout/Syphon capture reuses the same composite pass), and in
/// headless renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AlphaOutputMode {
    /// Decide per frame: Passthrough when every enabled layer is an effect tagged
    /// `alpha: true` (an overlay scene), else the NDI "Alpha from brightness"
    /// checkbox picks Luma, else Opaque. Matches pre-overlay behavior exactly for
    /// existing setups.
    #[default]
    Auto,
    /// Alpha forced to 1.0 (the historical behavior).
    Opaque,
    /// Alpha derived from output brightness — the legacy NDI luma-key look.
    Luma,
    /// The scene's real coverage alpha survives to the output (premultiplied; see
    /// docs/alpha.md).
    Passthrough,
}

impl AlphaOutputMode {
    pub const ALL: &'static [AlphaOutputMode] = &[
        AlphaOutputMode::Auto,
        AlphaOutputMode::Opaque,
        AlphaOutputMode::Luma,
        AlphaOutputMode::Passthrough,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            AlphaOutputMode::Auto => "Auto",
            AlphaOutputMode::Opaque => "Opaque",
            AlphaOutputMode::Luma => "Luma key",
            AlphaOutputMode::Passthrough => "Passthrough",
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
    /// Output alpha mode (overlay initiative). `#[serde(default)]` = Auto, which
    /// reproduces pre-overlay behavior byte-for-byte on old settings files.
    #[serde(default)]
    pub output_alpha: AlphaOutputMode,
    /// Keep the v1 two-side-panel layout instead of the v2 workspace shell
    /// (#3122). `default = "default_true"` like `auto_reconnect`, and for the
    /// same reason: a bare `#[serde(default)]` would move every existing
    /// install to the new layout on upgrade without anyone choosing it. The
    /// toggle ships for one release and goes away in v2.1.
    #[serde(default = "default_true")]
    pub classic_layout: bool,
    /// Display the second output window was last opened on, by name (#3122).
    /// A name rather than an index: displays come and go and winit reorders
    /// them, and an index would send the output to whatever took that slot.
    /// It seeds the picker only — the window is never opened on its own.
    #[serde(default)]
    pub output_display: Option<String>,
    /// Interface scale, 1.0 = 100 % (#3125): egui's zoom factor, so text,
    /// controls and spacing grow together. Clamped to 80–200 % where it is
    /// applied, so a hand-edited value can't make the interface unusable.
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f32,
    /// Guided tours finished or skipped, by key (#3126). The First run tour
    /// starts by itself until its key is here.
    #[serde(default)]
    pub tours_done: Vec<String>,
}

/// Serde default for [`SettingsConfig::ui_scale`]: `f32`'s `Default` is 0.
fn default_ui_scale() -> f32 {
    1.0
}

/// Serde default for [`SettingsConfig::auto_reconnect`] — see the note on that field.
fn default_true() -> bool {
    true
}

impl Default for SettingsConfig {
    fn default() -> Self {
        Self {
            version: 1,
            theme: ThemeMode::Gray,
            audio_device: None,
            band_scale: BandScale::default(),
            particle_quality: ParticleQuality::default(),
            webcam_device: None,
            use_ffmpeg_webcam: false,
            structure_tuning: StructureConfig::default(),
            tempo: TempoConfig::default(),
            auto_reconnect: true,
            favorite_effects: Vec::new(),
            output_alpha: AlphaOutputMode::default(),
            classic_layout: true,
            output_display: None,
            ui_scale: 1.0,
            tours_done: Vec::new(),
        }
    }
}

impl SettingsConfig {
    pub fn load() -> Self {
        let path = crate::paths::config_root().join("settings.json");
        match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let dir = crate::paths::config_root();
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
        assert_eq!(c.theme, ThemeMode::Gray);
        assert!(c.audio_device.is_none());
    }

    #[test]
    fn settings_config_serde_roundtrip() {
        let c = SettingsConfig::default();
        let json = serde_json::to_string(&c).unwrap();
        let c2: SettingsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.version, 1);
        assert_eq!(c2.theme, ThemeMode::Gray);
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
        let custom = ThemeMode::Custom("night-shift".into());
        for mode in ThemeMode::BUILT_IN.into_iter().chain([custom]) {
            let c = SettingsConfig {
                theme: mode.clone(),
                ..Default::default()
            };
            let json = serde_json::to_string(&c).unwrap();
            let c2: SettingsConfig = serde_json::from_str(&json).unwrap();
            assert_eq!(c2.theme, mode);
        }
    }

    #[test]
    fn settings_config_non_default_theme_persists() {
        let c = SettingsConfig {
            theme: ThemeMode::BlueOrange,
            ..Default::default()
        };
        let json = serde_json::to_string(&c).unwrap();
        let c2: SettingsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.theme, ThemeMode::BlueOrange);
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
    fn ui_scale_defaults_to_full_size_when_missing() {
        // A bare #[serde(default)] would load 0 %, and egui draws nothing at zoom 0.
        let json = r#"{"version":1,"theme":"Dark"}"#;
        let c: SettingsConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.ui_scale, 1.0);
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
    fn settings_with_a_retired_theme_keep_everything_else() {
        // settings.json is parsed whole: an unknown theme name would reset every
        // setting in it, so every name any release wrote still loads (#3125).
        for (old, now) in [
            ("Dark", ThemeMode::Gray),
            ("Midnight", ThemeMode::BlueOrange),
            ("Deuteranopia", ThemeMode::BlueOrange),
            ("Protanopia", ThemeMode::BlueOrange),
            ("Tritanopia", ThemeMode::Gray),
        ] {
            let json = format!(r#"{{"version":1,"theme":"{old}","auto_reconnect":false}}"#);
            let c: SettingsConfig = serde_json::from_str(&json).expect(old);
            assert_eq!(c.theme, now, "{old}");
            assert!(!c.auto_reconnect, "{old} reset the other settings");
        }
    }
}
