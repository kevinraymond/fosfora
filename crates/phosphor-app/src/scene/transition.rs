use wgpu::{
    BindGroupLayout, CommandEncoder, Device, Queue, RenderPipeline, Sampler, TextureFormat,
};

use crate::gpu::render_target::RenderTarget;

/// GPU crossfade renderer for dissolve transitions.
/// Lazily allocated — only created when first dissolve is triggered.
pub struct TransitionRenderer {
    pipeline: RenderPipeline,
    bgl: BindGroupLayout,
    sampler: Sampler,
    uniform_buffer: wgpu::Buffer,
    /// Snapshot of the outgoing scene's compositor output.
    snapshot: Option<RenderTarget>,
    /// Extra target for crossfade output.
    output: Option<RenderTarget>,
}

impl TransitionRenderer {
    pub fn new(device: &Device, hdr_format: TextureFormat) -> Self {
        let shader_src = include_str!("../../../../assets/shaders/builtin/crossfade.wgsl");
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("crossfade"),
            source: wgpu::ShaderSource::Wgsl(shader_src.into()),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("crossfade_bgl"),
            entries: &[
                // binding 0: texture A (outgoing)
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
                // binding 1: sampler
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // binding 2: texture B (incoming)
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // binding 3: uniform (progress)
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("crossfade_layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("crossfade_pipeline"),
            layout: Some(&pipeline_layout),
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
                    format: hdr_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("crossfade_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("crossfade_uniform"),
            size: 16, // vec4f (progress + padding)
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bgl,
            sampler,
            uniform_buffer,
            snapshot: None,
            output: None,
        }
    }

    /// Ensure snapshot and output targets are allocated at the given size.
    fn ensure_targets(&mut self, device: &Device, width: u32, height: u32, format: TextureFormat) {
        let needs = |t: &Option<RenderTarget>| {
            t.as_ref()
                .map_or(true, |r| r.width != width || r.height != height)
        };
        if needs(&self.snapshot) {
            self.snapshot = Some(RenderTarget::new(
                device,
                width,
                height,
                format,
                1.0,
                "crossfade-snapshot",
            ));
        }
        if needs(&self.output) {
            self.output = Some(RenderTarget::new(
                device,
                width,
                height,
                format,
                1.0,
                "crossfade-output",
            ));
        }
    }

    /// Capture the current compositor output to our snapshot texture.
    /// Uses the crossfade pipeline with progress=0 (blit A only).
    pub fn capture_snapshot(
        &mut self,
        device: &Device,
        queue: &Queue,
        encoder: &mut CommandEncoder,
        source: &RenderTarget,
    ) {
        self.ensure_targets(device, source.width, source.height, source.format);
        let snapshot = self.snapshot.as_ref().unwrap();

        // Blit source to snapshot using crossfade at progress=0
        // (mix(A, B, 0) = A, so B can be anything — we use source for both)
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&[0.0f32, 0.0f32, 0.0f32, 0.0f32]),
        );

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("crossfade_capture_bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&source.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("crossfade_capture"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &snapshot.view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Render crossfade between snapshot (outgoing) and incoming target.
    /// Returns the output render target.
    pub fn crossfade<'a>(
        &'a self,
        device: &Device,
        queue: &Queue,
        encoder: &mut CommandEncoder,
        incoming: &RenderTarget,
        progress: f32,
    ) -> Option<&'a RenderTarget> {
        let snapshot = self.snapshot.as_ref()?;
        let output = self.output.as_ref()?;

        // Upload progress
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&[progress, 0.0f32, 0.0f32, 0.0f32]),
        );

        // Build bind group
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("crossfade_bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&snapshot.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&incoming.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
            ],
        });

        // Render fullscreen pass
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("crossfade_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &output.view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1); // fullscreen triangle

        Some(output)
    }

    /// Resize targets on window resize.
    pub fn resize(&mut self, device: &Device, width: u32, height: u32, format: TextureFormat) {
        if self.snapshot.is_some() || self.output.is_some() {
            self.ensure_targets(device, width, height, format);
        }
    }

    /// Whether we have a valid snapshot to crossfade from.
    pub fn has_snapshot(&self) -> bool {
        self.snapshot.is_some()
    }
}
