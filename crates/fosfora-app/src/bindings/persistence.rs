use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::types::Binding;

#[derive(Debug, Serialize, Deserialize)]
pub struct BindingsFile {
    pub version: u32,
    pub bindings: Vec<Binding>,
}

fn config_dir() -> PathBuf {
    crate::paths::config_root()
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

pub(crate) fn load_from_path(path: &PathBuf) -> Vec<Binding> {
    match std::fs::read_to_string(path) {
        Ok(contents) => match serde_json::from_str::<BindingsFile>(&contents) {
            Ok(file) => {
                log::info!(
                    "Loaded {} bindings from {}",
                    file.bindings.len(),
                    path.display()
                );
                migrate_legacy_osc_sources(file.bindings)
            }
            Err(e) => {
                log::warn!("Failed to parse bindings file {}: {e}", path.display());
                Vec::new()
            }
        },
        Err(_) => Vec::new(),
    }
}

/// Bus keys for structured OSC messages come from the canonical `/fosfora/...`
/// address, so bindings saved before the rename (`osc./phosphor/...`) would never
/// match again. Rewrite them on load; the file is rewritten on the next save.
fn migrate_legacy_osc_sources(mut bindings: Vec<Binding>) -> Vec<Binding> {
    const OLD: &str = "osc./phosphor/";
    const NEW: &str = "osc./fosfora/";
    let mut rewritten = 0usize;
    for b in &mut bindings {
        if let Some(rest) = b.source.strip_prefix(OLD) {
            b.source = format!("{NEW}{rest}");
            rewritten += 1;
        }
    }
    if rewritten > 0 {
        log::info!("Migrated {rewritten} binding source(s) from osc./phosphor/ to osc./fosfora/");
    }
    bindings
}

/// Unit tests must never touch the real config dir: `remove_binding` saves
/// immediately, and a test bus starts empty — so a plain `cargo test` used
/// to overwrite ~/.config/fosfora/global-bindings.json with an empty list.
#[cfg(test)]
fn save_to_path(_path: &PathBuf, _bindings: &[Binding]) {}

#[cfg(not(test))]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::types::{BindingScope, BindingTarget};

    fn binding_with_source(source: &str) -> Binding {
        Binding {
            id: "b1".to_string(),
            name: "test".to_string(),
            enabled: true,
            scope: BindingScope::Global,
            source: source.to_string(),
            target: BindingTarget::Unset,
            transforms: Vec::new(),
        }
    }

    /// A file saved before the rename round-trips with its OSC sources rewritten to
    /// the canonical namespace; everything else is untouched.
    #[test]
    fn load_rewrites_legacy_osc_sources() {
        let dir = std::env::temp_dir().join("fosfora_bindings_migrate_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("global-bindings.json");

        let file = BindingsFile {
            version: 1,
            bindings: vec![
                binding_with_source("osc./phosphor/param/speed"),
                binding_with_source("audio.kick"),
                binding_with_source("osc./custom/fader"),
            ],
        };
        std::fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();

        let loaded = load_from_path(&path);
        let sources: Vec<&str> = loaded.iter().map(|b| b.source.as_str()).collect();
        assert_eq!(
            sources,
            [
                "osc./fosfora/param/speed",
                "audio.kick",
                "osc./custom/fader"
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
