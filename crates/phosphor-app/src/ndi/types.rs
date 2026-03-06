use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Output resolution for NDI capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputResolution {
    Match,
    Res720p,
    Res1080p,
    Res4K,
}

impl OutputResolution {
    pub const ALL: &[OutputResolution] = &[
        OutputResolution::Match,
        OutputResolution::Res720p,
        OutputResolution::Res1080p,
        OutputResolution::Res4K,
    ];

    pub fn dimensions(self, window_w: u32, window_h: u32) -> (u32, u32) {
        match self {
            OutputResolution::Match => (window_w, window_h),
            OutputResolution::Res720p => (1280, 720),
            OutputResolution::Res1080p => (1920, 1080),
            OutputResolution::Res4K => (3840, 2160),
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            OutputResolution::Match => "Match Window",
            OutputResolution::Res720p => "720p",
            OutputResolution::Res1080p => "1080p",
            OutputResolution::Res4K => "4K",
        }
    }
}

impl Default for OutputResolution {
    fn default() -> Self {
        OutputResolution::Match
    }
}

/// Persisted NDI output configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NdiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_source_name")]
    pub source_name: String,
    #[serde(default)]
    pub resolution: OutputResolution,
    #[serde(default)]
    pub alpha_from_luma: bool,
}

fn default_source_name() -> String {
    "Phosphor".to_string()
}

impl Default for NdiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            source_name: default_source_name(),
            resolution: OutputResolution::default(),
            alpha_from_luma: false,
        }
    }
}

impl NdiConfig {
    pub fn config_path() -> PathBuf {
        let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        config_dir.join("phosphor").join("ndi.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(config) => {
                    log::info!("Loaded NDI config from {}", path.display());
                    config
                }
                Err(e) => {
                    log::warn!("Failed to parse NDI config: {e}");
                    Self::default()
                }
            },
            Err(_) => {
                log::info!("No NDI config found, using defaults");
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
                    log::error!("Failed to write NDI config: {e}");
                } else {
                    log::debug!("Saved NDI config to {}", path.display());
                }
            }
            Err(e) => log::error!("Failed to serialize NDI config: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_resolution_match_passes_through() {
        assert_eq!(OutputResolution::Match.dimensions(800, 600), (800, 600));
        assert_eq!(OutputResolution::Match.dimensions(1920, 1080), (1920, 1080));
    }

    #[test]
    fn output_resolution_fixed() {
        assert_eq!(OutputResolution::Res720p.dimensions(800, 600), (1280, 720));
        assert_eq!(
            OutputResolution::Res1080p.dimensions(800, 600),
            (1920, 1080)
        );
        assert_eq!(OutputResolution::Res4K.dimensions(800, 600), (3840, 2160));
    }

    #[test]
    fn output_resolution_display_names() {
        for r in OutputResolution::ALL {
            assert!(!r.display_name().is_empty());
        }
    }

    #[test]
    fn ndi_config_defaults() {
        let c = NdiConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.source_name, "Phosphor");
        assert_eq!(c.resolution, OutputResolution::Match);
    }

    #[test]
    fn ndi_config_serde_roundtrip() {
        let c = NdiConfig::default();
        let json = serde_json::to_string(&c).unwrap();
        let c2: NdiConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.source_name, "Phosphor");
        assert!(!c2.enabled);
        assert!(!c2.alpha_from_luma);
    }

    #[test]
    fn ndi_config_alpha_from_luma_roundtrip() {
        let mut c = NdiConfig::default();
        c.alpha_from_luma = true;
        let json = serde_json::to_string(&c).unwrap();
        let c2: NdiConfig = serde_json::from_str(&json).unwrap();
        assert!(c2.alpha_from_luma);
    }

    // ---- Additional tests ----

    #[test]
    fn output_resolution_all_count() {
        assert_eq!(OutputResolution::ALL.len(), 4);
    }

    #[test]
    fn output_resolution_default_is_match() {
        assert_eq!(OutputResolution::default(), OutputResolution::Match);
    }

    #[test]
    fn output_resolution_serde_roundtrip() {
        for r in OutputResolution::ALL {
            let json = serde_json::to_string(r).unwrap();
            let r2: OutputResolution = serde_json::from_str(&json).unwrap();
            assert_eq!(*r, r2);
        }
    }

    #[test]
    fn output_resolution_exact_display_names() {
        assert_eq!(OutputResolution::Match.display_name(), "Match Window");
        assert_eq!(OutputResolution::Res720p.display_name(), "720p");
        assert_eq!(OutputResolution::Res1080p.display_name(), "1080p");
        assert_eq!(OutputResolution::Res4K.display_name(), "4K");
    }

    #[test]
    fn ndi_config_partial_json_defaults() {
        let json = r#"{"source_name": "Custom"}"#;
        let c: NdiConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.source_name, "Custom");
        assert!(!c.enabled); // default false
        assert_eq!(c.resolution, OutputResolution::Match); // default
    }
}
