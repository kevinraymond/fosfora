//! The config root (`<config_dir>/fosfora`) and the one-time migration from the
//! pre-rename `phosphor` directory.
//!
//! Every config, preset, scene, cache and model file lives under [`config_root`].
//! Subsystems build their own filenames on top of it; nothing else in the crate
//! may call `dirs::config_dir()` directly.

use std::path::{Path, PathBuf};

pub const CONFIG_DIR_NAME: &str = "fosfora";
const LEGACY_CONFIG_DIR_NAME: &str = "phosphor";

/// `<config_dir>/fosfora` — `~/.config/fosfora` on Linux,
/// `~/Library/Application Support/fosfora` on macOS, `%APPDATA%\fosfora` on Windows.
pub fn config_root() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(CONFIG_DIR_NAME)
}

/// One-time move of the legacy `phosphor` config directory to its `fosfora` name.
/// Called once at startup, before any config file is read.
pub fn migrate_legacy_config_dir() {
    if let Some(base) = dirs::config_dir() {
        migrate_in(&base);
    }
}

fn migrate_in(base: &Path) {
    let old = base.join(LEGACY_CONFIG_DIR_NAME);
    let new = base.join(CONFIG_DIR_NAME);
    if !old.is_dir() {
        return;
    }
    if new.exists() {
        // Never merge: the user may have intentionally diverged the two.
        log::warn!(
            "Config: both {} and {} exist; using {}. Merge or remove {} manually.",
            old.display(),
            new.display(),
            new.display(),
            old.display()
        );
        return;
    }
    // Same parent directory, so this cannot fail with EXDEV; failures here are
    // permissions or Windows file locks.
    match std::fs::rename(&old, &new) {
        Ok(()) => {
            log::info!("Config migrated: {} -> {}", old.display(), new.display());
        }
        Err(rename_err) => match copy_dir_all(&old, &new) {
            Ok(()) => {
                log::info!(
                    "Config copied: {} -> {} (rename failed: {rename_err}; the old \
                     directory was left in place)",
                    old.display(),
                    new.display()
                );
            }
            Err(copy_err) => {
                // Remove the partial copy so the next launch retries cleanly.
                let _ = std::fs::remove_dir_all(&new);
                log::error!(
                    "Config migration failed: {copy_err}. Old settings remain in {}; \
                     continuing with defaults.",
                    old.display()
                );
            }
        },
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            // Symlinks are copied by referent, matching what a user-level
            // "copy my settings" expects.
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh temp base per case; the house pattern (no tempfile dep in release deps).
    fn temp_base(case: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("fosfora_migrate_test_{case}"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("create temp base");
        base
    }

    #[test]
    fn moves_legacy_dir_with_contents() {
        let base = temp_base("move");
        let old = base.join(LEGACY_CONFIG_DIR_NAME);
        std::fs::create_dir_all(old.join("presets")).unwrap();
        std::fs::write(old.join("settings.json"), b"{}").unwrap();
        std::fs::write(old.join("presets").join("a.json"), b"{}").unwrap();

        migrate_in(&base);

        let new = base.join(CONFIG_DIR_NAME);
        assert!(!old.exists());
        assert_eq!(std::fs::read(new.join("settings.json")).unwrap(), b"{}");
        assert_eq!(
            std::fs::read(new.join("presets").join("a.json")).unwrap(),
            b"{}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn both_dirs_present_is_a_no_op() {
        let base = temp_base("both");
        let old = base.join(LEGACY_CONFIG_DIR_NAME);
        let new = base.join(CONFIG_DIR_NAME);
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("settings.json"), b"old").unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("settings.json"), b"new").unwrap();

        migrate_in(&base);

        assert_eq!(std::fs::read(old.join("settings.json")).unwrap(), b"old");
        assert_eq!(std::fs::read(new.join("settings.json")).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn no_legacy_dir_is_a_no_op() {
        let base = temp_base("absent");
        migrate_in(&base);
        assert!(!base.join(CONFIG_DIR_NAME).exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn copy_dir_all_copies_nested_files() {
        let base = temp_base("copy");
        let src = base.join("src");
        std::fs::create_dir_all(src.join("nested")).unwrap();
        std::fs::write(src.join("a.txt"), b"a").unwrap();
        std::fs::write(src.join("nested").join("b.txt"), b"b").unwrap();

        copy_dir_all(&src, &base.join("dst")).unwrap();

        assert_eq!(std::fs::read(base.join("dst").join("a.txt")).unwrap(), b"a");
        assert_eq!(
            std::fs::read(base.join("dst").join("nested").join("b.txt")).unwrap(),
            b"b"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
