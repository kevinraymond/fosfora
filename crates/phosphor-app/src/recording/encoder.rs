use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::Receiver;

use crate::audio::capture::RingBuffer;

use super::types::{Container, RecordingConfig, VideoCodec};

/// Frame data sent from the render thread to the encoder thread.
pub struct VideoFrame {
    pub data: Vec<u8>,
    #[allow(dead_code)]
    pub width: u32,
    #[allow(dead_code)]
    pub height: u32,
}

/// Audio source info for recording.
pub struct AudioSource {
    pub ring: Arc<RingBuffer>,
    pub sample_rate: u32,
}

/// Available encoder info (cached from ffmpeg probe).
#[derive(Debug, Clone, Default)]
pub struct EncoderInfo {
    pub ffmpeg_found: bool,
    pub hw_h264: bool,
    pub hw_hevc: bool,
    pub hw_av1: bool,
    pub sw_h264: bool,
    pub sw_hevc: bool,
    pub sw_av1: bool,
}

impl EncoderInfo {
    /// Whether a hardware encoder is available for the given codec.
    pub fn has_hw(&self, codec: VideoCodec) -> bool {
        match codec {
            VideoCodec::H264 => self.hw_h264,
            VideoCodec::Hevc => self.hw_hevc,
            VideoCodec::AV1 => self.hw_av1,
        }
    }

    /// Whether any encoder (hw or sw) is available for the given codec.
    pub fn has_any(&self, codec: VideoCodec) -> bool {
        match codec {
            VideoCodec::H264 => self.hw_h264 || self.sw_h264,
            VideoCodec::Hevc => self.hw_hevc || self.sw_hevc,
            VideoCodec::AV1 => self.hw_av1 || self.sw_av1,
        }
    }

    /// Pick the best encoder name for the given codec and preference.
    pub fn pick_encoder(&self, codec: VideoCodec, prefer_hw: bool) -> Option<&'static str> {
        if prefer_hw && self.has_hw(codec) {
            Some(codec.hw_encoder())
        } else {
            // Fallback to software
            let has_sw = match codec {
                VideoCodec::H264 => self.sw_h264,
                VideoCodec::Hevc => self.sw_hevc,
                VideoCodec::AV1 => self.sw_av1,
            };
            if has_sw {
                Some(codec.sw_encoder())
            } else if self.has_hw(codec) {
                // User said no HW but it's the only option
                Some(codec.hw_encoder())
            } else {
                None
            }
        }
    }

    /// Display label for a codec's encoder status.
    pub fn encoder_label(&self, codec: VideoCodec) -> &'static str {
        if self.has_hw(codec) {
            "HW"
        } else {
            let has_sw = match codec {
                VideoCodec::H264 => self.sw_h264,
                VideoCodec::Hevc => self.sw_hevc,
                VideoCodec::AV1 => self.sw_av1,
            };
            if has_sw { "SW" } else { "N/A" }
        }
    }
}

/// Probe ffmpeg for available encoders. Runs `ffmpeg -encoders` once.
pub fn probe_encoders() -> EncoderInfo {
    let output = match Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            log::warn!("ffmpeg not found: {e}");
            return EncoderInfo::default();
        }
    };

    if !output.status.success() {
        log::warn!("ffmpeg -encoders failed");
        return EncoderInfo {
            ffmpeg_found: true,
            ..Default::default()
        };
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut info = EncoderInfo {
        ffmpeg_found: true,
        ..Default::default()
    };

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.contains("h264_nvenc") {
            info.hw_h264 = true;
        }
        if trimmed.contains("hevc_nvenc") {
            info.hw_hevc = true;
        }
        if trimmed.contains("av1_nvenc") {
            info.hw_av1 = true;
        }
        if trimmed.contains("libx264") && !trimmed.contains("libx264rgb") {
            info.sw_h264 = true;
        }
        if trimmed.contains("libx265") {
            info.sw_hevc = true;
        }
        if trimmed.contains("libsvtav1") {
            info.sw_av1 = true;
        }
    }

    log::info!(
        "FFmpeg encoders: h264(hw={},sw={}) hevc(hw={},sw={}) av1(hw={},sw={})",
        info.hw_h264,
        info.sw_h264,
        info.hw_hevc,
        info.sw_hevc,
        info.hw_av1,
        info.sw_av1
    );

    info
}

/// Build the output file path for a new recording.
pub fn build_output_path(config: &RecordingConfig) -> PathBuf {
    // Ensure output directory exists
    if let Err(e) = std::fs::create_dir_all(&config.output_dir) {
        log::error!("Failed to create output dir: {e}");
    }

    // Format timestamp without chrono dependency
    let now = std::time::SystemTime::now();
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let timestamp = format_local_time(secs);
    let filename = format!("fosfora_{}.{}", timestamp, config.container.extension());
    config.output_dir.join(filename)
}

fn format_local_time(unix_secs: u64) -> String {
    // Shell out to date for reliable local time formatting (runs once per recording start)
    if let Ok(output) = std::process::Command::new("date")
        .args(["-d", &format!("@{unix_secs}"), "+%Y-%m-%d_%H-%M-%S"])
        .output()
    {
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout);
            return s.trim().to_string();
        }
    }
    // Fallback: use unix timestamp
    format!("{unix_secs}")
}

/// Create a named FIFO (pipe) at the given path. Returns the path.
/// On Linux/macOS this uses mkfifo. The FIFO is cleaned up by the caller.
pub fn create_audio_fifo() -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!("fosfora_audio_{}", std::process::id()));
    // Remove stale FIFO if it exists
    let _ = std::fs::remove_file(&path);

    #[cfg(unix)]
    {
        let status = Command::new("mkfifo")
            .arg(&path)
            .status()
            .map_err(|e| format!("mkfifo failed: {e}"))?;
        if !status.success() {
            return Err("mkfifo returned non-zero".to_string());
        }
        Ok(path)
    }

    #[cfg(not(unix))]
    {
        Err("Audio recording via FIFO not supported on this platform".to_string())
    }
}

/// Spawn the ffmpeg encoder subprocess.
pub fn spawn_ffmpeg(
    encoder_name: &str,
    config: &RecordingConfig,
    width: u32,
    height: u32,
    output_path: &Path,
    audio_fifo: Option<(&Path, u32)>,
) -> Result<Child, String> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-hide_banner"]);

    // Input 0: raw BGRA video frames from stdin
    cmd.args([
        "-f",
        "rawvideo",
        "-pix_fmt",
        "bgra",
        "-s",
        &format!("{width}x{height}"),
        "-r",
        &config.fps.to_string(),
        "-i",
        "pipe:0",
    ]);

    // Input 1: raw f32le mono audio from FIFO (if enabled)
    if let Some((fifo_path, sample_rate)) = &audio_fifo {
        cmd.args([
            "-f",
            "f32le",
            "-ar",
            &sample_rate.to_string(),
            "-ac",
            "1",
            "-i",
            &fifo_path.to_string_lossy(),
        ]);
    }

    // Video encoder + quality settings
    cmd.args(["-c:v", encoder_name]);

    if encoder_name.contains("nvenc") {
        cmd.args([
            "-preset",
            "p7",
            "-cq",
            &config.quality.to_string(),
            "-b:v",
            "0",
        ]);
    } else if encoder_name == "libx264" || encoder_name == "libx265" {
        cmd.args(["-crf", &config.quality.to_string(), "-preset", "fast"]);
    } else if encoder_name == "libsvtav1" {
        cmd.args(["-crf", &config.quality.to_string(), "-preset", "6"]);
    }

    // Output pixel format
    cmd.args(["-pix_fmt", "yuv420p"]);

    // Audio encoder (if audio input present)
    if audio_fifo.is_some() {
        cmd.args(["-c:a", "aac", "-b:a", "192k"]);
    }

    // Container-specific
    if config.container == Container::Mp4 {
        cmd.args(["-movflags", "+faststart"]);
    }

    // Shortest: stop when video stops (audio FIFO may have a slight delay)
    if audio_fifo.is_some() {
        cmd.args(["-shortest"]);
    }

    cmd.arg(output_path.as_os_str());

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let audio_info = if audio_fifo.is_some() { "+audio" } else { "" };
    log::info!(
        "Spawning ffmpeg: encoder={encoder_name} {}x{} {}fps cq={}{} → {}",
        width,
        height,
        config.fps,
        config.quality,
        audio_info,
        output_path.display()
    );

    cmd.spawn()
        .map_err(|e| format!("Failed to spawn ffmpeg: {e}"))
}

/// Spawn the encoder writer thread. Receives video frames and writes them to ffmpeg's stdin.
pub fn spawn_encoder_thread(
    mut child: Child,
    frame_rx: Receiver<VideoFrame>,
    shutdown: Arc<AtomicBool>,
    frame_counter: Arc<AtomicU64>,
    bytes_written: Arc<AtomicU64>,
) -> JoinHandle<Option<String>> {
    std::thread::Builder::new()
        .name("recording-encoder".into())
        .spawn(move || {
            let result = encoder_loop(
                &mut child,
                &frame_rx,
                &shutdown,
                &frame_counter,
                &bytes_written,
            );

            // Close stdin to signal ffmpeg to finalize
            drop(child.stdin.take());

            // Wait for ffmpeg to finish writing the file
            match child.wait() {
                Ok(status) => {
                    if !status.success() {
                        if let Some(mut stderr) = child.stderr.take() {
                            let mut buf = String::new();
                            use std::io::Read;
                            let _ = stderr.read_to_string(&mut buf);
                            if !buf.is_empty() {
                                log::error!(
                                    "ffmpeg stderr: {}",
                                    buf.chars().take(500).collect::<String>()
                                );
                            }
                        }
                        log::error!("ffmpeg exited with status: {status}");
                    } else {
                        log::info!("ffmpeg finished successfully");
                    }
                }
                Err(e) => log::error!("Failed to wait for ffmpeg: {e}"),
            }

            result
        })
        .unwrap_or_else(|e| {
            let msg = format!("Thread spawn failed: {e}");
            log::error!("Failed to spawn encoder thread: {e}");
            std::thread::Builder::new()
                .name("recording-encoder-noop".into())
                .spawn(move || Some(msg))
                .expect("failed to spawn noop thread")
        })
}

/// Spawn the audio writer thread. Drains the recording ring buffer and writes
/// raw f32le samples to the audio FIFO for ffmpeg to consume.
pub fn spawn_audio_writer_thread(
    fifo_path: PathBuf,
    audio_ring: Arc<RingBuffer>,
    shutdown: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("recording-audio".into())
        .spawn(move || {
            // The recording ring has been filling since app start; discard that
            // history so encoded audio begins at record start (keeps A/V in sync).
            audio_ring.skip_to_write_pos();

            // Open the FIFO for writing (blocks until ffmpeg opens it for reading)
            let file = match std::fs::OpenOptions::new().write(true).open(&fifo_path) {
                Ok(f) => f,
                Err(e) => {
                    log::error!("Failed to open audio FIFO: {e}");
                    return;
                }
            };
            let mut writer = std::io::BufWriter::with_capacity(16384, file);
            let mut read_buf = vec![0.0f32; 4096];

            log::info!("Audio writer thread started");

            while !shutdown.load(Ordering::Relaxed) {
                let available = audio_ring.available();
                if available == 0 {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }

                let to_read = available.min(read_buf.len());
                let read = audio_ring.read(&mut read_buf[..to_read]);
                if read == 0 {
                    continue;
                }

                // Write raw f32le bytes
                let bytes: &[u8] = bytemuck::cast_slice(&read_buf[..read]);
                if let Err(e) = writer.write_all(bytes) {
                    // Broken pipe means ffmpeg closed — normal on stop
                    if e.kind() != std::io::ErrorKind::BrokenPipe {
                        log::error!("Audio FIFO write error: {e}");
                    }
                    break;
                }
            }

            let _ = writer.flush();
            log::info!("Audio writer thread exiting");
        })
        .unwrap_or_else(|e| {
            log::error!("Failed to spawn audio writer thread: {e}");
            std::thread::Builder::new()
                .name("recording-audio-noop".into())
                .spawn(|| {})
                .expect("failed to spawn noop thread")
        })
}

fn encoder_loop(
    child: &mut Child,
    frame_rx: &Receiver<VideoFrame>,
    shutdown: &AtomicBool,
    frame_counter: &AtomicU64,
    bytes_written: &AtomicU64,
) -> Option<String> {
    let stdin = match child.stdin.as_mut() {
        Some(s) => s,
        None => return Some("ffmpeg stdin not available".to_string()),
    };

    while !shutdown.load(Ordering::Relaxed) {
        match frame_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => {
                let size = frame.data.len() as u64;
                if let Err(e) = stdin.write_all(&frame.data) {
                    return Some(format!("Write to ffmpeg stdin failed: {e}"));
                }
                frame_counter.fetch_add(1, Ordering::Relaxed);
                bytes_written.fetch_add(size, Ordering::Relaxed);
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    None
}
