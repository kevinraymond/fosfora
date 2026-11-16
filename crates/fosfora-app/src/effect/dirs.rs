//! Extra `.pfx` effect directories.
//!
//! `FOSFORA_EFFECT_DIRS` lists directories (same rules as
//! `FOSFORA_TRAMA_DIRS`: split like `PATH`, a leading `~` is home) whose
//! `*.pfx` files are layer effects exactly like `assets/effects/`: they appear
//! in the catalog (under "Yours" unless their author is "Fosfora"), load onto
//! layers, save in presets by name, and hot-reload when the `.pfx` or one of
//! its shaders changes. Each directory is watched recursively, so a `.pfx` and
//! its `.wgsl` can sit side by side or in subfolders.
//!
//! A relative `shader` path in an extra directory's `.pfx` resolves against
//! that `.pfx`'s own directory, not `assets/shaders/`, so a generator can ship
//! a self-contained folder. [`localize`] does the rewrite, at scan and again
//! on every hot reload of the file.
//!
//! A brand-new `.pfx` is picked up at the next start, as in `assets/effects/`.

use std::path::{Path, PathBuf};

use super::format::PfxEffect;

/// The variable naming the extra directories.
pub const ENV: &str = "FOSFORA_EFFECT_DIRS";

/// Extra directories from [`ENV`], absolute, in the order given.
pub fn extra_dirs() -> Vec<PathBuf> {
    crate::trama::dirs::parse(std::env::var_os(ENV), crate::trama::dirs::home())
}

/// Is `path` inside one of `dirs` (at any depth)?
fn is_under(path: &Path, dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|d| path.starts_with(d))
}

/// Make the relative shader paths of an effect read from `pfx_path`'s
/// directory, when that file lives in an extra directory. Built-in effects
/// and absolute paths are left alone, and a second call changes nothing.
pub fn localize(effect: &mut PfxEffect, pfx_path: &Path) {
    localize_in(effect, pfx_path, &extra_dirs());
}

fn localize_in(effect: &mut PfxEffect, pfx_path: &Path, dirs: &[PathBuf]) {
    let abs = std::path::absolute(pfx_path).unwrap_or_else(|_| pfx_path.to_path_buf());
    if !is_under(&abs, dirs) {
        return;
    }
    let Some(base) = abs.parent() else {
        return;
    };
    let fix = |shader: &mut String| {
        if !shader.is_empty() && Path::new(shader.as_str()).is_relative() {
            *shader = base.join(shader.as_str()).display().to_string();
        }
    };
    fix(&mut effect.shader);
    for pass in &mut effect.passes {
        fix(&mut pass.shader);
    }
}

/// Read one `.pfx`: parse it, apply the load-time fixups the built-in scan
/// applies, and remember where it came from. `None` (logged) if unreadable.
pub(crate) fn read_pfx(path: &Path) -> Option<PfxEffect> {
    read_pfx_in(path, &extra_dirs())
}

fn read_pfx_in(path: &Path, dirs: &[PathBuf]) -> Option<PfxEffect> {
    let json = std::fs::read_to_string(path)
        .map_err(|e| log::warn!("Failed to read {}: {e}", path.display()))
        .ok()?;
    let mut effect: PfxEffect = serde_json::from_str(&json)
        .map_err(|e| log::warn!("Failed to parse {}: {e}", path.display()))
        .ok()?;
    // `interaction_3d` implies `interaction`: set it so every reader of the
    // flag builds and reports the hash.
    if let Some(pd) = effect.particles.as_mut() {
        pd.interaction |= pd.interaction_3d;
    }
    // Localize against the path as listed: the extra directories are not
    // canonical, so a symlinked one would not match the canonical path.
    localize_in(&mut effect, path, dirs);
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    log::info!("Found effect: {} ({})", effect.name, path.display());
    effect.source_path = Some(path);
    Some(effect)
}

/// Every `*.pfx` under `dir`, recursively, sorted by path.
pub(crate) fn pfx_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for path in entries.filter_map(Result::ok).map(|e| e.path()) {
        if path.is_dir() {
            out.extend(pfx_files(&path));
        } else if path.extension().is_some_and(|e| e == "pfx") {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// The effects of every extra directory, in directory then path order.
pub(crate) fn scan_extra() -> Vec<PfxEffect> {
    let mut effects = Vec::new();
    for dir in extra_dirs() {
        if !dir.is_dir() {
            log::warn!("{ENV} names {}, which is not a directory", dir.display());
            continue;
        }
        let found: Vec<PfxEffect> = pfx_files(&dir).iter().filter_map(|p| read_pfx(p)).collect();
        log::info!("{} effect(s) from {}", found.len(), dir.display());
        effects.extend(found);
    }
    effects
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effect(shader: &str, pass_shaders: &[&str]) -> PfxEffect {
        let passes: Vec<serde_json::Value> = pass_shaders
            .iter()
            .map(|s| serde_json::json!({ "name": "p", "shader": s }))
            .collect();
        serde_json::from_value(serde_json::json!({
            "name": "T", "shader": shader, "passes": passes
        }))
        .unwrap()
    }

    // Roots are built from `temp_dir`, which is absolute on every platform: a `/srv/...`
    // literal would be relative on Windows (it becomes `D:\srv\...`), so it would not
    // start with the listed directory.
    #[test]
    fn extra_dir_effects_read_shaders_beside_their_pfx() {
        let root = std::env::temp_dir().join("tessera");
        let dirs = vec![root.clone()];
        let other = std::env::temp_dir().join("elsewhere").join("b.wgsl");
        let mut e = effect(
            "graphene.wgsl",
            &["sub/a.wgsl", &other.display().to_string()],
        );
        let pfx = root.join("graphene.pfx");
        localize_in(&mut e, &pfx, &dirs);
        assert_eq!(Path::new(&e.shader), root.join("graphene.wgsl"));
        assert_eq!(
            Path::new(&e.passes[0].shader),
            root.join("sub").join("a.wgsl")
        );
        assert_eq!(
            Path::new(&e.passes[1].shader),
            other,
            "absolute paths are kept"
        );

        let once = e.shader.clone();
        localize_in(&mut e, &pfx, &dirs);
        assert_eq!(e.shader, once, "a second call changes nothing");
    }

    #[test]
    fn built_in_effects_keep_assets_relative_shaders() {
        let dirs = vec![std::env::temp_dir().join("tessera")];
        let built_in = std::env::temp_dir().join("assets").join("aurora.pfx");
        let mut e = effect("aurora.wgsl", &[]);
        localize_in(&mut e, &built_in, &dirs);
        assert_eq!(e.shader, "aurora.wgsl");
    }

    #[test]
    fn pfx_files_are_found_recursively_and_sorted() {
        let root = std::env::temp_dir().join(format!("fosfora-effect-dirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        for f in ["b.pfx", "a.pfx", "sub/c.pfx", "shader.wgsl"] {
            std::fs::write(root.join(f), "{}").unwrap();
        }
        // Paths, not `display()` strings: a `\` separator would not match a `/` one.
        let found: Vec<PathBuf> = pfx_files(&root)
            .iter()
            .map(|p| p.strip_prefix(&root).unwrap().to_path_buf())
            .collect();
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            found,
            [
                PathBuf::from("a.pfx"),
                PathBuf::from("b.pfx"),
                Path::new("sub").join("c.pfx"),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_extra_dir_still_localizes_shaders() {
        let root = std::env::temp_dir().join(format!("fosfora-effect-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let real = root.join("real");
        let link = root.join("link");
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        std::fs::write(
            real.join("glow.pfx"),
            r#"{"name": "Glow", "shader": "glow.wgsl"}"#,
        )
        .unwrap();

        let e = read_pfx_in(&link.join("glow.pfx"), std::slice::from_ref(&link));
        let _ = std::fs::remove_dir_all(&root);
        let e = e.expect("the .pfx parses");
        assert_eq!(
            Path::new(&e.shader),
            link.join("glow.wgsl"),
            "shader resolves beside the .pfx, not in assets/shaders"
        );
    }
}
