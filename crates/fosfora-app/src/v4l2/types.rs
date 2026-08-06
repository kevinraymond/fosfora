use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// Re-export OutputResolution from shared gpu::types module.
pub use crate::gpu::types::OutputResolution;

/// Pixel format written to the loopback device.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum V4l2PixelFormat {
    /// Packed 4:2:2 YCbCr — what browsers (Chrome/Firefox getUserMedia) accept.
    #[default]
    Yuyv,
    /// 32-bit BGRX passthrough — near-free, but browsers reject RGB formats.
    Bgrx,
}

impl V4l2PixelFormat {
    pub const ALL: &'static [Self] = &[Self::Yuyv, Self::Bgrx];

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Yuyv => "YUYV (browser compatible)",
            Self::Bgrx => "BGRX (passthrough)",
        }
    }

    /// The V4L2 FourCC negotiated with the device.
    pub fn fourcc(self) -> &'static [u8; 4] {
        match self {
            Self::Yuyv => b"YUYV",
            // V4L2_PIX_FMT_BGR32: B,G,R,X memory order — matches our readback bytes.
            Self::Bgrx => b"BGR4",
        }
    }
}

/// Persisted v4l2 output configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct V4l2Config {
    #[serde(default)]
    pub enabled: bool,
    /// Loopback device to write to; `None` = first detected loopback device.
    #[serde(default)]
    pub device_path: Option<String>,
    #[serde(default)]
    pub resolution: OutputResolution,
    #[serde(default)]
    pub pixel_format: V4l2PixelFormat,
}

impl V4l2Config {
    pub fn config_path() -> PathBuf {
        crate::paths::config_root().join("v4l2.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(config) => {
                    log::info!("Loaded v4l2 config from {}", path.display());
                    config
                }
                Err(e) => {
                    log::warn!("Failed to parse v4l2 config: {e}");
                    Self::default()
                }
            },
            Err(_) => {
                log::info!("No v4l2 config found, using defaults");
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
                    log::error!("Failed to write v4l2 config: {e}");
                } else {
                    log::debug!("Saved v4l2 config to {}", path.display());
                }
            }
            Err(e) => log::error!("Failed to serialize v4l2 config: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v4l2_config_defaults() {
        let c = V4l2Config::default();
        assert!(!c.enabled);
        assert!(c.device_path.is_none());
        assert_eq!(c.resolution, OutputResolution::Match);
        assert_eq!(c.pixel_format, V4l2PixelFormat::Yuyv);
    }

    #[test]
    fn v4l2_config_serde_roundtrip() {
        let c = V4l2Config {
            enabled: true,
            device_path: Some("/dev/video10".into()),
            resolution: OutputResolution::Res1080p,
            pixel_format: V4l2PixelFormat::Bgrx,
        };
        let json = serde_json::to_string(&c).unwrap();
        let c2: V4l2Config = serde_json::from_str(&json).unwrap();
        assert!(c2.enabled);
        assert_eq!(c2.device_path.as_deref(), Some("/dev/video10"));
        assert_eq!(c2.resolution, OutputResolution::Res1080p);
        assert_eq!(c2.pixel_format, V4l2PixelFormat::Bgrx);
    }

    #[test]
    fn v4l2_config_partial_json_defaults() {
        let json = r#"{"enabled": true}"#;
        let c: V4l2Config = serde_json::from_str(json).unwrap();
        assert!(c.enabled);
        assert!(c.device_path.is_none());
        assert_eq!(c.resolution, OutputResolution::Match);
        assert_eq!(c.pixel_format, V4l2PixelFormat::Yuyv);
    }

    #[test]
    fn v4l2_pixel_format_fourccs() {
        assert_eq!(V4l2PixelFormat::Yuyv.fourcc(), b"YUYV");
        assert_eq!(V4l2PixelFormat::Bgrx.fourcc(), b"BGR4");
    }
}
