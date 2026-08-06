//! Shared background-download infrastructure: progress tracking + streamed
//! single-file download (.tmp → rename, cancel, 64 KB chunks).
//!
//! Extracted from `depth::model` (which keeps its archive-extraction logic
//! behind the `depth` feature) so the Splat demo-scene download (#1800) works
//! in default builds. Progress convention: 0–100 = percent, 101 = complete,
//! 102 = error (message in `error_message`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use anyhow::Result;

/// Progress of a background download (0–100), or special states.
/// Shared between download thread and UI.
pub struct DownloadProgress {
    /// 0-100 for percentage, 101 = complete, 102 = error
    pub progress: AtomicU8,
    pub cancel: AtomicBool,
    pub error_message: std::sync::Mutex<Option<String>>,
}

impl DownloadProgress {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            progress: AtomicU8::new(0),
            cancel: AtomicBool::new(false),
            error_message: std::sync::Mutex::new(None),
        })
    }

    pub fn percent(&self) -> u8 {
        self.progress.load(Ordering::Relaxed)
    }

    pub fn is_complete(&self) -> bool {
        self.progress.load(Ordering::Relaxed) == 101
    }

    pub fn is_error(&self) -> bool {
        self.progress.load(Ordering::Relaxed) == 102
    }

    pub fn is_downloading(&self) -> bool {
        let p = self.progress.load(Ordering::Relaxed);
        p <= 100
    }
}

/// Download a single file with progress tracking: streams to `<final>.tmp`
/// then renames, so an interrupted download never leaves a bad final file.
pub fn download_file(
    url: &str,
    final_path: &std::path::Path,
    name: &str,
    progress: &DownloadProgress,
) -> Result<()> {
    let tmp_path = final_path.with_extension("tmp");

    let response = ureq::get(url).call()?;

    let content_length = response
        .headers()
        .get("Content-Length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    let mut reader = response.into_body().into_reader();
    let mut file = std::fs::File::create(&tmp_path)?;
    let mut downloaded: u64 = 0;
    let mut buf = vec![0u8; 64 * 1024];

    loop {
        if progress.cancel.load(Ordering::Relaxed) {
            let _ = std::fs::remove_file(&tmp_path);
            anyhow::bail!("Download cancelled");
        }

        let n = std::io::Read::read(&mut reader, &mut buf)?;
        if n == 0 {
            break;
        }

        std::io::Write::write_all(&mut file, &buf[..n])?;
        downloaded += n as u64;

        if content_length > 0 {
            let pct = ((downloaded as f64 / content_length as f64) * 100.0).min(100.0) as u8;
            progress.progress.store(pct, Ordering::Relaxed);
        }
    }

    drop(file);
    std::fs::rename(&tmp_path, final_path)?;
    log::info!(
        "Downloaded {} ({:.1} MB)",
        name,
        downloaded as f64 / 1_048_576.0
    );
    Ok(())
}
