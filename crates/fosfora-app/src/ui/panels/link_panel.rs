use crate::link::LinkMode;

/// Snapshot of Link state for UI (avoids passing &mut LinkSystem into the UI).
#[derive(Clone, Default)]
pub struct LinkInfo {
    pub enabled: bool,
    pub mode: LinkMode,
    pub quantum: f64,
    pub start_stop_sync: bool,
    pub peers: u64,
    pub session_tempo: f64,
    /// Position on the session grid, `0..quantum` beats.
    pub quantum_phase: f64,
    pub playing: bool,
}
