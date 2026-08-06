use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::gpu::types::OutputResolution;

/// Video codec for recording output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VideoCodec {
    #[default]
    H264,
    Hevc,
    AV1,
}

impl VideoCodec {
    pub const ALL: &[VideoCodec] = &[VideoCodec::H264, VideoCodec::Hevc, VideoCodec::AV1];

    pub fn display_name(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::Hevc => "HEVC",
            VideoCodec::AV1 => "AV1",
        }
    }

    /// Hardware encoder name (NVENC).
    pub fn hw_encoder(self) -> &'static str {
        match self {
            VideoCodec::H264 => "h264_nvenc",
            VideoCodec::Hevc => "hevc_nvenc",
            VideoCodec::AV1 => "av1_nvenc",
        }
    }

    /// Software fallback encoder name.
    pub fn sw_encoder(self) -> &'static str {
        match self {
            VideoCodec::H264 => "libx264",
            VideoCodec::Hevc => "libx265",
            VideoCodec::AV1 => "libsvtav1",
        }
    }
}

/// Container format for recording output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Container {
    #[default]
    Mp4,
    Mkv,
}

impl Container {
    pub const ALL: &[Container] = &[Container::Mp4, Container::Mkv];

    pub fn display_name(self) -> &'static str {
        match self {
            Container::Mp4 => "MP4",
            Container::Mkv => "MKV",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
        }
    }
}

/// Persisted recording configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingConfig {
    #[serde(default)]
    pub codec: VideoCodec,
    #[serde(default)]
    pub resolution: OutputResolution,
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default = "default_quality")]
    pub quality: u32,
    #[serde(default)]
    pub container: Container,
    #[serde(default = "default_output_dir")]
    pub output_dir: PathBuf,
    #[serde(default = "default_true")]
    pub use_hw_encoder: bool,
    #[serde(default = "default_true")]
    pub record_audio: bool,
}

fn default_fps() -> u32 {
    60
}
fn default_quality() -> u32 {
    23
}
fn default_true() -> bool {
    true
}

fn default_output_dir() -> PathBuf {
    dirs::video_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("Phosphor")
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodec::default(),
            resolution: OutputResolution::default(),
            fps: default_fps(),
            quality: default_quality(),
            container: Container::default(),
            output_dir: default_output_dir(),
            use_hw_encoder: true,
            record_audio: true,
        }
    }
}

impl RecordingConfig {
    pub fn config_path() -> PathBuf {
        crate::paths::config_root().join("recording.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(config) => {
                    log::info!("Loaded recording config from {}", path.display());
                    config
                }
                Err(e) => {
                    log::warn!("Failed to parse recording config: {e}");
                    Self::default()
                }
            },
            Err(_) => {
                log::info!("No recording config found, using defaults");
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
                    log::error!("Failed to write recording config: {e}");
                } else {
                    log::debug!("Saved recording config to {}", path.display());
                }
            }
            Err(e) => log::error!("Failed to serialize recording config: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_config_defaults() {
        let c = RecordingConfig::default();
        assert_eq!(c.codec, VideoCodec::H264);
        assert_eq!(c.fps, 60);
        assert_eq!(c.quality, 23);
        assert_eq!(c.container, Container::Mp4);
        assert!(c.use_hw_encoder);
    }

    #[test]
    fn recording_config_serde_roundtrip() {
        let c = RecordingConfig::default();
        let json = serde_json::to_string(&c).unwrap();
        let c2: RecordingConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.codec, c.codec);
        assert_eq!(c2.fps, c.fps);
        assert_eq!(c2.quality, c.quality);
    }

    #[test]
    fn video_codec_display() {
        assert_eq!(VideoCodec::H264.display_name(), "H.264");
        assert_eq!(VideoCodec::Hevc.display_name(), "HEVC");
        assert_eq!(VideoCodec::AV1.display_name(), "AV1");
    }

    #[test]
    fn container_extensions() {
        assert_eq!(Container::Mp4.extension(), "mp4");
        assert_eq!(Container::Mkv.extension(), "mkv");
    }
}
