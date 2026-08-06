use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::Ordering;

use anyhow::Result;

const MODEL_FILENAME: &str = "midas_v21_small_256.onnx";
const MODEL_URL: &str =
    "https://huggingface.co/julienkay/sentis-MiDaS/resolve/main/onnx/midas_v21_small_256.onnx";

/// ONNX Runtime shared library filename per platform.
#[cfg(target_os = "linux")]
const ORT_LIB_FILENAME: &str = "libonnxruntime.so";
#[cfg(target_os = "macos")]
const ORT_LIB_FILENAME: &str = "libonnxruntime.dylib";
#[cfg(target_os = "windows")]
const ORT_LIB_FILENAME: &str = "onnxruntime.dll";

/// ONNX Runtime download URL (Microsoft official GitHub releases).
/// v1.23.0 provides ORT_API_VERSION 23, matching ort-sys 2.0.0-rc.11.
#[cfg(target_os = "linux")]
const ORT_LIB_URL: &str = "https://github.com/microsoft/onnxruntime/releases/download/v1.23.0/onnxruntime-linux-x64-1.23.0.tgz";
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const ORT_LIB_URL: &str = "https://github.com/microsoft/onnxruntime/releases/download/v1.23.0/onnxruntime-osx-arm64-1.23.0.tgz";
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const ORT_LIB_URL: &str = "https://github.com/microsoft/onnxruntime/releases/download/v1.23.0/onnxruntime-osx-x86_64-1.23.0.tgz";
#[cfg(target_os = "windows")]
const ORT_LIB_URL: &str = "https://github.com/microsoft/onnxruntime/releases/download/v1.23.0/onnxruntime-win-x64-1.23.0.zip";

static ORT_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Check whether the ONNX Runtime is available and initialized (cached).
/// If the runtime dylib exists in our models directory, loads it via init_from().
/// Returns false silently if not found.
pub fn ort_available() -> bool {
    *ORT_AVAILABLE.get_or_init(|| {
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

/// Check if both model + runtime are ready for depth estimation.
pub fn depth_ready() -> bool {
    model_path().is_file() && ort_lib_path().is_file()
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
        .name("phosphor-model-dl".into())
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

    // 1. Download ONNX Runtime if missing (~15-25MB compressed)
    let lib_path = ort_lib_path();
    if !lib_path.is_file() {
        log::info!("Downloading ONNX Runtime from {ORT_LIB_URL}");
        download_ort_runtime(&dir, progress)?;
        if progress.cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
    }

    // 2. Download MiDaS model if missing (~66MB)
    let model = model_path();
    if !model.is_file() {
        log::info!("Downloading MiDaS model from {MODEL_URL}");
        download_file(MODEL_URL, &model, MODEL_FILENAME, progress)?;
    }

    progress.progress.store(101, Ordering::Relaxed);
    Ok(())
}

/// Download and extract ONNX Runtime shared library from official release archive.
fn download_ort_runtime(dir: &std::path::Path, progress: &DownloadProgress) -> Result<()> {
    let is_zip = std::path::Path::new(ORT_LIB_URL)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
    let ext = if is_zip { "zip" } else { "tgz" };
    let archive_path = dir.join(format!("ort_runtime.{ext}"));

    download_file(ORT_LIB_URL, &archive_path, "ONNX Runtime", progress)?;

    let target_path = dir.join(ORT_LIB_FILENAME);
    let extracted = if is_zip {
        extract_from_zip(&archive_path, &target_path)?
    } else {
        extract_from_tgz(&archive_path, &target_path)?
    };

    let _ = std::fs::remove_file(&archive_path);

    if !extracted || !target_path.is_file() {
        anyhow::bail!("Failed to extract {} from archive", ORT_LIB_FILENAME);
    }

    Ok(())
}

/// Extract ONNX Runtime dylib from a .tgz archive (Linux/macOS).
/// Skips symlinks (0-byte entries) and grabs the real versioned file.
fn extract_from_tgz(archive_path: &std::path::Path, target_path: &std::path::Path) -> Result<bool> {
    let file = std::fs::File::open(archive_path)?;
    let decompressed = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decompressed);

    let prefix = ORT_LIB_FILENAME;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_size = entry.header().size().unwrap_or(0);
        let path = entry.path()?.to_path_buf();
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        // Match versioned lib (e.g. libonnxruntime.so.1.23.0) — must have real content
        if file_name.starts_with(prefix) && entry_size > 1024 {
            let mut out = std::fs::File::create(target_path)?;
            std::io::copy(&mut entry, &mut out)?;
            log::info!(
                "Extracted {} → {} ({:.1} MB)",
                file_name,
                ORT_LIB_FILENAME,
                entry_size as f64 / 1_048_576.0
            );
            return Ok(true);
        }
    }

    Ok(false)
}

/// Extract ONNX Runtime dll from a .zip archive (Windows).
fn extract_from_zip(archive_path: &std::path::Path, target_path: &std::path::Path) -> Result<bool> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let file_name = entry.name().rsplit('/').next().unwrap_or("").to_string();

        if file_name == ORT_LIB_FILENAME && entry.size() > 1024 {
            let mut out = std::fs::File::create(target_path)?;
            std::io::copy(&mut entry, &mut out)?;
            log::info!(
                "Extracted {} ({:.1} MB)",
                ORT_LIB_FILENAME,
                entry.size() as f64 / 1_048_576.0
            );
            return Ok(true);
        }
    }

    Ok(false)
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
