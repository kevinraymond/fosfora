use crate::ndi::types::OutputResolution;

/// Snapshot of NDI state for UI (avoids passing &mut NdiSystem into the UI).
#[derive(Clone, Default)]
pub struct NdiInfo {
    pub enabled: bool,
    pub running: bool,
    pub ndi_available: bool,
    pub source_name: String,
    pub resolution: OutputResolution,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub output_width: u32,
    pub output_height: u32,
    pub alpha_from_luma: bool,
    pub error: Option<String>,
}
