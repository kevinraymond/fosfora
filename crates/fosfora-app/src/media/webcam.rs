use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Receiver, Sender};
use nokhwa::Camera;
use nokhwa::pixel_format::RgbAFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
    Resolution,
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

/// How long a camera gets to open before the caller gives up on it.
const OPEN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// How long stopping waits for the capture thread to release the device.
const STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

/// The format closest to the wanted resolution, then closest to 30 fps,
/// then compressed before raw (raw at this size is often frame-rate limited
/// by USB bandwidth).
fn pick_format(formats: &[CameraFormat], (w, h): (u32, u32)) -> Option<CameraFormat> {
    formats.iter().copied().min_by_key(|f| {
        let dx = i64::from(f.resolution().width()) - i64::from(w);
        let dy = i64::from(f.resolution().height()) - i64::from(h);
        let raw = f.format() != FrameFormat::MJPEG;
        (dx * dx + dy * dy, f.frame_rate().abs_diff(30), raw)
    })
}

/// Open a camera at the format it offers nearest the requested resolution.
///
/// Chosen here from the camera's own list: asking the library for the
/// closest format fails unless the camera has that exact resolution in that
/// exact pixel layout, which left cameras without it (most virtual cameras,
/// and every camera on macOS) running at their largest size instead.
fn open_camera_with_fallback(
    device_name: &str,
    index: &CameraIndex,
    resolution: Option<(u32, u32)>,
) -> Result<Camera, String> {
    let new_camera = |kind| {
        Camera::new(index.clone(), RequestedFormat::new::<RgbAFormat>(kind))
            .map_err(|e| camera_error_message(device_name, &e.to_string()))
    };

    let mut camera = new_camera(RequestedFormatType::AbsoluteHighestResolution)?;
    let nearest = resolution.and_then(|want| {
        let formats = camera.compatible_camera_formats().unwrap_or_default();
        let format = if formats.is_empty() {
            // AVFoundation lists none; the wanted size is simply tried, in
            // the pixel layout the camera is already in.
            Some(CameraFormat::new(
                Resolution::new(want.0, want.1),
                camera.frame_format(),
                30,
            ))
        } else {
            pick_format(&formats, want)
        };
        format.filter(|f| *f != camera.camera_format())
    });
    if let Some(format) = nearest {
        // Released before it is opened again; V4L2 allows one owner.
        drop(camera);
        camera = match new_camera(RequestedFormatType::Exact(format)) {
            Ok(c) => c,
            Err(e) => {
                log::info!("{e} (at {format}); using the camera's largest format");
                new_camera(RequestedFormatType::AbsoluteHighestResolution)?
            }
        };
    }
    camera
        .open_stream()
        .map_err(|e| camera_error_message(device_name, &e.to_string()))?;
    Ok(camera)
}

impl WebcamCapture {
    /// Start capturing from the camera with this name (as `list_devices`
    /// names it) at the requested resolution. Returns once the camera is
    /// open, or with the reason it could not be opened.
    pub fn start(device_name: &str, resolution: Option<(u32, u32)>) -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        ensure_camera_access()?;

        // Looked up now rather than by a remembered index: the OS renumbers
        // cameras whenever one is plugged in or a virtual camera starts.
        let index = enumerate()?
            .into_iter()
            .find(|d| d.name == device_name)
            .map(|d| d.open)
            .ok_or_else(|| format!("Camera '{device_name}' is not connected"))?;

        let (frame_tx, frame_rx) = crossbeam_channel::bounded(2);
        let (opened_tx, opened_rx) = crossbeam_channel::bounded(1);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        let name = device_name.to_string();

        // The camera is opened on the capture thread (it is !Send) and the
        // result reported back, so the device is opened only once.
        let handle = std::thread::Builder::new()
            .name("webcam-capture".into())
            .spawn(move || {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    capture_thread(
                        &name,
                        &index,
                        resolution,
                        &opened_tx,
                        &frame_tx,
                        &shutdown_clone,
                    );
                })) {
                    Ok(()) => {}
                    Err(e) => {
                        let msg = if let Some(s) = e.downcast_ref::<&str>() {
                            (*s).to_string()
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

        let resolution = match opened_rx.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(res)) => res,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                // Still opening (or it panicked): the thread closes the
                // camera as soon as it sees the flag.
                shutdown.store(true, Ordering::Relaxed);
                return Err(format!("Camera '{device_name}' did not respond"));
            }
        };

        Ok(Self {
            frame_rx,
            shutdown,
            thread: Some(handle),
            device_name: device_name.to_string(),
            resolution,
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

    /// Stop capture and release the camera.
    ///
    /// Waits only briefly for the thread: it sits in the driver until the
    /// next frame, and a camera that has stopped delivering them (a virtual
    /// camera whose source went away) would otherwise hang the app here.
    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            if wait_finished(&handle, STOP_TIMEOUT) {
                let _ = handle.join();
            } else {
                log::warn!(
                    "Camera '{}' is not delivering frames; leaving its capture thread to exit on its own",
                    self.device_name
                );
            }
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

/// Wait for a thread to finish, up to `timeout`. True if it did.
pub(super) fn wait_finished(
    handle: &std::thread::JoinHandle<()>,
    timeout: std::time::Duration,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while !handle.is_finished() {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    true
}

/// A camera as the OS lists it right now.
struct ListedCamera {
    index: u32,
    name: String,
    /// What the backend opens it by.
    open: CameraIndex,
}

fn enumerate() -> Result<Vec<ListedCamera>, String> {
    let cameras =
        nokhwa::query(ApiBackend::Auto).map_err(|e| format!("Failed to query cameras: {e}"))?;
    let found = cameras
        .iter()
        .enumerate()
        .map(|(position, info)| {
            let index = match info.index() {
                CameraIndex::Index(i) => *i,
                CameraIndex::String(_) => position as u32,
            };
            // On macOS the index is only a position in a list that is
            // queried again on open; the unique ID names the same camera
            // however that list is ordered by then.
            #[cfg(target_os = "macos")]
            let open = CameraIndex::String(info.misc());
            #[cfg(not(target_os = "macos"))]
            let open = info.index().clone();
            (index, info.human_name(), open)
        })
        .collect();
    Ok(label_devices(found, cfg!(target_os = "linux"))
        .into_iter()
        .map(|(index, name, open)| ListedCamera { index, name, open })
        .collect())
}

/// Give every camera a name of its own, since the name is what a layer and
/// a preset know a camera by.
///
/// With `merge_same_name`, cameras sharing a name become one, the lowest
/// index: Linux V4L2 exposes several device nodes per physical camera (main,
/// metadata, IR). Elsewhere two cameras of the same model are two cameras,
/// and the later ones are numbered: "Cam", "Cam (2)".
fn label_devices<T>(
    mut found: Vec<(u32, String, T)>,
    merge_same_name: bool,
) -> Vec<(u32, String, T)> {
    found.sort_by_key(|(index, _, _)| *index);
    let mut seen = std::collections::HashMap::<String, u32>::new();
    let mut result = Vec::with_capacity(found.len());
    for (index, name, open) in found {
        let count = seen.entry(name.clone()).or_insert(0);
        *count += 1;
        match *count {
            1 => result.push((index, name, open)),
            _ if merge_same_name => {}
            n => result.push((index, format!("{name} ({n})"), open)),
        }
    }
    result
}

/// List available webcam devices. Returns Vec of (index, name). The name is
/// the camera's identity; the index is only good until the next listing.
pub fn list_devices() -> Result<Vec<(u32, String)>, String> {
    Ok(enumerate()?
        .into_iter()
        .map(|d| (d.index, d.name))
        .collect())
}

/// Check if any webcam is available. Cached via OnceLock.
#[allow(dead_code)]
pub fn webcam_available() -> bool {
    use std::sync::OnceLock;
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| list_devices().map_or(false, |d| !d.is_empty()))
}

/// Where macOS stands on letting Fosfora use a camera.
#[cfg(target_os = "macos")]
pub enum CameraAccess {
    Granted,
    Denied,
    /// The prompt is on screen; the receiver gets the user's answer.
    Asking(crossbeam_channel::Receiver<bool>),
}

#[cfg(target_os = "macos")]
pub const CAMERA_DENIED: &str = "Fosfora is not allowed to use the camera. Turn it on in \
     System Settings ▸ Privacy & Security ▸ Camera, then add the camera again.";

/// macOS lets an app use a camera only after the user has allowed it, and
/// the question is asked only when the app requests access. Without the
/// request the camera stayed dark and no prompt ever appeared (GH #212).
///
/// Waits briefly rather than for the user: a recorded answer comes back at
/// once, while a prompt on screen can stay there indefinitely and the frame
/// loop must not stop for it. Asking again while the prompt is up does not
/// show a second one.
#[cfg(target_os = "macos")]
pub fn request_camera_access() -> CameraAccess {
    if nokhwa::nokhwa_check() {
        return CameraAccess::Granted;
    }
    let (tx, rx) = crossbeam_channel::bounded(1);
    nokhwa::nokhwa_initialize(move |granted| {
        let _ = tx.send(granted);
    });
    let access = match rx.recv_timeout(std::time::Duration::from_millis(500)) {
        Ok(true) => CameraAccess::Granted,
        Ok(false) => CameraAccess::Denied,
        Err(_) => CameraAccess::Asking(rx),
    };
    log::info!(
        "Camera access: {}",
        match access {
            CameraAccess::Granted => "granted",
            CameraAccess::Denied => "denied",
            CameraAccess::Asking(_) => "asking the user",
        }
    );
    access
}

/// The same request, as a gate in front of opening a camera. Callers that
/// can finish their work once the user answers ask first themselves, as
/// adding a camera layer does.
#[cfg(target_os = "macos")]
fn ensure_camera_access() -> Result<(), String> {
    match request_camera_access() {
        CameraAccess::Granted => Ok(()),
        CameraAccess::Denied => Err(CAMERA_DENIED.into()),
        CameraAccess::Asking(_) => {
            Err("Allow Fosfora to use the camera when macOS asks, then try again.".into())
        }
    }
}

/// Format a user-friendly camera error message.
fn camera_error_message(device_name: &str, err: &str) -> String {
    if err.contains("Device or resource busy") {
        format!(
            "Camera '{device_name}' is in use by another application. \
             If OBS is running, right-click the webcam source and Deactivate it to release the device."
        )
    } else {
        format!("Failed to open camera '{device_name}': {err}")
    }
}

fn capture_thread(
    device_name: &str,
    index: &CameraIndex,
    resolution: Option<(u32, u32)>,
    opened_tx: &Sender<Result<(u32, u32), String>>,
    frame_tx: &Sender<WebcamFrame>,
    shutdown: &AtomicBool,
) {
    let mut camera = match open_camera_with_fallback(device_name, index, resolution) {
        Ok(c) => c,
        Err(e) => {
            log::error!("{e}");
            let _ = opened_tx.send(Err(e));
            return;
        }
    };

    let res = camera.resolution();
    log::info!(
        "Webcam capture started: {}x{} on '{device_name}'",
        res.width(),
        res.height()
    );
    let _ = opened_tx.send(Ok((res.width(), res.height())));

    let mut consecutive_panics: u32 = 0;
    const MAX_CONSECUTIVE_PANICS: u32 = 10;

    while !shutdown.load(Ordering::Relaxed) {
        match camera.frame() {
            Ok(buffer) => {
                // decode_image can panic on corrupted MJPEG frames (libjpeg fatal error).
                // Catch the panic so one bad frame doesn't kill the capture thread.
                let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    buffer.decode_image::<RgbAFormat>()
                }));
                match decoded {
                    Ok(Ok(img)) => {
                        consecutive_panics = 0;
                        // The decoded size, not the negotiated one: it is
                        // what the pixel data actually measures.
                        let (width, height) = img.dimensions();
                        let frame = WebcamFrame {
                            data: img.into_raw(),
                            width,
                            height,
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
    log::info!("Webcam capture stopped: '{device_name}'");
}

#[cfg(test)]
mod tests {
    use super::{label_devices, pick_format};
    use nokhwa::utils::{CameraFormat, FrameFormat, Resolution};

    fn names(found: Vec<(u32, &str)>, merge: bool) -> Vec<(u32, String)> {
        let found = found
            .into_iter()
            .map(|(i, n)| (i, n.to_string(), ()))
            .collect();
        label_devices(found, merge)
            .into_iter()
            .map(|(i, n, ())| (i, n))
            .collect()
    }

    /// Two cameras of one model were listed as a single camera, so the
    /// second could never be picked.
    #[test]
    fn same_model_cameras_stay_separate() {
        assert_eq!(
            names(vec![(1, "C920"), (0, "FaceTime"), (2, "C920")], false),
            vec![
                (0, "FaceTime".to_string()),
                (1, "C920".to_string()),
                (2, "C920 (2)".to_string()),
            ]
        );
    }

    #[test]
    fn picks_the_format_nearest_the_wanted_size() {
        let fmt = |w, h, layout, fps| CameraFormat::new(Resolution::new(w, h), layout, fps);
        let formats = [
            fmt(3840, 2160, FrameFormat::MJPEG, 30),
            fmt(1280, 720, FrameFormat::YUYV, 10),
            fmt(1280, 720, FrameFormat::MJPEG, 30),
            fmt(640, 480, FrameFormat::YUYV, 30),
        ];
        assert_eq!(
            pick_format(&formats, (1280, 720)),
            Some(fmt(1280, 720, FrameFormat::MJPEG, 30))
        );
        // A camera with one size only (a virtual camera) still opens.
        let only = [fmt(1920, 1080, FrameFormat::YUYV, 30)];
        assert_eq!(pick_format(&only, (1280, 720)), Some(only[0]));
        assert_eq!(pick_format(&[], (1280, 720)), None);
    }

    #[test]
    fn linux_device_nodes_of_one_camera_merge() {
        assert_eq!(
            names(vec![(0, "C920"), (1, "C920"), (2, "Loopback")], true),
            vec![(0, "C920".to_string()), (2, "Loopback".to_string())]
        );
    }
}
