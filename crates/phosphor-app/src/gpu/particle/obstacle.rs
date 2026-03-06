use wgpu::{Device, Queue};

/// 2D obstacle texture for particle collision.
/// Stores alpha-channel shape data. Particles test alpha against a threshold
/// and respond with bounce/stick/flow-around behavior.
pub struct ObstacleTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    pub width: u32,
    pub height: u32,
}

impl ObstacleTexture {
    /// Create a 1x1 transparent placeholder (no collision).
    pub fn placeholder(device: &Device, queue: &Queue) -> Self {
        let size = wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("obstacle-placeholder"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("obstacle-placeholder-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Transparent black
        let data = [0u8; 4];
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            size,
        );

        Self {
            texture,
            view,
            sampler,
            width: 1,
            height: 1,
        }
    }

    /// Create from RGBA byte data.
    ///
    /// If the image has no meaningful alpha variation (e.g. JPEG, opaque PNG
    /// where all pixels are alpha=255), luminance is written into the alpha
    /// channel so the obstacle shape comes from brightness instead.
    pub fn from_rgba(device: &Device, queue: &Queue, data: &[u8], w: u32, h: u32) -> Self {
        let processed = preprocess_alpha(data);

        let size = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("obstacle-texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("obstacle-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &processed,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            size,
        );

        Self {
            texture,
            view,
            sampler,
            width: w,
            height: h,
        }
    }

    /// Update texture data in-place (for webcam per-frame updates).
    /// If dimensions match, reuses existing texture. Otherwise recreates.
    pub fn update(&mut self, device: &Device, queue: &Queue, data: &[u8], w: u32, h: u32) {
        let processed = preprocess_alpha(data);
        if w == self.width && h == self.height {
            // Same size — just write new data
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &processed,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        } else {
            // Different size — recreate
            *self = Self::from_rgba(device, queue, data, w, h);
        }
    }
}

/// If the image has no meaningful alpha (all pixels ≥ 250), replace alpha
/// with luminance so opaque images (JPEG, opaque PNG) work as obstacles
/// based on their brightness.
fn preprocess_alpha(data: &[u8]) -> Vec<u8> {
    // Check if alpha channel has meaningful variation
    let has_alpha = data.chunks_exact(4).any(|px| px[3] < 250);

    if has_alpha {
        // Image has real alpha — use as-is
        data.to_vec()
    } else {
        // No alpha variation — write luminance into alpha channel
        let mut out = data.to_vec();
        for px in out.chunks_exact_mut(4) {
            let lum = (px[0] as f32 * 0.299 + px[1] as f32 * 0.587 + px[2] as f32 * 0.114) as u8;
            px[3] = lum;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_alpha_preserves_real_alpha() {
        // Image with transparent pixels — alpha should be preserved
        let data = vec![
            255, 0, 0, 128, // semi-transparent red
            0, 255, 0, 0, // fully transparent green
        ];
        let result = preprocess_alpha(&data);
        assert_eq!(result[3], 128);
        assert_eq!(result[7], 0);
    }

    #[test]
    fn preprocess_alpha_uses_luminance_for_opaque() {
        // All alpha=255 — should replace with luminance
        let data = vec![
            255, 255, 255, 255, // white → lum ≈ 255
            0, 0, 0, 255, // black → lum = 0
        ];
        let result = preprocess_alpha(&data);
        // White pixel should have high alpha
        assert!(result[3] > 200);
        // Black pixel should have zero alpha
        assert_eq!(result[7], 0);
    }
}
