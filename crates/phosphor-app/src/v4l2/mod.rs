pub mod device;
pub mod sink;
pub mod types;

use wgpu::{CommandEncoder, Device, TextureFormat};

use self::device::LoopbackDevice;
use self::sink::V4l2Sink;
use self::types::V4l2Config;
use crate::gpu::postprocess::PostProcessChain;
use crate::gpu::render_target::RenderTarget;
use crate::output::pipeline::{OutputPipeline, OutputState};

/// Central v4l2 output system: config + the shared output pipeline.
pub struct V4l2System {
    pub config: V4l2Config,
    pub pipeline: OutputPipeline,
    /// Cached device list; refreshed on demand from the panel.
    pub devices: Vec<LoopbackDevice>,
    /// The device `start()` actually opened (auto-selection made concrete).
    resolved_path: Option<String>,
}

impl V4l2System {
    pub fn new(device: &Device, format: TextureFormat, window_w: u32, window_h: u32) -> Self {
        let mut sys = Self {
            config: V4l2Config::load(),
            pipeline: OutputPipeline::new(),
            devices: device::enumerate_loopback_devices(),
            resolved_path: None,
        };

        if sys.config.enabled {
            sys.start(device, format, window_w, window_h);
        }

        sys
    }

    /// Start v4l2 output. The device is opened and format-negotiated
    /// synchronously on the render thread so failure is immediate and visible;
    /// only frame writes happen on the sender thread. Errors land in
    /// `pipeline.state` (shown by the panel), never panic.
    pub fn start(&mut self, device: &Device, format: TextureFormat, window_w: u32, window_h: u32) {
        self.pipeline.stop();
        self.resolved_path = None;

        let path = match self.config.device_path.clone() {
            Some(p) => p,
            None => {
                self.devices = device::enumerate_loopback_devices();
                match self.devices.first() {
                    Some(d) => d.path.clone(),
                    None => {
                        let msg = "No v4l2loopback device found".to_string();
                        log::warn!("v4l2 output: {msg}");
                        self.pipeline.state = OutputState::Error(msg);
                        return;
                    }
                }
            }
        };

        let (w, h) = self.config.resolution.dimensions(window_w, window_h);
        let pixel_format = self.config.pixel_format;

        let loopback = match device::open_output(&path, w, h, pixel_format.fourcc()) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("v4l2 output: {e}");
                self.pipeline.state = OutputState::Error(e);
                return;
            }
        };

        match self.pipeline.start(
            device,
            format,
            w,
            h,
            "v4l2-capture",
            "v4l2-sender",
            move || Ok(V4l2Sink::new(loopback, pixel_format, w, h)),
        ) {
            Ok(()) => {
                self.resolved_path = Some(path);
                let (w2, h2) = (w, h);
                log::info!("v4l2 output started: {w2}x{h2}");
            }
            Err(e) => log::error!("v4l2 output failed to start: {e}"),
        }
    }

    pub fn stop(&mut self) {
        self.pipeline.stop();
        self.resolved_path = None;
        log::info!("v4l2 output stopped");
    }

    /// Toggle v4l2 output on/off.
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

    /// Restart with new config (device, resolution or format changed).
    pub fn restart(
        &mut self,
        device: &Device,
        format: TextureFormat,
        window_w: u32,
        window_h: u32,
    ) {
        if self.is_running() {
            self.stop();
            self.start(device, format, window_w, window_h);
        }
    }

    pub fn is_running(&self) -> bool {
        self.pipeline.is_running()
    }

    /// The device path the running stream actually uses (auto resolved).
    pub fn resolved_path(&self) -> Option<&str> {
        self.resolved_path.as_deref()
    }

    pub fn refresh_devices(&mut self) {
        self.devices = device::enumerate_loopback_devices();
    }

    /// Run the capture pipeline (composite → staging → sender thread).
    pub fn capture_frame(
        &mut self,
        device: &Device,
        encoder: &mut CommandEncoder,
        post_process: &PostProcessChain,
        source: &RenderTarget,
    ) {
        self.pipeline
            .capture_frame(device, encoder, post_process, source);
    }

    /// Called after queue.submit() — request async map on the staging buffer.
    pub fn post_submit(&mut self) {
        self.pipeline.post_submit();
    }

    pub fn frames_sent(&self) -> u64 {
        self.pipeline.frames_sent()
    }

    /// Capture output dimensions (for UI display).
    pub fn capture_dimensions(&self) -> (u32, u32) {
        self.pipeline.capture_dimensions()
    }
}

// Deliberately no `resize()`: v4l2 readers cannot tolerate mid-stream geometry
// changes, so the negotiated size is held for the stream's lifetime — the
// composite blit scales whatever the window is into the capture texture.
// Changing resolution in the panel restarts the stream instead.
