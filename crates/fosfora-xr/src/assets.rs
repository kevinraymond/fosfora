//! APK assets → internal storage ("Assets on Android" in `docs/xr/XR_DESIGN.md`).
//!
//! The core finds effects and shaders on the filesystem, so the Gradle build
//! stages the needed subset of `assets/` into the APK together with
//! `assets/xr_manifest.txt` (a content stamp, then one relative path per
//! line). On first run, or when the stamp changes, every listed file is copied
//! to `<internal data>/assets/`, and that directory is pinned as the core's
//! assets dir before any other core call.

use std::ffi::CString;
use std::path::{Path, PathBuf};

use android_activity::AndroidApp;
use anyhow::{Context, Result, anyhow};
use log::info;

const MANIFEST: &str = "assets/xr_manifest.txt";
const STAMP_FILE: &str = ".stamp";

/// Directories the app uses under internal storage.
pub struct AppDirs {
    pub assets: PathBuf,
    pub config: PathBuf,
}

/// Unpack the APK's Fosfora assets if the stamp changed and point the core at
/// them. Returns the directories so the caller can find scenes under
/// `assets/xr/`.
pub fn install(app: &AndroidApp) -> Result<AppDirs> {
    let data = app
        .internal_data_path()
        .ok_or_else(|| anyhow!("no internal data path"))?;
    let assets = data.join("assets");
    let config = data.join("config");
    std::fs::create_dir_all(&config).with_context(|| format!("creating {}", config.display()))?;

    let manifest = read_asset(app, MANIFEST)?;
    let manifest = String::from_utf8(manifest).context("xr_manifest.txt is not UTF-8")?;
    let mut lines = manifest.lines();
    let stamp = lines.next().unwrap_or_default().trim().to_owned();
    let files: Vec<&str> = lines.map(str::trim).filter(|l| !l.is_empty()).collect();

    let installed = std::fs::read_to_string(assets.join(STAMP_FILE)).unwrap_or_default();
    if installed.trim() == stamp && !stamp.is_empty() {
        info!(
            "assets up to date ({} files, stamp {})",
            files.len(),
            &stamp[..12.min(stamp.len())]
        );
    } else {
        info!(
            "unpacking {} asset files to {} (stamp {})",
            files.len(),
            assets.display(),
            &stamp[..12.min(stamp.len())]
        );
        let started = std::time::Instant::now();
        let mut bytes = 0usize;
        for rel in &files {
            let data = read_asset(app, &format!("assets/{rel}"))?;
            let dest = assets.join(rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(&dest, &data).with_context(|| format!("writing {}", dest.display()))?;
            bytes += data.len();
        }
        std::fs::write(assets.join(STAMP_FILE), &stamp)?;
        info!(
            "unpacked {} files, {:.1} MB in {:.0} ms",
            files.len(),
            bytes as f64 / 1e6,
            started.elapsed().as_secs_f64() * 1e3
        );
    }

    pin_core_dirs(&assets, &config);
    Ok(AppDirs { assets, config })
}

fn pin_core_dirs(assets: &Path, config: &Path) {
    if let Err(existing) = fosfora_app::effect::loader::set_assets_dir(assets.to_path_buf()) {
        log::warn!("assets dir already pinned to {}", existing.display());
    }
    if let Err(existing) = fosfora_app::paths::set_config_root(config.to_path_buf()) {
        log::warn!("config root already pinned to {}", existing.display());
    }
}

/// Read one file out of the APK through the NDK asset manager.
fn read_asset(app: &AndroidApp, path: &str) -> Result<Vec<u8>> {
    let manager = app.asset_manager();
    let cpath = CString::new(path).context("asset path with NUL")?;
    let mut asset = manager
        .open(&cpath)
        .ok_or_else(|| anyhow!("asset {path} not in the APK"))?;
    let buf = asset
        .buffer()
        .with_context(|| format!("reading asset {path}"))?;
    Ok(buf.to_vec())
}
