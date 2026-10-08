use crate::spout::types::OutputResolution;

/// Snapshot of Spout state for UI (avoids passing &mut SpoutSystem into the UI).
#[derive(Clone, Default)]
pub struct SpoutInfo {
    pub enabled: bool,
    pub running: bool,
    pub sender_name: String,
    pub resolution: OutputResolution,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub output_width: u32,
    pub output_height: u32,
    pub error: Option<String>,
}
