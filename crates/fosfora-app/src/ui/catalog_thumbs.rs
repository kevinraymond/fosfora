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

use std::collections::HashMap;
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
}

impl CatalogThumbs {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            cache: HashMap::new(),
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
