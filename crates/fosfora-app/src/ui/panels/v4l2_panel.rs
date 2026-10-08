use crate::v4l2::types::{OutputResolution, V4l2PixelFormat};

/// Snapshot of v4l2 state for UI (avoids passing &mut V4l2System into the UI).
#[derive(Clone, Default)]
pub struct V4l2Info {
    pub enabled: bool,
    pub running: bool,
    /// (path, card label) for each detected loopback device.
    pub devices: Vec<(String, String)>,
    /// Configured device; `None` = auto (first loopback).
    pub device_path: Option<String>,
    /// What auto-selection actually opened, when running.
    pub resolved_path: Option<String>,
    pub resolution: OutputResolution,
    pub pixel_format: V4l2PixelFormat,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub output_width: u32,
    pub output_height: u32,
    pub error: Option<String>,
}
