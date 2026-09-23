//! Layer-stack thumbnails for the v2 structure column (#3123).
//!
//! Each layer row shows two pictures: the stack *so far* — layers 1..n
//! composited with their blend modes, opacity and chains — and, inset, the
//! layer on its own. It is the same idea as the previews on trama nodes, so
//! the targets follow `trama/exec/preview.rs`: small `Rgba8Unorm` targets
//! whose blit does the linear→sRGB encode itself, because egui treats sampled
//! user textures as already gamma-encoded.
//!
//! Two things differ from the trama previews, both on purpose:
//!
//! - **Fixed slots, allocated once.** One pair per possible layer
//!   ([`MAX_LAYERS`]), keyed by stack index and rewritten every frame the
//!   shell is visible, so there is no lifecycle to follow when layers are
//!   added, removed or reordered, and no resize: the textures are always
//!   16:9 and the row draws them at the output's aspect, which un-squeezes a
//!   frame of any other shape.
//! - **A box filter, not one bilinear tap.** At 1080p one thumbnail texel
//!   covers about ten source texels, and a single tap picks one of them — a
//!   particle field reads as sparse noise that crawls. Sixteen taps spread
//!   over the footprint average it instead.
//!
//! The taps are driven from `frame_graph::execute_and_composite`, which knows
//! the moment each stage of the blend exists; this module only owns the
//! targets and the blit.

use wgpu::{
    BindGroupLayout, CommandEncoder, Device, RenderPipeline, Sampler, TextureFormat, TextureView,
};

use super::fullscreen_quad::FULLSCREEN_TRIANGLE_VS_WITH_UV;
use crate::bindings::catalog::MAX_LAYERS;

pub const THUMB_W: u32 = 192;
pub const THUMB_H: u32 = 108;
const THUMB_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

/// Sixteen taps over one thumbnail texel's footprint, clamped per tap (so one
/// HDR spike cannot dominate its neighbors — the output tonemaps it away
/// too), then encoded for egui. Alpha is forced to 1: an overlay layer on its
/// own shows over black rather than blending with the row behind it.
const THUMB_FS: &str = "
@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_samp: sampler;
fn srgb_encode(c: f32) -> f32 {
    return select(12.92 * c, 1.055 * pow(c, 1.0 / 2.4) - 0.055, c > 0.0031308);
}
@fragment
fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
    let texel = 1.0 / vec2f(192.0, 108.0);
    var acc = vec3f(0.0);
    for (var j = 0; j < 4; j++) {
        for (var i = 0; i < 4; i++) {
            let o = (vec2f(f32(i), f32(j)) + 0.5) / 4.0 - 0.5;
            let s = textureSampleLevel(src_tex, src_samp, uv + o * texel, 0.0).rgb;
            acc += clamp(s, vec3f(0.0), vec3f(1.0));
        }
    }
    let c = acc / 16.0;
    return vec4f(srgb_encode(c.r), srgb_encode(c.g), srgb_encode(c.b), 1.0);
}
";

/// Which of a row's two pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbKind {
    /// Layers 1..n blended, n being this row's layer.
    Blended,
    /// This layer alone, after its chain, before blending.
    Alone,
}

struct Thumb {
    #[cfg_attr(not(test), allow(dead_code))]
    texture: wgpu::Texture,
    view: TextureView,
    tex_id: Option<egui::TextureId>,
}

impl Thumb {
    fn new(device: &Device, label: &str) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: THUMB_W,
                height: THUMB_H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: THUMB_FORMAT,
            // COPY_SRC for the probe's readback; it costs nothing on a
            // texture this size.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            tex_id: None,
        }
    }
}

pub struct LayerThumbs {
    pipeline: RenderPipeline,
    bgl: BindGroupLayout,
    sampler: Sampler,
    blended: Vec<Thumb>,
    alone: Vec<Thumb>,
}

impl LayerThumbs {
    pub fn new(device: &Device) -> Self {
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer-thumbs-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("layer-thumbs"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{FULLSCREEN_TRIANGLE_VS_WITH_UV}\n{THUMB_FS}").into(),
            ),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("layer-thumbs-layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("layer-thumbs-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: THUMB_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("layer-thumbs-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let slots = |label: &str| {
            (0..MAX_LAYERS)
                .map(|i| Thumb::new(device, &format!("{label}-{i}")))
                .collect::<Vec<_>>()
        };
        Self {
            pipeline,
            bgl,
            sampler,
            blended: slots("layer-thumb-blended"),
            alone: slots("layer-thumb-alone"),
        }
    }

    fn slots(&self, kind: ThumbKind) -> &[Thumb] {
        match kind {
            ThumbKind::Blended => &self.blended,
            ThumbKind::Alone => &self.alone,
        }
    }

    /// Register every target with egui. Idempotent; the targets never change
    /// identity, so after the first call this does nothing.
    pub fn register(&mut self, device: &Device, renderer: &mut egui_wgpu::Renderer) {
        for thumb in self.blended.iter_mut().chain(self.alone.iter_mut()) {
            if thumb.tex_id.is_none() {
                thumb.tex_id = Some(renderer.register_native_texture(
                    device,
                    &thumb.view,
                    wgpu::FilterMode::Linear,
                ));
            }
        }
    }

    /// The egui texture for stack index `slot`, once registered.
    pub fn tex(&self, kind: ThumbKind, slot: usize) -> Option<egui::TextureId> {
        self.slots(kind).get(slot).and_then(|t| t.tex_id)
    }

    /// Downscale `src` into the `kind` thumbnail of stack index `slot`. A slot
    /// past the cap is ignored rather than a panic mid-frame.
    pub fn tap(
        &self,
        device: &Device,
        encoder: &mut CommandEncoder,
        src: &TextureView,
        kind: ThumbKind,
        slot: usize,
    ) {
        let Some(thumb) = self.slots(kind).get(slot) else {
            return;
        };
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("layer-thumb-bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("layer-thumb"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &thumb.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Read one thumbnail back as RGBA8 bytes (probe tests only).
    #[cfg(test)]
    pub fn read(
        &self,
        device: &Device,
        queue: &wgpu::Queue,
        kind: ThumbKind,
        slot: usize,
    ) -> Vec<u8> {
        let thumb = &self.slots(kind)[slot];
        // 192 × 4 = 768 bytes a row, a multiple of the 256-byte copy alignment.
        let bytes_per_row = THUMB_W * 4;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("layer-thumb-readback"),
            size: (bytes_per_row * THUMB_H) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &thumb.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(THUMB_H),
                },
            },
            wgpu::Extent3d {
                width: THUMB_W,
                height: THUMB_H,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .expect("poll");
        let data = slice.get_mapped_range().to_vec();
        readback.unmap();
        data
    }
}
