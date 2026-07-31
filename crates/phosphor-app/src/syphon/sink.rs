use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice, MTLOrigin,
    MTLPixelFormat, MTLRegion, MTLSize, MTLTexture, MTLTextureDescriptor,
};

use super::ffi::SyphonServer;
use crate::output::sink::{FrameLayout, FrameSink, OutputFrame};

/// Syphon server sink: publishes readback frames as a Syphon Metal server.
///
/// Owns its own `MTLDevice` (no wgpu interop — CPU readback, like the other
/// sinks). Each frame is uploaded into a private `BGRA8Unorm` staging texture
/// (deliberately not `_sRGB`: Syphon convention is untagged bytes, byte-exact
/// transport) and published; Syphon blits it into its own IOSurface-backed
/// texture on our command buffer. Constructed on the sender thread — ObjC
/// objects are not `Send`, and `FrameSink` was designed for exactly that.
pub struct SyphonSink {
    // Field order = drop order: the server publishes textures created on
    // `device`, so it goes first.
    server: SyphonServer,
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    texture: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    texture_dims: (u32, u32),
    /// Reused scratch for the RGBA→BGRA swizzle (empty in the BGRA fast path).
    swizzle_scratch: Vec<u8>,
}

impl SyphonSink {
    pub fn new(server_name: &str) -> Result<Self, String> {
        let device = MTLCreateSystemDefaultDevice()
            .ok_or_else(|| "No Metal device available".to_string())?;
        let queue = device
            .newCommandQueue()
            .ok_or_else(|| "Failed to create Metal command queue".to_string())?;
        let server = SyphonServer::new(server_name, &device)?;
        Ok(Self {
            server,
            device,
            queue,
            texture: None,
            texture_dims: (0, 0),
            swizzle_scratch: Vec::new(),
        })
    }

    fn create_texture(
        &self,
        width: u32,
        height: u32,
    ) -> Result<Retained<ProtocolObject<dyn MTLTexture>>, String> {
        // SAFETY: plain descriptor construction; width/height are the frame
        // dims (non-zero, ≤ texture limits — the pipeline's resize guard
        // enforces that) and BGRA8Unorm is a valid 2D texture format.
        let desc = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                MTLPixelFormat::BGRA8Unorm,
                width as usize,
                height as usize,
                false,
            )
        };
        self.device
            .newTextureWithDescriptor(&desc)
            .ok_or_else(|| format!("Failed to create {width}x{height} Metal texture"))
    }
}

impl FrameSink for SyphonSink {
    fn write_frame(&mut self, frame: &OutputFrame) -> Result<(), String> {
        let (w, h) = (frame.width, frame.height);
        let expected = w as usize * h as usize * 4;
        if frame.data.len() != expected {
            return Err(format!(
                "Syphon frame size mismatch: {} bytes for {w}x{h} (expected {expected})",
                frame.data.len()
            ));
        }

        // The staging texture is always BGRA; swizzle when the surface handed
        // us RGBA instead (same policy as the v4l2 sink's CPU conversion).
        if frame.layout == FrameLayout::Rgba8 {
            self.swizzle_scratch.clear();
            self.swizzle_scratch.reserve(expected);
            for px in frame.data.chunks_exact(4) {
                self.swizzle_scratch
                    .extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
        }
        let bytes: &[u8] = if frame.layout == FrameLayout::Rgba8 {
            &self.swizzle_scratch
        } else {
            &frame.data
        };

        if self.texture.is_none() || self.texture_dims != (w, h) {
            self.texture = Some(self.create_texture(w, h)?);
            self.texture_dims = (w, h);
        }
        let texture = self.texture.as_ref().expect("texture created above");

        let region = MTLRegion {
            origin: MTLOrigin { x: 0, y: 0, z: 0 },
            size: MTLSize {
                width: w as usize,
                height: h as usize,
                depth: 1,
            },
        };
        // SAFETY: `bytes` holds exactly height * bytesPerRow tightly-packed
        // bytes (checked above) and outlives the call; replaceRegion copies
        // synchronously. NonNull is valid: `expected` > 0 only reaches here
        // with a non-empty slice (the pipeline never sends 0-dim frames).
        unsafe {
            texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                region,
                0,
                NonNull::new(bytes.as_ptr() as *mut std::ffi::c_void)
                    .ok_or("frame data pointer was null")?,
                w as usize * 4,
            );
        }

        let command_buffer = self
            .queue
            .commandBuffer()
            .ok_or_else(|| "Failed to create Metal command buffer".to_string())?;
        self.server.publish_frame(texture, &command_buffer, w, h);
        command_buffer.commit();
        // Wait so the next replaceRegion can't race Syphon's in-flight blit
        // out of this texture. Off the render thread, so the stall is free.
        command_buffer.waitUntilCompleted();
        Ok(())
    }
}
