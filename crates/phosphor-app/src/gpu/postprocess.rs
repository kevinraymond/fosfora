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

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
struct PostParams {
    bloom_intensity: f32,
    ca_intensity: f32,
    vignette_strength: f32,
    grain_intensity: f32,
    time: f32,
    rms: f32,
    _pad: [f32; 2],
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
                tex_entry(0),       // scene
                sampler_entry(1),   // scene sampler
                tex_entry(2),       // bloom
                sampler_entry(3),   // bloom sampler
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
        let bloom_params_buffer = create_uniform_buffer(device, "bloom-params", std::mem::size_of::<BloomParams>());
        let blur_h_params_buffer = create_uniform_buffer(device, "blur-h-params", std::mem::size_of::<BlurParams>());
        let blur_v_params_buffer = create_uniform_buffer(device, "blur-v-params", std::mem::size_of::<BlurParams>());
        let post_params_buffer = create_uniform_buffer(device, "post-params", std::mem::size_of::<PostParams>());

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
        queue.write_buffer(&self.bloom_params_buffer, 0, bytemuck::bytes_of(&bloom_params));

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
        queue.write_buffer(&self.blur_h_params_buffer, 0, bytemuck::bytes_of(&blur_h_params));
        queue.write_buffer(&self.blur_v_params_buffer, 0, bytemuck::bytes_of(&blur_v_params));

        let bloom_active = overrides.bloom_enabled;

        let post_params = PostParams {
            bloom_intensity: if bloom_active { overrides.bloom_intensity } else { 0.0 },
            ca_intensity: if overrides.ca_enabled { onset * overrides.ca_intensity * 0.03 } else { 0.0 },
            vignette_strength: if overrides.vignette_enabled { overrides.vignette } else { 0.0 },
            grain_intensity: if overrides.grain_enabled { flatness * overrides.grain_intensity * 0.08 } else { 0.0 },
            time,
            rms,
            _pad: [0.0; 2],
        };
        queue.write_buffer(&self.post_params_buffer, 0, bytemuck::bytes_of(&post_params));

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
    pub fn render_composite_to(
        &self,
        device: &Device,
        encoder: &mut CommandEncoder,
        source: &RenderTarget,
        capture_view: &TextureView,
    ) {
        if !self.enabled {
            let bg = device.create_bind_group(&BindGroupDescriptor {
                label: Some("ndi-blit-bg"),
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
            run_fullscreen_pass(encoder, "ndi-blit", &self.blit_pipeline, &bg, capture_view);
            return;
        }

        let bg = device.create_bind_group(&BindGroupDescriptor {
            label: Some("ndi-composite-bg"),
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
            "ndi-composite",
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
