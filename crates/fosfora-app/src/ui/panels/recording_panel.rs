use crate::recording::encoder::EncoderInfo;
use crate::recording::types::RecordingConfig;

/// Snapshot of recording state for UI (avoids passing &mut RecordingSystem into the UI).
#[derive(Clone)]
pub struct RecordingInfo {
    pub recording: bool,
    pub has_audio: bool,
    pub ffmpeg_found: bool,
    pub encoder_info: EncoderInfo,
    pub config: RecordingConfig,
    pub duration_secs: f64,
    pub frames_encoded: u64,
    pub bytes_written: u64,
    pub output_width: u32,
    pub output_height: u32,
    pub encoder_name: String,
    pub error: Option<String>,
    pub audio_active: bool,
}

impl Default for RecordingInfo {
    fn default() -> Self {
        Self {
            recording: false,
            has_audio: false,
            ffmpeg_found: false,
            encoder_info: EncoderInfo::default(),
            config: RecordingConfig::default(),
            duration_secs: 0.0,
            frames_encoded: 0,
            bytes_written: 0,
            output_width: 0,
            output_height: 0,
            encoder_name: String::new(),
            error: None,
            audio_active: false,
        }
    }
}
