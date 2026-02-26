use bytemuck::{Pod, Zeroable};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, BufferBindingType, ColorTargetState,
    CommandEncoder, Device, FragmentState, PipelineCompilationOptions, PipelineLayoutDescriptor,
    PrimitiveState, Queue, RenderPipeline, SamplerBindingType, ShaderStages, TextureFormat,
    TextureSampleType, TextureViewDimension, VertexState,
};

use super::fullscreen_quad::FULLSCREEN_TRIANGLE_VS_WITH_UV;
use super::layer::BlendMode;
use super::render_target::{PingPongTarget, RenderTarget};

const COMPOSITE_FS: &str = include_str!("../../../../assets/shaders/builtin/composite.wgsl");
const BLIT_FS: &str = include_str!("../../../../assets/shaders/builtin/blit.wgsl");

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
struct CompositeUniforms {
    blend_mode: u32,
    opacity: f32,
    _pad0: f32,
    _pad1: f32,
}

/// GPU compositor that blends multiple layer outputs together.
pub struct Compositor {
    composite_pipeline: RenderPipeline,
    blit_pipeline: RenderPipeline,
    composite_bgl: BindGroupLayout,
    blit_bgl: BindGroupLayout,
    uniform_buffers: Vec<wgpu::Buffer>,
    /// Ping-pong accumulator for sequential compositing.
    pub accumulator: PingPongTarget,
}

impl Compositor {
    pub fn new(
        device: &Device,
        hdr_format: TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        // Composite pipeline: bg + fg + uniforms → blended output
        let composite_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("compositor-composite-bgl"),
            entries: &[
                tex_entry(0),      // background
                sampler_entry(1),  // bg sampler
                tex_entry(2),      // foreground
                sampler_entry(3),  // fg sampler
                uniform_entry(4, std::mem::size_of::<CompositeUniforms>()),
            ],
        });
        let composite_pipeline = create_fs_pipeline(
            device,
            "compositor-composite",
            &composite_bgl,
            COMPOSITE_FS,
            hdr_format,
        );

        // Blit pipeline: copy first layer to accumulator
        let blit_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("compositor-blit-bgl"),
            entries: &[tex_entry(0), sampler_entry(1)],
        });
        let blit_pipeline = create_fs_pipeline(
            device,
            "compositor-blit",
            &blit_bgl,
            BLIT_FS,
            hdr_format,
        );

        // One uniform buffer per composite pass (max 8: 1 for first-layer opacity + 7 for layers[1..])
        let uniform_buffers: Vec<wgpu::Buffer> = (0..8)
            .map(|i| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("compositor-uniforms-{i}")),
                    size: std::mem::size_of::<CompositeUniforms>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();

        let accumulator = PingPongTarget::new(device, width, height, hdr_format, 1.0);

        Self {
            composite_pipeline,
            blit_pipeline,
            composite_bgl,
            blit_bgl,
            uniform_buffers,
            accumulator,
        }
    }

    /// Composite multiple layer outputs into a single HDR result.
    /// Returns a reference to the final composited render target.
    ///
    /// `layers` is a list of (render_target, blend_mode, opacity) for each enabled layer.
    pub fn composite<'a>(
        &'a self,
        device: &Device,
        queue: &Queue,
        encoder: &mut CommandEncoder,
        layers: &[(&RenderTarget, BlendMode, f32)],
    ) -> &'a RenderTarget {
        assert!(!layers.is_empty());

        let (first, _, first_opacity) = layers[0];

        // Handle first layer: blit if fully opaque, composite against black if not
        if first_opacity < 1.0 {
            // Composite first layer against cleared-to-black accumulator to apply opacity.
            // run_fullscreen_pass clears to black, so bg is black and fg is the first layer.
            let uniforms = CompositeUniforms {
                blend_mode: BlendMode::Normal.as_u32(),
                opacity: first_opacity,
                _pad0: 0.0,
                _pad1: 0.0,
            };
            queue.write_buffer(&self.uniform_buffers[0], 0, bytemuck::bytes_of(&uniforms));

            // We need a black background. Use the other accumulator target (cleared to black).
            let write_idx = self.accumulator.current;
            let bg_idx = 1 - write_idx;
            // Clear the bg target by running a blit-like pass (it will be cleared by LoadOp::Clear)
            // Actually, just use the composite pass — bg will be the cleared target.
            // We need to clear bg_idx first. Run a dummy clear by beginning+ending a pass.
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("compositor-clear-bg"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.accumulator.targets[bg_idx].view,
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
            }

            let composite_bg = device.create_bind_group(&BindGroupDescriptor {
                label: Some("compositor-first-layer-bg"),
                layout: &self.composite_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(&self.accumulator.targets[bg_idx].view),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::Sampler(&self.accumulator.targets[bg_idx].sampler),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: BindingResource::TextureView(&first.view),
                    },
                    BindGroupEntry {
                        binding: 3,
                        resource: BindingResource::Sampler(&first.sampler),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: self.uniform_buffers[0].as_entire_binding(),
                    },
                ],
            });

            run_fullscreen_pass(
                encoder,
                "compositor-first-opacity",
                &self.composite_pipeline,
                &composite_bg,
                &self.accumulator.targets[write_idx].view,
            );
        } else {
            // Fast path: blit first layer directly (opacity == 1.0)
            let blit_bg = device.create_bind_group(&BindGroupDescriptor {
                label: Some("compositor-blit-bg"),
                layout: &self.blit_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(&first.view),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::Sampler(&first.sampler),
                    },
                ],
            });
            run_fullscreen_pass(
                encoder,
                "compositor-blit",
                &self.blit_pipeline,
                &blit_bg,
                &self.accumulator.write_target().view,
            );
        }

        if layers.len() == 1 {
            return self.accumulator.write_target();
        }

        // Composite subsequent layers using per-pass uniform buffers.
        // After first layer handling, result is in write_target (accumulator.current).
        let mut read_idx = self.accumulator.current;

        for (pass_idx, &(fg, blend_mode, opacity)) in layers[1..].iter().enumerate() {
            let write_idx = 1 - read_idx;
            // Use buffer [pass_idx + 1] since buffer [0] may be used for first layer opacity
            let buf_idx = pass_idx + 1;

            let uniforms = CompositeUniforms {
                blend_mode: blend_mode.as_u32(),
                opacity,
                _pad0: 0.0,
                _pad1: 0.0,
            };
            queue.write_buffer(&self.uniform_buffers[buf_idx], 0, bytemuck::bytes_of(&uniforms));

            let bg_target = &self.accumulator.targets[read_idx];
            let write_target = &self.accumulator.targets[write_idx];

            let composite_bg = device.create_bind_group(&BindGroupDescriptor {
                label: Some("compositor-composite-bg"),
                layout: &self.composite_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(&bg_target.view),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::Sampler(&bg_target.sampler),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: BindingResource::TextureView(&fg.view),
                    },
                    BindGroupEntry {
                        binding: 3,
                        resource: BindingResource::Sampler(&fg.sampler),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: self.uniform_buffers[buf_idx].as_entire_binding(),
                    },
                ],
            });

            run_fullscreen_pass(
                encoder,
                "compositor-composite",
                &self.composite_pipeline,
                &composite_bg,
                &write_target.view,
            );

            read_idx = write_idx;
        }

        &self.accumulator.targets[read_idx]
    }

    pub fn resize(&mut self, device: &Device, width: u32, height: u32) {
        self.accumulator.resize(device, width, height);
    }
}

// --- Helper functions (same pattern as postprocess.rs) ---

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
    target: &wgpu::TextureView,
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
