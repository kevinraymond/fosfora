use anyhow::Result;
use wgpu::{
    BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingType,
    BufferBindingType, ColorTargetState, Device, FragmentState, MultisampleState,
    PipelineCompilationOptions, PipelineLayoutDescriptor, PrimitiveState, RenderPipeline,
    SamplerBindingType, ShaderModule, ShaderStages, TextureFormat, TextureSampleType,
    TextureViewDimension, VertexState,
};

use super::fullscreen_quad::FULLSCREEN_TRIANGLE_VS;

pub struct ShaderPipeline {
    pub pipeline: RenderPipeline,
    pub bind_group_layout: BindGroupLayout,
}

impl ShaderPipeline {
    pub fn new(
        device: &Device,
        format: TextureFormat,
        fragment_source: &str,
        cache: Option<&wgpu::PipelineCache>,
    ) -> Result<Self> {
        let bind_group_layout = Self::create_bind_group_layout(device);

        // Combine vertex + fragment into one module
        let full_source = format!("{}\n{}", FULLSCREEN_TRIANGLE_VS, fragment_source);

        device.push_error_scope(wgpu::ErrorFilter::Validation);

        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("phosphor-shader"),
            source: wgpu::ShaderSource::Wgsl(full_source.into()),
        });

        let pipeline =
            Self::create_pipeline(device, format, &bind_group_layout, &shader_module, cache);

        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            return Err(anyhow::anyhow!("{error}"));
        }

        Ok(Self {
            pipeline,
            bind_group_layout,
        })
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &Device,
        format: TextureFormat,
        fragment_source: &str,
        cache: Option<&wgpu::PipelineCache>,
    ) -> Result<(), String> {
        let full_source = format!("{}\n{}", FULLSCREEN_TRIANGLE_VS, fragment_source);

        // Push an error scope to catch shader compilation and pipeline creation errors
        // instead of letting wgpu panic with the default error handler.
        device.push_error_scope(wgpu::ErrorFilter::Validation);

        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("phosphor-shader"),
            source: wgpu::ShaderSource::Wgsl(full_source.into()),
        });

        let pipeline =
            Self::create_pipeline(device, format, &self.bind_group_layout, &shader_module, cache);

        // Check if any validation errors occurred during shader/pipeline creation.
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            return Err(format!("{error}"));
        }

        self.pipeline = pipeline;
        Ok(())
    }

    fn create_bind_group_layout(device: &Device) -> BindGroupLayout {
        device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("phosphor-bind-group-layout"),
            entries: &[
                // binding(0): uniform buffer
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding(1): previous frame texture (feedback)
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // binding(2): sampler for previous frame
                BindGroupLayoutEntry {
                    binding: 2,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        })
    }

    fn create_pipeline(
        device: &Device,
        format: TextureFormat,
        bind_group_layout: &BindGroupLayout,
        shader_module: &ShaderModule,
        cache: Option<&wgpu::PipelineCache>,
    ) -> RenderPipeline {
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("phosphor-pipeline-layout"),
            bind_group_layouts: &[bind_group_layout],
            push_constant_ranges: &[],
        });

        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("phosphor-render-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: shader_module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: PipelineCompilationOptions::default(),
            },
            fragment: Some(FragmentState {
                module: shader_module,
                entry_point: Some("fs_main"),
                targets: &[Some(ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: PipelineCompilationOptions::default(),
            }),
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview: None,
            cache,
        })
    }
}
