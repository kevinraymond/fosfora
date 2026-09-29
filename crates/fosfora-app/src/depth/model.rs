use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use anyhow::Result;

const MODEL_FILENAME: &str = "midas_v21_small_256.onnx";
/// Pinned to a repository commit, not `main`, so the file cannot change under us.
const MODEL_URL: &str = "https://huggingface.co/julienkay/sentis-MiDaS/resolve/3186add7635419f19189ce96a7c9b6280b9a1aed/onnx/midas_v21_small_256.onnx";
const MODEL_SHA256: &str = "b0a5b3f12625137e626805167907fe0410665bec671685d59daaa2daab19f977";

/// ONNX Runtime shared library filename per platform.
#[cfg(target_os = "linux")]
const ORT_LIB_FILENAME: &str = "libonnxruntime.so";
#[cfg(target_os = "macos")]
const ORT_LIB_FILENAME: &str = "libonnxruntime.dylib";
#[cfg(target_os = "windows")]
const ORT_LIB_FILENAME: &str = "onnxruntime.dll";

/// ONNX Runtime download (Microsoft official GitHub releases): the archive URL
/// and its SHA-256, the library's path inside it (the real file; the unversioned
/// names are 0-byte symlinks), and the extracted library's SHA-256.
/// v1.23.0 provides ORT_API_VERSION 23, matching ort-sys 2.0.0-rc.11.
struct OrtRelease {
    url: &'static str,
    archive_sha256: &'static str,
    member: &'static str,
    lib_sha256: &'static str,
}

#[cfg(target_os = "linux")]
const ORT: OrtRelease = OrtRelease {
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.23.0/onnxruntime-linux-x64-1.23.0.tgz",
    archive_sha256: "b6deea7f2e22c10c043019f294a0ea4d2a6c0ae52a009c34847640db75ec5580",
    member: "onnxruntime-linux-x64-1.23.0/lib/libonnxruntime.so.1.23.0",
    lib_sha256: "98b0253652d36c706cd9b873f3e8dc74e107c26cf9694672fb4d88da1c00f250",
};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const ORT: OrtRelease = OrtRelease {
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.23.0/onnxruntime-osx-arm64-1.23.0.tgz",
    archive_sha256: "8182db0ebb5caa21036a3c78178f17fabb98a7916bdab454467c8f4cf34bcfdf",
    member: "onnxruntime-osx-arm64-1.23.0/lib/libonnxruntime.1.23.0.dylib",
    lib_sha256: "d3859aecdb70ea099f5b5f4185fe16f0527c6680b18731e6e96fc971ec767cca",
};
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const ORT: OrtRelease = OrtRelease {
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.23.0/onnxruntime-osx-x86_64-1.23.0.tgz",
    archive_sha256: "a8e43edcaa349cbfc51578a7fc61ea2b88793ccf077b4bc65aca58999d20cf0f",
    member: "onnxruntime-osx-x86_64-1.23.0/lib/libonnxruntime.1.23.0.dylib",
    lib_sha256: "091d265e49da84ac8eafd6ff76b67688555192a272d784a252d550a858797d6f",
};
#[cfg(target_os = "windows")]
const ORT: OrtRelease = OrtRelease {
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.23.0/onnxruntime-win-x64-1.23.0.zip",
    archive_sha256: "72c23470310ec79a7d42d27fe9d257e6c98540c73fa5a1db1f67f538c6c16f2f",
    member: "onnxruntime-win-x64-1.23.0/lib/onnxruntime.dll",
    lib_sha256: "b4b7f9aed3cf6b04000f595bddcbdf12e87214bc401d1b81beadae3dbf28d2bd",
};

/// Result of the last ONNX Runtime load attempt. `Some(false)` is cleared when a
/// download replaces the library, so a fixed install loads without a restart.
static ORT_AVAILABLE: Mutex<Option<bool>> = Mutex::new(None);

/// Check whether the ONNX Runtime is available and initialized (cached).
/// If the runtime dylib exists in our models directory, loads it via init_from().
/// Returns false silently if not found.
pub fn ort_available() -> bool {
    let mut cached = ORT_AVAILABLE.lock().unwrap_or_else(|e| e.into_inner());
    *cached.get_or_insert_with(|| {
        let lib_path = ort_lib_path();
        if !lib_path.is_file() {
            log::info!("ONNX Runtime not found at {}", lib_path.display());
            return false;
        }

        // Temporarily suppress panic hook — ort panics internally if load fails
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));

        let result = std::panic::catch_unwind(|| match ort::init_from(&lib_path) {
            Ok(builder) => {
                builder.commit();
                Ok(true)
            }
            Err(e) => Err(format!("{e}")),
        });

        std::panic::set_hook(prev_hook);

        match result {
            Ok(Ok(true)) => {
                log::info!("ONNX Runtime loaded from {}", lib_path.display());
                true
            }
            Ok(Err(e)) => {
                log::warn!("ONNX Runtime init_from failed: {e}");
                false
            }
            Ok(Ok(false)) => {
                log::warn!("ONNX Runtime session builder failed after init");
                false
            }
            Err(_) => {
                log::info!(
                    "ONNX Runtime panicked during load from {}",
                    lib_path.display()
                );
                false
            }
        }
    })
}

/// Returns the directory where models and runtime are stored.
pub fn model_dir() -> PathBuf {
    crate::paths::config_root().join("models")
}

/// Returns the full path to the MiDaS model file.
pub fn model_path() -> PathBuf {
    model_dir().join(MODEL_FILENAME)
}

/// Returns the path where we store the ONNX Runtime shared library.
pub fn ort_lib_path() -> PathBuf {
    model_dir().join(ORT_LIB_FILENAME)
}

/// Check if both the model and the runtime exist on disk.
pub fn model_exists() -> bool {
    model_path().is_file()
}

/// Check if both model + runtime are ready for depth estimation. A runtime that
/// failed to load (e.g. a library truncated by an older, non-atomic extract)
/// reads as not ready, so the UI offers the download again to replace it.
pub fn depth_ready() -> bool {
    model_path().is_file()
        && ort_lib_path().is_file()
        && *ORT_AVAILABLE.lock().unwrap_or_else(|e| e.into_inner()) != Some(false)
}

/// Shared download infrastructure (moved to `crate::download` for the Splat
/// demo scene, #1800); re-exported so existing `depth::model::…` paths hold.
pub use crate::download::{DownloadProgress, download_file};

/// Download the MiDaS model AND ONNX Runtime on a background thread.
/// Returns a shared progress tracker.
pub fn download_model() -> Arc<DownloadProgress> {
    let progress = DownloadProgress::new();
    let progress_clone = progress.clone();

    std::thread::Builder::new()
        .name("fosfora-model-dl".into())
        .spawn(move || {
            if let Err(e) = download_all(&progress_clone) {
                log::error!("Depth download failed: {e}");
                if let Ok(mut msg) = progress_clone.error_message.lock() {
                    *msg = Some(e.to_string());
                }
                progress_clone.progress.store(102, Ordering::Relaxed);
            }
        })
        .ok();

    progress
}

fn download_all(progress: &DownloadProgress) -> Result<()> {
    let dir = model_dir();
    std::fs::create_dir_all(&dir)?;

    // 1. Download ONNX Runtime if missing or not the pinned build (~8-78MB compressed)
    let lib_path = ort_lib_path();
    if !crate::download::file_matches(&lib_path, ORT.lib_sha256) {
        log::info!("Downloading ONNX Runtime from {}", ORT.url);
        download_ort_runtime(&dir, progress)?;
        let mut cached = ORT_AVAILABLE.lock().unwrap_or_else(|e| e.into_inner());
        if *cached == Some(false) {
            *cached = None;
        }
        drop(cached);
        if progress.cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
    }

    // 2. Download MiDaS model if missing or not the pinned file (~66MB)
    let model = model_path();
    if !crate::download::file_matches(&model, MODEL_SHA256) {
        log::info!("Downloading MiDaS model from {MODEL_URL}");
        download_file(MODEL_URL, &model, MODEL_FILENAME, MODEL_SHA256, progress)?;
    }

    progress.progress.store(101, Ordering::Relaxed);
    Ok(())
}

/// Download and extract ONNX Runtime shared library from official release archive.
/// The library is extracted to a `.tmp` path, hashed, and renamed into place, so an
/// interrupted extract never leaves a library that later runs pick up.
fn download_ort_runtime(dir: &std::path::Path, progress: &DownloadProgress) -> Result<()> {
    let is_zip = std::path::Path::new(ORT.url)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
    let ext = if is_zip { "zip" } else { "tgz" };
    let archive_path = dir.join(format!("ort_runtime.{ext}"));

    download_file(
        ORT.url,
        &archive_path,
        "ONNX Runtime",
        ORT.archive_sha256,
        progress,
    )?;

    let target_path = dir.join(ORT_LIB_FILENAME);
    let tmp_path = target_path.with_extension("tmp");
    let extracted = if is_zip {
        extract_from_zip(&archive_path, ORT.member, &tmp_path)
    } else {
        extract_from_tgz(&archive_path, ORT.member, &tmp_path)
    };
    let _ = std::fs::remove_file(&archive_path);

    let result = extracted.and_then(|found| {
        if !found {
            anyhow::bail!("{} not found in the ONNX Runtime archive", ORT.member);
        }
        if !crate::download::file_matches(&tmp_path, ORT.lib_sha256) {
            anyhow::bail!("{ORT_LIB_FILENAME} failed its integrity check after extraction");
        }
        std::fs::rename(&tmp_path, &target_path)?;
        Ok(())
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result
}

/// Archive member path without a leading `./` (the macOS archives carry one).
fn member_path(path: &str) -> &str {
    path.strip_prefix("./").unwrap_or(path)
}

/// Extract `member` from a .tgz archive (Linux/macOS) to `out_path`.
fn extract_from_tgz(
    archive_path: &std::path::Path,
    member: &str,
    out_path: &std::path::Path,
) -> Result<bool> {
    let file = std::fs::File::open(archive_path)?;
    let decompressed = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decompressed);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        if member_path(&path) == member {
            let mut out = std::fs::File::create(out_path)?;
            let n = std::io::copy(&mut entry, &mut out)?;
            out.sync_all()?;
            log::info!(
                "Extracted {member} → {ORT_LIB_FILENAME} ({:.1} MB)",
                n as f64 / 1_048_576.0
            );
            return Ok(true);
        }
    }

    Ok(false)
}

/// Extract `member` from a .zip archive (Windows) to `out_path`.
fn extract_from_zip(
    archive_path: &std::path::Path,
    member: &str,
    out_path: &std::path::Path,
) -> Result<bool> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    let Ok(mut entry) = archive.by_name(member) else {
        return Ok(false);
    };
    let mut out = std::fs::File::create(out_path)?;
    let n = std::io::copy(&mut entry, &mut out)?;
    out.sync_all()?;
    log::info!(
        "Extracted {member} → {ORT_LIB_FILENAME} ({:.1} MB)",
        n as f64 / 1_048_576.0
    );
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_path_ends_with_onnx() {
        let p = model_path();
        assert!(p.to_string_lossy().ends_with(".onnx"));
    }

    #[test]
    fn model_dir_is_under_fosfora() {
        let d = model_dir();
        assert!(d.to_string_lossy().contains("fosfora"));
    }

    #[test]
    fn ort_lib_path_has_correct_extension() {
        let p = ort_lib_path();
        let s = p.to_string_lossy();
        assert!(s.ends_with(".so") || s.ends_with(".dylib") || s.ends_with(".dll"));
    }

    #[test]
    fn pinned_member_is_this_platforms_library() {
        let name = ORT.member.rsplit('/').next().unwrap();
        let stem = ORT_LIB_FILENAME.split('.').next().unwrap();
        assert!(name.starts_with(stem), "{name} vs {ORT_LIB_FILENAME}");
        assert_eq!(ORT.archive_sha256.len(), 64);
        assert_eq!(ORT.lib_sha256.len(), 64);
        assert_eq!(MODEL_SHA256.len(), 64);
        assert!(!MODEL_URL.contains("/resolve/main/"));
    }

    /// The macOS archive layout: a `./` prefix and the unversioned name as a
    /// symlink to the real, versioned file. Only the pinned member is taken.
    #[test]
    fn tgz_extract_takes_the_pinned_member_not_the_symlink() {
        let dir = std::env::temp_dir().join(format!("fosfora-ort-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("ort.tgz");
        {
            let gz = flate2::write::GzEncoder::new(
                std::fs::File::create(&archive).unwrap(),
                flate2::Compression::fast(),
            );
            let mut tar = tar::Builder::new(gz);
            let mut link = tar::Header::new_gnu();
            link.set_entry_type(tar::EntryType::Symlink);
            link.set_size(0);
            tar.append_link(
                &mut link,
                "./ort/lib/libonnxruntime.dylib",
                "libonnxruntime.1.23.0.dylib",
            )
            .unwrap();
            let body = vec![7u8; 4096];
            let mut real = tar::Header::new_gnu();
            real.set_size(body.len() as u64);
            real.set_mode(0o755);
            real.set_cksum();
            tar.append_data(
                &mut real,
                "./ort/lib/libonnxruntime.1.23.0.dylib",
                body.as_slice(),
            )
            .unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }
        let out = dir.join("lib.tmp");
        assert!(extract_from_tgz(&archive, "ort/lib/libonnxruntime.1.23.0.dylib", &out).unwrap());
        assert_eq!(std::fs::read(&out).unwrap(), vec![7u8; 4096]);
        assert!(!extract_from_tgz(&archive, "ort/lib/missing.dylib", &out).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_progress_initial_state() {
        let p = DownloadProgress::new();
        assert_eq!(p.percent(), 0);
        assert!(!p.is_complete());
        assert!(!p.is_error());
        assert!(p.is_downloading());
    }

    #[test]
    fn download_progress_complete() {
        let p = DownloadProgress::new();
        p.progress.store(101, Ordering::Relaxed);
        assert!(p.is_complete());
        assert!(!p.is_downloading());
    }

    #[test]
    fn download_progress_error() {
        let p = DownloadProgress::new();
        p.progress.store(102, Ordering::Relaxed);
        assert!(p.is_error());
        assert!(!p.is_downloading());
        assert!(!p.is_complete());
    }
}
