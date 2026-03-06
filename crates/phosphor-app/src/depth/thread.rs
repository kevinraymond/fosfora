use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};

use super::estimator::DepthEstimator;

/// A frame to be processed for depth estimation.
struct DepthRequest {
    data: Vec<u8>,
    width: u32,
    height: u32,
}

/// Result of depth estimation.
pub struct DepthFrame {
    pub data: Vec<u8>, // 256×256 grayscale
    pub width: u32,
    pub height: u32,
}

/// Background thread that runs MiDaS depth estimation.
pub struct DepthThread {
    request_tx: Sender<DepthRequest>,
    result_rx: Receiver<DepthFrame>,
    shutdown: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl DepthThread {
    /// Start the depth estimation background thread.
    pub fn start(model_path: PathBuf) -> Result<Self> {
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        // Bounded(1) — drop stale frames if depth thread is behind
        let (request_tx, request_rx) = crossbeam_channel::bounded::<DepthRequest>(1);
        // Bounded(1) — main thread drains latest result
        let (result_tx, result_rx) = crossbeam_channel::bounded::<DepthFrame>(1);

        let handle = std::thread::Builder::new()
            .name("phosphor-depth".into())
            .spawn(move || {
                let mut estimator = match DepthEstimator::new(&model_path) {
                    Ok(e) => e,
                    Err(e) => {
                        log::error!("Failed to load depth model: {e}");
                        return;
                    }
                };
                log::info!("Depth estimator initialized (MiDaS v2.1 small)");

                while !shutdown_clone.load(Ordering::Relaxed) {
                    match request_rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(req) => {
                            match estimator.estimate(&req.data, req.width, req.height) {
                                Ok(depth_data) => {
                                    let size = super::estimator::DEPTH_SIZE;
                                    let frame = DepthFrame {
                                        data: depth_data,
                                        width: size,
                                        height: size,
                                    };
                                    // Drop old result if main thread hasn't consumed it yet
                                    let _ = result_tx.try_send(frame);
                                }
                                Err(e) => {
                                    log::warn!("Depth estimation failed: {e}");
                                }
                            }
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    }
                }

                log::info!("Depth thread shutting down");
            })?;

        Ok(Self {
            request_tx,
            result_rx,
            shutdown,
            handle: Some(handle),
        })
    }

    /// Send a webcam frame for depth processing.
    /// Uses try_send — drops frame if depth thread is still processing.
    pub fn send_frame(&self, data: Vec<u8>, w: u32, h: u32) {
        let _ = self.request_tx.try_send(DepthRequest {
            data,
            width: w,
            height: h,
        });
    }

    /// Non-blocking drain — returns the latest depth result (if any).
    pub fn try_recv_depth(&self) -> Option<DepthFrame> {
        let mut latest = None;
        while let Ok(frame) = self.result_rx.try_recv() {
            latest = Some(frame);
        }
        latest
    }

    /// Shut down the depth thread.
    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for DepthThread {
    fn drop(&mut self) {
        self.stop();
    }
}
