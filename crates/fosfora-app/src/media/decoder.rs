use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32};

use super::types::DecodedFrame;

/// How far a decode has got, shared with the thread doing it, so the UI can
/// say so: a video pre-decodes every frame and can take many seconds.
#[derive(Default)]
pub struct MediaProgress {
    /// Frames decoded so far.
    pub done: AtomicU32,
    /// Frames the probe expects; 0 until probed, and for an image.
    pub total: AtomicU32,
    /// Set to stop the decode early; it then returns an error.
    pub cancel: AtomicBool,
}

/// Decoded media source: either a static image or animated frames.
/// Video files are pre-decoded to Animated (same as GIF), enabling instant random access.
pub enum MediaSource {
    /// Single static image.
    Static(DecodedFrame),
    /// Animated image/video: pre-decoded frames + frame delays in milliseconds.
    Animated {
        frames: Vec<DecodedFrame>,
        delays_ms: Vec<u32>,
        /// True if this was decoded from a video file (affects UI: show time, hide direction).
        #[cfg(feature = "video")]
        from_video: bool,
    },
    /// Live webcam feed — frames arrive from capture thread, not stored here.
    #[cfg(feature = "webcam")]
    Live { width: u32, height: u32 },
}

impl MediaSource {
    pub fn frame_count(&self) -> usize {
        match self {
            MediaSource::Static(_) => 1,
            MediaSource::Animated { frames, .. } => frames.len(),
            #[cfg(feature = "webcam")]
            MediaSource::Live { .. } => 1,
        }
    }

    pub fn is_animated(&self) -> bool {
        matches!(self, MediaSource::Animated { .. })
    }

    pub fn is_video(&self) -> bool {
        #[cfg(feature = "video")]
        if let MediaSource::Animated { from_video, .. } = self {
            return *from_video;
        }
        false
    }

    pub fn is_live(&self) -> bool {
        #[cfg(feature = "webcam")]
        if let MediaSource::Live { .. } = self {
            return true;
        }
        false
    }

    /// Get frame dimensions.
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            MediaSource::Static(f) => (f.width, f.height),
            MediaSource::Animated { frames, .. } => {
                frames.first().map_or((1, 1), |f| (f.width, f.height))
            }
            #[cfg(feature = "webcam")]
            MediaSource::Live { width, height } => (*width, *height),
        }
    }
}

/// Video file extensions.
#[cfg(feature = "video")]
pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "avi", "mkv", "webm", "m4v", "flv"];

/// Load an image or animation from a file path.
pub fn load_media(path: &Path) -> Result<MediaSource, String> {
    load_media_with(path, &MediaProgress::default())
}

/// [`load_media`], reporting progress and honoring a cancel as it goes.
pub fn load_media_with(path: &Path, progress: &MediaProgress) -> Result<MediaSource, String> {
    #[cfg(not(feature = "video"))]
    let _ = progress;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    #[cfg(feature = "video")]
    if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        return load_video(path, progress);
    }

    match ext.as_str() {
        "gif" => load_gif(path),
        "webp" => load_webp(path),
        _ => load_static_image(path),
    }
}

/// Load a video file by pre-decoding all frames via ffmpeg.
#[cfg(feature = "video")]
fn load_video(path: &Path, progress: &MediaProgress) -> Result<MediaSource, String> {
    use super::video::{MAX_PREDECODE_SECS, decode_all_frames, ffmpeg_available, probe_video};

    if !ffmpeg_available() {
        return Err("ffmpeg/ffprobe not found on PATH".to_string());
    }

    let meta = probe_video(path)?;
    log::info!(
        "Video probe: {}x{}, {:.2} fps, {:.1}s",
        meta.width,
        meta.height,
        meta.fps,
        meta.duration_secs,
    );

    if meta.duration_secs > MAX_PREDECODE_SECS {
        return Err(format!(
            "Video too long for pre-decode ({:.0}s > {:.0}s max). \
             Use a shorter clip or trim with ffmpeg.",
            meta.duration_secs, MAX_PREDECODE_SECS,
        ));
    }

    let (frames, delays_ms) = decode_all_frames(path, &meta, progress)?;
    Ok(MediaSource::Animated {
        frames,
        delays_ms,
        from_video: true,
    })
}

/// Load a static image (PNG, JPEG, etc.) via the `image` crate.
fn load_static_image(path: &Path) -> Result<MediaSource, String> {
    let img = image::open(path).map_err(|e| format!("Failed to open image: {e}"))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();

    Ok(MediaSource::Static(DecodedFrame {
        data: rgba.into_raw(),
        width: w,
        height: h,
    }))
}

/// Load an animated GIF, pre-decoding all frames.
fn load_gif(path: &Path) -> Result<MediaSource, String> {
    use std::fs::File;

    let file = File::open(path).map_err(|e| format!("Failed to open GIF: {e}"))?;
    let mut decoder = gif::DecodeOptions::new();
    decoder.set_color_output(gif::ColorOutput::RGBA);
    let mut reader = decoder
        .read_info(file)
        .map_err(|e| format!("Failed to decode GIF: {e}"))?;

    let width = reader.width() as u32;
    let height = reader.height() as u32;

    let mut frames = Vec::new();
    let mut delays_ms = Vec::new();

    // Accumulator for compositing (GIF frames can be partial updates)
    let mut canvas = vec![0u8; (width * height * 4) as usize];

    while let Some(frame) = reader
        .read_next_frame()
        .map_err(|e| format!("GIF frame error: {e}"))?
    {
        let delay = frame.delay as u32 * 10; // GIF delay is in centiseconds
        delays_ms.push(delay.max(20)); // minimum 20ms to prevent zero-delay

        // Composite frame onto canvas at the correct offset
        let fx = frame.left as u32;
        let fy = frame.top as u32;
        let fw = frame.width as u32;
        let fh = frame.height as u32;

        for y in 0..fh {
            for x in 0..fw {
                let src_idx = ((y * fw + x) * 4) as usize;
                let dst_x = fx + x;
                let dst_y = fy + y;
                if dst_x < width && dst_y < height {
                    let dst_idx = ((dst_y * width + dst_x) * 4) as usize;
                    let src = &frame.buffer[src_idx..src_idx + 4];
                    // Only overwrite if source pixel is not fully transparent
                    if src[3] > 0 {
                        canvas[dst_idx..dst_idx + 4].copy_from_slice(src);
                    }
                }
            }
        }

        frames.push(DecodedFrame {
            data: canvas.clone(),
            width,
            height,
        });
    }

    if frames.is_empty() {
        return Err("GIF has no frames".to_string());
    }

    log::info!("Loaded GIF: {}x{}, {} frames", width, height, frames.len());

    Ok(MediaSource::Animated {
        frames,
        delays_ms,
        #[cfg(feature = "video")]
        from_video: false,
    })
}

/// Load a WebP image, detecting animation automatically.
fn load_webp(path: &Path) -> Result<MediaSource, String> {
    use image_webp::WebPDecoder;
    use std::fs::File;
    use std::io::BufReader;

    let file = File::open(path).map_err(|e| format!("Failed to open WebP: {e}"))?;
    let mut decoder = WebPDecoder::new(BufReader::new(file))
        .map_err(|e| format!("Failed to decode WebP: {e}"))?;

    let (width, height) = decoder.dimensions();

    if !decoder.is_animated() {
        // Static WebP — decode single frame
        let buf_size = decoder
            .output_buffer_size()
            .ok_or("Cannot determine WebP buffer size")?;
        let mut buf = vec![0u8; buf_size];
        decoder
            .read_image(&mut buf)
            .map_err(|e| format!("Failed to read WebP image: {e}"))?;

        // Ensure RGBA (WebP without alpha returns RGB)
        let data = if decoder.has_alpha() {
            buf
        } else {
            rgb_to_rgba(&buf)
        };

        return Ok(MediaSource::Static(DecodedFrame {
            data,
            width,
            height,
        }));
    }

    // Animated WebP — decode all frames
    let num_frames = decoder.num_frames() as usize;
    let mut frames = Vec::with_capacity(num_frames);
    let mut delays_ms = Vec::with_capacity(num_frames);
    let buf_size = decoder
        .output_buffer_size()
        .ok_or("Cannot determine WebP buffer size")?;

    loop {
        let mut buf = vec![0u8; buf_size];
        match decoder.read_frame(&mut buf) {
            Ok(duration_ms) => {
                delays_ms.push(duration_ms.max(20)); // minimum 20ms

                let data = if decoder.has_alpha() {
                    buf
                } else {
                    rgb_to_rgba(&buf)
                };

                frames.push(DecodedFrame {
                    data,
                    width,
                    height,
                });
            }
            Err(_) => break, // NoMoreFrames
        }
    }

    if frames.is_empty() {
        return Err("Animated WebP has no frames".to_string());
    }

    log::info!(
        "Loaded animated WebP: {}x{}, {} frames",
        width,
        height,
        frames.len()
    );

    Ok(MediaSource::Animated {
        frames,
        delays_ms,
        #[cfg(feature = "video")]
        from_video: false,
    })
}

/// Convert RGB buffer to RGBA (opaque).
fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let pixel_count = rgb.len() / 3;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for chunk in rgb.chunks_exact(3) {
        rgba.extend_from_slice(chunk);
        rgba.push(255);
    }
    rgba
}

#[cfg(all(test, feature = "video"))]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// A 2 s, 10 fps test clip, or None where ffmpeg is missing.
    fn clip(dir: &Path) -> Option<std::path::PathBuf> {
        if !crate::media::video::ffmpeg_available() {
            return None;
        }
        let path = dir.join("clip.mp4");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg("testsrc=size=64x36:rate=10:duration=2")
            .args(["-pix_fmt", "yuv420p"])
            .arg(&path)
            .status()
            .is_ok_and(|s| s.success());
        ok.then_some(path)
    }

    // The loading row's numbers: the probe's frame count up front, then
    // every decoded frame counted as it lands.
    #[test]
    fn a_video_decode_reports_its_progress() {
        let dir = tempfile::tempdir().unwrap();
        let Some(path) = clip(dir.path()) else {
            eprintln!("ffmpeg not found; skipped");
            return;
        };
        let progress = MediaProgress::default();
        let source = load_media_with(&path, &progress).expect("decodes");
        let total = progress.total.load(Ordering::Relaxed);
        let done = progress.done.load(Ordering::Relaxed);
        assert_eq!(done as usize, source.frame_count());
        assert!((19..=21).contains(&total), "probe expected {total} frames");
        assert!(done >= total - 1, "{done} of {total}");
    }

    // Cancel stops the decode rather than finishing it and throwing it away.
    #[test]
    fn a_cancelled_decode_stops() {
        let dir = tempfile::tempdir().unwrap();
        let Some(path) = clip(dir.path()) else {
            eprintln!("ffmpeg not found; skipped");
            return;
        };
        let progress = MediaProgress::default();
        progress.cancel.store(true, Ordering::Relaxed);
        assert!(load_media_with(&path, &progress).is_err());
        assert_eq!(progress.done.load(Ordering::Relaxed), 0);
    }
}
