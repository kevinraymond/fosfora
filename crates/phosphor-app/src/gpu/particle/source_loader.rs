use std::path::PathBuf;
use std::sync::OnceLock;
use std::thread;

use crossbeam_channel::{Receiver, TryRecvError, bounded};

use crate::media::types::DecodedFrame;

/// Discover built-in raster_*.png images in the assets/images/ directory.
/// Returns display names (e.g. "skull", "phoenix") sorted alphabetically.
/// Cached after first call.
pub fn builtin_raster_images() -> &'static Vec<String> {
    static IMAGES: OnceLock<Vec<String>> = OnceLock::new();
    IMAGES.get_or_init(|| {
        let images_dir = crate::effect::loader::assets_dir().join("images");
        let mut names = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&images_dir) {
            for entry in entries.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name.starts_with("raster_") && file_name.ends_with(".png") {
                    let display = file_name
                        .strip_prefix("raster_")
                        .unwrap_or(&file_name)
                        .strip_suffix(".png")
                        .unwrap_or(&file_name)
                        .to_string();
                    names.push(display);
                }
            }
        }
        names.sort();
        names
    })
}

/// Get the full path for a built-in raster image by display name.
pub fn builtin_raster_path(display_name: &str) -> PathBuf {
    crate::effect::loader::assets_dir()
        .join("images")
        .join(format!("raster_{display_name}.png"))
}

/// Request to load a particle source in the background.
pub enum ParticleSourceRequest {
    /// Load a static image (PNG/JPEG/WebP).
    Image(PathBuf),
    /// Load an animated GIF/WebP — becomes video-like with per-frame updates.
    AnimatedImage(PathBuf),
    /// Load a video file (feature-gated).
    #[cfg(feature = "video")]
    Video(PathBuf),
}

/// Result from background particle source loading.
pub enum ParticleSourceResult {
    /// Static image loaded successfully.
    Image {
        path: String,
        data: Vec<u8>,
        width: u32,
        height: u32,
    },
    /// Animated source (GIF or video) loaded successfully.
    Animated {
        path: String,
        frames: Vec<DecodedFrame>,
        delays_ms: Vec<u32>,
    },
    /// Loading failed.
    Error(String),
}

/// Manages background loading of particle image/video sources.
/// Designed for single in-flight load at a time (new request cancels previous via generation).
pub struct ParticleSourceLoader {
    result_rx: Receiver<(u64, ParticleSourceResult)>,
    generation: u64,
    pub loading: bool,
    pub loading_name: String,
}

impl ParticleSourceLoader {
    pub fn new() -> Self {
        // Create a dummy channel (no thread yet — threads are spawned per-request)
        let (_tx, rx) = bounded(1);
        Self {
            result_rx: rx,
            generation: 0,
            loading: false,
            loading_name: String::new(),
        }
    }

    /// Start loading an image file in the background.
    pub fn load_image(&mut self, path: PathBuf) {
        self.generation += 1;
        let load_gen = self.generation;
        self.loading = true;
        self.loading_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let (tx, rx) = bounded(1);
        self.result_rx = rx;

        thread::Builder::new()
            .name("particle-source-loader".into())
            .spawn(move || {
                let result = load_image_sync(&path);
                let _ = tx.send((load_gen, result));
            })
            .expect("failed to spawn particle source loader thread");
    }

    /// Start loading a video file in the background.
    #[cfg(feature = "video")]
    pub fn load_video(&mut self, path: PathBuf) {
        self.generation += 1;
        let load_gen = self.generation;
        self.loading = true;
        self.loading_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let (tx, rx) = bounded(1);
        self.result_rx = rx;

        thread::Builder::new()
            .name("particle-source-loader".into())
            .spawn(move || {
                let result = load_video_sync(&path);
                let _ = tx.send((load_gen, result));
            })
            .expect("failed to spawn particle source loader thread");
    }

    /// Open a file dialog for images on a background thread, then decode.
    /// The dialog + decode both run off the main thread to avoid freezing.
    pub fn open_image_dialog(&mut self) {
        self.generation += 1;
        let load_gen = self.generation;
        self.loading = true;
        self.loading_name = "choosing file...".to_string();

        let (tx, rx) = bounded(1);
        self.result_rx = rx;

        thread::Builder::new()
            .name("particle-source-dialog".into())
            .spawn(move || {
                let dialog = rfd::FileDialog::new()
                    .set_title("Load Image for Particle Source")
                    .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif"]);
                if let Some(path) = dialog.pick_file() {
                    let result = load_image_sync(&path);
                    let _ = tx.send((load_gen, result));
                }
                // If dialog cancelled, tx drops → Disconnected on rx → loading resets
            })
            .expect("failed to spawn particle source dialog thread");
    }

    /// Open a file dialog for video on a background thread, then decode.
    #[cfg(feature = "video")]
    pub fn open_video_dialog(&mut self) {
        self.generation += 1;
        let load_gen = self.generation;
        self.loading = true;
        self.loading_name = "choosing file...".to_string();

        let (tx, rx) = bounded(1);
        self.result_rx = rx;

        thread::Builder::new()
            .name("particle-source-dialog".into())
            .spawn(move || {
                let mut dialog = rfd::FileDialog::new().set_title("Load Video for Particle Source");
                if crate::media::video::ffmpeg_available() {
                    dialog =
                        dialog.add_filter("Video", &["mp4", "mov", "avi", "mkv", "webm", "m4v"]);
                }
                if let Some(path) = dialog.pick_file() {
                    let result = load_video_sync(&path);
                    let _ = tx.send((load_gen, result));
                }
            })
            .expect("failed to spawn particle source dialog thread");
    }

    /// Check for completed results. Returns None if still loading or no result.
    pub fn try_recv(&mut self) -> Option<ParticleSourceResult> {
        match self.result_rx.try_recv() {
            Ok((load_gen, result)) => {
                if load_gen == self.generation {
                    self.loading = false;
                    self.loading_name.clear();
                    Some(result)
                } else {
                    None // Stale result from cancelled load
                }
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.loading = false;
                self.loading_name.clear();
                None
            }
        }
    }
}

/// Synchronous image loading (runs on background thread).
fn load_image_sync(path: &std::path::Path) -> ParticleSourceResult {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    // Check if it's an animated format
    match ext.as_str() {
        "gif" => load_gif_sync(path),
        "webp" => {
            // Try animated WebP first, fall back to static
            match load_animated_webp_sync(path) {
                Some(result) => result,
                None => load_static_image_sync(path),
            }
        }
        _ => load_static_image_sync(path),
    }
}

fn load_static_image_sync(path: &std::path::Path) -> ParticleSourceResult {
    match image::open(path) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            ParticleSourceResult::Image {
                path: path.to_string_lossy().to_string(),
                data: rgba.into_raw(),
                width: w,
                height: h,
            }
        }
        Err(e) => ParticleSourceResult::Error(format!("Failed to load image: {e}")),
    }
}

fn load_gif_sync(path: &std::path::Path) -> ParticleSourceResult {
    use std::fs::File;

    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => return ParticleSourceResult::Error(format!("Failed to open GIF: {e}")),
    };
    let mut decoder = gif::DecodeOptions::new();
    decoder.set_color_output(gif::ColorOutput::RGBA);
    let mut reader = match decoder.read_info(file) {
        Ok(r) => r,
        Err(e) => return ParticleSourceResult::Error(format!("Failed to decode GIF: {e}")),
    };

    let width = reader.width() as u32;
    let height = reader.height() as u32;
    let mut frames = Vec::new();
    let mut delays_ms = Vec::new();
    let mut canvas = vec![0u8; (width * height * 4) as usize];

    loop {
        match reader.read_next_frame() {
            Ok(Some(frame)) => {
                let delay = frame.delay as u32 * 10;
                delays_ms.push(delay.max(20));

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
            Ok(None) => break,
            Err(e) => return ParticleSourceResult::Error(format!("GIF frame error: {e}")),
        }
    }

    if frames.is_empty() {
        return ParticleSourceResult::Error("GIF has no frames".to_string());
    }

    // Single-frame GIF → treat as static
    if frames.len() == 1 {
        let frame = frames.remove(0);
        return ParticleSourceResult::Image {
            path: path.to_string_lossy().to_string(),
            data: frame.data,
            width: frame.width,
            height: frame.height,
        };
    }

    ParticleSourceResult::Animated {
        path: path.to_string_lossy().to_string(),
        frames,
        delays_ms,
    }
}

fn load_animated_webp_sync(path: &std::path::Path) -> Option<ParticleSourceResult> {
    use image_webp::WebPDecoder;
    use std::fs::File;
    use std::io::BufReader;

    let file = File::open(path).ok()?;
    let mut decoder = WebPDecoder::new(BufReader::new(file)).ok()?;

    if !decoder.is_animated() {
        return None; // Not animated, fall back to static
    }

    let (width, height) = decoder.dimensions();
    let num_frames = decoder.num_frames() as usize;
    let mut frames = Vec::with_capacity(num_frames);
    let mut delays_ms = Vec::with_capacity(num_frames);
    let buf_size = decoder.output_buffer_size()?;

    loop {
        let mut buf = vec![0u8; buf_size];
        match decoder.read_frame(&mut buf) {
            Ok(duration_ms) => {
                delays_ms.push(duration_ms.max(20));
                let data = if decoder.has_alpha() {
                    buf
                } else {
                    // Convert RGB to RGBA
                    let pixel_count = buf.len() / 3;
                    let mut rgba = Vec::with_capacity(pixel_count * 4);
                    for chunk in buf.chunks_exact(3) {
                        rgba.extend_from_slice(chunk);
                        rgba.push(255);
                    }
                    rgba
                };
                frames.push(DecodedFrame {
                    data,
                    width,
                    height,
                });
            }
            Err(_) => break,
        }
    }

    if frames.is_empty() {
        return Some(ParticleSourceResult::Error(
            "Animated WebP has no frames".to_string(),
        ));
    }

    Some(ParticleSourceResult::Animated {
        path: path.to_string_lossy().to_string(),
        frames,
        delays_ms,
    })
}

/// Synchronous video loading (runs on background thread).
#[cfg(feature = "video")]
fn load_video_sync(path: &std::path::Path) -> ParticleSourceResult {
    use crate::media::video::{
        MAX_PREDECODE_SECS, decode_all_frames, ffmpeg_available, probe_video,
    };

    if !ffmpeg_available() {
        return ParticleSourceResult::Error("ffmpeg/ffprobe not found on PATH".to_string());
    }

    let meta = match probe_video(path) {
        Ok(m) => m,
        Err(e) => return ParticleSourceResult::Error(format!("Failed to probe video: {e}")),
    };

    if meta.duration_secs > MAX_PREDECODE_SECS {
        return ParticleSourceResult::Error(format!(
            "Video too long ({:.0}s > {:.0}s max)",
            meta.duration_secs, MAX_PREDECODE_SECS,
        ));
    }

    match decode_all_frames(path, &meta) {
        Ok((frames, delays_ms)) => ParticleSourceResult::Animated {
            path: path.to_string_lossy().to_string(),
            frames,
            delays_ms,
        },
        Err(e) => ParticleSourceResult::Error(format!("Failed to decode video: {e}")),
    }
}
