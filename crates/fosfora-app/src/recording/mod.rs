pub mod encoder;
pub mod types;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::Sender;
use wgpu::{CommandEncoder, Device, TextureFormat};

use crate::gpu::frame_capture::FrameCapture;
use crate::gpu::postprocess::PostProcessChain;
use crate::gpu::render_target::RenderTarget;

use self::encoder::{AudioSource, EncoderInfo, VideoFrame, probe_encoders};
use self::types::RecordingConfig;

/// Recording state.
#[derive(Debug, Clone)]
pub enum RecordingState {
    Idle,
    Recording {
        start_time: Instant,
        #[allow(dead_code)]
        path: PathBuf,
        encoder_name: String,
        has_audio: bool,
    },
    Error(String),
}

/// Central recording system: owns capture target, encoder thread, config.
pub struct RecordingSystem {
    pub config: RecordingConfig,
    pub encoder_info: EncoderInfo,
    pub state: RecordingState,
    capture: Option<FrameCapture>,
    frame_tx: Option<Sender<VideoFrame>>,
    shutdown: Option<Arc<AtomicBool>>,
    encoder_handle: Option<JoinHandle<Option<String>>>,
    audio_handle: Option<JoinHandle<()>>,
    audio_fifo_path: Option<PathBuf>,
    pub frame_counter: Arc<AtomicU64>,
    pub bytes_written: Arc<AtomicU64>,
    output_width: u32,
    output_height: u32,
}

impl RecordingSystem {
    pub fn new() -> Self {
        let config = RecordingConfig::load();
        let encoder_info = probe_encoders();

        Self {
            config,
            encoder_info,
            state: RecordingState::Idle,
            capture: None,
            frame_tx: None,
            shutdown: None,
            encoder_handle: None,
            audio_handle: None,
            audio_fifo_path: None,
            frame_counter: Arc::new(AtomicU64::new(0)),
            bytes_written: Arc::new(AtomicU64::new(0)),
            output_width: 0,
            output_height: 0,
        }
    }

    /// Start recording: create capture target, probe encoder, spawn ffmpeg.
    /// If `audio` is provided and config.record_audio is true, audio will be muxed in.
    pub fn start(
        &mut self,
        device: &Device,
        format: TextureFormat,
        window_w: u32,
        window_h: u32,
        audio: Option<AudioSource>,
    ) -> Result<PathBuf, String> {
        self.stop();

        if !self.encoder_info.ffmpeg_found {
            let err = "ffmpeg not found on PATH".to_string();
            self.state = RecordingState::Error(err.clone());
            return Err(err);
        }

        let encoder_name = match self
            .encoder_info
            .pick_encoder(self.config.codec, self.config.use_hw_encoder)
        {
            Some(e) => e,
            None => {
                let err = format!(
                    "No encoder available for {}",
                    self.config.codec.display_name()
                );
                self.state = RecordingState::Error(err.clone());
                return Err(err);
            }
        };

        let (w, h) = self.config.resolution.dimensions(window_w, window_h);
        self.output_width = w;
        self.output_height = h;

        // Create capture target
        self.capture = Some(FrameCapture::new(device, w, h, format, "recording-capture"));

        let output_path = encoder::build_output_path(&self.config);

        // Set up audio FIFO if audio is requested
        let audio_fifo_info = if self.config.record_audio {
            if let Some(ref audio_src) = audio {
                match encoder::create_audio_fifo() {
                    Ok(fifo_path) => {
                        log::info!(
                            "Audio FIFO created: {} ({}Hz)",
                            fifo_path.display(),
                            audio_src.sample_rate
                        );
                        Some((fifo_path, audio_src.sample_rate))
                    }
                    Err(e) => {
                        log::warn!("Failed to create audio FIFO, recording without audio: {e}");
                        None
                    }
                }
            } else {
                log::info!("No audio source available, recording without audio");
                None
            }
        } else {
            None
        };

        // Spawn ffmpeg with optional audio input
        let ffmpeg_audio = audio_fifo_info
            .as_ref()
            .map(|(path, sr)| (path.as_path(), *sr));
        let child =
            encoder::spawn_ffmpeg(encoder_name, &self.config, w, h, &output_path, ffmpeg_audio)?;

        // Spawn encoder thread
        let (tx, rx) = crossbeam_channel::bounded(2);
        let shutdown = Arc::new(AtomicBool::new(false));
        self.frame_counter.store(0, Ordering::Relaxed);
        self.bytes_written.store(0, Ordering::Relaxed);

        let handle = encoder::spawn_encoder_thread(
            child,
            rx,
            shutdown.clone(),
            self.frame_counter.clone(),
            self.bytes_written.clone(),
        );

        // Spawn audio writer thread if we have a FIFO
        let has_audio = if let (Some((fifo_path, _)), Some(audio_src)) = (&audio_fifo_info, audio) {
            let audio_handle = encoder::spawn_audio_writer_thread(
                fifo_path.clone(),
                audio_src.ring,
                shutdown.clone(),
            );
            self.audio_handle = Some(audio_handle);
            self.audio_fifo_path = Some(fifo_path.clone());
            true
        } else {
            false
        };

        self.frame_tx = Some(tx);
        self.shutdown = Some(shutdown);
        self.encoder_handle = Some(handle);
        self.state = RecordingState::Recording {
            start_time: Instant::now(),
            path: output_path.clone(),
            encoder_name: encoder_name.to_string(),
            has_audio,
        };

        let audio_str = if has_audio { " +audio" } else { "" };
        log::info!(
            "Recording started: {}x{}{} → {}",
            w,
            h,
            audio_str,
            output_path.display()
        );
        Ok(output_path)
    }

    /// Stop recording: close ffmpeg stdin, wait for threads to finish.
    pub fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            shutdown.store(true, Ordering::Relaxed);
        }
        // Drop the channel sender so ffmpeg stdin closes.
        self.frame_tx = None;

        // Wait for audio writer thread to finish (it will get BrokenPipe when ffmpeg closes)
        if let Some(handle) = self.audio_handle.take() {
            let _ = handle.join();
        }

        // Clean up FIFO
        if let Some(ref path) = self.audio_fifo_path.take() {
            let _ = std::fs::remove_file(path);
        }

        if let Some(handle) = self.encoder_handle.take() {
            match handle.join() {
                Ok(Some(err)) => {
                    log::error!("Recording encoder error: {err}");
                    self.state = RecordingState::Error(err);
                    return;
                }
                Ok(None) => {}
                Err(_) => {
                    log::error!("Recording encoder thread panicked");
                }
            }
        }
        self.capture = None;
        if matches!(self.state, RecordingState::Recording { .. }) {
            log::info!("Recording stopped");
        }
        self.state = RecordingState::Idle;
    }

    pub fn is_recording(&self) -> bool {
        matches!(self.state, RecordingState::Recording { .. })
    }

    /// Recording duration (if recording).
    pub fn duration(&self) -> Option<std::time::Duration> {
        match &self.state {
            RecordingState::Recording { start_time, .. } => Some(start_time.elapsed()),
            _ => None,
        }
    }

    /// Current output file path (if recording).
    #[allow(dead_code)]
    pub fn output_path(&self) -> Option<&PathBuf> {
        match &self.state {
            RecordingState::Recording { path, .. } => Some(path),
            _ => None,
        }
    }

    /// Whether current recording includes audio.
    pub fn has_audio(&self) -> bool {
        matches!(
            self.state,
            RecordingState::Recording {
                has_audio: true,
                ..
            }
        )
    }

    /// Capture output dimensions.
    pub fn capture_dimensions(&self) -> (u32, u32) {
        (self.output_width, self.output_height)
    }

    /// Run the recording capture pipeline (same pattern as NDI).
    pub fn capture_frame(
        &mut self,
        device: &Device,
        encoder: &mut CommandEncoder,
        post_process: &PostProcessChain,
        source: &RenderTarget,
    ) {
        let capture = match self.capture.as_mut() {
            Some(c) => c,
            None => return,
        };

        // Read previous frame's staging data (1-frame latency).
        let prev_data = capture.take_mapped_data(device);

        // If previous map still outstanding, skip this frame.
        if capture.is_map_pending() {
            return;
        }

        // Render composite to capture texture.
        post_process.render_composite_to(device, encoder, source, &capture.view);

        // Copy to staging.
        capture.copy_to_staging(encoder);

        // Send previous frame data to encoder thread.
        if let (Some(data), Some(tx)) = (prev_data, &self.frame_tx) {
            let frame = VideoFrame {
                data,
                width: capture.width,
                height: capture.height,
            };
            let _ = tx.try_send(frame);
        }
    }

    /// Called after queue.submit() — request async map on the staging buffer.
    pub fn post_submit(&mut self) {
        if let Some(ref mut capture) = self.capture {
            capture.request_map();
        }
    }

    pub fn frames_encoded(&self) -> u64 {
        self.frame_counter.load(Ordering::Relaxed)
    }

    pub fn total_bytes_written(&self) -> u64 {
        self.bytes_written.load(Ordering::Relaxed)
    }
}

impl Drop for RecordingSystem {
    fn drop(&mut self) {
        self.stop();
    }
}
