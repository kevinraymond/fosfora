pub mod decoder;
pub mod types;
#[cfg(feature = "video")]
pub mod video;
#[cfg(feature = "webcam")]
pub mod webcam;

use std::path::PathBuf;

use bytemuck::{Pod, Zeroable};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, BufferBindingType, ColorTargetState,
    CommandEncoder, Device, FragmentState, PipelineCompilationOptions, PipelineLayoutDescriptor,
    PrimitiveState, Queue, RenderPipeline, SamplerBindingType, ShaderStages, TextureFormat,
    TextureSampleType, TextureViewDimension, VertexState,
};

use crate::gpu::fullscreen_quad::FULLSCREEN_TRIANGLE_VS_WITH_UV;
use crate::gpu::render_target::RenderTarget;
use decoder::MediaSource;
use types::{PlayDirection, TransportState};

const MEDIA_BLIT_FS: &str = include_str!("../../../../assets/shaders/builtin/media_blit.wgsl");

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
struct MediaUniforms {
    scale: [f32; 2],
    offset: [f32; 2],
    mirror: u32,
    _pad: u32,
}

pub struct MediaLayer {
    pub source: MediaSource,
    pub file_path: PathBuf,
    pub file_name: String,
    pub transport: TransportState,
    pub current_frame: usize,
    frame_elapsed_ms: f64,
    // GPU resources
    frame_texture: wgpu::Texture,
    frame_view: wgpu::TextureView,
    frame_sampler: wgpu::Sampler,
    pub output_target: RenderTarget,
    blit_pipeline: RenderPipeline,
    bind_group_layout: BindGroupLayout,
    bind_group: BindGroup,
    uniform_buffer: wgpu::Buffer,
    // Dimensions
    pub media_width: u32,
    pub media_height: u32,
    needs_upload: bool,
    // PingPong direction for GIF
    pingpong_forward: bool,
    // Live webcam frame data (set externally by capture thread)
    #[cfg(feature = "webcam")]
    live_frame_data: Option<Vec<u8>>,
    /// Mirror horizontally (for selfie cameras).
    #[cfg(feature = "webcam")]
    pub mirror: bool,
}

impl MediaLayer {
    pub fn new(
        device: &Device,
        queue: &Queue,
        hdr_format: TextureFormat,
        width: u32,
        height: u32,
        source: MediaSource,
        file_path: PathBuf,
    ) -> Self {
        let (media_width, media_height) = source.dimensions();
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let total_frames = source.frame_count();
        let duration = match &source {
            MediaSource::Animated { delays_ms, .. } => {
                delays_ms.iter().map(|&d| d as f64).sum::<f64>()
            }
            MediaSource::Static(_) => 0.0,
            #[cfg(feature = "webcam")]
            MediaSource::Live { .. } => 0.0,
        };

        let mut transport = TransportState::default();
        transport.duration = duration;
        // Static images and live sources don't use transport playback
        if !source.is_animated() {
            transport.playing = false;
        }

        // Create frame texture (sRGB for auto-conversion on sample)
        let frame_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("media-frame"),
            size: wgpu::Extent3d {
                width: media_width,
                height: media_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let frame_view = frame_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let frame_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("media-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        // Upload first frame (black for live sources)
        #[cfg(feature = "webcam")]
        let black_placeholder: Vec<u8>;
        let first_frame_data: &[u8] = match &source {
            MediaSource::Static(f) => &f.data,
            MediaSource::Animated { frames, .. } => &frames[0].data,
            #[cfg(feature = "webcam")]
            MediaSource::Live { width, height } => {
                black_placeholder = vec![0u8; (*width as usize) * (*height as usize) * 4];
                &black_placeholder
            }
        };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &frame_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            first_frame_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(media_width * 4),
                rows_per_image: Some(media_height),
            },
            wgpu::Extent3d {
                width: media_width,
                height: media_height,
                depth_or_array_layers: 1,
            },
        );

        // Output HDR render target (same as effect layers)
        let output_target =
            RenderTarget::new(device, width, height, hdr_format, 1.0, "media-output");

        // Uniform buffer for letterbox transform
        let uniforms = compute_media_uniforms(media_width, media_height, width, height, false);
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("media-uniforms"),
            size: std::mem::size_of::<MediaUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        // Bind group layout: texture(0), sampler(1), uniform(2)
        let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("media-blit-bgl"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 2,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(
                            std::mem::size_of::<MediaUniforms>() as u64,
                        ),
                    },
                    count: None,
                },
            ],
        });

        // Blit pipeline
        let full_source = format!("{FULLSCREEN_TRIANGLE_VS_WITH_UV}\n{MEDIA_BLIT_FS}");
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("media-blit"),
            source: wgpu::ShaderSource::Wgsl(full_source.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("media-blit-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("media-blit-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader_module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: PipelineCompilationOptions::default(),
            },
            fragment: Some(FragmentState {
                module: &shader_module,
                entry_point: Some("fs_main"),
                targets: &[Some(ColorTargetState {
                    format: hdr_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: PipelineCompilationOptions::default(),
            }),
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Bind group
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("media-blit-bg"),
            layout: &bind_group_layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&frame_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&frame_sampler),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: uniform_buffer.as_entire_binding(),
                },
            ],
        });

        log::info!(
            "Media layer created: {}x{}, {} frame{}",
            media_width,
            media_height,
            total_frames,
            if total_frames == 1 { "" } else { "s" }
        );

        Self {
            source,
            file_path,
            file_name,
            transport,
            current_frame: 0,
            frame_elapsed_ms: 0.0,
            frame_texture,
            frame_view,
            frame_sampler,
            output_target,
            blit_pipeline,
            bind_group_layout,
            bind_group,
            uniform_buffer,
            media_width,
            media_height,
            needs_upload: false,
            pingpong_forward: true,
            #[cfg(feature = "webcam")]
            live_frame_data: None,
            #[cfg(feature = "webcam")]
            mirror: false,
        }
    }

    /// Execute the blit pass, rendering the media frame to the HDR output target.
    pub fn execute(&self, encoder: &mut CommandEncoder) -> &RenderTarget {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("media-blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.output_target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.blit_pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
        &self.output_target
    }

    /// Advance media playback by dt seconds. Sets needs_upload if frame changed.
    pub fn advance(&mut self, dt_secs: f32) {
        if !self.transport.playing {
            return;
        }

        let delays_ms = match &self.source {
            MediaSource::Animated { delays_ms, .. } => delays_ms,
            MediaSource::Static(_) => return,
            #[cfg(feature = "webcam")]
            MediaSource::Live { .. } => return, // live frames set externally
        };

        let num_frames = delays_ms.len();
        if num_frames <= 1 {
            return;
        }

        let dt_ms = dt_secs as f64 * 1000.0 * self.transport.speed as f64;
        self.frame_elapsed_ms += dt_ms;

        let current_delay = delays_ms[self.current_frame] as f64;
        if self.frame_elapsed_ms >= current_delay {
            self.frame_elapsed_ms -= current_delay;
            let prev_frame = self.current_frame;

            match self.transport.direction {
                PlayDirection::Forward => {
                    self.current_frame += 1;
                    if self.current_frame >= num_frames {
                        if self.transport.looping {
                            self.current_frame = 0;
                        } else {
                            self.current_frame = num_frames - 1;
                            self.transport.playing = false;
                        }
                    }
                }
                PlayDirection::Reverse => {
                    if self.current_frame == 0 {
                        if self.transport.looping {
                            self.current_frame = num_frames - 1;
                        } else {
                            self.transport.playing = false;
                        }
                    } else {
                        self.current_frame -= 1;
                    }
                }
                PlayDirection::PingPong => {
                    if self.pingpong_forward {
                        self.current_frame += 1;
                        if self.current_frame >= num_frames {
                            self.current_frame = num_frames.saturating_sub(2);
                            self.pingpong_forward = false;
                        }
                    } else {
                        if self.current_frame == 0 {
                            self.pingpong_forward = true;
                            self.current_frame = 1.min(num_frames - 1);
                        } else {
                            self.current_frame -= 1;
                        }
                    }
                }
            }

            if self.current_frame != prev_frame {
                self.needs_upload = true;
            }
        }
    }

    /// Upload current frame data to GPU texture if needed.
    pub fn upload_frame(&mut self, queue: &Queue) {
        if !self.needs_upload {
            return;
        }
        self.needs_upload = false;

        // For live sources, use the externally-set frame data
        #[cfg(feature = "webcam")]
        let live_data;
        let frame_data: &[u8] = match &self.source {
            MediaSource::Static(f) => &f.data,
            MediaSource::Animated { frames, .. } => {
                &frames[self.current_frame.min(frames.len() - 1)].data
            }
            #[cfg(feature = "webcam")]
            MediaSource::Live { .. } => {
                live_data = self.live_frame_data.take();
                match &live_data {
                    Some(data) => data,
                    None => return,
                }
            }
        };

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.frame_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            frame_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.media_width * 4),
                rows_per_image: Some(self.media_height),
            },
            wgpu::Extent3d {
                width: self.media_width,
                height: self.media_height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Resize output target and recompute letterbox uniforms. Rebuilds bind group.
    pub fn resize(&mut self, device: &Device, queue: &Queue, width: u32, height: u32) {
        self.output_target.resize(device, width, height);

        // Recompute letterbox
        let mirror = {
            #[cfg(feature = "webcam")]
            { self.mirror }
            #[cfg(not(feature = "webcam"))]
            { false }
        };
        let uniforms = compute_media_uniforms(self.media_width, self.media_height, width, height, mirror);
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        // Rebuild bind group (output_target view changed but frame texture/sampler/uniform didn't)
        self.bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("media-blit-bg"),
            layout: &self.bind_group_layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&self.frame_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&self.frame_sampler),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
            ],
        });
    }

    pub fn is_animated(&self) -> bool {
        self.source.is_animated()
    }

    pub fn is_video(&self) -> bool {
        self.source.is_video()
    }

    pub fn is_live(&self) -> bool {
        self.source.is_live()
    }

    /// Update mirror state and re-upload uniforms.
    #[cfg(feature = "webcam")]
    pub fn set_mirror(&mut self, queue: &Queue, mirror: bool) {
        self.mirror = mirror;
        let uniforms = compute_media_uniforms(
            self.media_width,
            self.media_height,
            self.output_target.width,
            self.output_target.height,
            mirror,
        );
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Set live frame data from webcam capture thread.
    #[cfg(feature = "webcam")]
    pub fn set_live_frame(&mut self, data: Vec<u8>) {
        self.live_frame_data = Some(data);
        self.needs_upload = true;
    }

    pub fn frame_count(&self) -> usize {
        self.source.frame_count()
    }

    /// Seek to a specific frame index. Instant random access (pre-decoded).
    pub fn seek_to_frame(&mut self, frame: usize) {
        let num_frames = self.source.frame_count();
        if num_frames == 0 {
            return;
        }
        let target = frame.min(num_frames - 1);
        if target != self.current_frame {
            self.current_frame = target;
            self.frame_elapsed_ms = 0.0;
            self.needs_upload = true;
        }
    }

    /// Seek to a time position in seconds. Converts to frame index.
    pub fn seek_to_secs(&mut self, secs: f64) {
        if let MediaSource::Animated { delays_ms, .. } = &self.source {
            // Walk delays to find frame at this time offset
            let target_ms = secs * 1000.0;
            let mut accum = 0.0;
            for (i, &d) in delays_ms.iter().enumerate() {
                accum += d as f64;
                if accum > target_ms {
                    self.seek_to_frame(i);
                    return;
                }
            }
            // Past the end — seek to last frame
            self.seek_to_frame(delays_ms.len().saturating_sub(1));
        }
    }

    /// Current playback position in seconds (computed from current_frame).
    pub fn position_secs(&self) -> f64 {
        if let MediaSource::Animated { delays_ms, .. } = &self.source {
            let ms: f64 = delays_ms
                .iter()
                .take(self.current_frame)
                .map(|&d| d as f64)
                .sum();
            ms / 1000.0
        } else {
            0.0
        }
    }

    /// Total duration in seconds.
    pub fn duration_secs(&self) -> f64 {
        self.transport.duration / 1000.0
    }
}

/// Compute letterbox scale and offset to fit media into viewport (fit mode).
fn compute_media_uniforms(
    media_w: u32,
    media_h: u32,
    viewport_w: u32,
    viewport_h: u32,
    mirror: bool,
) -> MediaUniforms {
    let media_aspect = media_w as f32 / media_h.max(1) as f32;
    let viewport_aspect = viewport_w as f32 / viewport_h.max(1) as f32;

    let (scale_x, scale_y) = if media_aspect > viewport_aspect {
        // Media is wider — fit width, letterbox top/bottom
        (1.0, viewport_aspect / media_aspect)
    } else {
        // Media is taller — fit height, pillarbox left/right
        (media_aspect / viewport_aspect, 1.0)
    };

    let offset_x = (1.0 - scale_x) * 0.5;
    let offset_y = (1.0 - scale_y) * 0.5;

    MediaUniforms {
        scale: [scale_x, scale_y],
        offset: [offset_x, offset_y],
        mirror: mirror as u32,
        _pad: 0,
    }
}
