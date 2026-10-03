use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam_channel::Receiver;

use super::webcam::WebcamFrame;

/// FFmpeg-based webcam capture for DirectShow/virtual cameras.
pub struct FfmpegCapture {
    frame_rx: Receiver<WebcamFrame>,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The running ffmpeg, kept so stopping can end it: the capture thread
    /// sits in a read that only returns when ffmpeg writes or exits.
    child: Arc<Mutex<Option<Child>>>,
    pub device_name: String,
    pub resolution: (u32, u32),
}

/// What a camera is opened with: frame size and frame rate.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CaptureMode {
    size: (u32, u32),
    fps: f64,
}

impl CaptureMode {
    fn size_arg(&self) -> String {
        format!("{}x{}", self.size.0, self.size.1)
    }

    fn fps_arg(&self) -> String {
        format!("{}", self.fps)
    }
}

/// Check if ffmpeg is available on PATH.
pub fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// List webcam devices. Returns Vec of (index, device_identifier).
///
/// On Linux: scans `/dev/video*` via sysfs (reliable, no ffmpeg parsing).
/// On Windows: parses `ffmpeg -f dshow -list_devices`.
/// On macOS: parses `ffmpeg -f avfoundation -list_devices`.
///
/// The device_identifier is what ffmpeg expects as input:
/// - Linux: `/dev/video0`
/// - Windows: `Integrated Camera` (DirectShow name)
/// - macOS: `0` (avfoundation index)
#[allow(clippy::unnecessary_wraps)] // Returns Result for cross-platform API; Linux path is infallible
pub fn list_devices() -> Result<Vec<(u32, String)>, String> {
    #[cfg(target_os = "linux")]
    {
        Ok(list_devices_linux())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let (format_flag, input_arg) = platform_capture_args();
        let output = Command::new("ffmpeg")
            .args(["-f", format_flag, "-list_devices", "true", "-i", input_arg])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("Failed to run ffmpeg: {e}"))?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        parse_device_list(&stderr)
    }
}

/// Linux: scan /sys/class/video4linux to find capture-capable devices.
/// Deduplicates by card name (keeps lowest-numbered device per card).
#[cfg(target_os = "linux")]
fn list_devices_linux() -> Vec<(u32, String)> {
    let sysfs = std::path::Path::new("/sys/class/video4linux");
    if !sysfs.exists() {
        return Vec::new();
    }
    let mut entries: Vec<(u32, String, String)> = Vec::new(); // (dev_num, path, card_name)
    if let Ok(dir) = std::fs::read_dir(sysfs) {
        for entry in dir.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with("video") {
                continue;
            }
            let dev_num: u32 = name_str
                .trim_start_matches("video")
                .parse()
                .unwrap_or(u32::MAX);
            let dev_path = format!("/dev/{name_str}");
            let card_name = std::fs::read_to_string(entry.path().join("name"))
                .unwrap_or_default()
                .trim()
                .to_string();
            if card_name.is_empty() {
                continue;
            }
            entries.push((dev_num, dev_path, card_name));
        }
    }
    entries.sort_by_key(|(num, _, _)| *num);

    // Deduplicate by card name (keep lowest device number per card)
    let mut seen = std::collections::HashMap::<String, usize>::new();
    let mut result: Vec<(u32, String)> = Vec::new();
    for (_, dev_path, card_name) in &entries {
        if !seen.contains_key(card_name) {
            let idx = result.len() as u32;
            seen.insert(card_name.clone(), result.len());
            result.push((idx, dev_path.clone()));
        }
    }
    result
}

impl FfmpegCapture {
    /// Start capturing from the given device name at the requested resolution.
    pub fn start(device_name: &str, resolution: Option<(u32, u32)>) -> Result<Self, String> {
        if !ffmpeg_available() {
            return Err(
                "FFmpeg not found. Install FFmpeg and ensure it is in your PATH.".to_string(),
            );
        }

        let res = resolution.unwrap_or((1280, 720));
        let (frame_tx, frame_rx) = crossbeam_channel::bounded(2);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        let name = device_name.to_string();
        let name_clone = name.clone();
        let child = Arc::new(Mutex::new(None));
        let child_clone = child.clone();

        // Find a mode the camera accepts by starting ffmpeg briefly
        let mode = probe_mode(&name, res)?;
        let actual_res = mode.size;

        let handle = std::thread::Builder::new()
            .name("ffmpeg-webcam".into())
            .spawn(move || {
                capture_thread(&name_clone, mode, frame_tx, shutdown_clone, child_clone);
            })
            .map_err(|e| format!("Failed to spawn ffmpeg capture thread: {e}"))?;

        log::info!(
            "FFmpeg webcam started: {}x{} on '{}'",
            actual_res.0,
            actual_res.1,
            name
        );

        Ok(Self {
            frame_rx,
            shutdown,
            thread: Some(handle),
            child,
            device_name: name,
            resolution: actual_res,
        })
    }

    /// Non-blocking read of the latest frame.
    pub fn try_recv_frame(&self) -> Option<WebcamFrame> {
        let mut latest = None;
        while let Ok(frame) = self.frame_rx.try_recv() {
            latest = Some(frame);
        }
        latest
    }

    /// Stop capture: end ffmpeg, which releases the camera and wakes the
    /// thread out of its read.
    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(mut child) = self.child.lock().ok().and_then(|mut c| c.take()) {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(handle) = self.thread.take() {
            if super::webcam::wait_finished(&handle, std::time::Duration::from_millis(500)) {
                let _ = handle.join();
            }
        }
    }

    /// Check if the capture thread is still alive.
    pub fn is_running(&self) -> bool {
        if self.shutdown.load(Ordering::Relaxed) {
            return false;
        }
        match &self.thread {
            Some(h) => !h.is_finished(),
            None => false,
        }
    }
}

impl Drop for FfmpegCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Returns (format_flag, dummy_input) for the current platform.
fn platform_capture_args() -> (&'static str, &'static str) {
    #[cfg(target_os = "windows")]
    {
        ("dshow", "dummy")
    }
    #[cfg(target_os = "linux")]
    {
        ("v4l2", "/dev/null")
    }
    #[cfg(target_os = "macos")]
    {
        ("avfoundation", "")
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        ("v4l2", "/dev/null")
    }
}

/// Build the ffmpeg input argument for the device name.
fn device_input_arg(device_name: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        format!("video={device_name}")
    }
    #[cfg(target_os = "macos")]
    {
        // avfoundation uses device name or index directly
        device_name.to_string()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        // Linux v4l2: device_name is typically /dev/video0
        device_name.to_string()
    }
}

/// Find a mode the camera accepts, starting from the requested resolution
/// at 30 fps.
///
/// A camera that refuses it says which modes it has (a virtual camera often
/// has exactly one, and not 1280x720); the nearest of those is tried next.
fn probe_mode(device_name: &str, requested: (u32, u32)) -> Result<CaptureMode, String> {
    let wanted = CaptureMode {
        size: requested,
        fps: 30.0,
    };
    let stderr = match probe_one_frame(device_name, wanted)? {
        Ok(()) => return Ok(wanted),
        Err(stderr) => stderr,
    };
    if let Some(mode) = nearest_mode(&parse_supported_modes(&stderr), wanted) {
        if mode != wanted {
            log::info!(
                "Camera '{device_name}' has no {} mode, trying {} at {} fps",
                wanted.size_arg(),
                mode.size_arg(),
                mode.fps_arg()
            );
            return match probe_one_frame(device_name, mode)? {
                Ok(()) => Ok(mode),
                Err(stderr) => Err(open_error(device_name, &stderr)),
            };
        }
    }
    if stderr.contains("Could not") || stderr.contains("Error") {
        Err(open_error(device_name, &stderr))
    } else {
        // Probe failed but not fatally — use requested resolution
        Ok(wanted)
    }
}

fn open_error(device_name: &str, stderr: &str) -> String {
    // The first line that is ffmpeg's own; macOS puts framework notices
    // ahead of it.
    let line = stderr
        .lines()
        .find(|l| l.starts_with('[') || l.contains("Error") || l.contains("Could not"))
        .or_else(|| stderr.lines().next())
        .unwrap_or_default();
    format!("FFmpeg could not open camera '{device_name}': {line}")
}

/// Read one frame from the camera in this mode. `Ok(Err(stderr))` when
/// ffmpeg ran but delivered no frame.
fn probe_one_frame(device_name: &str, mode: CaptureMode) -> Result<Result<(), String>, String> {
    let (format_flag, _) = platform_capture_args();
    let input = device_input_arg(device_name);

    let mut child = Command::new("ffmpeg")
        .args([
            "-f",
            format_flag,
            "-video_size",
            &mode.size_arg(),
            "-framerate",
            &mode.fps_arg(),
            "-i",
            &input,
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-loglevel",
            "error",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to probe camera with ffmpeg: {e}"))?;

    let expected_bytes = (mode.size.0 as usize) * (mode.size.1 as usize) * 4;
    let mut buf = vec![0u8; expected_bytes];
    let stdout = child.stdout.as_mut().ok_or("No stdout from ffmpeg")?;

    let read = read_exact_timeout(stdout, &mut buf, std::time::Duration::from_secs(10));
    let _ = child.kill();
    match read {
        Ok(()) => {
            let _ = child.wait();
            Ok(Ok(()))
        }
        Err(_) => Ok(Err(child
            .wait_with_output()
            .map(|o| String::from_utf8_lossy(&o.stderr).to_string())
            .unwrap_or_default())),
    }
}

/// The modes ffmpeg lists when a camera refuses the one asked for, as
/// (width, height, min fps, max fps). avfoundation prints them as
/// `1920x1080@[15.000000 30.000000]fps`.
fn parse_supported_modes(stderr: &str) -> Vec<(u32, u32, f64, f64)> {
    stderr
        .lines()
        .filter_map(|line| {
            let (size, rates) = line.split_once("@[")?;
            let size = size.rsplit(' ').next()?;
            let (w, h) = size.split_once('x')?;
            let (min, max) = rates.split_once(']')?.0.split_once(' ')?;
            Some((
                w.parse().ok()?,
                h.parse().ok()?,
                min.parse().ok()?,
                max.parse().ok()?,
            ))
        })
        .collect()
}

/// The listed mode nearest the wanted size, at the frame rate nearest the
/// wanted one that the mode allows.
fn nearest_mode(modes: &[(u32, u32, f64, f64)], wanted: CaptureMode) -> Option<CaptureMode> {
    let distance = |&&(w, h, _, _): &&(u32, u32, f64, f64)| {
        let dx = i64::from(w) - i64::from(wanted.size.0);
        let dy = i64::from(h) - i64::from(wanted.size.1);
        dx * dx + dy * dy
    };
    modes
        .iter()
        .min_by_key(distance)
        .map(|&(w, h, min, max)| CaptureMode {
            size: (w, h),
            fps: wanted.fps.clamp(min.min(max), max),
        })
}

fn read_exact_timeout(
    reader: &mut dyn Read,
    buf: &mut [u8],
    timeout: std::time::Duration,
) -> Result<(), String> {
    let start = std::time::Instant::now();
    let mut filled = 0;
    while filled < buf.len() {
        if start.elapsed() > timeout {
            return Err("Timeout reading from ffmpeg".into());
        }
        match reader.read(&mut buf[filled..]) {
            Ok(0) => return Err("EOF from ffmpeg".into()),
            Ok(n) => filled += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("Read error: {e}")),
        }
    }
    Ok(())
}

#[allow(clippy::needless_pass_by_value)] // owned by the thread it runs on
fn capture_thread(
    device_name: &str,
    mode: CaptureMode,
    frame_tx: crossbeam_channel::Sender<WebcamFrame>,
    shutdown: Arc<AtomicBool>,
    child_slot: Arc<Mutex<Option<Child>>>,
) {
    let (format_flag, _) = platform_capture_args();
    let input = device_input_arg(device_name);
    let resolution = mode.size;

    let mut child = match spawn_ffmpeg(format_flag, &input, mode) {
        Ok(c) => c,
        Err(e) => {
            log::error!("Failed to start ffmpeg capture: {e}");
            return;
        }
    };

    let frame_bytes = (resolution.0 as usize) * (resolution.1 as usize) * 4;
    let mut buf = vec![0u8; frame_bytes];
    let mut stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            log::error!("No stdout from ffmpeg process");
            let _ = child.kill();
            return;
        }
    };
    // Handed over so `stop` can end it while this thread is in a read.
    let end_child = || {
        if let Some(mut child) = child_slot.lock().ok().and_then(|mut c| c.take()) {
            let _ = child.kill();
            let _ = child.wait();
        }
    };
    if let Ok(mut slot) = child_slot.lock() {
        *slot = Some(child);
    }
    if shutdown.load(Ordering::Relaxed) {
        end_child();
        return;
    }

    log::info!(
        "FFmpeg capture thread started: {}x{} on '{}'",
        resolution.0,
        resolution.1,
        device_name
    );

    while !shutdown.load(Ordering::Relaxed) {
        let mut filled = 0;
        let mut failed = false;
        while filled < frame_bytes {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }
            match stdout.read(&mut buf[filled..]) {
                Ok(0) => {
                    if !shutdown.load(Ordering::Relaxed) {
                        log::warn!("FFmpeg process closed stdout (EOF)");
                    }
                    failed = true;
                    break;
                }
                Ok(n) => filled += n,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    log::warn!("FFmpeg read error: {e}");
                    failed = true;
                    break;
                }
            }
        }

        if failed || shutdown.load(Ordering::Relaxed) {
            break;
        }

        let frame = WebcamFrame {
            data: buf.clone(),
            width: resolution.0,
            height: resolution.1,
        };
        let _ = frame_tx.try_send(frame);
    }

    end_child();
    log::info!("FFmpeg capture thread stopped");
}

fn spawn_ffmpeg(format_flag: &str, input: &str, mode: CaptureMode) -> Result<Child, String> {
    Command::new("ffmpeg")
        .args([
            "-f",
            format_flag,
            "-video_size",
            &mode.size_arg(),
            "-framerate",
            &mode.fps_arg(),
            "-i",
            input,
            // One frame per captured frame: left alone, ffmpeg repeats
            // frames up to the device's nominal clock rate.
            "-r",
            &mode.fps_arg(),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-loglevel",
            "error",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn ffmpeg: {e}"))
}

/// Parse ffmpeg device list output. Platform-specific parsing.
// `Result` is kept for parity with `list_devices()` (which can fail to spawn
// ffmpeg); this parser itself never errors.
#[cfg(not(target_os = "linux"))]
#[allow(clippy::unnecessary_wraps)]
fn parse_device_list(stderr: &str) -> Result<Vec<(u32, String)>, String> {
    let mut devices = Vec::new();

    #[cfg(target_os = "windows")]
    {
        let mut idx = 0u32;
        // Parse dshow output: lines like [dshow @ ...] "Device Name" (video)
        for line in stderr.lines() {
            if line.contains("(video)") {
                if let Some(start) = line.find('"') {
                    if let Some(end) = line[start + 1..].find('"') {
                        let name = line[start + 1..start + 1 + end].to_string();
                        devices.push((idx, name));
                        idx += 1;
                    }
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        // Parse avfoundation output: [AVFoundation ...] [0] Device Name
        for line in stderr.lines() {
            // The audio devices follow the video ones, numbered from 0 again.
            if line.contains("AVFoundation audio devices") {
                break;
            }
            if line.contains("AVFoundation") && line.contains("] [") {
                if let Some(bracket_start) = line.rfind("] [") {
                    let after = &line[bracket_start + 3..];
                    if let Some(bracket_end) = after.find(']') {
                        let idx_str = &after[..bracket_end];
                        if let Ok(dev_idx) = idx_str.parse::<u32>() {
                            let name = after[bracket_end + 1..].trim().to_string();
                            if !name.is_empty() {
                                devices.push((dev_idx, name));
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ffmpeg_available_check() {
        // Just ensure it doesn't panic; result depends on system
        let _ = ffmpeg_available();
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn test_parse_dshow_devices() {
        let stderr = r#"[dshow @ 00000001] "Integrated Camera" (video)
[dshow @ 00000001]   Alternative name "@device_pnp_..."
[dshow @ 00000001] "Irium Webcam" (video)
[dshow @ 00000001]   Alternative name "@device_sw_..."
[dshow @ 00000001] "Microphone Array" (audio)
"#;
        let devices = parse_device_list(stderr).unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0], (0, "Integrated Camera".to_string()));
        assert_eq!(devices[1], (1, "Irium Webcam".to_string()));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn avfoundation_list_stops_at_the_audio_devices() {
        let stderr = "[AVFoundation indev @ 0x1] AVFoundation video devices:\n\
[AVFoundation indev @ 0x1] [0] RTMP Virtual Camera\n\
[AVFoundation indev @ 0x1] [1] FaceTime HD-Kamera\n\
[AVFoundation indev @ 0x1] AVFoundation audio devices:\n\
[AVFoundation indev @ 0x1] [0] MacBook Pro-Mikrofon\n";
        assert_eq!(
            parse_device_list(stderr).unwrap(),
            vec![
                (0, "RTMP Virtual Camera".to_string()),
                (1, "FaceTime HD-Kamera".to_string()),
            ]
        );
    }

    /// A virtual camera with one mode refused the fixed 1280x720 request
    /// and never opened.
    #[test]
    fn falls_back_to_a_mode_the_camera_lists() {
        let stderr = "[in#0 @ 0x1] Selected video size (1280x720) is not supported by the device.\n\
[in#0 @ 0x1] Supported modes:\n\
[in#0 @ 0x1]   1920x1080@[30.000000 30.000000]fps\n\
[in#0 @ 0x1]   640x480@[15.000000 60.000000]fps\n\
Error opening input: Input/output error\n";
        let modes = parse_supported_modes(stderr);
        assert_eq!(
            modes,
            vec![(1920, 1080, 30.0, 30.0), (640, 480, 15.0, 60.0)]
        );
        let wanted = CaptureMode {
            size: (1280, 720),
            fps: 30.0,
        };
        // The one mode a virtual camera has, whatever its size.
        assert_eq!(
            nearest_mode(&modes[..1], wanted),
            Some(CaptureMode {
                size: (1920, 1080),
                fps: 30.0
            })
        );
        assert_eq!(
            nearest_mode(&modes, wanted),
            Some(CaptureMode {
                size: (640, 480),
                fps: 30.0
            })
        );
        // A mode that cannot do 30 fps runs as fast as it can.
        assert_eq!(
            nearest_mode(&[(1280, 720, 5.0, 24.0)], wanted),
            Some(CaptureMode {
                size: (1280, 720),
                fps: 24.0
            })
        );
        assert_eq!(nearest_mode(&[], wanted), None);
        assert_eq!(wanted.size_arg(), "1280x720");
        assert_eq!(wanted.fps_arg(), "30");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_list_devices_linux() {
        // Should not panic; result depends on system
        let result = list_devices();
        assert!(result.is_ok());
    }
}
