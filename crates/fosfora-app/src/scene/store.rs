use std::path::PathBuf;

use anyhow::Result;

use super::types::SceneSet;

/// Manages scene files on disk (~/.config/fosfora/scenes/*.json).
pub struct SceneStore {
    pub scenes: Vec<(String, SceneSet)>,
    pub current_scene: Option<usize>,
    /// Where the scene files live: [`Self::scenes_dir`], or a scratch
    /// folder in tests.
    dir: PathBuf,
}

/// `base` if no scene has that name, else `base 2`, `base 3`, ...
pub fn first_free_name(names: &[String], base: &str) -> String {
    if !names.iter().any(|n| n == base) {
        return base.to_string();
    }
    // One of the first names.len() + 2 numbers is always free.
    (2..names.len() + 3)
        .map(|i| format!("{base} {i}"))
        .find(|c| !names.iter().any(|n| n == c))
        .unwrap_or_default()
}

impl SceneStore {
    pub fn new() -> Self {
        Self::in_dir(Self::scenes_dir())
    }

    fn in_dir(dir: PathBuf) -> Self {
        Self {
            scenes: Vec::new(),
            current_scene: None,
            dir,
        }
    }

    fn path_of(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.json"))
    }

    fn names(&self) -> Vec<String> {
        self.scenes.iter().map(|(n, _)| n.clone()).collect()
    }

    pub fn scenes_dir() -> PathBuf {
        crate::paths::config_root().join("scenes")
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
        // By characters: a byte slice panicked on a multibyte name.
        sanitized
            .trim()
            .chars()
            .take(64)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// Scan scenes directory and reload all scenes. The current scene stays
    /// current if its file is still there: deleting any other scene used to
    /// leave none current, and the cue list vanished with it.
    pub fn scan(&mut self) {
        let current = self.current_name().map(str::to_string);
        self.scenes.clear();

        let dir = self.dir.clone();
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
        self.current_scene = current.and_then(|c| self.scenes.iter().position(|(n, _)| *n == c));

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

        std::fs::create_dir_all(&self.dir)?;

        let path = self.path_of(&name);
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

        let path = self.path_of(name);
        if path.exists() {
            std::fs::remove_file(&path)?;
            log::info!("Deleted scene '{}'", name);
        }

        self.scan();
        Ok(())
    }

    /// Give scene `index` a new name. Returns its index after the rename.
    pub fn rename(&mut self, index: usize, new_name: &str) -> Result<usize> {
        let new_name = Self::sanitize_name(new_name);
        if new_name.is_empty() {
            anyhow::bail!("Scene name cannot be empty");
        }
        let (old, scene) = self
            .scenes
            .get(index)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Invalid scene index"))?;
        if old == new_name {
            return Ok(index);
        }
        if self.scenes.iter().any(|(n, _)| *n == new_name) {
            anyhow::bail!("A scene named '{new_name}' already exists");
        }
        let was_current = self.current_scene == Some(index);
        let mut scene = scene;
        scene.name = new_name.clone();
        std::fs::write(
            self.path_of(&new_name),
            serde_json::to_string_pretty(&scene)?,
        )?;
        std::fs::remove_file(self.path_of(&old))?;
        self.scan();
        let idx = self
            .scenes
            .iter()
            .position(|(n, _)| *n == new_name)
            .unwrap_or(0);
        if was_current {
            self.current_scene = Some(idx);
        }
        Ok(idx)
    }

    /// Copy scene `index` as "<name> copy". Returns the copy's index; the
    /// current scene does not change.
    pub fn duplicate(&mut self, index: usize) -> Result<usize> {
        let (name, scene) = self
            .scenes
            .get(index)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Invalid scene index"))?;
        let copy = first_free_name(&self.names(), &Self::sanitize_name(&format!("{name} copy")));
        let mut scene = scene;
        scene.name = copy.clone();
        std::fs::write(self.path_of(&copy), serde_json::to_string_pretty(&scene)?)?;
        self.scan();
        Ok(self
            .scenes
            .iter()
            .position(|(n, _)| *n == copy)
            .unwrap_or(0))
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
    fn sanitize_name_cuts_a_multibyte_name_by_characters() {
        let long = "é".repeat(100);
        assert_eq!(SceneStore::sanitize_name(&long).chars().count(), 64);
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

    fn scene(name: &str) -> SceneSet {
        SceneSet::new(name)
    }

    fn names_of(s: &SceneStore) -> Vec<&str> {
        s.scenes.iter().map(|(n, _)| n.as_str()).collect()
    }

    #[test]
    fn first_free_name_counts_up() {
        let names = vec!["Scene".to_string(), "Scene 2".to_string()];
        assert_eq!(first_free_name(&names, "Scene"), "Scene 3");
        assert_eq!(first_free_name(&names, "Opener"), "Opener");
    }

    // Deleting another scene rescanned the folder and left NO scene
    // current, so the cue list being edited disappeared from the panel.
    #[test]
    fn deleting_another_scene_keeps_the_current_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = SceneStore::in_dir(dir.path().to_path_buf());
        s.save("Close", scene("Close")).unwrap();
        s.save("Opener", scene("Opener")).unwrap();
        assert_eq!(s.current_name(), Some("Opener"));
        s.delete(0).unwrap(); // Close
        assert_eq!(names_of(&s), ["Opener"]);
        assert_eq!(s.current_name(), Some("Opener"));
        s.delete(0).unwrap();
        assert_eq!(s.current_name(), None);
    }

    #[test]
    fn rename_moves_the_file_and_keeps_it_current() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = SceneStore::in_dir(dir.path().to_path_buf());
        s.save("B", scene("B")).unwrap();
        s.save("C", scene("C")).unwrap();
        s.current_scene = Some(1); // C
        let idx = s.rename(1, "A").unwrap();
        assert_eq!(names_of(&s), ["A", "B"]);
        assert_eq!(idx, 0);
        assert_eq!(s.current_name(), Some("A"));
        assert_eq!(s.scenes[0].1.name, "A");
        assert!(!dir.path().join("C.json").exists());
        // A taken name is refused and changes nothing.
        assert!(s.rename(0, "B").is_err());
        assert_eq!(names_of(&s), ["A", "B"]);
        assert!(s.rename(0, "  ").is_err());
    }

    #[test]
    fn duplicate_adds_a_copy_and_leaves_the_current_scene() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = SceneStore::in_dir(dir.path().to_path_buf());
        s.save("Opener", scene("Opener")).unwrap();
        s.save("Zed", scene("Zed")).unwrap();
        s.duplicate(0).unwrap();
        s.duplicate(0).unwrap();
        assert_eq!(
            names_of(&s),
            ["Opener", "Opener copy", "Opener copy 2", "Zed"]
        );
        assert_eq!(s.current_name(), Some("Zed"));
    }
}
