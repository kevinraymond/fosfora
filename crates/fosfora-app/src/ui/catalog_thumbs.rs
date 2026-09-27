//! The catalog's pictures (#3124).
//!
//! One still per effect, rendered ahead of time by
//! `scripts/build_thumbnails.py` through the app's own headless renderer and
//! shipped as `assets/thumbs/<pfx stem>.webp`. They live outside
//! `assets/effects` on purpose: that directory is watched recursively, and a
//! watcher counts a READ as a change.
//!
//! Decoded on first use and kept for the session. An effect with no file — a
//! user's own, or one added after the last render — gets `None`, and the
//! catalog draws a placeholder instead.
//!
//! Beside each still, `<stem>.anim.webp` is a short loop that plays while the
//! picture is hovered. Those decode on a thread when first hovered (a loop is
//! ~30 frames, too slow to decode inside a frame) and only the few most
//! recently hovered stay in memory.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use egui::{ColorImage, TextureHandle, TextureId, TextureOptions};

use crate::effect::format::PfxEffect;

/// The extensions a picture may have, in the order they are tried. WebP is
/// what ships; PNG lets someone drop in a picture for their own effect.
const EXTENSIONS: [&str; 2] = ["webp", "png"];

pub struct CatalogThumbs {
    dir: PathBuf,
    /// Keyed by `.pfx` file stem. `None` = looked, nothing there.
    cache: HashMap<String, Option<TextureHandle>>,
    /// Hover loops, keyed by stem, and the order they were last asked for.
    anims: HashMap<String, Anim>,
    recent: VecDeque<String>,
}

/// Hover loops kept decoded at once: ~30 frames of 352x198 is ~8 MB each.
const KEEP_ANIMS: usize = 6;

enum Anim {
    Loading(crossbeam_channel::Receiver<Result<Loop, String>>),
    Ready {
        frames: Loop,
        tex: TextureHandle,
        shown: usize,
    },
    /// No loop for this effect, or it failed to decode: the still stays.
    Missing,
}

/// A decoded loop: its frames and how long each one shows, in seconds.
pub struct Loop {
    pub frames: Vec<ColorImage>,
    pub frame_secs: f64,
}

impl CatalogThumbs {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            cache: HashMap::new(),
            anims: HashMap::new(),
            recent: VecDeque::new(),
        }
    }

    /// The shipped location: `assets/thumbs`.
    pub fn shipped() -> Self {
        Self::new(crate::effect::loader::assets_dir().join("thumbs"))
    }

    /// The picture for an effect, loading it the first time it is asked for.
    pub fn get(&mut self, ctx: &egui::Context, effect: &PfxEffect) -> Option<TextureId> {
        let stem = stem_of(effect)?;
        if !self.cache.contains_key(&stem) {
            let tex = find(&self.dir, &stem).and_then(|path| match decode(&path) {
                Ok(img) => Some(ctx.load_texture(
                    format!("catalog_thumb_{stem}"),
                    img,
                    TextureOptions::LINEAR,
                )),
                Err(e) => {
                    log::warn!("Catalog picture {} unreadable: {e}", path.display());
                    None
                }
            });
            self.cache.insert(stem.clone(), tex);
        }
        self.cache.get(&stem)?.as_ref().map(|t| t.id())
    }
}

impl CatalogThumbs {
    /// The effect's loop, at the frame for `secs` into the hover. `None`
    /// while it decodes, or when there is none: show the still.
    pub fn animated(
        &mut self,
        ctx: &egui::Context,
        effect: &PfxEffect,
        secs: f64,
    ) -> Option<TextureId> {
        let stem = stem_of(effect)?;
        self.recent.retain(|s| *s != stem);
        self.recent.push_back(stem.clone());
        while self.recent.len() > KEEP_ANIMS {
            if let Some(old) = self.recent.pop_front() {
                self.anims.remove(&old);
            }
        }
        let dir = &self.dir;
        let anim = self.anims.entry(stem.clone()).or_insert_with(|| {
            let path = dir.join(format!("{stem}.anim.webp"));
            if !path.is_file() {
                return Anim::Missing;
            }
            let (tx, rx) = crossbeam_channel::bounded(1);
            let spawned = std::thread::Builder::new()
                .name("catalog-anim".into())
                .spawn(move || {
                    let _ = tx.send(decode_loop(&path));
                });
            match spawned {
                Ok(_) => Anim::Loading(rx),
                Err(_) => Anim::Missing,
            }
        });
        if let Anim::Loading(rx) = anim {
            match rx.try_recv() {
                Ok(Ok(frames)) if !frames.frames.is_empty() => {
                    let tex = ctx.load_texture(
                        format!("catalog_anim_{stem}"),
                        frames.frames[0].clone(),
                        TextureOptions::LINEAR,
                    );
                    *anim = Anim::Ready {
                        frames,
                        tex,
                        shown: 0,
                    };
                }
                Ok(Ok(_)) => *anim = Anim::Missing,
                Ok(Err(e)) => {
                    log::warn!("Catalog preview for {stem} unreadable: {e}");
                    *anim = Anim::Missing;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    ctx.request_repaint();
                    return None;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => *anim = Anim::Missing,
            }
        }
        match anim {
            Anim::Ready { frames, tex, shown } => {
                let n = frames.frames.len();
                let i = ((secs.max(0.0) / frames.frame_secs) as usize) % n;
                if i != *shown {
                    tex.set(frames.frames[i].clone(), TextureOptions::LINEAR);
                    *shown = i;
                }
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(frames.frame_secs));
                Some(tex.id())
            }
            _ => None,
        }
    }
}

/// Decode an animated WebP into frames at its own frame rate.
pub fn decode_loop(path: &Path) -> Result<Loop, String> {
    use image::AnimationDecoder;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let decoder = image::codecs::webp::WebPDecoder::new(std::io::BufReader::new(file))
        .map_err(|e| e.to_string())?;
    let raw = decoder
        .into_frames()
        .collect_frames()
        .map_err(|e| e.to_string())?;
    let frame_secs = raw.first().map_or(1.0 / 12.0, |f| {
        let (n, d) = f.delay().numer_denom_ms();
        (n as f64 / d.max(1) as f64 / 1000.0).max(1.0 / 60.0)
    });
    let frames = raw
        .into_iter()
        .map(|f| {
            let img = f.into_buffer();
            let size = [img.width() as usize, img.height() as usize];
            ColorImage::from_rgba_unmultiplied(size, img.as_raw())
        })
        .collect();
    Ok(Loop { frames, frame_secs })
}

/// The name a picture is filed under: the effect's `.pfx` file stem, which
/// stays put when an effect is renamed in its JSON.
pub fn stem_of(effect: &PfxEffect) -> Option<String> {
    effect
        .source_path
        .as_deref()
        .and_then(Path::file_stem)
        .map(|s| s.to_string_lossy().into_owned())
}

/// The picture file for `stem` in `dir`, if there is one.
pub fn find(dir: &Path, stem: &str) -> Option<PathBuf> {
    EXTENSIONS
        .iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|p| p.is_file())
}

fn decode(path: &Path) -> Result<ColorImage, image::ImageError> {
    let img = image::open(path)?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}
