pub mod capture;
pub mod ffi;
pub mod sender;
pub mod types;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

use crossbeam_channel::Sender;
use wgpu::{CommandEncoder, Device, TextureFormat};

use self::capture::NdiCapture;
use self::sender::{NdiFrame, spawn_sender_thread};
use self::types::NdiConfig;
use crate::gpu::postprocess::PostProcessChain;
use crate::gpu::render_target::RenderTarget;

/// Central NDI output system: owns capture target, sender thread, config.
pub struct NdiSystem {
    pub config: NdiConfig,
    pub capture: Option<NdiCapture>,
    frame_tx: Option<Sender<NdiFrame>>,
    shutdown: Option<Arc<AtomicBool>>,
    sender_handle: Option<JoinHandle<()>>,
    pub frame_counter: Arc<AtomicU64>,
    /// Cached output dimensions (for detecting resolution changes).
    output_width: u32,
    output_height: u32,
}

impl NdiSystem {
    pub fn new(device: &Device, format: TextureFormat, window_w: u32, window_h: u32) -> Self {
        let config = NdiConfig::load();
        let frame_counter = Arc::new(AtomicU64::new(0));

        let mut sys = Self {
            config,
            capture: None,
            frame_tx: None,
            shutdown: None,
            sender_handle: None,
            frame_counter,
            output_width: 0,
            output_height: 0,
        };

        if sys.config.enabled {
            sys.start(device, format, window_w, window_h);
        }

        sys
    }

    /// Start NDI output: create capture target + sender thread.
    pub fn start(&mut self, device: &Device, format: TextureFormat, window_w: u32, window_h: u32) {
        self.stop();

        let (w, h) = self.config.resolution.dimensions(window_w, window_h);
        self.output_width = w;
        self.output_height = h;

        self.capture = Some(NdiCapture::new(device, w, h, format));

        let (tx, rx) = crossbeam_channel::bounded(2);
        let shutdown = Arc::new(AtomicBool::new(false));
        self.frame_counter.store(0, Ordering::Relaxed);

        let handle = spawn_sender_thread(
            self.config.source_name.clone(),
            rx,
            shutdown.clone(),
            self.frame_counter.clone(),
        );

        self.frame_tx = Some(tx);
        self.shutdown = Some(shutdown);
        self.sender_handle = Some(handle);
        self.config.enabled = true;

        log::info!("NDI output started: {}x{}", w, h);
    }

    /// Stop NDI output: shutdown sender thread and release capture resources.
    pub fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            shutdown.store(true, Ordering::Relaxed);
        }
        // Drop the channel sender so the recv side disconnects.
        self.frame_tx = None;
        if let Some(handle) = self.sender_handle.take() {
            let _ = handle.join();
        }
        self.capture = None;
        self.config.enabled = false;
        log::info!("NDI output stopped");
    }

    /// Toggle NDI on/off.
    pub fn set_enabled(
        &mut self,
        enabled: bool,
        device: &Device,
        format: TextureFormat,
        window_w: u32,
        window_h: u32,
    ) {
        if enabled && !self.is_running() {
            self.start(device, format, window_w, window_h);
        } else if !enabled && self.is_running() {
            self.stop();
        }
        self.config.enabled = enabled;
        self.config.save();
    }

    /// Restart with new config (source name or resolution changed).
    pub fn restart(
        &mut self,
        device: &Device,
        format: TextureFormat,
        window_w: u32,
        window_h: u32,
    ) {
        if self.is_running() {
            self.stop();
            self.config.enabled = true; // stop() sets it false
            self.start(device, format, window_w, window_h);
        }
    }

    pub fn is_running(&self) -> bool {
        self.sender_handle.is_some()
    }

    /// Resize capture target when window/resolution changes.
    pub fn resize(&mut self, device: &Device, window_w: u32, window_h: u32) {
        if !self.is_running() {
            return;
        }
        let (w, h) = self.config.resolution.dimensions(window_w, window_h);
        if w == self.output_width && h == self.output_height {
            return;
        }
        self.output_width = w;
        self.output_height = h;
        if let Some(ref mut cap) = self.capture {
            cap.resize(device, w, h);
        }
    }

    /// Run the NDI capture pipeline:
    /// 1. Read previously-mapped staging data (non-blocking).
    /// 2. Render the post-process composite to the capture texture.
    /// 3. Copy capture texture → staging buffer.
    /// 4. Request async map on the staging buffer.
    /// 5. Send the previous frame's data to the NDI thread.
    pub fn capture_frame(
        &mut self,
        device: &Device,
        encoder: &mut CommandEncoder,
        post_process: &PostProcessChain,
        source: &RenderTarget,
    ) {
        let capture = match self.capture.as_mut() {
            Some(c) => c,
            None => return,
        };

        // 1. Read previous frame's staging data (1-frame latency).
        let prev_data = capture.take_mapped_data(device);

        // If previous map is still outstanding (GPU readback not ready), skip this frame
        // to avoid submitting commands that reference a still-mapped buffer.
        if capture.is_map_pending() {
            return;
        }

        // 2. Render composite to capture texture.
        post_process.render_composite_to(device, encoder, source, &capture.view);

        // 3. Copy to staging.
        capture.copy_to_staging(encoder);

        // 4. Will request map after queue.submit() — called from post_submit().

        // 5. Send previous frame data to NDI thread.
        if let (Some(data), Some(tx)) = (prev_data, &self.frame_tx) {
            let frame = NdiFrame {
                data,
                width: capture.width,
                height: capture.height,
            };
            // try_send: drop frame if NDI thread is behind (VJ performance > NDI latency).
            let _ = tx.try_send(frame);
        }
    }

    /// Called after queue.submit() — request async map on the staging buffer.
    pub fn post_submit(&mut self) {
        if let Some(ref mut capture) = self.capture {
            capture.request_map();
        }
    }

    pub fn frames_sent(&self) -> u64 {
        self.frame_counter.load(Ordering::Relaxed)
    }
}

impl Drop for NdiSystem {
    fn drop(&mut self) {
        self.stop();
    }
}
