//! Loading a scene directory into the renderer's stores — same file
//! classification as `analyze/validate.rs`, same serde types as the app.
//!
//! Layout expected in `<dir>` (what `scripts/generate_scene.py` writes):
//!
//! ```text
//! <Name>.json             a Preset
//! <Name>.bindings.json    its sidecar (optional)
//! _scene.json             the SceneSet naming those presets
//! ```

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::preset::Preset;
use crate::scene::types::SceneSet;

#[cfg_attr(not(feature = "analyze"), allow(dead_code))]
pub struct LoadedScene {
    pub presets: Vec<(String, Preset)>,
    pub scene: SceneSet,
}

#[cfg_attr(not(feature = "analyze"), allow(dead_code))]
pub fn load_scene_dir(dir: &Path) -> Result<LoadedScene> {
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    entries.sort();

    // A `<Name>.bindings.json` also ends in `.json`; scene files are the
    // `_`-prefixed ones — identical classification to `--validate`.
    let is_sidecar = |p: &Path| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".bindings.json"))
    };
    let is_scene = |p: &Path| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('_'))
    };

    let mut presets = Vec::new();
    for path in entries.iter().filter(|p| !is_sidecar(p) && !is_scene(p)) {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let preset: Preset = serde_json::from_str(&text)
            .with_context(|| format!("preset '{name}' does not parse"))?;
        presets.push((name, preset));
    }
    if presets.is_empty() {
        bail!("no presets in {}", dir.display());
    }

    let scene_path = entries
        .iter()
        .find(|p| is_scene(p))
        .with_context(|| format!("no _scene.json in {}", dir.display()))?;
    let text = std::fs::read_to_string(scene_path)
        .with_context(|| format!("reading {}", scene_path.display()))?;
    let scene: SceneSet = serde_json::from_str(&text).context("_scene.json does not parse")?;

    Ok(LoadedScene { presets, scene })
}
