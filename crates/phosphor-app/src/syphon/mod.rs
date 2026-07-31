pub mod ffi;
pub mod sink;
pub mod types;

use wgpu::{CommandEncoder, Device, TextureFormat};

use self::sink::SyphonSink;
use self::types::SyphonConfig;
use crate::gpu::postprocess::PostProcessChain;
use crate::gpu::render_target::RenderTarget;
use crate::output::pipeline::{OutputPipeline, OutputState};

/// Central Syphon output system: config + the shared output pipeline.
pub struct SyphonSystem {
    pub config: SyphonConfig,
    pub pipeline: OutputPipeline,
}

impl SyphonSystem {
    pub fn new(device: &Device, format: TextureFormat, window_w: u32, window_h: u32) -> Self {
        let mut sys = Self {
            config: SyphonConfig::load(),
            pipeline: OutputPipeline::new(),
        };

        if sys.config.enabled {
            sys.start(device, format, window_w, window_h);
        }

        sys
    }

    /// Start Syphon output: create capture target + sender thread. The Metal
    /// device and Syphon server are created on the sender thread; creation
    /// failure surfaces through `poll_health` into `pipeline.state` (shown by
    /// the panel), never panics. The framework itself is probed here so a
    /// missing Syphon.framework fails fast with the search diagnostics.
    pub fn start(&mut self, device: &Device, format: TextureFormat, window_w: u32, window_h: u32) {
        if !ffi::syphon_available() {
            let msg =
                "Syphon.framework not found — see Searched locations in the panel".to_string();
            log::warn!("Syphon output: {msg}");
            self.pipeline.state = OutputState::Error(msg);
            return;
        }

        let (w, h) = self.config.resolution.dimensions(window_w, window_h);
        let server_name = self.config.effective_server_name();
        match self.pipeline.start(
            device,
            format,
            w,
            h,
            "syphon-capture",
            "syphon-server",
            move || SyphonSink::new(&server_name),
        ) {
            Ok(()) => log::info!("Syphon output started: {w}x{h}"),
            Err(e) => log::error!("Syphon output failed to start: {e}"),
        }
    }

    /// Stop Syphon output: shutdown sender thread and release capture resources.
    pub fn stop(&mut self) {
        self.pipeline.stop();
        log::info!("Syphon output stopped");
    }

    /// Toggle Syphon output on/off.
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

    /// Restart with new config (server name or resolution changed).
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

    /// Resize capture target when window/resolution changes. Syphon clients
    /// adapt to server size changes mid-stream, so this follows the window —
    /// same policy as NDI and Spout (unlike v4l2).
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
