use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};
use wgpu::{CommandEncoder, Device, TextureFormat};

use crate::gpu::frame_capture::FrameCapture;
use crate::gpu::postprocess::PostProcessChain;
use crate::gpu::render_target::RenderTarget;

use super::sink::{FrameLayout, FrameSink, OutputFrame, spawn_sink_thread};

/// Sink lifecycle state, surfaced to the UI (same shape as `RecordingState`).
#[derive(Debug, Clone)]
pub enum OutputState {
    Idle,
    Running,
    Error(String),
}

/// Shared render-thread plumbing for a CPU-readback output sink: capture target,
/// frame channel, sender thread, and health reporting. Per-sink systems (NDI,
/// v4l2, ...) own one of these next to their config.
pub struct OutputPipeline {
    pub state: OutputState,
    capture: Option<FrameCapture>,
    capture_label: String,
    frame_tx: Option<Sender<OutputFrame>>,
    shutdown: Option<Arc<AtomicBool>>,
    sender_handle: Option<JoinHandle<()>>,
    error_rx: Option<Receiver<String>>,
    frame_counter: Arc<AtomicU64>,
    /// Frames not sent because the sender thread was behind. Render-thread only.
    frames_dropped: u64,
    layout: FrameLayout,
    /// Cached output dimensions (for detecting resolution changes).
    output_width: u32,
    output_height: u32,
}

impl OutputPipeline {
    pub fn new() -> Self {
        Self {
            state: OutputState::Idle,
            capture: None,
            capture_label: String::new(),
            frame_tx: None,
            shutdown: None,
            sender_handle: None,
            error_rx: None,
            frame_counter: Arc::new(AtomicU64::new(0)),
            frames_dropped: 0,
            layout: FrameLayout::Bgra8,
            output_width: 0,
            output_height: 0,
        }
    }

    /// Start the pipeline: create the capture target and sender thread.
    /// `width`/`height` are the already-resolved output dimensions.
    /// On failure the error is also recorded in `state` for the UI.
    pub fn start<S: FrameSink>(
        &mut self,
        device: &Device,
        format: TextureFormat,
        width: u32,
        height: u32,
        capture_label: &str,
        thread_name: &str,
        make_sink: impl FnOnce() -> Result<S, String> + Send + 'static,
    ) -> Result<(), String> {
        self.stop();

        let layout = match FrameLayout::from_texture_format(format) {
            Ok(l) => l,
            Err(e) => {
                self.state = OutputState::Error(e.clone());
                return Err(e);
            }
        };

        self.layout = layout;
        self.output_width = width;
        self.output_height = height;
        self.capture_label = capture_label.to_string();
        self.capture = Some(FrameCapture::new(
            device,
            width,
            height,
            format,
            capture_label,
        ));

        let (tx, rx) = crossbeam_channel::bounded(2);
        let (error_tx, error_rx) = crossbeam_channel::bounded(1);
        let shutdown = Arc::new(AtomicBool::new(false));
        self.frame_counter.store(0, Ordering::Relaxed);
        self.frames_dropped = 0;

        let handle = spawn_sink_thread(
            thread_name,
            make_sink,
            rx,
            shutdown.clone(),
            self.frame_counter.clone(),
            error_tx,
        );

        self.frame_tx = Some(tx);
        self.shutdown = Some(shutdown);
        self.sender_handle = Some(handle);
        self.error_rx = Some(error_rx);
        self.state = OutputState::Running;
        Ok(())
    }

    /// Stop the pipeline: shut down the sender thread, release capture resources.
    /// Never touches any sink config. A failure the thread reported before the
    /// stop is preserved in `state`; otherwise the state returns to `Idle`.
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
        self.state = match self.error_rx.take().and_then(|rx| rx.try_recv().ok()) {
            Some(e) => OutputState::Error(e),
            None => OutputState::Idle,
        };
    }

    /// Per-frame health check: if the sender thread reported a failure, tear the
    /// pipeline down and surface the error. This is what turns the status dot off
    /// when the sender dies instead of leaving it green with zero frames sent.
    pub fn poll_health(&mut self) {
        if !matches!(self.state, OutputState::Running) {
            return;
        }
        let msg = self.error_rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(msg) = msg {
            self.stop();
            self.state = OutputState::Error(msg);
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state, OutputState::Running)
    }

    pub fn last_error(&self) -> Option<&str> {
        match &self.state {
            OutputState::Error(e) => Some(e),
            _ => None,
        }
    }

    /// Run the capture pipeline:
    /// 1. Read previously-mapped staging data (non-blocking).
    /// 2. Render the post-process composite to the capture texture.
    /// 3. Copy capture texture → staging buffer.
    /// 4. Request async map after queue.submit() — see `post_submit()`.
    /// 5. Send the previous frame's data to the sender thread.
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

        // 5. Send previous frame data to the sender thread.
        if let (Some(data), Some(tx)) = (prev_data, &self.frame_tx) {
            let frame = OutputFrame {
                data,
                width: capture.width,
                height: capture.height,
                layout: self.layout,
            };
            match tx.try_send(frame) {
                Ok(()) => {}
                // Drop frame if the sender thread is behind (VJ performance > sink latency).
                Err(crossbeam_channel::TrySendError::Full(_)) => self.frames_dropped += 1,
                // Thread died; poll_health surfaces the error next frame.
                Err(crossbeam_channel::TrySendError::Disconnected(_)) => {}
            }
        }
    }

    /// Called after queue.submit() — request async map on the staging buffer.
    pub fn post_submit(&mut self) {
        if let Some(ref mut capture) = self.capture {
            capture.request_map();
        }
    }

    /// Resize the capture target. Sinks whose consumers cannot tolerate mid-stream
    /// geometry changes (v4l2) simply never call this.
    pub fn resize(&mut self, device: &Device, width: u32, height: u32) {
        if !self.is_running() {
            return;
        }
        if width == self.output_width && height == self.output_height {
            return;
        }
        self.output_width = width;
        self.output_height = height;
        let label = self.capture_label.clone();
        if let Some(ref mut cap) = self.capture {
            cap.resize(device, width, height, &label);
        }
    }

    /// Capture output dimensions (for UI display).
    pub fn capture_dimensions(&self) -> (u32, u32) {
        self.capture
            .as_ref()
            .map(|c| (c.width, c.height))
            .unwrap_or((0, 0))
    }

    pub fn frames_sent(&self) -> u64 {
        self.frame_counter.load(Ordering::Relaxed)
    }

    pub fn frames_dropped(&self) -> u64 {
        self.frames_dropped
    }
}

impl Drop for OutputPipeline {
    fn drop(&mut self) {
        self.stop();
    }
}
