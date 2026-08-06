use spout2::dx;

use crate::output::sink::{FrameLayout, FrameSink, OutputFrame};

/// Spout sender sink: publishes readback frames as a Spout DirectX 11 sender.
///
/// The `dx::Sender` owns its own internal D3D11 device (no wgpu interop), and
/// `send_image` uploads our tightly-packed 4-byte pixels as-is — the shared
/// texture format tells receivers how to read them, so no CPU conversion or
/// flip is needed. Constructed on the sender thread (`dx::Sender` is not
/// `Send`, and D3D11 device creation is slow and can fail under RDP/headless).
pub struct SpoutSink {
    sender: dx::Sender,
}

impl SpoutSink {
    pub fn new(name: &str, layout: FrameLayout) -> Result<Self, String> {
        let mut sender = dx::Sender::new(name)
            .map_err(|e| format!("Failed to create Spout sender '{name}': {e}"))?;
        // Spout's default shared-texture format is BGRA, matching our usual
        // readback bytes; retag when the platform hands us RGBA instead.
        if layout == FrameLayout::Rgba8 {
            sender.set_format(dx::format::R8G8B8A8_UNORM);
        }
        Ok(Self { sender })
    }
}

impl FrameSink for SpoutSink {
    fn write_frame(&mut self, frame: &OutputFrame) -> Result<(), String> {
        self.sender
            .send_image(&frame.data, frame.width, frame.height)
            .map_err(|e| format!("Spout send failed: {e}"))
    }
}
