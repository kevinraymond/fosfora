use std::path::PathBuf;
use std::sync::OnceLock;
use std::thread;

use crossbeam_channel::{Receiver, TryRecvError, bounded};

use crate::media::types::DecodedFrame;

/// The display name a `raster_*.png` file is listed under, or `None` if the file is not a
/// built-in raster image. Paired with [`builtin_raster_path`], which must rebuild exactly
/// this file name from the result — the picker hands the display name back to be re-loaded,
/// so the two have to agree. Kept as one function so the convention has a single definition
/// (and so the round-trip is testable without an assets directory on the CWD).
fn builtin_display_name(file_name: &str) -> Option<String> {
    let stem = file_name.strip_prefix("raster_")?;
    let ext = std::path::Path::new(stem).extension()?;
    if !ext.eq_ignore_ascii_case("png") {
        return None;
    }
    Some(stem[..stem.len() - ext.len() - 1].to_string())
}

/// Discover built-in raster_*.png images in the assets/images/ directory.
/// Returns display names (e.g. "skull", "phoenix") sorted alphabetically.
/// Cached after first call.
pub fn builtin_raster_images() -> &'static [String] {
    static IMAGES: OnceLock<Vec<String>> = OnceLock::new();
    IMAGES.get_or_init(|| {
        let images_dir = crate::effect::loader::assets_dir().join("images");
        let mut names = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&images_dir) {
            for entry in entries.flatten() {
                if let Some(display) = builtin_display_name(&entry.file_name().to_string_lossy()) {
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
    #[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The real `assets/images`. `builtin_raster_images()` finds it through `assets_dir()`,
    /// which is CWD-relative first, and cargo runs tests from the crate directory rather
    /// than the repo root — so calling it here would report an empty set and prove nothing.
    fn images_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/images")
    }

    /// The built-in picker in the particles panel lists `builtin_raster_images()` and hands
    /// the chosen *display* name back, which `builtin_raster_path()` has to turn into a real
    /// file again. The two halves strip and re-apply the `raster_` prefix and `.png` suffix
    /// independently, so a change to either silently produces menu entries that load nothing
    /// — and the panel gives no feedback for a path that does not exist. egui buttons cannot
    /// be driven from outside the app, so this round-trip is the only automated check the
    /// picker gets (board #2002).
    #[test]
    fn builtin_image_names_round_trip_to_real_files() {
        let dir = images_dir();
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("assets/images must exist") {
            let file = entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .to_string();
            let Some(display) = builtin_display_name(&file) else {
                continue;
            };
            checked += 1;

            // What the user picks must rebuild the exact filename it came from.
            let rebuilt = builtin_raster_path(&display);
            assert_eq!(
                rebuilt.file_name().expect("file name").to_string_lossy(),
                file,
                "picker entry {display:?} rebuilds to {rebuilt:?}, not {file:?}"
            );
        }
        assert!(
            checked > 0,
            "no raster_*.png in {} — the picker would be an empty menu",
            dir.display()
        );
    }

    /// The naming convention itself, independent of what happens to be on disk.
    #[test]
    fn builtin_display_name_strips_only_real_raster_pngs() {
        assert_eq!(
            builtin_display_name("raster_skull.png").as_deref(),
            Some("skull")
        );
        assert_eq!(
            builtin_display_name("raster_samurai_mask.PNG").as_deref(),
            Some("samurai_mask"),
            "extension match is case-insensitive"
        );
        assert_eq!(
            builtin_display_name("raster_a.b.png").as_deref(),
            Some("a.b"),
            "only the final extension comes off"
        );
        assert_eq!(builtin_display_name("skull.png"), None, "needs the prefix");
        assert_eq!(builtin_display_name("raster_notes.txt"), None);
        assert_eq!(builtin_display_name("raster_noext"), None);
    }

    /// A name from the picker must actually decode into particle positions. Catches an image
    /// that is present but unreadable (wrong format, truncated) landing in the menu.
    #[test]
    fn builtin_images_sample_into_particles() {
        use crate::gpu::particle::types::ImageSampleDef;

        let sample_def = ImageSampleDef {
            mode: "grid".to_string(),
            threshold: 0.1,
            scale: 1.0,
        };
        let mut checked = 0;
        for entry in std::fs::read_dir(images_dir()).expect("assets/images must exist") {
            let path = entry.expect("dir entry").path();
            if !path
                .file_name()
                .is_some_and(|f| f.to_string_lossy().starts_with("raster_"))
            {
                continue;
            }
            checked += 1;
            let aux = super::super::image_source::sample_image(&path, &sample_def, 4096)
                .unwrap_or_else(|e| {
                    panic!("built-in image {} failed to sample: {e}", path.display())
                });
            assert!(
                !aux.is_empty(),
                "built-in image {} sampled to zero particles",
                path.display()
            );
        }
        assert!(checked > 0, "no built-in images sampled");
    }
}
