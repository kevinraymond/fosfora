pub mod sink;
pub mod types;

use wgpu::{CommandEncoder, Device, TextureFormat};

use self::sink::SpoutSink;
use self::types::SpoutConfig;
use crate::gpu::postprocess::PostProcessChain;
use crate::gpu::render_target::RenderTarget;
use crate::output::pipeline::{OutputPipeline, OutputState};
use crate::output::sink::FrameLayout;

/// Central Spout output system: config + the shared output pipeline.
pub struct SpoutSystem {
    pub config: SpoutConfig,
    pub pipeline: OutputPipeline,
}

impl SpoutSystem {
    pub fn new(device: &Device, format: TextureFormat, window_w: u32, window_h: u32) -> Self {
        let mut sys = Self {
            config: SpoutConfig::load(),
            pipeline: OutputPipeline::new(),
        };

        if sys.config.enabled {
            sys.start(device, format, window_w, window_h);
        }

        sys
    }

    /// Start Spout output: create capture target + sender thread. The Spout
    /// sender itself (with its internal D3D11 device) is created on the sender
    /// thread; creation failure surfaces through `poll_health` into
    /// `pipeline.state` (shown by the panel), never panics.
    pub fn start(&mut self, device: &Device, format: TextureFormat, window_w: u32, window_h: u32) {
        // The sink needs the layout up front to pick the shared-texture format;
        // resolve it here so an unmappable format fails before thread spawn.
        let layout = match FrameLayout::from_texture_format(format) {
            Ok(l) => l,
            Err(e) => {
                log::warn!("Spout output: {e}");
                self.pipeline.state = OutputState::Error(e);
                return;
            }
        };

        let (w, h) = self.config.resolution.dimensions(window_w, window_h);
        let sender_name = self.config.effective_sender_name();
        match self.pipeline.start(
            device,
            format,
            w,
            h,
            "spout-capture",
            "spout-sender",
            move || SpoutSink::new(&sender_name, layout),
        ) {
            Ok(()) => log::info!("Spout output started: {w}x{h}"),
            Err(e) => log::error!("Spout output failed to start: {e}"),
        }
    }

    /// Stop Spout output: shutdown sender thread and release capture resources.
    pub fn stop(&mut self) {
        self.pipeline.stop();
        log::info!("Spout output stopped");
    }

    /// Toggle Spout output on/off.
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

    /// Restart with new config (sender name or resolution changed).
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

    /// Resize capture target when window/resolution changes. Spout senders
    /// handle mid-stream size changes (receivers adapt), so unlike v4l2 this
    /// follows the window — same policy as NDI.
    pub fn resize(&mut self, device: &Device, window_w: u32, window_h: u32) {
        let (w, h) = self.config.resolution.dimensions(window_w, window_h);
        self.pipeline.resize(device, w, h);
    }

    /// Capture output dimensions (for UI display).
    pub fn capture_dimensions(&self) -> (u32, u32) {
        self.pipeline.capture_dimensions()
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
}
