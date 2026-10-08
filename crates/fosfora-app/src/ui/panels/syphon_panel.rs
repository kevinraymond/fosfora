use crate::syphon::types::OutputResolution;

/// Snapshot of Syphon state for UI (avoids passing &mut SyphonSystem into the UI).
#[derive(Clone, Default)]
pub struct SyphonInfo {
    pub available: bool,
    pub enabled: bool,
    pub running: bool,
    pub server_name: String,
    pub resolution: OutputResolution,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub output_width: u32,
    pub output_height: u32,
    pub error: Option<String>,
}
