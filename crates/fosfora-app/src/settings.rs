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

/// Photosensitivity flash limiter (#108): how many flashes a second the output
/// may carry, on screen and into every output and recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FlashLimit {
    /// Standard, or Strict when the operating system asks for reduced motion (#109).
    #[default]
    Auto,
    /// At most three a second: the WCAG 2.x / ITU-R BT.1702 general-flash threshold.
    Standard,
    /// At most one a second.
    Strict,
    /// No limit, for a venue that has decided against one.
    Off,
}

impl FlashLimit {
    pub const ALL: [FlashLimit; 4] = [
        FlashLimit::Auto,
        FlashLimit::Standard,
        FlashLimit::Strict,
        FlashLimit::Off,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            FlashLimit::Auto => "Auto",
            FlashLimit::Standard => "Standard (3 a second)",
            FlashLimit::Strict => "Strict (1 a second)",
            FlashLimit::Off => "Off",
        }
    }

    /// Flashes allowed per second, 0 = no limit. `reduce_motion` is the OS
    /// accessibility setting; only Auto follows it.
    pub fn flashes_per_second(self, reduce_motion: bool) -> f32 {
        use crate::gpu::postprocess::{FLASH_BUDGET_STANDARD, FLASH_BUDGET_STRICT};
        match self {
            FlashLimit::Auto if reduce_motion => FLASH_BUDGET_STRICT,
            FlashLimit::Auto | FlashLimit::Standard => FLASH_BUDGET_STANDARD,
            FlashLimit::Strict => FLASH_BUDGET_STRICT,
            FlashLimit::Off => 0.0,
        }
    }
}

/// A network video stream offered as a camera: RTMP, or anything else
/// FFmpeg opens by URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RtmpStream {
    /// What the camera lists and presets know the stream by.
    pub name: String,
    pub url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Wait at `url` for the sender to connect, rather than connecting to a
    /// server there.
    #[serde(default)]
    pub listen: bool,
}

impl RtmpStream {
    /// Whether it is listed as a camera: switched on, named and addressed.
    pub fn is_usable(&self) -> bool {
        self.enabled && !self.name.trim().is_empty() && !self.url.trim().is_empty()
    }
}

/// The names the usable `streams` are listed under, in order. A stream
/// named like an entry of `taken` (the connected cameras) or like an earlier
/// stream is left out: a name has to mean one source.
pub fn stream_names<'a>(streams: &'a [RtmpStream], taken: &[&str]) -> Vec<&'a str> {
    let mut names: Vec<&str> = Vec::new();
    for stream in streams.iter().filter(|s| s.is_usable()) {
        let name = stream.name.as_str();
        if !taken.contains(&name) && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsConfig {
    pub version: u32,
    pub theme: ThemeMode,
    #[serde(default)]
    pub audio_device: Option<String>,
    #[serde(default)]
    pub band_scale: BandScale,
    /// Gain applied to the input before analysis, dB (#84). Recordings are not affected.
    #[serde(default)]
    pub input_trim_db: f32,
    #[serde(default)]
    pub particle_quality: ParticleQuality,
    #[serde(default)]
    pub webcam_device: Option<u32>,
    /// The default camera by name. Preferred over `webcam_device`: the OS
    /// renumbers cameras as they come and go, so a saved index goes stale.
    #[serde(default)]
    pub webcam_device_name: Option<String>,
    #[serde(default)]
    pub use_ffmpeg_webcam: bool,
    /// Network streams listed among the cameras.
    #[serde(default)]
    pub rtmp_streams: Vec<RtmpStream>,
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
    /// Photosensitivity flash limiter (#108). A settings file from before it
    /// existed loads as Auto, so the limit is on for upgrading installs too.
    #[serde(default)]
    pub flash_limit: FlashLimit,
    /// How a plain preset switch (a click, Next/Prev Preset, the web remote)
    /// changes the picture (#217). Cue switches use their own cue's
    /// transition. Cut by default, which is how every switch behaved before.
    #[serde(default)]
    pub preset_transition: crate::scene::types::TransitionType,
    /// Length of [`Self::preset_transition`] in seconds.
    #[serde(default = "default_preset_transition_secs")]
    pub preset_transition_secs: f32,
    /// Keep the outgoing preset animating through a Dissolve (preset switch
    /// or cue) instead of fading from a still of it. On by default; off for a
    /// rig that cannot render two presets at once, since both run every frame
    /// of the fade.
    #[serde(default = "default_true")]
    pub dissolve_keeps_moving: bool,
}

/// Serde default for [`SettingsConfig::preset_transition_secs`], matching a
/// new cue's transition length.
fn default_preset_transition_secs() -> f32 {
    1.0
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
            input_trim_db: 0.0,
            particle_quality: ParticleQuality::default(),
            webcam_device: None,
            webcam_device_name: None,
            use_ffmpeg_webcam: false,
            rtmp_streams: Vec::new(),
            structure_tuning: StructureConfig::default(),
            tempo: TempoConfig::default(),
            auto_reconnect: true,
            favorite_effects: Vec::new(),
            output_alpha: AlphaOutputMode::default(),
            output_display: None,
            ui_scale: 1.0,
            tours_done: Vec::new(),
            flash_limit: FlashLimit::Auto,
            preset_transition: crate::scene::types::TransitionType::Cut,
            preset_transition_secs: default_preset_transition_secs(),
            dissolve_keeps_moving: true,
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
    fn flash_limit_is_on_for_old_settings_files() {
        let c: SettingsConfig = serde_json::from_str(r#"{"version":1,"theme":"Gray"}"#).unwrap();
        assert_eq!(c.flash_limit, FlashLimit::Auto);
        assert_eq!(c.flash_limit.flashes_per_second(false), 3.0);
    }

    #[test]
    fn only_auto_follows_reduced_motion() {
        assert_eq!(FlashLimit::Auto.flashes_per_second(true), 1.0);
        assert_eq!(FlashLimit::Standard.flashes_per_second(true), 3.0);
        assert_eq!(FlashLimit::Strict.flashes_per_second(false), 1.0);
        assert_eq!(FlashLimit::Off.flashes_per_second(true), 0.0);
    }

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
    fn an_older_settings_file_keeps_cutting_between_presets() {
        // Before #217 every preset switch was a cut; a settings file from then
        // must keep that, with a sensible length ready if the user picks one.
        let json = r#"{"version":1,"theme":"Dark"}"#;
        let c: SettingsConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            c.preset_transition,
            crate::scene::types::TransitionType::Cut
        );
        assert_eq!(c.preset_transition_secs, 1.0);
        assert!(c.dissolve_keeps_moving);
    }

    #[test]
    fn a_settings_file_that_chose_classic_still_loads() {
        // v2.0.x saved `classic_layout`; the Classic layout went in v2.1.0.
        // The key is ignored and the rest of the file is kept.
        let json = r#"{"version":1,"theme":"Dark","classic_layout":true,"ui_scale":1.5}"#;
        let c: SettingsConfig = serde_json::from_str(json).unwrap();
        assert!((c.ui_scale - 1.5).abs() < f32::EPSILON);
    }

    #[test]
    fn streams_are_listed_under_names_of_their_own() {
        let stream = |name: &str, url: &str, enabled| RtmpStream {
            name: name.into(),
            url: url.into(),
            enabled,
            listen: false,
        };
        let streams = [
            stream("Stage", "rtmp://a/live/1", true),
            stream("Off", "rtmp://a/live/2", false),
            stream("FaceTime", "rtmp://a/live/3", true),
            stream("Stage", "rtmp://a/live/4", true),
            stream("No address", " ", true),
            stream("Crowd", "rtmp://a/live/5", true),
        ];
        assert_eq!(stream_names(&streams, &["FaceTime"]), ["Stage", "Crowd"]);

        // A stream saved without the later fields is on and connects out.
        let json =
            r#"{"version":1,"theme":"Dark","rtmp_streams":[{"name":"A","url":"rtmp://a/b"}]}"#;
        let c: SettingsConfig = serde_json::from_str(json).unwrap();
        assert!(c.rtmp_streams[0].enabled && !c.rtmp_streams[0].listen);
        let json = r#"{"version":1,"theme":"Dark"}"#;
        let c: SettingsConfig = serde_json::from_str(json).unwrap();
        assert!(c.rtmp_streams.is_empty());
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
