use std::path::PathBuf;

use anyhow::Result;

use super::types::SceneSet;

/// Manages scene files on disk (~/.config/phosphor/scenes/*.json).
pub struct SceneStore {
    pub scenes: Vec<(String, SceneSet)>,
    pub current_scene: Option<usize>,
}

impl SceneStore {
    pub fn new() -> Self {
        Self {
            scenes: Vec::new(),
            current_scene: None,
        }
    }

    pub fn scenes_dir() -> PathBuf {
        let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        config_dir.join("phosphor").join("scenes")
    }

    fn sanitize_name(name: &str) -> String {
        let sanitized: String = name
            .chars()
            .map(|c| {
                if c == '/' || c == '\\' || c == '.' {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        let trimmed = sanitized.trim();
        if trimmed.len() > 64 {
            trimmed[..64].to_string()
        } else {
            trimmed.to_string()
        }
    }

    /// Scan scenes directory and reload all scenes.
    pub fn scan(&mut self) {
        self.scenes.clear();

        let dir = Self::scenes_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => {
                log::info!("No scenes directory found at {}", dir.display());
                self.current_scene = None;
                return;
            }
        };

        let mut user_scenes: Vec<(String, SceneSet)> = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(contents) => match serde_json::from_str::<SceneSet>(&contents) {
                    Ok(scene) => {
                        user_scenes.push((name, scene));
                    }
                    Err(e) => {
                        log::warn!("Failed to parse scene {}: {e}", path.display());
                    }
                },
                Err(e) => {
                    log::warn!("Failed to read scene {}: {e}", path.display());
                }
            }
        }

        user_scenes.sort_by(|a, b| a.0.cmp(&b.0));
        self.scenes = user_scenes;
        self.current_scene = None;

        log::info!(
            "Scanned {} scenes from {}",
            self.scenes.len(),
            dir.display()
        );
    }

    /// Save a scene to disk and re-scan.
    pub fn save(&mut self, name: &str, scene: SceneSet) -> Result<usize> {
        let name = Self::sanitize_name(name);
        if name.is_empty() {
            anyhow::bail!("Scene name cannot be empty");
        }

        let dir = Self::scenes_dir();
        std::fs::create_dir_all(&dir)?;

        let path = dir.join(format!("{name}.json"));
        let json = serde_json::to_string_pretty(&scene)?;
        std::fs::write(&path, json)?;
        log::info!("Saved scene '{}' to {}", name, path.display());

        self.scan();

        let idx = self
            .scenes
            .iter()
            .position(|(n, _)| n == &name)
            .unwrap_or(0);
        self.current_scene = Some(idx);
        Ok(idx)
    }

    /// Load a scene by index.
    pub fn load(&self, index: usize) -> Option<&SceneSet> {
        self.scenes.get(index).map(|(_, s)| s)
    }

    /// Delete a scene from disk and re-scan.
    pub fn delete(&mut self, index: usize) -> Result<()> {
        let (name, _) = self
            .scenes
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Invalid scene index"))?;

        let path = Self::scenes_dir().join(format!("{name}.json"));
        if path.exists() {
            std::fs::remove_file(&path)?;
            log::info!("Deleted scene '{}'", name);
        }

        self.scan();
        Ok(())
    }

    /// Get the name of the currently loaded scene.
    pub fn current_name(&self) -> Option<&str> {
        self.current_scene
            .and_then(|i| self.scenes.get(i))
            .map(|(name, _)| name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_store_new_empty() {
        let s = SceneStore::new();
        assert!(s.scenes.is_empty());
        assert!(s.current_scene.is_none());
    }

    #[test]
    fn sanitize_name_strips_slashes() {
        assert_eq!(SceneStore::sanitize_name("a/b\\c"), "a_b_c");
    }

    #[test]
    fn sanitize_name_strips_dots() {
        assert_eq!(SceneStore::sanitize_name("my.scene"), "my_scene");
    }

    #[test]
    fn sanitize_name_trims_whitespace() {
        assert_eq!(SceneStore::sanitize_name("  hello  "), "hello");
    }

    #[test]
    fn sanitize_name_max_64_chars() {
        let long = "a".repeat(100);
        assert_eq!(SceneStore::sanitize_name(&long).len(), 64);
    }

    #[test]
    fn sanitize_name_whitespace_only() {
        assert_eq!(SceneStore::sanitize_name("   "), "");
    }

    #[test]
    fn current_name_returns_correct() {
        let mut s = SceneStore::new();
        let scene = SceneSet::new("Test Scene");
        s.scenes.push(("Test Scene".into(), scene));
        s.current_scene = Some(0);
        assert_eq!(s.current_name(), Some("Test Scene"));
    }

    #[test]
    fn current_name_none_when_empty() {
        let s = SceneStore::new();
        assert!(s.current_name().is_none());
    }
}
