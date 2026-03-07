use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Receiver, Sender};
use nokhwa::Camera;
use nokhwa::pixel_format::RgbAFormat;
use nokhwa::utils::{
    ApiBackend, CameraIndex, CameraInfo, RequestedFormat, RequestedFormatType, Resolution,
};

/// A single decoded webcam frame (RGBA).
pub struct WebcamFrame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Cross-platform webcam capture running on a dedicated thread.
pub struct WebcamCapture {
    frame_rx: Receiver<WebcamFrame>,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub device_name: String,
    pub resolution: (u32, u32),
}

fn requested_format(resolution: Option<(u32, u32)>) -> RequestedFormat<'static> {
    match resolution {
        Some((w, h)) => RequestedFormat::new::<RgbAFormat>(RequestedFormatType::Closest(
            nokhwa::utils::CameraFormat::new(
                Resolution::new(w, h),
                nokhwa::utils::FrameFormat::MJPEG,
                30,
            ),
        )),
        None => RequestedFormat::new::<RgbAFormat>(RequestedFormatType::AbsoluteHighestResolution),
    }
}

/// Try to open a camera with MJPEG preference, falling back to any supported format.
/// Many Windows webcams only support raw formats (YUYV/NV12), not MJPEG.
fn open_camera_with_fallback(
    device_index: u32,
    resolution: Option<(u32, u32)>,
) -> Result<Camera, String> {
    let try_open = |fmt| -> Result<Camera, String> {
        let mut c =
            Camera::new(CameraIndex::Index(device_index), fmt).map_err(|e| e.to_string())?;
        c.open_stream().map_err(|e| e.to_string())?;
        Ok(c)
    };

    try_open(requested_format(resolution)).or_else(|first_err| {
        log::warn!("Preferred webcam format failed ({first_err}), trying fallback...");
        try_open(RequestedFormat::new::<RgbAFormat>(
            RequestedFormatType::AbsoluteHighestResolution,
        ))
        .map_err(|_| camera_error_message(device_index, &first_err))
    })
}

impl WebcamCapture {
    /// Start capturing from the given camera index at the requested resolution.
    /// Validates the camera can be opened before spawning the capture thread.
    pub fn start(device_index: u32, resolution: Option<(u32, u32)>) -> Result<Self, String> {
        let (frame_tx, frame_rx) = crossbeam_channel::bounded(2);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        // Probe device name on main thread first
        let devices = list_devices().unwrap_or_default();
        let device_name = devices
            .iter()
            .find(|(idx, _)| *idx == device_index)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| format!("Camera {device_index}"));

        // Validate camera access on calling thread (Camera is !Send so we can't move it).
        // Open, check it works, then close so the capture thread can reopen it.
        let actual_res = {
            let mut camera = open_camera_with_fallback(device_index, resolution)?;
            let r = camera.resolution();
            let res = (r.width(), r.height());
            let _ = camera.stop_stream();
            drop(camera);
            res
        };

        log::info!(
            "Webcam validated: {}x{} on device {device_index}",
            actual_res.0,
            actual_res.1
        );

        let handle = std::thread::Builder::new()
            .name("webcam-capture".into())
            .spawn(move || {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    capture_thread(device_index, resolution, frame_tx, shutdown_clone);
                })) {
                    Ok(()) => {}
                    Err(e) => {
                        let msg = if let Some(s) = e.downcast_ref::<&str>() {
                            s.to_string()
                        } else if let Some(s) = e.downcast_ref::<String>() {
                            s.clone()
                        } else {
                            "unknown panic".into()
                        };
                        log::error!("Webcam capture thread panicked: {msg}");
                    }
                }
            })
            .map_err(|e| format!("Failed to spawn webcam thread: {e}"))?;

        Ok(Self {
            frame_rx,
            shutdown,
            thread: Some(handle),
            device_name,
            resolution: actual_res,
        })
    }

    /// Non-blocking read of the latest frame.
    pub fn try_recv_frame(&self) -> Option<WebcamFrame> {
        // Drain to get the latest frame (drop old ones)
        let mut latest = None;
        while let Ok(frame) = self.frame_rx.try_recv() {
            latest = Some(frame);
        }
        latest
    }

    /// Stop capture and join the thread.
    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }

    /// Check if the capture thread is still alive.
    pub fn is_running(&self) -> bool {
        if self.shutdown.load(Ordering::Relaxed) {
            return false;
        }
        // Detect if the thread exited unexpectedly
        match &self.thread {
            Some(h) => !h.is_finished(),
            None => false,
        }
    }
}

impl Drop for WebcamCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// List available webcam devices. Returns Vec of (index, human_name).
///
/// Deduplicates by name (keeps lowest index per name) since Linux V4L2 exposes
/// multiple device nodes per physical camera (main, metadata, IR, etc.).
pub fn list_devices() -> Result<Vec<(u32, String)>, String> {
    let cameras =
        nokhwa::query(ApiBackend::Auto).map_err(|e| format!("Failed to query cameras: {e}"))?;
    let mut seen = std::collections::HashMap::<String, u32>::new();
    for info in &cameras {
        let idx = match info.index() {
            CameraIndex::Index(i) => *i,
            CameraIndex::String(_) => 0,
        };
        let name = info.human_name().to_string();
        seen.entry(name)
            .and_modify(|existing| *existing = (*existing).min(idx))
            .or_insert(idx);
    }
    let mut result: Vec<(u32, String)> = seen.into_iter().map(|(name, idx)| (idx, name)).collect();
    result.sort_by_key(|(idx, _)| *idx);
    Ok(result)
}

/// Check if any webcam is available. Cached via OnceLock.
pub fn webcam_available() -> bool {
    use std::sync::OnceLock;
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| list_devices().map_or(false, |d| !d.is_empty()))
}

/// Format a user-friendly camera error message.
fn camera_error_message(device_index: u32, err: &str) -> String {
    if err.contains("Device or resource busy") {
        format!(
            "Camera {device_index} is in use by another application. \
             If OBS is running, right-click the webcam source and Deactivate it to release the device."
        )
    } else {
        format!("Failed to open camera {device_index}: {err}")
    }
}

fn capture_thread(
    device_index: u32,
    resolution: Option<(u32, u32)>,
    frame_tx: Sender<WebcamFrame>,
    shutdown: Arc<AtomicBool>,
) {
    let mut camera = match open_camera_with_fallback(device_index, resolution) {
        Ok(c) => c,
        Err(e) => {
            log::error!("{e}");
            return;
        }
    };

    let res = camera.resolution();
    log::info!(
        "Webcam capture started: {}x{} on device {device_index}",
        res.width(),
        res.height()
    );

    let mut consecutive_panics: u32 = 0;
    const MAX_CONSECUTIVE_PANICS: u32 = 10;

    while !shutdown.load(Ordering::Relaxed) {
        match camera.frame() {
            Ok(buffer) => {
                let res = buffer.resolution();
                // decode_image can panic on corrupted MJPEG frames (libjpeg fatal error).
                // Catch the panic so one bad frame doesn't kill the capture thread.
                let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    buffer.decode_image::<RgbAFormat>()
                }));
                match decoded {
                    Ok(Ok(img)) => {
                        consecutive_panics = 0;
                        let frame = WebcamFrame {
                            data: img.into_raw(),
                            width: res.width(),
                            height: res.height(),
                        };
                        // try_send: drop frame if consumer is behind
                        let _ = frame_tx.try_send(frame);
                    }
                    Ok(Err(e)) => {
                        log::warn!("Failed to decode webcam frame: {e}");
                    }
                    Err(_) => {
                        consecutive_panics += 1;
                        log::warn!(
                            "Skipped corrupted webcam frame (decode panic, {consecutive_panics}/{MAX_CONSECUTIVE_PANICS})"
                        );
                        if consecutive_panics >= MAX_CONSECUTIVE_PANICS {
                            log::error!(
                                "Webcam producing only corrupted frames — stopping capture thread"
                            );
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                }
            }
            Err(e) => {
                if !shutdown.load(Ordering::Relaxed) {
                    log::warn!("Webcam frame error: {e}");
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
    }

    let _ = camera.stop_stream();
    log::info!("Webcam capture stopped");
}
