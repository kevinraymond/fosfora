//! Resolve ffmpeg/ffprobe to absolute paths.
//!
//! Finder-launched macOS apps often have a PATH that does not include Homebrew.
//! Always spawn the resolved absolute binary rather than a bare `"ffmpeg"` name.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// Cached toolchain for this process (PATH/config do not change mid-show).
static RESOLVED: OnceLock<VideoToolchain> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoToolchain {
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
    pub ffmpeg_version: Option<String>,
}

impl VideoToolchain {
    /// Configured path → PATH → `/opt/homebrew/bin` → `/usr/local/bin`.
    pub fn resolve(configured: Option<&Path>) -> Self {
        let ffmpeg = configured
            .filter(|p| p.is_file())
            .map(Path::to_path_buf)
            .or_else(|| find_binary("ffmpeg"));
        let ffprobe = ffmpeg
            .as_ref()
            .and_then(|ff| sibling_or_search(ff, "ffprobe"))
            .or_else(|| find_binary("ffprobe"));
        let ffmpeg_version = ffmpeg.as_ref().and_then(|p| version_line(p));
        Self {
            ffmpeg,
            ffprobe,
            ffmpeg_version,
        }
    }

    pub fn global(configured: Option<&Path>) -> &'static Self {
        RESOLVED.get_or_init(|| Self::resolve(configured))
    }

    pub fn ffmpeg_cmd(&self) -> Option<Command> {
        self.ffmpeg.as_ref().map(Command::new)
    }

    pub fn ffprobe_cmd(&self) -> Option<Command> {
        self.ffprobe.as_ref().map(Command::new)
    }

    pub fn available(&self) -> bool {
        self.ffmpeg.is_some() && self.ffprobe.is_some()
    }

    pub fn status_line(&self) -> String {
        match (&self.ffmpeg, &self.ffmpeg_version) {
            (Some(path), Some(ver)) => format!("{} ({ver})", path.display()),
            (Some(path), None) => path.display().to_string(),
            _ => "ffmpeg not found".to_string(),
        }
    }
}

fn sibling_or_search(ffmpeg: &Path, name: &str) -> Option<PathBuf> {
    if let Some(dir) = ffmpeg.parent() {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    find_binary(name)
}

fn find_binary(name: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    for dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
        let candidate = PathBuf::from(dir).join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn version_line(binary: &Path) -> Option<String> {
    let out = Command::new(binary)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prefers_configured_existing_path() {
        let ffmpeg = find_binary("ffmpeg");
        let Some(path) = ffmpeg else {
            return;
        };
        let tool = VideoToolchain::resolve(Some(&path));
        assert_eq!(tool.ffmpeg.as_deref(), Some(path.as_path()));
        assert!(tool.available());
        assert!(tool.status_line().contains(&path.display().to_string()));
    }

    #[test]
    fn resolve_ignores_missing_configured_path() {
        let missing = PathBuf::from("/definitely/not/a/real/ffmpeg-binary");
        let tool = VideoToolchain::resolve(Some(&missing));
        if let Some(found) = &tool.ffmpeg {
            assert_ne!(found, &missing);
        }
    }

    #[test]
    fn homebrew_fallback_is_searched() {
        let brew = PathBuf::from("/opt/homebrew/bin/ffmpeg");
        if brew.is_file() {
            let tool = VideoToolchain::resolve(None);
            assert!(tool.ffmpeg.is_some());
        }
    }
}
