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
    /// `input_count` is the number of prior-pass outputs this shader samples as
    /// `input0..inputN-1` (multi-input pass graph, #1481). Each reserves a
    /// texture at binding `7+2i` and a sampler at `8+2i`. Single-shader effects
    /// pass 0.
    pub fn new(
        device: &Device,
        format: TextureFormat,
        fragment_source: &str,
        cache: Option<&wgpu::PipelineCache>,
        input_count: usize,
    ) -> Result<Self> {
        let bind_group_layout = Self::create_bind_group_layout(device, input_count);

        // Combine vertex + fragment into one module
        let full_source = format!("{}\n{}", FULLSCREEN_TRIANGLE_VS, fragment_source);

        device.push_error_scope(wgpu::ErrorFilter::Validation);

        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fosfora-shader"),
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

    #[allow(dead_code)]
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
            label: Some("fosfora-shader"),
            source: wgpu::ShaderSource::Wgsl(full_source.into()),
        });

        let pipeline = Self::create_pipeline(
            device,
            format,
            &self.bind_group_layout,
            &shader_module,
            cache,
        );

        // Check if any validation errors occurred during shader/pipeline creation.
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            return Err(format!("{error}"));
        }

        self.pipeline = pipeline;
        Ok(())
    }

    fn create_bind_group_layout(device: &Device, input_count: usize) -> BindGroupLayout {
        // Fixed bindings 0-6, then `input_count` multi-pass inputs at 7+2i (texture) /
        // 8+2i (sampler) — the #1481 pass graph. Built as a Vec so the input tail is
        // variable-length.
        let mut entries: Vec<BindGroupLayoutEntry> = vec![
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
            // A17 audio textures (batched ABI bump #1505). All are
            // Float{filterable} 2D views, so a 1x1 placeholder and the real
            // waveform/spectrum/spectrogram textures satisfy the same layout.
            // binding(3): waveform (min/max-decimated PCM)
            BindGroupLayoutEntry {
                binding: 3,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            // binding(4): spectrum (log-resampled magnitude)
            BindGroupLayoutEntry {
                binding: 4,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            // binding(5): spectrogram (scrolling mel-band history)
            BindGroupLayoutEntry {
                binding: 5,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            // binding(6): shared sampler for the audio textures
            BindGroupLayoutEntry {
                binding: 6,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Sampler(SamplerBindingType::Filtering),
                count: None,
            },
        ];

        // Multi-pass graph inputs (#1481): a texture + sampler pair per declared input.
        for i in 0..input_count as u32 {
            entries.push(BindGroupLayoutEntry {
                binding: 7 + 2 * i,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
            entries.push(BindGroupLayoutEntry {
                binding: 8 + 2 * i,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Sampler(SamplerBindingType::Filtering),
                count: None,
            });
        }

        device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("fosfora-bind-group-layout"),
            entries: &entries,
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
            label: Some("fosfora-pipeline-layout"),
            bind_group_layouts: &[bind_group_layout],
            push_constant_ranges: &[],
        });

        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fosfora-render-pipeline"),
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
