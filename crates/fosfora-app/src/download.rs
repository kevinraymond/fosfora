//! Shared background-download infrastructure: progress tracking + streamed
//! single-file download (.tmp → SHA-256 check → rename, cancel, 64 KB chunks).
//!
//! Extracted from `depth::model` (which keeps its archive-extraction logic
//! behind the `depth` feature) so the Splat demo-scene download (#1800) works
//! in default builds. Progress convention: 0–100 = percent, 101 = complete,
//! 102 = error (message in `error_message`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use anyhow::Result;
use sha2::{Digest, Sha256};

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

/// Lowercase hex SHA-256 of a finished hasher.
pub(crate) fn hex_digest(hasher: Sha256) -> String {
    use std::fmt::Write;
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Whether the file at `path` exists and hashes to `sha256` (lowercase hex).
/// Used to skip a download only when the cached file is the pinned one, so a
/// truncated or swapped file is fetched again instead of reused forever.
pub fn file_matches(path: &std::path::Path, sha256: &str) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut hasher = Sha256::new();
    if std::io::copy(&mut file, &mut hasher).is_err() {
        return false;
    }
    hex_digest(hasher) == sha256
}

/// Download a single file with progress tracking: streams to `<final>.tmp`,
/// checks the pinned SHA-256, then renames, so an interrupted, truncated or
/// tampered download never becomes the final file.
pub fn download_file(
    url: &str,
    final_path: &std::path::Path,
    name: &str,
    sha256: &str,
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
    let mut hasher = Sha256::new();
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
        hasher.update(&buf[..n]);
        downloaded += n as u64;

        if content_length > 0 {
            let pct = ((downloaded as f64 / content_length as f64) * 100.0).min(100.0) as u8;
            progress.progress.store(pct, Ordering::Relaxed);
        }
    }

    drop(file);
    let actual = hex_digest(hasher);
    if actual != sha256 {
        let _ = std::fs::remove_file(&tmp_path);
        anyhow::bail!(
            "{name} failed its integrity check ({downloaded} bytes, SHA-256 {actual}, expected {sha256})"
        );
    }
    std::fs::rename(&tmp_path, final_path)?;
    log::info!(
        "Downloaded {} ({:.1} MB)",
        name,
        downloaded as f64 / 1_048_576.0
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_digest_matches_known_vector() {
        let mut h = Sha256::new();
        h.update(b"abc");
        assert_eq!(
            hex_digest(h),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn file_matches_only_the_pinned_content() {
        let dir = std::env::temp_dir().join(format!("fosfora-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.bin");
        std::fs::write(&path, b"abc").unwrap();
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(file_matches(&path, abc));
        // Truncated: the same file cut short no longer matches.
        std::fs::write(&path, b"ab").unwrap();
        assert!(!file_matches(&path, abc));
        assert!(!file_matches(&dir.join("missing"), abc));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
