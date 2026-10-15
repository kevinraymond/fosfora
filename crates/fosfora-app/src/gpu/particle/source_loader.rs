use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Instant;

use crossbeam_channel::{Receiver, TryRecvError, bounded};

use crate::media::decoder::MediaProgress;
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

/// Built-in images that were renamed, as (old file, new file). The PHOSPHOR
/// picture became FOSFORA with the app.
const RENAMED_IMAGES: &[(&str, &str)] = &[("raster_phosphor.png", "raster_fosfora.png")];

/// `path` with a renamed built-in image's old file name swapped for its new
/// one, in the same folder: presets store the full path to a built-in image,
/// and one saved before the rename would otherwise fail to load.
pub fn current_image_path(path: &str) -> String {
    let p = std::path::Path::new(path);
    let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
        return path.to_string();
    };
    match RENAMED_IMAGES.iter().find(|(old, _)| *old == name) {
        // Swap the name as a string suffix rather than through `Path::with_file_name`,
        // which would rejoin with the host separator and hand a Windows user a
        // `/a/images\raster_fosfora.png` for a preset saved with forward slashes.
        Some((_, new)) => match path.strip_suffix(name) {
            Some(dir) => format!("{dir}{new}"),
            None => p.with_file_name(new).to_string_lossy().into_owned(),
        },
        None => path.to_string(),
    }
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

/// Where a load was asked for, frozen at request time (#2012).
///
/// The file dialog and the decode both run off the main thread, so between the
/// click and the result landing the user can select a different layer or a
/// different morph slot. Routing the result by whatever is live at DRAIN time
/// puts the media on the wrong layer; the splat loader already avoids this by
/// carrying its `layer_idx` through the request, and this is the same idea for
/// every field the drain needs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SourceRequest {
    /// The layer that asked. A result whose layer is gone, is no longer an
    /// effect, or no longer has a particle system is simply dropped.
    pub layer_idx: usize,
    /// Load into a morph TARGET rather than over the base source.
    pub as_morph_target: bool,
    /// Which morph slot; `None` means the first empty one.
    pub morph_slot: Option<u32>,
    /// A video for the layer's OBSTACLE rather than its particle source (#3159).
    pub as_obstacle: bool,
    /// Transport a preset restores onto a video source once it lands; `None`
    /// keeps the source's defaults.
    pub video_speed: Option<f32>,
    pub video_looping: Option<bool>,
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
    /// A 3D model was CHOSEN (#1993) — the path only, not decoded content.
    ///
    /// Unlike images and video, a model cannot be decoded on this background
    /// thread: turning it into particles means rastering it, which needs the
    /// device and queue in step with the render loop. So the dialog thread's
    /// whole job is picking the file, and the main thread does the work when the
    /// path arrives — the same split the obstacle model loader uses.
    Model { path: String },
    /// Loading failed.
    Error(String),
}

/// One source load as the panel shows it: which file, how far, how long.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceLoading {
    /// Position in the loader's list, what Cancel names.
    pub index: usize,
    pub name: String,
    /// Frames decoded, and how many the probe expects (0 = not known yet).
    pub done: u32,
    pub total: u32,
    pub secs: f32,
}

/// A load on its own thread, with the context it was asked in.
struct InFlight {
    request: SourceRequest,
    name: String,
    progress: Arc<MediaProgress>,
    rx: Receiver<ParticleSourceResult>,
    started: Instant,
}

impl InFlight {
    /// The role a load fills; a new request for the same role supersedes it.
    fn role(&self) -> (usize, bool, bool) {
        role_of(&self.request)
    }
}

fn role_of(r: &SourceRequest) -> (usize, bool, bool) {
    (r.layer_idx, r.as_morph_target, r.as_obstacle)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Background loading of particle image/video sources and obstacle videos.
///
/// Several loads can be in flight at once (a preset that restores a video on
/// two layers), each with its own progress and cancel. A new request for the
/// same layer and role cancels the one still running for it, so a result can
/// never land over a newer choice; a cancelled or dismissed load leaves the
/// list without a result (#3159).
pub struct ParticleSourceLoader {
    loads: Vec<InFlight>,
}

impl ParticleSourceLoader {
    pub fn new() -> Self {
        Self { loads: Vec::new() }
    }

    /// Something is still decoding or a file dialog is still open.
    pub fn is_loading(&self) -> bool {
        !self.loads.is_empty()
    }

    /// Every load in flight, in list order, for the panel.
    pub fn loads(&self) -> Vec<SourceLoading> {
        self.loads
            .iter()
            .enumerate()
            .map(|(index, l)| SourceLoading {
                index,
                name: l.name.clone(),
                done: l.progress.done.load(Ordering::Relaxed),
                total: l.progress.total.load(Ordering::Relaxed),
                secs: l.started.elapsed().as_secs_f32(),
            })
            .collect()
    }

    /// Stop load `index` of [`Self::loads`] (the panel's Cancel). Its thread
    /// notices on the next frame it decodes and its result is never delivered.
    pub fn cancel(&mut self, index: usize) {
        if index < self.loads.len() {
            let load = self.loads.remove(index);
            load.progress.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Stop every load: the stack they were meant for is being replaced.
    pub fn cancel_all(&mut self) {
        for load in self.loads.drain(..) {
            load.progress.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Start loading an image file in the background.
    pub fn load_image(&mut self, path: PathBuf, request: SourceRequest) -> std::io::Result<()> {
        let name = file_name(&path);
        self.start("particle-source-loader", name, request, move |_| {
            Some(load_image_sync(&path))
        })
    }

    /// Start decoding a video file in the background. Without the `video`
    /// feature the result is an error saying so.
    pub fn load_video(&mut self, path: PathBuf, request: SourceRequest) -> std::io::Result<()> {
        let name = file_name(&path);
        self.start("particle-source-loader", name, request, move |p| {
            Some(load_video_sync(&path, p))
        })
    }

    /// Open a file dialog for images on a background thread, then decode.
    /// The dialog + decode both run off the main thread to avoid freezing.
    #[cfg(feature = "desktop")]
    pub fn open_image_dialog(&mut self, request: SourceRequest) -> std::io::Result<()> {
        self.start(
            "particle-source-dialog",
            "choosing file...".to_string(),
            request,
            move |_| {
                let dialog = rfd::FileDialog::new()
                    .set_title("Load Image for Particle Source")
                    .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif"]);
                dialog.pick_file().map(|path| load_image_sync(&path))
            },
        )
    }

    /// Open a file dialog for video on a background thread, then decode. With
    /// `as_obstacle` set the video is for the layer's obstacle.
    #[cfg(feature = "video")]
    pub fn open_video_dialog(&mut self, request: SourceRequest) -> std::io::Result<()> {
        let title = if request.as_obstacle {
            "Load Video for Obstacle"
        } else {
            "Load Video for Particle Source"
        };
        self.start(
            "particle-source-dialog",
            "choosing file...".to_string(),
            request,
            move |p| {
                let mut dialog = rfd::FileDialog::new().set_title(title);
                if crate::media::video::ffmpeg_available() {
                    dialog = dialog.add_filter("Video", crate::media::decoder::VIDEO_EXTENSIONS);
                }
                dialog.pick_file().map(|path| load_video_sync(&path, p))
            },
        )
    }

    /// Open a file dialog for a 3D model on a background thread (#1993).
    ///
    /// Sends back the chosen path only — see [`ParticleSourceResult::Model`] for
    /// why the raster cannot happen here.
    #[cfg(feature = "desktop")]
    pub fn open_model_dialog(&mut self, request: SourceRequest) -> std::io::Result<()> {
        self.start(
            "particle-source-dialog",
            "choosing file...".to_string(),
            request,
            move |_| {
                let dialog = rfd::FileDialog::new()
                    .set_title("Load 3D Model for Particle Source")
                    .add_filter("3D model", super::model_source::MODEL_EXTENSIONS);
                dialog.pick_file().map(|path| ParticleSourceResult::Model {
                    path: path.to_string_lossy().to_string(),
                })
            },
        )
    }

    /// Run `job` on a thread named `thread_name` and track it. `None` from the
    /// job means nothing was chosen (a dismissed dialog) and the load just
    /// goes away. A load already running for the same layer and role is
    /// cancelled first. If the thread cannot be spawned nothing is tracked and
    /// the caller only has to report the error.
    fn start(
        &mut self,
        thread_name: &str,
        name: String,
        request: SourceRequest,
        job: impl FnOnce(&MediaProgress) -> Option<ParticleSourceResult> + Send + 'static,
    ) -> std::io::Result<()> {
        let role = role_of(&request);
        self.loads.retain(|l| {
            let same = l.role() == role;
            if same {
                l.progress.cancel.store(true, Ordering::Relaxed);
            }
            !same
        });
        let progress = Arc::new(MediaProgress::default());
        let (tx, rx) = bounded(1);
        let p = progress.clone();
        thread::Builder::new()
            .name(thread_name.into())
            .spawn(move || {
                if let Some(result) = job(&p) {
                    let _ = tx.send(result);
                }
            })?;
        self.loads.push(InFlight {
            request,
            name,
            progress,
            rx,
            started: Instant::now(),
        });
        Ok(())
    }

    /// One finished load, with the context it was requested in, or `None`
    /// when nothing has finished. Dismissed dialogs and dead threads leave
    /// the list here.
    pub fn try_recv(&mut self) -> Option<(SourceRequest, ParticleSourceResult)> {
        let mut i = 0;
        while i < self.loads.len() {
            match self.loads[i].rx.try_recv() {
                Ok(result) => {
                    let load = self.loads.remove(i);
                    return Some((load.request, result));
                }
                Err(TryRecvError::Empty) => i += 1,
                Err(TryRecvError::Disconnected) => {
                    self.loads.remove(i);
                }
            }
        }
        None
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
fn load_video_sync(path: &Path, progress: &MediaProgress) -> ParticleSourceResult {
    match crate::media::video::decode_video(path, progress) {
        Ok((frames, delays_ms)) => ParticleSourceResult::Animated {
            path: path.to_string_lossy().to_string(),
            frames,
            delays_ms,
        },
        Err(e) => ParticleSourceResult::Error(format!("Failed to load video: {e}")),
    }
}

#[cfg(not(feature = "video"))]
fn load_video_sync(path: &Path, _progress: &MediaProgress) -> ParticleSourceResult {
    ParticleSourceResult::Error(format!(
        "This build has no video support, cannot load '{}'",
        path.display()
    ))
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
    fn a_renamed_image_resolves_to_its_new_file() {
        assert_eq!(
            current_image_path("/opt/fosfora/assets/images/raster_phosphor.png"),
            "/opt/fosfora/assets/images/raster_fosfora.png"
        );
        assert_eq!(
            current_image_path("raster_phosphor.png"),
            "raster_fosfora.png"
        );
        assert_eq!(
            current_image_path("/x/raster_skull.png"),
            "/x/raster_skull.png"
        );
        // Every new file ships, and no old one does.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/images");
        for (old, new) in RENAMED_IMAGES {
            assert!(dir.join(new).exists(), "{new} does not ship");
            assert!(!dir.join(old).exists(), "{old} still ships");
        }
    }

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

    // ---- the loader itself: no ffmpeg, no GPU, jobs are closures ----

    fn req(layer_idx: usize) -> SourceRequest {
        SourceRequest {
            layer_idx,
            ..Default::default()
        }
    }

    fn model(path: &str) -> ParticleSourceResult {
        ParticleSourceResult::Model {
            path: path.to_string(),
        }
    }

    fn path_of(r: &ParticleSourceResult) -> &str {
        match r {
            ParticleSourceResult::Model { path } => path,
            _ => panic!("expected a Model result"),
        }
    }

    /// Poll `try_recv` until `n` results or a deadline, like the frame loop does.
    fn collect(
        l: &mut ParticleSourceLoader,
        n: usize,
    ) -> Vec<(SourceRequest, ParticleSourceResult)> {
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        let mut got = Vec::new();
        while got.len() < n && Instant::now() < deadline {
            match l.try_recv() {
                Some(r) => got.push(r),
                None => thread::sleep(std::time::Duration::from_millis(1)),
            }
        }
        got
    }

    #[test]
    fn loads_for_different_layers_run_side_by_side() {
        let mut l = ParticleSourceLoader::new();
        l.start("t", "a".into(), req(0), |_| Some(model("a")))
            .unwrap();
        l.start("t", "b".into(), req(1), |_| Some(model("b")))
            .unwrap();
        assert_eq!(l.loads().len(), 2, "both stay listed until they land");
        let mut got: Vec<usize> = collect(&mut l, 2)
            .iter()
            .map(|(r, _)| r.layer_idx)
            .collect();
        got.sort_unstable();
        assert_eq!(got, vec![0, 1]);
        assert!(!l.is_loading());
    }

    #[test]
    fn a_new_request_for_the_same_layer_cancels_the_old_one() {
        let mut l = ParticleSourceLoader::new();
        let (gate_tx, gate_rx) = bounded::<()>(1);
        let (saw_tx, saw_rx) = bounded::<bool>(1);
        l.start("t", "old".into(), req(0), move |p| {
            let _ = gate_rx.recv();
            let _ = saw_tx.send(p.cancel.load(Ordering::Relaxed));
            Some(model("old"))
        })
        .unwrap();
        l.start("t", "new".into(), req(0), |_| Some(model("new")))
            .unwrap();
        assert_eq!(
            l.loads().len(),
            1,
            "the superseded load left the list at once"
        );
        assert_eq!(l.loads()[0].name, "new");

        let got = collect(&mut l, 1);
        assert_eq!(path_of(&got[0].1), "new");
        gate_tx.send(()).unwrap();
        assert!(saw_rx.recv().unwrap(), "the old thread was told to stop");
        assert!(
            collect(&mut l, 1).is_empty(),
            "the old result never arrives"
        );
    }

    #[test]
    fn a_different_role_on_the_same_layer_is_not_superseded() {
        let mut l = ParticleSourceLoader::new();
        l.start("t", "src".into(), req(0), |_| Some(model("src")))
            .unwrap();
        let obstacle = SourceRequest {
            as_obstacle: true,
            ..req(0)
        };
        l.start("t", "obs".into(), obstacle, |_| Some(model("obs")))
            .unwrap();
        assert_eq!(l.loads().len(), 2);
        let got = collect(&mut l, 2);
        assert_eq!(got.len(), 2);
        assert!(got.iter().any(|(r, _)| r.as_obstacle));
    }

    #[test]
    fn cancel_drops_the_load_and_flags_its_thread() {
        let mut l = ParticleSourceLoader::new();
        let (gate_tx, gate_rx) = bounded::<()>(1);
        let (saw_tx, saw_rx) = bounded::<bool>(1);
        l.start("t", "slow".into(), req(0), move |p| {
            let _ = gate_rx.recv();
            let _ = saw_tx.send(p.cancel.load(Ordering::Relaxed));
            Some(model("slow"))
        })
        .unwrap();
        l.cancel(0);
        assert!(!l.is_loading());
        gate_tx.send(()).unwrap();
        assert!(saw_rx.recv().unwrap());
        assert!(collect(&mut l, 1).is_empty());
    }

    #[test]
    fn cancel_all_empties_the_list() {
        let mut l = ParticleSourceLoader::new();
        let (_gate_tx, gate_rx) = bounded::<()>(1);
        let gate_rx2 = gate_rx.clone();
        l.start("t", "a".into(), req(0), move |_| {
            let _ = gate_rx.recv();
            Some(model("a"))
        })
        .unwrap();
        l.start("t", "b".into(), req(1), move |_| {
            let _ = gate_rx2.recv();
            Some(model("b"))
        })
        .unwrap();
        l.cancel_all();
        assert!(!l.is_loading());
        assert!(l.loads().is_empty());
    }

    #[test]
    fn a_dismissed_dialog_clears_the_load() {
        let mut l = ParticleSourceLoader::new();
        l.start("t", "choosing file...".into(), req(0), |_| None)
            .unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while l.is_loading() && Instant::now() < deadline {
            assert!(l.try_recv().is_none());
            thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!l.is_loading());
    }

    #[test]
    fn a_result_carries_the_request_it_was_asked_with() {
        let mut l = ParticleSourceLoader::new();
        let request = SourceRequest {
            layer_idx: 3,
            video_speed: Some(0.5),
            video_looping: Some(false),
            ..Default::default()
        };
        l.start("t", "v".into(), request, |_| Some(model("v")))
            .unwrap();
        let got = collect(&mut l, 1);
        assert_eq!(got[0].0, request);
    }
}
