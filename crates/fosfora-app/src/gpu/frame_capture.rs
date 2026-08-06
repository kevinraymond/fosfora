use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use wgpu::{
    Buffer, BufferDescriptor, BufferUsages, CommandEncoder, Device, Extent3d, Texture,
    TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureView,
};

/// GPU capture target with double-buffered staging for CPU readback.
/// Shared by NDI output and video recording.
pub struct FrameCapture {
    pub texture: Texture,
    pub view: TextureView,
    pub width: u32,
    pub height: u32,
    #[allow(dead_code)]
    pub format: TextureFormat,
    staging: [Buffer; 2],
    /// Bytes per row, padded to wgpu's COPY_BYTES_PER_ROW_ALIGNMENT (256).
    padded_bytes_per_row: u32,
    /// Actual bytes per row (width * 4).
    unpadded_bytes_per_row: u32,
    current: usize,
    /// Whether a map has been requested on staging[1 - current] (the "previous" buffer).
    map_pending: bool,
    /// Set to true by the map_async callback when the map completes.
    map_ready: Arc<AtomicBool>,
}

impl FrameCapture {
    pub fn new(
        device: &Device,
        width: u32,
        height: u32,
        format: TextureFormat,
        label: &str,
    ) -> Self {
        let texture = device.create_texture(&TextureDescriptor {
            label: Some(label),
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let unpadded_bytes_per_row = width * 4;
        let padded_bytes_per_row =
            align_to(unpadded_bytes_per_row, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer_size = (padded_bytes_per_row * height) as u64;

        let staging = [
            device.create_buffer(&BufferDescriptor {
                label: Some(&format!("{label}-staging-0")),
                size: buffer_size,
                usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            device.create_buffer(&BufferDescriptor {
                label: Some(&format!("{label}-staging-1")),
                size: buffer_size,
                usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
        ];

        Self {
            texture,
            view,
            width,
            height,
            format,
            staging,
            padded_bytes_per_row,
            unpadded_bytes_per_row,
            current: 0,
            map_pending: false,
            map_ready: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Copy the capture texture to the current staging buffer.
    pub fn copy_to_staging(&self, encoder: &mut CommandEncoder) {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.staging[self.current],
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Whether a map is still outstanding (caller should skip capture if true).
    pub fn is_map_pending(&self) -> bool {
        self.map_pending
    }

    /// Request async map on the current staging buffer, then flip to the other buffer.
    pub fn request_map(&mut self) {
        if self.map_pending {
            return; // Previous map still outstanding, skip.
        }
        let ready = Arc::new(AtomicBool::new(false));
        let ready_clone = ready.clone();
        let buf = &self.staging[self.current];
        buf.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            if result.is_ok() {
                ready_clone.store(true, Ordering::Release);
            }
        });
        self.map_ready = ready;
        self.map_pending = true;
        self.current = 1 - self.current;
    }

    /// Non-blocking: try to read the previously-mapped staging buffer.
    /// Returns frame data (tightly packed BGRA rows) if ready, None otherwise.
    pub fn take_mapped_data(&mut self, device: &Device) -> Option<Vec<u8>> {
        if !self.map_pending {
            return None;
        }

        // Poll to drive the map callback. Non-blocking.
        let _ = device.poll(wgpu::PollType::Poll);

        // Check if the map_async callback has fired.
        if !self.map_ready.load(Ordering::Acquire) {
            return None; // Not ready yet, will retry next frame.
        }

        // The "previous" buffer is the one we mapped (1 - current after flip).
        let prev = 1 - self.current;
        let buf = &self.staging[prev];

        let slice = buf.slice(..);
        let mapped = slice.get_mapped_range();

        let data = if self.padded_bytes_per_row == self.unpadded_bytes_per_row {
            // No padding — copy directly.
            mapped.to_vec()
        } else {
            // Strip row padding.
            let mut out = Vec::with_capacity((self.unpadded_bytes_per_row * self.height) as usize);
            for row in 0..self.height {
                let start = (row * self.padded_bytes_per_row) as usize;
                let end = start + self.unpadded_bytes_per_row as usize;
                out.extend_from_slice(&mapped[start..end]);
            }
            out
        };

        drop(mapped);
        buf.unmap();
        self.map_pending = false;

        Some(data)
    }

    /// Resize capture resources. Recreates texture and staging buffers.
    #[allow(dead_code)]
    pub fn resize(&mut self, device: &Device, width: u32, height: u32, label: &str) {
        if width == self.width && height == self.height {
            return;
        }
        // Unmap any pending buffer before recreating.
        if self.map_pending {
            let prev = 1 - self.current;
            self.staging[prev].unmap();
            self.map_pending = false;
        }
        *self = Self::new(device, width, height, self.format, label);
    }
}

/// Align `value` up to the next multiple of `alignment`.
fn align_to(value: u32, alignment: u32) -> u32 {
    (value + alignment - 1) & !(alignment - 1)
}
