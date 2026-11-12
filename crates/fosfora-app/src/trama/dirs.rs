//! Extra trama effect directories.
//!
//! `FOSFORA_TRAMA_DIRS` lists directories (`:`-separated on Unix, `;` on
//! Windows; a leading `~` is the home directory) whose `*.wgsl` files are
//! trama effects exactly like `assets/trama/effects/`: loaded at startup,
//! hot-reloaded on change, registered live when new, and kept at the last
//! good version when a save breaks them. An external generator (a shader DSL,
//! another project's render pipeline) can write straight into one.
//!
//! The built-in directory loads first, then each extra directory in order. An
//! effect id defined in more than one place takes the LAST definition, which
//! is also what a hot reload does (the changed file wins), so the two paths
//! agree.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::effect::{TramaRegistry, trama_effects_dir};
use crate::effect::loader::EffectLoader;

/// The variable naming the extra directories.
pub const ENV: &str = "FOSFORA_TRAMA_DIRS";

/// Extra directories from [`ENV`], absolute, in the order given.
pub fn extra_dirs() -> Vec<PathBuf> {
    parse(std::env::var_os(ENV), std::env::var_os("HOME"))
}

/// The built-in directory, then every extra one: load and reload order.
pub fn all_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![trama_effects_dir()];
    dirs.extend(extra_dirs());
    dirs
}

/// Is `path` a trama effect file directly inside one of `dirs`? The watcher
/// routes by this: the built-in directory is recognized by its name
/// (`trama/effects`), an extra one only by being listed.
pub fn is_effect_in(path: &Path, dirs: &[PathBuf]) -> bool {
    path.extension().is_some_and(|e| e == "wgsl")
        && path.parent().is_some_and(|p| dirs.iter().any(|d| d == p))
}

/// A `PATH`-style directory list: split, `~` expanded against `home`, made
/// absolute. Shared with `effect::dirs` (`FOSFORA_EFFECT_DIRS`).
pub(crate) fn parse(raw: Option<OsString>, home: Option<OsString>) -> Vec<PathBuf> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    std::env::split_paths(&raw)
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| expand_home(&p, home.as_deref()))
        .map(|p| std::path::absolute(&p).unwrap_or(p))
        .collect()
}

/// `~` and `~/rest` against `home`; anything else unchanged.
fn expand_home(path: &Path, home: Option<&std::ffi::OsStr>) -> PathBuf {
    let (Some(home), Ok(rest)) = (home, path.strip_prefix("~")) else {
        return path.to_path_buf();
    };
    PathBuf::from(home).join(rest)
}

impl TramaRegistry {
    /// [`TramaRegistry::load`] over several directories, in order; a later
    /// directory's effect replaces an earlier one with the same id.
    pub fn load_all(
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        loader: &EffectLoader,
        dirs: &[PathBuf],
    ) -> Self {
        let Some((first, rest)) = dirs.split_first() else {
            return Self::load(device, cache, loader, &trama_effects_dir());
        };
        let mut registry = Self::load(device, cache, loader, first);
        for dir in rest {
            if !dir.is_dir() {
                log::warn!(
                    "trama: {ENV} names {}, which is not a directory",
                    dir.display()
                );
                continue;
            }
            let more = Self::load(device, cache, loader, dir);
            log::info!(
                "trama: {} effect(s) from {}",
                more.effects.len(),
                dir.display()
            );
            for def in more.effects {
                if registry.effects.iter().any(|e| e.id == def.id) {
                    log::info!(
                        "trama: {} from {} replaces the earlier one",
                        def.id.0,
                        dir.display()
                    );
                    registry.effects.retain(|e| e.id != def.id);
                }
                registry.effects.push(def);
            }
            registry.errors.extend(more.errors);
        }
        registry.effects.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/srv/me";

    /// `parse` takes `std::env::var_os` results, which are `Option`s.
    #[allow(clippy::unnecessary_wraps)]
    fn os(s: &str) -> Option<OsString> {
        Some(OsString::from(s))
    }

    #[test]
    fn unset_or_empty_means_no_extra_dirs() {
        assert!(parse(None, os(HOME)).is_empty());
        assert!(parse(os(""), os(HOME)).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn dirs_split_on_colon_and_expand_home() {
        let dirs = parse(os("~/a/layers:/abs/b::~"), os(HOME));
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/srv/me/a/layers"),
                PathBuf::from("/abs/b"),
                PathBuf::from("/srv/me"),
            ]
        );
    }

    #[test]
    fn only_a_leading_tilde_component_is_home() {
        let home = Some(std::ffi::OsStr::new(HOME));
        assert_eq!(
            expand_home(Path::new("~other/x"), home),
            PathBuf::from("~other/x")
        );
        assert_eq!(expand_home(Path::new("/a/~"), home), PathBuf::from("/a/~"));
    }

    #[test]
    fn only_wgsl_directly_in_a_listed_dir_is_an_effect() {
        let dirs = vec![PathBuf::from("/srv/tessera/out/layers")];
        assert!(is_effect_in(
            Path::new("/srv/tessera/out/layers/rings.wgsl"),
            &dirs
        ));
        assert!(!is_effect_in(
            Path::new("/srv/tessera/out/layers/notes.txt"),
            &dirs
        ));
        assert!(!is_effect_in(
            Path::new("/srv/tessera/out/layers/sub/rings.wgsl"),
            &dirs
        ));
        assert!(!is_effect_in(Path::new("/srv/other/rings.wgsl"), &dirs));
    }

    /// End to end through the real watcher: a file a generator writes into an
    /// extra directory arrives on the TRAMA route, not the layer-shader one
    /// (which would match it against no pass and drop it silently).
    #[cfg(all(target_os = "linux", feature = "desktop"))]
    #[test]
    fn a_new_file_in_an_extra_dir_reaches_the_trama_route() {
        use crate::shader::hot_reload::ShaderWatcher;
        use std::time::Duration;

        let root = std::env::temp_dir().join(format!("fosfora-extra-trama-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let shaders = root.join("shaders");
        let effects = root.join("effects");
        let trama = root.join("trama/effects");
        let extra = root.join("tessera/out/layers");
        for d in [&shaders, &effects, &trama, &extra] {
            std::fs::create_dir_all(d).unwrap();
        }
        let mut w = ShaderWatcher::watching(
            &shaders,
            &effects,
            &trama,
            std::slice::from_ref(&extra),
            &[],
        );
        std::thread::sleep(Duration::from_millis(200));

        let file = extra.join("rings.wgsl");
        std::fs::write(&file, "/*! trama */").unwrap();
        let mut trama_changes = Vec::new();
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(50));
            trama_changes.extend(w.drain_trama_changes());
            if !trama_changes.is_empty() {
                break;
            }
        }
        let shader_changes = w.drain_changes();
        let _ = std::fs::remove_dir_all(&root);

        assert!(
            trama_changes
                .iter()
                .any(|p| p.ends_with("tessera/out/layers/rings.wgsl")),
            "new extra-dir file reported as a trama change: {trama_changes:?}"
        );
        assert!(
            shader_changes.is_empty(),
            "not as a layer shader: {shader_changes:?}"
        );
    }

    #[test]
    fn relative_dirs_become_absolute() {
        let dirs = parse(os("rel/layers"), None);
        assert!(dirs[0].is_absolute());
        assert!(dirs[0].ends_with("rel/layers"));
    }
}
