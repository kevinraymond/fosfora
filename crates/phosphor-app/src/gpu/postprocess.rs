use bytemuck::{Pod, Zeroable};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, BufferBindingType, ColorTargetState,
    CommandEncoder, Device, FragmentState, PipelineCompilationOptions, PipelineLayoutDescriptor,
    PrimitiveState, Queue, RenderPipeline, SamplerBindingType, ShaderStages, TextureFormat,
    TextureSampleType, TextureView, TextureViewDimension, VertexState,
};

use crate::effect::format::PostProcessDef;

use super::fullscreen_quad::FULLSCREEN_TRIANGLE_VS_WITH_UV;
use super::render_target::RenderTarget;

const BLOOM_EXTRACT_FS: &str =
    include_str!("../../../../assets/shaders/builtin/bloom_extract.wgsl");
const BLOOM_BLUR_FS: &str = include_str!("../../../../assets/shaders/builtin/bloom_blur.wgsl");
const POST_COMPOSITE_FS: &str =
    include_str!("../../../../assets/shaders/builtin/post_composite.wgsl");
const BLIT_FS: &str = include_str!("../../../../assets/shaders/builtin/blit.wgsl");

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
struct BloomParams {
    threshold: f32,
    soft_knee: f32,
    rms: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
struct BlurParams {
    direction: [f32; 2],
    _pad: [f32; 2],
}

/// The per-frame resolved output-alpha mode the composite shader executes.
///
/// This is the *resolved* form of [`crate::settings::AlphaOutputMode`] — Auto has
/// already been decided by `frame_graph::resolve_output_alpha` by the time it gets
/// here. Values are the WGSL-side encoding in `PostParams::alpha_mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaMode {
    /// Alpha forced to 1.0 (historical behavior).
    Opaque,
    /// Alpha derived from output brightness (legacy NDI luma key).
    Luma,
    /// The scene's real coverage alpha survives to the output (premultiplied).
    Passthrough,
}

impl AlphaMode {
    fn as_f32(self) -> f32 {
        match self {
            AlphaMode::Opaque => 0.0,
            AlphaMode::Luma => 1.0,
            AlphaMode::Passthrough => 2.0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
struct PostParams {
    bloom_intensity: f32,
    ca_intensity: f32,
    vignette_strength: f32,
    grain_intensity: f32,
    time: f32,
    rms: f32,
    /// 0 = opaque, 1 = luma-derived, 2 = scene-alpha passthrough ([`AlphaMode`]).
    alpha_mode: f32,
    tonemap_mode: f32, // 0 = ACES (house look), 1 = linear passthrough (SuperSplat-faithful)
    grain_rate: f32,   // grain updates per second; <= 0 = every frame (see #1983)
    _pad: [f32; 3],
}

pub struct PostProcessChain {
    pub enabled: bool,
    // Quarter-res targets for bloom
    bloom_extract_target: RenderTarget,
    bloom_blur_h_target: RenderTarget,
    bloom_blur_v_target: RenderTarget,
    // Pipelines
    extract_pipeline: RenderPipeline,
    blur_pipeline: RenderPipeline,
    composite_pipeline: RenderPipeline,
    blit_pipeline: RenderPipeline,
    // Bind group layouts
    extract_bgl: BindGroupLayout,
    blur_bgl: BindGroupLayout,
    composite_bgl: BindGroupLayout,
    blit_bgl: BindGroupLayout,
    // Uniform buffers
    bloom_params_buffer: wgpu::Buffer,
    blur_h_params_buffer: wgpu::Buffer,
    blur_v_params_buffer: wgpu::Buffer,
    post_params_buffer: wgpu::Buffer,
    // Stored for potential resize rebuilds
    #[allow(dead_code)]
    surface_format: TextureFormat,
    #[allow(dead_code)]
    hdr_format: TextureFormat,
}

impl PostProcessChain {
    pub fn new(
        device: &Device,
        surface_format: TextureFormat,
        hdr_format: TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        // Quarter-res bloom targets
        let bloom_extract_target =
            RenderTarget::new(device, width, height, hdr_format, 0.25, "bloom-extract");
        let bloom_blur_h_target =
            RenderTarget::new(device, width, height, hdr_format, 0.25, "bloom-blur-h");
        let bloom_blur_v_target =
            RenderTarget::new(device, width, height, hdr_format, 0.25, "bloom-blur-v");

        // --- Bloom Extract pipeline ---
        let extract_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("bloom-extract-bgl"),
            entries: &[
                tex_entry(0),
                sampler_entry(1),
                uniform_entry(2, std::mem::size_of::<BloomParams>()),
            ],
        });
        let extract_pipeline = create_fs_pipeline(
            device,
            "bloom-extract",
            &extract_bgl,
            BLOOM_EXTRACT_FS,
            hdr_format,
        );

        // --- Bloom Blur pipeline (same for H and V, direction via uniform) ---
        let blur_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("bloom-blur-bgl"),
            entries: &[
                tex_entry(0),
                sampler_entry(1),
                uniform_entry(2, std::mem::size_of::<BlurParams>()),
            ],
        });
        let blur_pipeline =
            create_fs_pipeline(device, "bloom-blur", &blur_bgl, BLOOM_BLUR_FS, hdr_format);

        // --- Composite pipeline (scene + bloom → surface) ---
        let composite_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("post-composite-bgl"),
            entries: &[
                tex_entry(0),     // scene
                sampler_entry(1), // scene sampler
                tex_entry(2),     // bloom
                sampler_entry(3), // bloom sampler
                uniform_entry(4, std::mem::size_of::<PostParams>()),
            ],
        });
        let composite_pipeline = create_fs_pipeline(
            device,
            "post-composite",
            &composite_bgl,
            POST_COMPOSITE_FS,
            surface_format,
        );

        // --- Simple blit pipeline (fallback when post-processing disabled) ---
        let blit_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("post-blit-bgl"),
            entries: &[tex_entry(0), sampler_entry(1)],
        });
        let blit_pipeline =
            create_fs_pipeline(device, "post-blit", &blit_bgl, BLIT_FS, surface_format);

        // Uniform buffers
        let bloom_params_buffer =
            create_uniform_buffer(device, "bloom-params", std::mem::size_of::<BloomParams>());
        let blur_h_params_buffer =
            create_uniform_buffer(device, "blur-h-params", std::mem::size_of::<BlurParams>());
        let blur_v_params_buffer =
            create_uniform_buffer(device, "blur-v-params", std::mem::size_of::<BlurParams>());
        let post_params_buffer =
            create_uniform_buffer(device, "post-params", std::mem::size_of::<PostParams>());

        Self {
            enabled: true,
            bloom_extract_target,
            bloom_blur_h_target,
            bloom_blur_v_target,
            extract_pipeline,
            blur_pipeline,
            composite_pipeline,
            blit_pipeline,
            extract_bgl,
            blur_bgl,
            composite_bgl,
            blit_bgl,
            bloom_params_buffer,
            blur_h_params_buffer,
            blur_v_params_buffer,
            post_params_buffer,
            surface_format,
            hdr_format,
        }
    }

    pub fn resize(&mut self, device: &Device, width: u32, height: u32) {
        self.bloom_extract_target.resize(device, width, height);
        self.bloom_blur_h_target.resize(device, width, height);
        self.bloom_blur_v_target.resize(device, width, height);
    }

    /// Render the post-processing chain.
    /// `source` is the HDR effect output, renders to `surface_view`.
    pub fn render(
        &self,
        device: &Device,
        queue: &Queue,
        encoder: &mut CommandEncoder,
        source: &RenderTarget,
        surface_view: &TextureView,
        time: f32,
        rms: f32,
        onset: f32,
        flatness: f32,
        overrides: &PostProcessDef,
        alpha_mode: AlphaMode,
    ) {
        if !self.enabled {
            // Simple blit fallback
            let bg = device.create_bind_group(&BindGroupDescriptor {
                label: Some("post-blit-bg"),
                layout: &self.blit_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(&source.view),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::Sampler(&source.sampler),
                    },
                ],
            });
            run_fullscreen_pass(encoder, "post-blit", &self.blit_pipeline, &bg, surface_view);
            return;
        }

        // --- Update uniforms ---
        let bloom_params = BloomParams {
            threshold: overrides.bloom_threshold,
            soft_knee: 0.3,
            rms,
            _pad: 0.0,
        };
        queue.write_buffer(
            &self.bloom_params_buffer,
            0,
            bytemuck::bytes_of(&bloom_params),
        );

        // Blur directions in texel units
        let blur_w = self.bloom_extract_target.width as f32;
        let blur_h = self.bloom_extract_target.height as f32;
        let blur_h_params = BlurParams {
            direction: [1.0 / blur_w, 0.0],
            _pad: [0.0; 2],
        };
        let blur_v_params = BlurParams {
            direction: [0.0, 1.0 / blur_h],
            _pad: [0.0; 2],
        };
        queue.write_buffer(
            &self.blur_h_params_buffer,
            0,
            bytemuck::bytes_of(&blur_h_params),
        );
        queue.write_buffer(
            &self.blur_v_params_buffer,
            0,
            bytemuck::bytes_of(&blur_v_params),
        );

        let bloom_active = overrides.bloom_enabled;

        let post_params = PostParams {
            bloom_intensity: if bloom_active {
                overrides.bloom_intensity
            } else {
                0.0
            },
            ca_intensity: if overrides.ca_enabled {
                onset * overrides.ca_intensity * 0.03
            } else {
                0.0
            },
            vignette_strength: if overrides.vignette_enabled {
                overrides.vignette
            } else {
                0.0
            },
            grain_intensity: if overrides.grain_enabled {
                flatness * overrides.grain_intensity * 0.08
            } else {
                0.0
            },
            time,
            rms,
            alpha_mode: alpha_mode.as_f32(),
            tonemap_mode: if overrides.tonemap == "linear" {
                1.0
            } else {
                0.0
            },
            grain_rate: if overrides.grain_enabled {
                overrides.grain_rate
            } else {
                0.0
            },
            _pad: [0.0; 3],
        };
        queue.write_buffer(
            &self.post_params_buffer,
            0,
            bytemuck::bytes_of(&post_params),
        );

        // --- Bloom passes (skip all 3 when bloom disabled) ---
        if bloom_active {
            // Pass 1: Bloom Extract (HDR scene → quarter-res bright pixels)
            {
                let bg = device.create_bind_group(&BindGroupDescriptor {
                    label: Some("bloom-extract-bg"),
                    layout: &self.extract_bgl,
                    entries: &[
                        BindGroupEntry {
                            binding: 0,
                            resource: BindingResource::TextureView(&source.view),
                        },
                        BindGroupEntry {
                            binding: 1,
                            resource: BindingResource::Sampler(&source.sampler),
                        },
                        BindGroupEntry {
                            binding: 2,
                            resource: self.bloom_params_buffer.as_entire_binding(),
                        },
                    ],
                });
                run_fullscreen_pass(
                    encoder,
                    "bloom-extract",
                    &self.extract_pipeline,
                    &bg,
                    &self.bloom_extract_target.view,
                );
            }

            // Pass 2: Horizontal blur
            {
                let bg = device.create_bind_group(&BindGroupDescriptor {
                    label: Some("bloom-blur-h-bg"),
                    layout: &self.blur_bgl,
                    entries: &[
                        BindGroupEntry {
                            binding: 0,
                            resource: BindingResource::TextureView(&self.bloom_extract_target.view),
                        },
                        BindGroupEntry {
                            binding: 1,
                            resource: BindingResource::Sampler(&self.bloom_extract_target.sampler),
                        },
                        BindGroupEntry {
                            binding: 2,
                            resource: self.blur_h_params_buffer.as_entire_binding(),
                        },
                    ],
                });
                run_fullscreen_pass(
                    encoder,
                    "bloom-blur-h",
                    &self.blur_pipeline,
                    &bg,
                    &self.bloom_blur_h_target.view,
                );
            }

            // Pass 3: Vertical blur
            {
                let bg = device.create_bind_group(&BindGroupDescriptor {
                    label: Some("bloom-blur-v-bg"),
                    layout: &self.blur_bgl,
                    entries: &[
                        BindGroupEntry {
                            binding: 0,
                            resource: BindingResource::TextureView(&self.bloom_blur_h_target.view),
                        },
                        BindGroupEntry {
                            binding: 1,
                            resource: BindingResource::Sampler(&self.bloom_blur_h_target.sampler),
                        },
                        BindGroupEntry {
                            binding: 2,
                            resource: self.blur_v_params_buffer.as_entire_binding(),
                        },
                    ],
                });
                run_fullscreen_pass(
                    encoder,
                    "bloom-blur-v",
                    &self.blur_pipeline,
                    &bg,
                    &self.bloom_blur_v_target.view,
                );
            }
        }

        // --- Composite pass (scene + blurred bloom → surface) ---
        {
            let bg = device.create_bind_group(&BindGroupDescriptor {
                label: Some("post-composite-bg"),
                layout: &self.composite_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(&source.view),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::Sampler(&source.sampler),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: BindingResource::TextureView(&self.bloom_blur_v_target.view),
                    },
                    BindGroupEntry {
                        binding: 3,
                        resource: BindingResource::Sampler(&self.bloom_blur_v_target.sampler),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: self.post_params_buffer.as_entire_binding(),
                    },
                ],
            });
            run_fullscreen_pass(
                encoder,
                "post-composite",
                &self.composite_pipeline,
                &bg,
                surface_view,
            );
        }
    }

    /// Render the final composite (or blit) to a secondary capture target.
    /// Reuses existing bloom results and uniform buffers — only runs the final pass.
    #[allow(dead_code)]
    pub fn render_composite_to(
        &self,
        device: &Device,
        encoder: &mut CommandEncoder,
        source: &RenderTarget,
        capture_view: &TextureView,
    ) {
        if !self.enabled {
            let bg = device.create_bind_group(&BindGroupDescriptor {
                label: Some("output-blit-bg"),
                layout: &self.blit_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(&source.view),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::Sampler(&source.sampler),
                    },
                ],
            });
            run_fullscreen_pass(
                encoder,
                "output-blit",
                &self.blit_pipeline,
                &bg,
                capture_view,
            );
            return;
        }

        let bg = device.create_bind_group(&BindGroupDescriptor {
            label: Some("output-composite-bg"),
            layout: &self.composite_bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&source.view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&source.sampler),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::TextureView(&self.bloom_blur_v_target.view),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: BindingResource::Sampler(&self.bloom_blur_v_target.sampler),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: self.post_params_buffer.as_entire_binding(),
                },
            ],
        });
        run_fullscreen_pass(
            encoder,
            "output-composite",
            &self.composite_pipeline,
            &bg,
            capture_view,
        );
    }
}

// --- Helper functions ---

fn tex_entry(binding: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Texture {
            sample_type: TextureSampleType::Float { filterable: true },
            view_dimension: TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Sampler(SamplerBindingType::Filtering),
        count: None,
    }
}

fn uniform_entry(binding: u32, size: usize) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Buffer {
            ty: BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: std::num::NonZeroU64::new(size as u64),
        },
        count: None,
    }
}

fn create_uniform_buffer(device: &Device, label: &str, size: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_fs_pipeline(
    device: &Device,
    label: &str,
    bgl: &BindGroupLayout,
    fragment_src: &str,
    target_format: TextureFormat,
) -> RenderPipeline {
    let full_source = format!("{FULLSCREEN_TRIANGLE_VS_WITH_UV}\n{fragment_src}");
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(full_source.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some(&format!("{label}-layout")),
        bind_group_layouts: &[bgl],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(&format!("{label}-pipeline")),
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
                format: target_format,
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
    })
}

fn run_fullscreen_pass(
    encoder: &mut CommandEncoder,
    label: &str,
    pipeline: &RenderPipeline,
    bind_group: &BindGroup,
    target: &TextureView,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
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
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The WGSL mirror of `PostParams` is maintained by hand and uniform structs
    /// need a 16-byte multiple. `grain_rate` took the struct from 32 to 48 with
    /// three pad words; if that stops being true the shader's copy must follow.
    #[test]
    fn post_params_stay_forty_eight_bytes() {
        assert_eq!(std::mem::size_of::<PostParams>(), 48);
    }

    const PROBE_DIM: u32 = 64;

    /// Render one post-composite pass over a flat mid-grey scene and read it
    /// back. Rgba8Unorm rather than the production surface format: one row is
    /// exactly the 256-byte copy alignment and the readback needs no decode.
    fn probe_composite(device: &Device, queue: &Queue, params: PostParams) -> Vec<u8> {
        let dim = PROBE_DIM;
        let format = TextureFormat::Rgba8Unorm;

        let make_input = |label: &str, fill: u8| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: dim,
                    height: dim,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let mut px = vec![fill; (dim * dim * 4) as usize];
            for y in 0..dim {
                for x in 0..dim {
                    px[((y * dim + x) * 4 + 3) as usize] = 255;
                }
            }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &px,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(dim * 4),
                    rows_per_image: Some(dim),
                },
                wgpu::Extent3d {
                    width: dim,
                    height: dim,
                    depth_or_array_layers: 1,
                },
            );
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            (tex, view)
        };

        // Mid-grey scene, black bloom: the only thing varying across pixels is
        // the grain, so any difference between two renders is the grain moving.
        let (_scene_tex, scene_view) = make_input("probe-scene", 128);
        let (_bloom_tex, bloom_view) = make_input("probe-bloom", 0);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("probe-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let out = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("probe-out"),
            size: wgpu::Extent3d {
                width: dim,
                height: dim,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let out_view = out.create_view(&wgpu::TextureViewDescriptor::default());

        let bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("probe-post-bgl"),
            entries: &[
                tex_entry(0),
                sampler_entry(1),
                tex_entry(2),
                sampler_entry(3),
                uniform_entry(4, std::mem::size_of::<PostParams>()),
            ],
        });
        let pipeline = create_fs_pipeline(device, "probe-post", &bgl, POST_COMPOSITE_FS, format);

        let ubo =
            create_uniform_buffer(device, "probe-post-ubo", std::mem::size_of::<PostParams>());
        queue.write_buffer(&ubo, 0, bytemuck::bytes_of(&params));

        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("probe-post-bg"),
            layout: &bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&scene_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&sampler),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::TextureView(&bloom_view),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: BindingResource::Sampler(&sampler),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: ubo.as_entire_binding(),
                },
            ],
        });

        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe-post-readback"),
            size: (dim * dim * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&Default::default());
        run_fullscreen_pass(
            &mut encoder,
            "probe-post-pass",
            &pipeline,
            &bind_group,
            &out_view,
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &out,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(dim * 4),
                    rows_per_image: Some(dim),
                },
            },
            wgpu::Extent3d {
                width: dim,
                height: dim,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::once(encoder.finish()));

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

    /// Grain only, no bloom/CA/vignette, linear tonemap so the noise is not
    /// compressed away. `grain_intensity` here is the final shader multiplier —
    /// the CPU side folds flatness and the 0.08 scale in before this point.
    fn grain_only(time: f32, grain_rate: f32) -> PostParams {
        PostParams {
            bloom_intensity: 0.0,
            ca_intensity: 0.0,
            vignette_strength: 0.0,
            grain_intensity: 0.5,
            time,
            rms: 0.0,
            alpha_mode: 0.0,
            tonemap_mode: 1.0,
            grain_rate,
            _pad: [0.0; 3],
        }
    }

    /// #1983: the grain must hold its pattern for the whole of a tick and
    /// change across a tick boundary. That is the entire mitigation — a
    /// display that repeats a frame can then only repeat a pattern the grain
    /// was already going to hold, instead of freezing a boiling field.
    ///
    /// Run: cargo test -p phosphor-app -- --ignored grain_holds_within_a_tick
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn grain_holds_within_a_tick() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, queue) = crate::gpu::test_gpu::test_gpu();

        // At 24 Hz a tick is 41.7 ms. 1.000 and 1.030 share tick 24; 1.050 is
        // tick 25. All three are more than a 60 Hz frame apart, so without
        // quantization every one of them would differ.
        let a = probe_composite(&device, &queue, grain_only(1.000, 24.0));
        let b = probe_composite(&device, &queue, grain_only(1.030, 24.0));
        let c = probe_composite(&device, &queue, grain_only(1.050, 24.0));

        assert_eq!(a, b, "grain changed inside a single 24 Hz tick");
        assert_ne!(b, c, "grain did not change across a 24 Hz tick boundary");

        // The probe is only meaningful if there is grain to see at all.
        let flat = probe_composite(&device, &queue, {
            let mut p = grain_only(1.000, 24.0);
            p.grain_intensity = 0.0;
            p
        });
        assert_ne!(
            a, flat,
            "no grain in the probe — the assertions prove nothing"
        );
    }

    /// The opt-out has to be exact: `grain_rate = 0` restores the every-frame
    /// grain this field was added to slow down, so a preset that sets it gets
    /// the pre-#1983 look and not an approximation of it.
    ///
    /// Run: cargo test -p phosphor-app -- --ignored grain_rate_zero_updates_every_frame
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn grain_rate_zero_updates_every_frame() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, queue) = crate::gpu::test_gpu::test_gpu();

        // Same two times that share a tick at 24 Hz: with the rate off they
        // must differ, which is what "every frame" means.
        let a = probe_composite(&device, &queue, grain_only(1.000, 0.0));
        let b = probe_composite(&device, &queue, grain_only(1.030, 0.0));
        assert_ne!(a, b, "grain_rate = 0 should still update every frame");
    }
}
