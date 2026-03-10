use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::types::Binding;

#[derive(Debug, Serialize, Deserialize)]
pub struct BindingsFile {
    pub version: u32,
    pub bindings: Vec<Binding>,
}

fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("phosphor")
}

fn global_path() -> PathBuf {
    config_dir().join("global-bindings.json")
}

fn preset_path(name: &str) -> PathBuf {
    config_dir()
        .join("presets")
        .join(format!("{name}.bindings.json"))
}

/// Returns true if the global bindings file exists (for migration check).
pub fn global_exists() -> bool {
    global_path().exists()
}

/// Load global-scoped bindings.
pub fn load_global() -> Vec<Binding> {
    load_from_path(&global_path())
}

/// Save global-scoped bindings.
pub fn save_global(bindings: &[Binding]) {
    save_to_path(&global_path(), bindings);
}

/// Load preset-scoped bindings (sidecar file).
pub fn load_preset(name: &str) -> Vec<Binding> {
    load_from_path(&preset_path(name))
}

/// Save preset-scoped bindings (sidecar file).
pub fn save_preset(name: &str, bindings: &[Binding]) {
    save_to_path(&preset_path(name), bindings);
}

fn load_from_path(path: &PathBuf) -> Vec<Binding> {
    match std::fs::read_to_string(path) {
        Ok(contents) => match serde_json::from_str::<BindingsFile>(&contents) {
            Ok(file) => {
                log::info!(
                    "Loaded {} bindings from {}",
                    file.bindings.len(),
                    path.display()
                );
                file.bindings
            }
            Err(e) => {
                log::warn!("Failed to parse bindings file {}: {e}", path.display());
                Vec::new()
            }
        },
        Err(_) => Vec::new(),
    }
}

fn save_to_path(path: &PathBuf, bindings: &[Binding]) {
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::error!("Failed to create bindings dir: {e}");
            return;
        }
    }

    let file = BindingsFile {
        version: 1,
        bindings: bindings.to_vec(),
    };

    match serde_json::to_string_pretty(&file) {
        Ok(json) => {
            if let Err(e) = std::fs::write(path, json) {
                log::error!("Failed to write bindings: {e}");
            } else {
                log::debug!("Saved {} bindings to {}", bindings.len(), path.display());
            }
        }
        Err(e) => log::error!("Failed to serialize bindings: {e}"),
    }
}
