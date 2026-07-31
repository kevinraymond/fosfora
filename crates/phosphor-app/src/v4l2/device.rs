//! Loopback device discovery and output-format negotiation.

use v4l::capability::Flags;
use v4l::format::Colorspace;
use v4l::video::Output;
use v4l::{Format, FourCC};

/// A v4l2loopback device found on the system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopbackDevice {
    /// Device node, e.g. "/dev/video10".
    pub path: String,
    /// Card label, e.g. "OBS Virtual Camera".
    pub name: String,
}

/// Scan sysfs for v4l2loopback devices. A node is a loopback iff its sysfs entry
/// canonicalizes under /sys/devices/virtual/ (v4l2loopback registers a virtual
/// platform device). Opens no device nodes — real cameras are never woken.
pub fn enumerate_loopback_devices() -> Vec<LoopbackDevice> {
    let mut devices = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/video4linux") else {
        return devices;
    };
    for entry in entries.flatten() {
        let node = entry.file_name();
        let Some(node) = node.to_str() else { continue };
        if !node.starts_with("video") {
            continue;
        }
        let Ok(real) = std::fs::canonicalize(entry.path()) else {
            continue;
        };
        if !real.starts_with("/sys/devices/virtual/") {
            continue;
        }
        let name = std::fs::read_to_string(entry.path().join("name"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        devices.push(LoopbackDevice {
            path: format!("/dev/{node}"),
            name,
        });
    }
    // Sort by node number so "first loopback" is deterministic.
    devices.sort_by_key(|d| {
        d.path
            .trim_start_matches("/dev/video")
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });
    devices
}

/// Open a loopback device and negotiate the output format.
/// Errors are user-facing strings shown in the panel.
pub fn open_output(
    path: &str,
    width: u32,
    height: u32,
    fourcc: &[u8; 4],
) -> Result<v4l::Device, String> {
    let dev = v4l::Device::with_path(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!("{path}: permission denied — add your user to the 'video' group?")
        } else {
            format!("Failed to open {path}: {e}")
        }
    })?;

    let caps = dev
        .query_caps()
        .map_err(|e| format!("{path}: QUERYCAP failed: {e}"))?;
    if !caps.capabilities.contains(Flags::VIDEO_OUTPUT) {
        return Err(format!(
            "{path} is not a video output device — is v4l2loopback loaded?"
        ));
    }

    let mut fmt = Format::new(width, height, FourCC::new(fourcc));
    // Tag what the bytes actually are: BT.601/SMPTE170M is what camera
    // consumers assume for YUYV.
    fmt.colorspace = Colorspace::SMPTE170M;
    let actual =
        Output::set_format(&dev, &fmt).map_err(|e| format!("{path}: S_FMT failed: {e}"))?;

    if actual.width != width || actual.height != height {
        return Err(format!(
            "{path} negotiated {}x{} instead of {width}x{height}",
            actual.width, actual.height
        ));
    }
    if actual.fourcc != FourCC::new(fourcc) {
        return Err(format!(
            "{path} refused format {} (driver gave {})",
            FourCC::new(fourcc),
            actual.fourcc
        ));
    }

    log::info!(
        "v4l2 output negotiated: {path} {width}x{height} {}",
        actual.fourcc
    );
    Ok(dev)
}
