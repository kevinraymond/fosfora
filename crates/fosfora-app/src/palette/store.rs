use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;

use super::types::Palette;

pub struct PaletteStore {
    pub root: PathBuf,
    pub palettes: HashMap<String, Palette>,
    dirty: bool,
    last_edit: Option<Instant>,
}

impl PaletteStore {
    pub fn from_map(root: PathBuf, palettes: HashMap<String, Palette>) -> Self {
        Self {
            root,
            palettes,
            dirty: false,
            last_edit: None,
        }
    }

    pub fn insert(&mut self, palette: Palette) {
        self.palettes.insert(palette.id.clone(), palette);
        self.touch();
    }

    pub fn touch(&mut self) {
        self.dirty = true;
        self.last_edit = Some(Instant::now());
    }

    pub fn flush_if_due(&mut self, now: Instant) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let Some(t) = self.last_edit else {
            return Ok(());
        };
        if now.saturating_duration_since(t) < Duration::from_millis(400) {
            return Ok(());
        }
        self.save_all()
    }

    pub fn save_all(&mut self) -> Result<()> {
        let dir = self.root.join("palettes");
        fs::create_dir_all(&dir)?;
        for p in self.palettes.values() {
            let tmp = dir.join(format!(".{}.json.tmp", p.id));
            let dest = dir.join(format!("{}.json", p.id));
            let json = serde_json::to_string_pretty(p)?;
            fs::write(&tmp, json)?;
            fs::rename(&tmp, dest)?;
        }
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_save_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = PaletteStore::from_map(dir.path().to_path_buf(), HashMap::new());
        store.insert(Palette::solid_test("earth", "Earth"));
        store.save_all().unwrap();
        let raw = fs::read_to_string(dir.path().join("palettes/earth.json")).unwrap();
        let p: Palette = serde_json::from_str(&raw).unwrap();
        assert_eq!(p.id, "earth");
    }
}
