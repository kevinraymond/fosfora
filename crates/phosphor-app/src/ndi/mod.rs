pub mod ffi;
pub mod sender;
pub mod types;

use wgpu::{CommandEncoder, Device, TextureFormat};

use self::sender::NdiSink;
use self::types::NdiConfig;
use crate::gpu::postprocess::PostProcessChain;
use crate::gpu::render_target::RenderTarget;
use crate::output::pipeline::OutputPipeline;

/// Central NDI output system: config + the shared output pipeline.
pub struct NdiSystem {
    pub config: NdiConfig,
    pub pipeline: OutputPipeline,
}

impl NdiSystem {
    pub fn new(device: &Device, format: TextureFormat, window_w: u32, window_h: u32) -> Self {
        let mut sys = Self {
            config: NdiConfig::load(),
            pipeline: OutputPipeline::new(),
        };

        if sys.config.enabled {
            sys.start(device, format, window_w, window_h);
        }

        sys
    }

    /// Start NDI output: create capture target + sender thread.
    /// Failure lands in `pipeline.state` (shown by the panel), never panics.
    pub fn start(&mut self, device: &Device, format: TextureFormat, window_w: u32, window_h: u32) {
        let (w, h) = self.config.resolution.dimensions(window_w, window_h);
        let source_name = self.config.source_name.clone();
        match self.pipeline.start(
            device,
            format,
            w,
            h,
            "ndi-capture",
            "ndi-sender",
            move || NdiSink::new(&source_name),
        ) {
            Ok(()) => log::info!("NDI output started: {w}x{h}"),
            Err(e) => log::error!("NDI output failed to start: {e}"),
        }
    }

    /// Stop NDI output: shutdown sender thread and release capture resources.
    pub fn stop(&mut self) {
        self.pipeline.stop();
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
            self.start(device, format, window_w, window_h);
        }
    }

    pub fn is_running(&self) -> bool {
        self.pipeline.is_running()
    }

    /// Resize capture target when window/resolution changes.
    pub fn resize(&mut self, device: &Device, window_w: u32, window_h: u32) {
        let (w, h) = self.config.resolution.dimensions(window_w, window_h);
        self.pipeline.resize(device, w, h);
    }

    /// Capture output dimensions (for UI display).
    pub fn capture_dimensions(&self) -> (u32, u32) {
        self.pipeline.capture_dimensions()
    }

    /// Run the NDI capture pipeline (composite → staging → sender thread).
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
