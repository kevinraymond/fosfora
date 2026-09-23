use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::definition::ShowDefinition;
use crate::palette::bindings::PaletteBindingSet;
use crate::palette::types::Palette;
use crate::preset::Preset;

pub struct ShowPack {
    pub root: PathBuf,
    pub definition: ShowDefinition,
    pub palettes: HashMap<String, Palette>,
    pub bindings: HashMap<String, PaletteBindingSet>,
    pub presets: HashMap<String, Preset>,
}

impl ShowPack {
    pub fn open(show_json: &Path) -> Result<Self> {
        let show_json = show_json.canonicalize().unwrap_or_else(|_| show_json.to_path_buf());
        let root = show_json
            .parent()
            .map(Path::to_path_buf)
            .context("show.json has no parent directory")?;
        let raw = fs::read_to_string(&show_json)
            .with_context(|| format!("read {}", show_json.display()))?;
        let definition: ShowDefinition = serde_json::from_str(&raw).context("parse show.json")?;

        let palettes = load_json_map::<Palette>(&root.join("palettes"), |p| p.id.clone())?;
        let mut bindings = HashMap::new();
        let bind_dir = root.join("palette-bindings");
        if bind_dir.is_dir() {
            for entry in fs::read_dir(&bind_dir)? {
                let path = entry?.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let raw = fs::read_to_string(&path)?;
                let set: PaletteBindingSet = serde_json::from_str(&raw)?;
                bindings.insert(set.preset.clone(), set);
            }
        }

        let mut presets = HashMap::new();
        let preset_dir = root.join("presets");
        if preset_dir.is_dir() {
            for entry in fs::read_dir(&preset_dir)? {
                let path = entry?.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
                let raw = fs::read_to_string(&path)?;
                let preset: Preset = serde_json::from_str(&raw)
                    .with_context(|| format!("parse preset {}", path.display()))?;
                presets.insert(id, preset);
            }
        }

        Ok(Self {
            root,
            definition,
            palettes,
            bindings,
            presets,
        })
    }
}

fn load_json_map<T: serde::de::DeserializeOwned>(
    dir: &Path,
    id: impl Fn(&T) -> String,
) -> Result<HashMap<String, T>> {
    let mut out = HashMap::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let raw = fs::read_to_string(&path)?;
        let value: T = serde_json::from_str(&raw)
            .with_context(|| format!("parse {}", path.display()))?;
        out.insert(id(&value), value);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn opens_in_place_and_resolves_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("presets")).unwrap();
        fs::create_dir_all(root.join("palettes")).unwrap();
        let show = r#"{
            "schema_version": 1,
            "id": "pack-test",
            "name": "Pack Test",
            "duration_secs": 60,
            "scene_track": [],
            "palette_track": []
        }"#;
        fs::write(root.join("show.json"), show).unwrap();
        let mut f = fs::File::create(root.join("palettes/earth.json")).unwrap();
        f.write_all(
            serde_json::to_string(&Palette::solid_test("earth", "Earth"))
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        let pack = ShowPack::open(&root.join("show.json")).unwrap();
        assert_eq!(pack.definition.id, "pack-test");
        assert!(pack.palettes.contains_key("earth"));
        let moved = tempfile::tempdir().unwrap();
        let dest = moved.path().join("copied");
        fs::create_dir_all(&dest).unwrap();
        fs::copy(root.join("show.json"), dest.join("show.json")).unwrap();
        fs::create_dir_all(dest.join("palettes")).unwrap();
        fs::copy(root.join("palettes/earth.json"), dest.join("palettes/earth.json")).unwrap();
        let pack2 = ShowPack::open(&dest.join("show.json")).unwrap();
        assert_eq!(pack2.definition.id, "pack-test");
    }
}
