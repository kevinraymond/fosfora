use std::path::Path;

/// A loaded sprite atlas texture for particle rendering.
pub struct SpriteAtlas {
    #[allow(dead_code)]
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    pub cols: u32,
    pub rows: u32,
    pub frames: u32,
    pub animated: bool,
}

impl SpriteAtlas {
    /// Load a sprite atlas from a PNG/JPEG file.
    pub fn load(device: &wgpu::Device, queue: &wgpu::Queue, path: &Path) -> Result<Self, String> {
        let img = image::open(path)
            .map_err(|e| format!("Failed to load sprite: {e}"))?
            .to_rgba8();

        let (width, height) = img.dimensions();
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sprite-atlas"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &img,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sprite-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Ok(Self {
            texture,
            view,
            sampler,
            cols: 1,
            rows: 1,
            frames: 1,
            animated: false,
        })
    }

    /// Load with atlas grid parameters from a SpriteDef.
    pub fn load_with_def(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &Path,
        cols: u32,
        rows: u32,
        animated: bool,
        frames: u32,
    ) -> Result<Self, String> {
        let mut atlas = Self::load(device, queue, path)?;
        atlas.cols = cols.max(1);
        atlas.rows = rows.max(1);
        atlas.animated = animated;
        atlas.frames = if frames > 0 { frames } else { cols * rows };
        Ok(atlas)
    }
}

/// Create a 1x1 white placeholder texture for when no sprite is loaded.
pub fn create_placeholder_sprite(device: &wgpu::Device, queue: &wgpu::Queue) -> SpriteAtlas {
    let size = wgpu::Extent3d {
        width: 1,
        height: 1,
        depth_or_array_layers: 1,
    };

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sprite-placeholder"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[255u8, 255, 255, 255],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        size,
    );

    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("sprite-placeholder-sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    SpriteAtlas {
        texture,
        view,
        sampler,
        cols: 1,
        rows: 1,
        frames: 1,
        animated: false,
    }
}
