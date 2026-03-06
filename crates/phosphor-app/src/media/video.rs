//! Video pre-decode via ffmpeg subprocess (feature-gated behind `video`).
//!
//! - `ffprobe` probes metadata (dimensions, fps, duration) synchronously at load time
//! - `ffmpeg -f rawvideo -pix_fmt rgba` decodes ALL frames to memory in one pass
//! - Returns `MediaSource::Animated` — identical to GIF, instant random access
//! - RAM cost: ~3.7MB per frame at 1280x720. A 30s@30fps clip = ~3.3GB.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use super::types::DecodedFrame;

/// Check if ffmpeg/ffprobe are available on the system. Cached per process.
pub fn ffmpeg_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Command::new("ffprobe")
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Video metadata from ffprobe.
#[derive(Debug, Clone)]
pub struct VideoMeta {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration_secs: f64,
}

/// Probe video metadata using ffprobe.
pub fn probe_video(path: &Path) -> Result<VideoMeta, String> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("ffprobe failed to execute: {e}"))?;

    if !output.status.success() {
        return Err("ffprobe returned non-zero exit code".to_string());
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse ffprobe JSON: {e}"))?;

    let streams = json["streams"]
        .as_array()
        .ok_or("No streams in ffprobe output")?;

    let video_stream = streams
        .iter()
        .find(|s| s["codec_type"].as_str() == Some("video"))
        .ok_or("No video stream found")?;

    let width = video_stream["width"].as_u64().ok_or("Missing width")? as u32;
    let height = video_stream["height"].as_u64().ok_or("Missing height")? as u32;

    let fps = parse_frame_rate(video_stream["r_frame_rate"].as_str().unwrap_or("30/1"));

    let duration_secs = json["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| {
            video_stream["duration"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
        })
        .unwrap_or(0.0);

    Ok(VideoMeta {
        width,
        height,
        fps,
        duration_secs,
    })
}

fn parse_frame_rate(rate: &str) -> f64 {
    if let Some((num, den)) = rate.split_once('/') {
        let n: f64 = num.parse().unwrap_or(30.0);
        let d: f64 = den.parse().unwrap_or(1.0);
        if d > 0.0 { n / d } else { 30.0 }
    } else {
        rate.parse().unwrap_or(30.0)
    }
}

/// Pre-decode all video frames via a single ffmpeg run.
/// Returns (frames, delays_ms) ready for `MediaSource::Animated`.
pub fn decode_all_frames(
    path: &Path,
    meta: &VideoMeta,
) -> Result<(Vec<DecodedFrame>, Vec<u32>), String> {
    let frame_size = (meta.width as usize) * (meta.height as usize) * 4;
    let delay_ms = (1000.0 / meta.fps).round() as u32;

    // Estimate RAM and warn
    let est_frames = (meta.duration_secs * meta.fps).ceil() as usize;
    let est_ram_mb = (est_frames * frame_size) / (1024 * 1024);
    log::info!(
        "Pre-decoding video: ~{} frames, ~{}MB RAM",
        est_frames,
        est_ram_mb,
    );

    let mut child = Command::new("ffmpeg")
        .args(["-i"])
        .arg(path)
        .args([
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-s",
            &format!("{}x{}", meta.width, meta.height),
            "-v",
            "quiet",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn ffmpeg: {e}"))?;

    let mut stdout = child.stdout.take().ok_or("ffmpeg: no stdout pipe")?;

    let mut frames = Vec::with_capacity(est_frames);
    let mut delays_ms = Vec::with_capacity(est_frames);
    let mut buf = vec![0u8; frame_size];

    loop {
        match stdout.read_exact(&mut buf) {
            Ok(()) => {
                frames.push(DecodedFrame {
                    data: buf.clone(),
                    width: meta.width,
                    height: meta.height,
                });
                delays_ms.push(delay_ms.max(1));
            }
            Err(_) => break, // EOF
        }
    }

    let _ = child.wait();

    if frames.is_empty() {
        return Err("ffmpeg decoded zero frames".to_string());
    }

    log::info!(
        "Decoded {} video frames ({}MB)",
        frames.len(),
        (frames.len() * frame_size) / (1024 * 1024),
    );

    Ok((frames, delays_ms))
}

/// Maximum video duration (seconds) we'll pre-decode. Beyond this, reject.
pub const MAX_PREDECODE_SECS: f64 = 60.0;
