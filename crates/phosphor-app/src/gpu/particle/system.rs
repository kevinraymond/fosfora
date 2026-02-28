use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, BufferBindingType, ColorTargetState,
    CommandEncoder, ComputePipeline, Device, FragmentState, PipelineCompilationOptions,
    PipelineLayoutDescriptor, PrimitiveState, Queue, RenderPipeline, ShaderStages, TextureFormat,
    TextureView, VertexState,
};

use super::flow_field::FlowFieldTexture;
use super::spatial_hash::SpatialHashGrid;
use super::sprite::SpriteAtlas;
use super::types::{Particle, ParticleAux, ParticleDef, ParticleRenderUniforms, ParticleUniforms};

const WORKGROUP_SIZE: u32 = 256;

/// GPU compute particle system with ping-pong storage buffers,
/// alive/dead index lists, and indirect draw for GPU-driven rendering.
pub struct ParticleSystem {
    pub max_particles: u32,
    pub uniforms: ParticleUniforms,
    pub render_uniforms: ParticleRenderUniforms,
    pub alive_count: u32,

    // Ping-pong particle storage buffers
    storage_buffers: [wgpu::Buffer; 2],
    current: usize,

    // Counter buffer: 4 x atomic<u32> = [alive_count, dead_count, emit_used, reserved]
    counter_buffer: wgpu::Buffer,

    // Alive index buffers (ping-pong): tightly packed indices of living particles
    alive_index_buffers: [wgpu::Buffer; 2],

    // Dead index buffer: indices of dead particles (CPU-initialized, read-only in sim)
    dead_index_buffer: wgpu::Buffer,

    // Indirect draw args: DrawIndirectArgs [vertex_count, instance_count, first_vertex, first_instance]
    indirect_args_buffer: wgpu::Buffer,

    // Auxiliary data buffer (home positions, packed colors for image decomposition)
    aux_buffer: wgpu::Buffer,
    pub has_aux_data: bool,

    // Uniform buffers
    uniform_buffer: wgpu::Buffer,
    render_uniform_buffer: wgpu::Buffer,

    // Compute (sim)
    compute_pipeline: ComputePipeline,
    compute_bind_groups: [BindGroup; 2],
    compute_bgl: BindGroupLayout,

    // Compute (prepare indirect args)
    prepare_indirect_pipeline: ComputePipeline,
    prepare_indirect_bind_groups: [BindGroup; 2],

    // Render (additive blend — default)
    render_pipeline_additive: RenderPipeline,
    // Render (alpha blend — for non-glowing sprites)
    render_pipeline_alpha: RenderPipeline,
    render_bind_groups: [BindGroup; 2],
    render_bgl: BindGroupLayout,
    // Sprite texture bind group (bind group 1)
    sprite_bind_group: BindGroup,
    sprite_bgl: BindGroupLayout,
    pub sprite: Option<SpriteAtlas>,
    /// Active blend mode: "additive" or "alpha"
    pub blend_mode: String,

    // Flow field (group 1 for compute)
    flow_field: FlowFieldTexture,
    flow_field_bgl: BindGroupLayout,
    flow_field_bind_group: BindGroup,

    // Trail rendering
    trail_buffer: Option<wgpu::Buffer>,
    trail_length: u32,
    trail_width: f32,
    frame_index: u32,
    trail_render_pipeline: Option<RenderPipeline>,
    trail_render_bgl: Option<BindGroupLayout>,
    trail_render_bind_groups: Option<[BindGroup; 2]>,
    trail_compute_bgl: Option<BindGroupLayout>,
    trail_compute_bind_group: Option<BindGroup>,
    trail_indirect_args_buffer: Option<wgpu::Buffer>,
    trail_prepare_indirect_pipeline: Option<ComputePipeline>,
    trail_prepare_indirect_bind_group: Option<BindGroup>,

    // Spatial hash grid for particle-particle interaction
    spatial_hash: Option<SpatialHashGrid>,

    // Placeholder empty BGL for padding bind group layouts (contiguous group indices)
    empty_bgl: BindGroupLayout,

    // Counter readback: staging buffer + async map state
    counter_readback: wgpu::Buffer,
    counter_map_pending: std::sync::Arc<std::sync::atomic::AtomicBool>,

    // Emission accumulator (fractional particles per frame)
    emit_accumulator: f32,
    pub emit_rate: f32,
    pub burst_on_beat: u32,
    pub def: ParticleDef,
    /// Tracked for content-change detection in hot-reload.
    pub current_compute_source: String,
}

impl ParticleSystem {
    pub fn new(
        device: &Device,
        queue: &Queue,
        hdr_format: TextureFormat,
        def: &ParticleDef,
        compute_source: &str,
    ) -> Result<Self, String> {
        let max_particles = def.max_count;
        let particle_size = std::mem::size_of::<Particle>() as u64;
        let buffer_size = particle_size * max_particles as u64;

        // Create storage buffers (ping-pong)
        let storage_buffers = [
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("particles-a"),
                size: buffer_size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("particles-b"),
                size: buffer_size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
        ];

        // Counter buffer: 4 x u32 = 16 bytes
        // [0]=alive_count, [1]=dead_count, [2]=emit_used, [3]=reserved
        let counter_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle-counters"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // Alive index buffers (ping-pong): max_particles * 4 bytes each
        let alive_index_size = max_particles as u64 * 4;
        let alive_index_buffers = [
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("alive-indices-a"),
                size: alive_index_size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("alive-indices-b"),
                size: alive_index_size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
        ];

        // Dead index buffer: pre-populated with [0, 1, 2, ..., max_particles-1]
        let dead_indices: Vec<u32> = (0..max_particles).collect();
        let dead_index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dead-indices"),
            size: alive_index_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&dead_index_buffer, 0, bytemuck::cast_slice(&dead_indices));

        // Indirect draw args buffer: DrawIndirectArgs = 4 x u32 = 16 bytes
        let indirect_args_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle-indirect-args"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Initialize with zero instances
        queue.write_buffer(&indirect_args_buffer, 0, bytemuck::cast_slice(&[6u32, 0, 0, 0]));

        // Auxiliary buffer (home positions for image decomposition)
        let aux_size = std::mem::size_of::<ParticleAux>() as u64;
        let aux_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle-aux"),
            size: aux_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Uniform buffers
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle-uniforms"),
            size: std::mem::size_of::<ParticleUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let render_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle-render-uniforms"),
            size: std::mem::size_of::<ParticleRenderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // --- Compute pipeline (sim) ---
        let compute_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("particle-compute-bgl"),
            entries: &[
                // binding 0: uniforms
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 1: particles_in (read)
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 2: particles_out (write)
                BindGroupLayoutEntry {
                    binding: 2,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 3: counters (atomic, replaces emit_counter)
                BindGroupLayoutEntry {
                    binding: 3,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 4: auxiliary data (home positions, packed colors)
                BindGroupLayoutEntry {
                    binding: 4,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 5: dead_indices (read)
                BindGroupLayoutEntry {
                    binding: 5,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 6: alive_indices_out (write)
                BindGroupLayoutEntry {
                    binding: 6,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // --- Flow field (group 1 for compute) ---
        let flow_field = if def.flow_field {
            FlowFieldTexture::new(device, queue)
        } else {
            FlowFieldTexture::placeholder(device, queue)
        };

        let flow_field_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("particle-flow-field-bgl"),
            entries: &[
                // binding 0: flow field 3D texture
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                // binding 1: flow field sampler
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let flow_field_bind_group = create_flow_field_bind_group(device, &flow_field_bgl, &flow_field);

        let compute_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particle-compute"),
            source: wgpu::ShaderSource::Wgsl(compute_source.into()),
        });

        let compute_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("particle-compute-layout"),
            bind_group_layouts: &[&compute_bgl, &flow_field_bgl],
            push_constant_ranges: &[],
        });

        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("particle-compute-pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &compute_shader,
            entry_point: Some("cs_main"),
            compilation_options: PipelineCompilationOptions::default(),
            cache: None,
        });

        let compute_bind_groups = create_compute_bind_groups(
            device,
            &compute_bgl,
            &uniform_buffer,
            &storage_buffers,
            &counter_buffer,
            &aux_buffer,
            &dead_index_buffer,
            &alive_index_buffers,
        );

        // --- Prepare indirect pipeline ---
        let (prepare_indirect_pipeline, prepare_indirect_bind_groups) =
            create_prepare_indirect_pipeline(
                device,
                &counter_buffer,
                &indirect_args_buffer,
            );

        // --- Render pipeline ---
        let render_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("particle-render-bgl"),
            entries: &[
                // binding 0: particles (read)
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 1: render uniforms
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 2: alive_indices (read) — for GPU-driven indirect draw
                BindGroupLayoutEntry {
                    binding: 2,
                    visibility: ShaderStages::VERTEX,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // Sprite texture bind group layout (bind group 1)
        let sprite_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("particle-sprite-bgl"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // Create placeholder sprite for when no sprite texture is loaded
        let placeholder_sprite = super::sprite::create_placeholder_sprite(device, queue);
        let sprite_bind_group = create_sprite_bind_group(device, &sprite_bgl, &placeholder_sprite);

        let render_source =
            include_str!("../../../../../assets/shaders/builtin/particle_render.wgsl");
        let render_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particle-render"),
            source: wgpu::ShaderSource::Wgsl(render_source.into()),
        });

        let render_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("particle-render-layout"),
            bind_group_layouts: &[&render_bgl, &sprite_bgl],
            push_constant_ranges: &[],
        });

        // Additive blend (default: SrcAlpha + One)
        let render_pipeline_additive =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("particle-render-additive"),
                layout: Some(&render_pipeline_layout),
                vertex: VertexState {
                    module: &render_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: PipelineCompilationOptions::default(),
                },
                fragment: Some(FragmentState {
                    module: &render_shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(ColorTargetState {
                        format: hdr_format,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::SrcAlpha,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: PipelineCompilationOptions::default(),
                }),
                primitive: PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

        // Alpha blend (SrcAlpha + OneMinusSrcAlpha) for non-glowing sprites
        let render_pipeline_alpha =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("particle-render-alpha"),
                layout: Some(&render_pipeline_layout),
                vertex: VertexState {
                    module: &render_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: PipelineCompilationOptions::default(),
                },
                fragment: Some(FragmentState {
                    module: &render_shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(ColorTargetState {
                        format: hdr_format,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::SrcAlpha,
                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: PipelineCompilationOptions::default(),
                }),
                primitive: PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

        let blend_mode = def.blend.clone();

        let render_bind_groups = create_render_bind_groups(
            device,
            &render_bgl,
            &storage_buffers,
            &render_uniform_buffer,
            &alive_index_buffers,
        );

        Ok(Self {
            max_particles,
            uniforms: bytemuck::Zeroable::zeroed(),
            render_uniforms: bytemuck::Zeroable::zeroed(),
            alive_count: 0,
            storage_buffers,
            current: 0,
            counter_buffer,
            alive_index_buffers,
            dead_index_buffer,
            indirect_args_buffer,
            aux_buffer,
            has_aux_data: false,
            uniform_buffer,
            render_uniform_buffer,
            compute_pipeline,
            compute_bind_groups,
            compute_bgl,
            prepare_indirect_pipeline,
            prepare_indirect_bind_groups,
            render_pipeline_additive,
            render_pipeline_alpha,
            render_bind_groups,
            render_bgl,
            sprite_bind_group,
            sprite_bgl,
            sprite: None,
            blend_mode,
            flow_field,
            flow_field_bgl,
            flow_field_bind_group,
            trail_buffer: None,
            trail_length: 0,
            trail_width: 0.005,
            frame_index: 0,
            trail_render_pipeline: None,
            trail_render_bgl: None,
            trail_render_bind_groups: None,
            trail_compute_bgl: None,
            trail_compute_bind_group: None,
            trail_indirect_args_buffer: None,
            trail_prepare_indirect_pipeline: None,
            trail_prepare_indirect_bind_group: None,
            spatial_hash: None,
            empty_bgl: device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("empty-bgl"),
                entries: &[],
            }),
            counter_readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("particle-counter-readback"),
                size: 16,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            counter_map_pending: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            emit_accumulator: 0.0,
            emit_rate: def.emit_rate,
            burst_on_beat: def.burst_on_beat,
            def: def.clone(),
            current_compute_source: compute_source.to_string(),
        })
    }

    /// Build the list of bind group layouts for compute pipeline creation.
    /// Groups: 0=core, 1=flow field, 2=trails (or empty placeholder), 3=spatial hash (if enabled).
    fn compute_bind_group_layouts(&self) -> Vec<&BindGroupLayout> {
        let mut layouts: Vec<&BindGroupLayout> = vec![&self.compute_bgl, &self.flow_field_bgl];

        if let Some(hash) = &self.spatial_hash {
            // Group 3 requires group 2 to exist (contiguous indices).
            // Use trail BGL if available, otherwise empty placeholder.
            if let Some(trail_bgl) = &self.trail_compute_bgl {
                layouts.push(trail_bgl);
            } else {
                layouts.push(&self.empty_bgl);
            }
            layouts.push(&hash.query_bgl);
        } else if let Some(trail_bgl) = &self.trail_compute_bgl {
            layouts.push(trail_bgl);
        }

        layouts
    }

    /// Replace the compute shader and rebuild the pipeline (for switching shader at runtime).
    /// Unlike recompile_compute, this also updates current_compute_source.
    #[allow(dead_code)]
    pub fn set_compute_shader(
        &mut self,
        device: &Device,
        source: &str,
    ) -> Result<(), String> {
        self.recompile_compute(device, source)?;
        self.current_compute_source = source.to_string();
        Ok(())
    }

    /// Recompile the compute pipeline (for hot-reload).
    pub fn recompile_compute(
        &mut self,
        device: &Device,
        source: &str,
    ) -> Result<(), String> {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particle-compute-hotreload"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });

        // Build layout with optional trail (group 2) and spatial hash (group 3)
        let bind_group_layouts = self.compute_bind_group_layouts();
        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("particle-compute-layout"),
            bind_group_layouts: &bind_group_layouts,
            push_constant_ranges: &[],
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("particle-compute-pipeline"),
            layout: Some(&layout),
            module: &shader,
            entry_point: Some("cs_main"),
            compilation_options: PipelineCompilationOptions::default(),
            cache: None,
        });

        self.compute_pipeline = pipeline;
        Ok(())
    }

    /// Update uniforms from app state. Call before dispatch().
    pub fn update_uniforms(
        &mut self,
        dt: f32,
        time: f32,
        resolution: [f32; 2],
        beat: f32,
    ) {
        // Accumulate emissions
        self.emit_accumulator += self.emit_rate * dt;

        // Beat burst — use dedicated beat trigger instead of onset threshold
        if beat > 0.5 && self.burst_on_beat > 0 {
            self.emit_accumulator += self.burst_on_beat as f32;
        }

        let emit_count = self.emit_accumulator as u32;
        self.emit_accumulator -= emit_count as f32;

        self.uniforms.delta_time = dt;
        self.uniforms.time = time;
        self.uniforms.max_particles = self.max_particles;
        self.uniforms.emit_count = emit_count;

        // Emitter config from def
        self.uniforms.emitter_pos = self.def.emitter.position;
        self.uniforms.emitter_radius = self.def.emitter.radius;
        self.uniforms.emitter_shape = self.def.emitter.shape_index();
        self.uniforms.lifetime = self.def.lifetime;
        self.uniforms.initial_speed = self.def.initial_speed;
        self.uniforms.initial_size = self.def.initial_size;
        self.uniforms.size_end = self.def.size_end;
        self.uniforms.gravity = self.def.gravity;
        self.uniforms.drag = self.def.drag;
        self.uniforms.turbulence = self.def.turbulence;
        self.uniforms.attraction_point = [0.0, 0.0];
        self.uniforms.attraction_strength = self.def.attraction_strength;

        // Seed for randomness (different each frame)
        self.uniforms.seed = time * 1000.0 % 65536.0;

        // Resolution for aspect ratio correction in compute shader
        self.uniforms.resolution = resolution;

        // Flow field params from def
        self.uniforms.flow_strength = self.def.flow_strength;
        self.uniforms.flow_scale = self.def.flow_scale;
        self.uniforms.flow_speed = self.def.flow_speed;
        self.uniforms.flow_enabled = if self.def.flow_field { 1.0 } else { 0.0 };

        // Trail params
        self.uniforms.trail_length = self.trail_length;
        self.uniforms.trail_width = self.trail_width;

        self.render_uniforms.resolution = resolution;
        self.render_uniforms.time = time;
        self.render_uniforms.frame_index = self.frame_index;
        self.render_uniforms.trail_length = self.trail_length;
        self.render_uniforms.trail_width = self.trail_width;
    }

    /// Copy audio features into particle uniforms.
    pub fn update_audio(
        &mut self,
        sub_bass: f32,
        bass: f32,
        mid: f32,
        rms: f32,
        kick: f32,
        onset: f32,
        centroid: f32,
        flux: f32,
        beat: f32,
        beat_phase: f32,
    ) {
        self.uniforms.sub_bass = sub_bass;
        self.uniforms.bass = bass;
        self.uniforms.mid = mid;
        self.uniforms.rms = rms;
        self.uniforms.kick = kick;
        self.uniforms.onset = onset;
        self.uniforms.centroid = centroid;
        self.uniforms.flux = flux;
        self.uniforms.beat = beat;
        self.uniforms.beat_phase = beat_phase;
    }

    /// Run the compute dispatch (particle simulation + prepare indirect args).
    pub fn dispatch(&self, encoder: &mut CommandEncoder, queue: &Queue) {
        // Reset counters to 0 (alive_count, dead_count, emit_used, reserved)
        queue.write_buffer(&self.counter_buffer, 0, &[0u8; 16]);

        // Upload uniforms
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&self.uniforms));

        let workgroups = (self.max_particles + WORKGROUP_SIZE - 1) / WORKGROUP_SIZE;

        // 0. Spatial hash (if interaction enabled) — build before sim
        if let Some(hash) = &self.spatial_hash {
            // Read from the input buffer (current side of ping-pong)
            hash.dispatch(encoder, queue, self.current);
        }

        // 1. Particle simulation
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("particle-sim"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.compute_pipeline);
            pass.set_bind_group(0, &self.compute_bind_groups[self.current], &[]);
            pass.set_bind_group(1, &self.flow_field_bind_group, &[]);
            if let Some(trail_bg) = &self.trail_compute_bind_group {
                pass.set_bind_group(2, trail_bg, &[]);
            }
            if let Some(hash) = &self.spatial_hash {
                pass.set_bind_group(3, &hash.query_bind_group, &[]);
            }
            pass.dispatch_workgroups(workgroups, 1, 1);
        }

        // 2. Prepare indirect draw args (reads alive_count from counters, writes DrawIndirectArgs)
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("particle-prepare-indirect"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.prepare_indirect_pipeline);
            // Use the bind group matching current ping-pong state
            // (counter_buffer is shared, but we pick the right alive buffer)
            pass.set_bind_group(0, &self.prepare_indirect_bind_groups[self.current], &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }

        // 3. Prepare trail indirect draw args (if trails active)
        if let (Some(pipeline), Some(bg)) = (
            &self.trail_prepare_indirect_pipeline,
            &self.trail_prepare_indirect_bind_group,
        ) {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("particle-trail-prepare-indirect"),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bg, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }

        // 4. Copy counter buffer to staging for CPU readback (1-frame latency)
        // Skip if previous map is still pending (buffer would be mapped → submit error)
        if !self.counter_map_pending.load(std::sync::atomic::Ordering::Relaxed) {
            encoder.copy_buffer_to_buffer(&self.counter_buffer, 0, &self.counter_readback, 0, 16);
        }
    }

    /// Request async map of the counter readback buffer.
    /// Call once per frame after queue.submit().
    pub fn request_counter_readback(&self) {
        use std::sync::atomic::Ordering;
        if self.counter_map_pending.load(Ordering::Relaxed) {
            return; // Previous map still pending
        }
        let flag = self.counter_map_pending.clone();
        self.counter_readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                if result.is_ok() {
                    flag.store(true, Ordering::Release);
                }
            });
    }

    /// Poll the counter readback. If the map completed, read alive count and unmap.
    /// Call once per frame before dispatch.
    pub fn poll_counter_readback(&mut self) {
        use std::sync::atomic::Ordering;
        if !self.counter_map_pending.load(Ordering::Acquire) {
            return;
        }
        {
            let view = self.counter_readback.slice(..).get_mapped_range();
            let data: &[u32] = bytemuck::cast_slice(&view);
            self.alive_count = data[0];
        }
        self.counter_readback.unmap();
        self.counter_map_pending.store(false, Ordering::Release);
    }

    /// Render particles into the given target using indirect draw.
    pub fn render(&self, encoder: &mut CommandEncoder, queue: &Queue, target: &TextureView) {
        // Upload render uniforms
        queue.write_buffer(
            &self.render_uniform_buffer,
            0,
            bytemuck::bytes_of(&self.render_uniforms),
        );

        // The output buffer is the one we just wrote to in compute
        let output_idx = 1 - self.current;

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("particle-render"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load, // Composite on top of existing content
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        // Render trails first (behind particles) if active
        if let (Some(trail_pipeline), Some(trail_bgs), Some(trail_indirect)) = (
            &self.trail_render_pipeline,
            &self.trail_render_bind_groups,
            &self.trail_indirect_args_buffer,
        ) {
            pass.set_pipeline(trail_pipeline);
            pass.set_bind_group(0, &trail_bgs[output_idx], &[]);
            pass.draw_indirect(trail_indirect, 0);
        }

        let pipeline = if self.blend_mode == "alpha" {
            &self.render_pipeline_alpha
        } else {
            &self.render_pipeline_additive
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.render_bind_groups[output_idx], &[]);
        pass.set_bind_group(1, &self.sprite_bind_group, &[]);
        // GPU-driven indirect draw: instance_count set by prepare_indirect shader
        pass.draw_indirect(&self.indirect_args_buffer, 0);
    }

    /// Load a sprite atlas and update the sprite bind group.
    pub fn set_sprite(&mut self, device: &Device, atlas: SpriteAtlas) {
        self.sprite_bind_group = create_sprite_bind_group(device, &self.sprite_bgl, &atlas);
        self.render_uniforms.render_mode = if atlas.animated { 2 } else { 1 };
        self.render_uniforms.sprite_cols = atlas.cols;
        self.render_uniforms.sprite_rows = atlas.rows;
        self.render_uniforms.sprite_frames = atlas.frames;
        self.sprite = Some(atlas);
    }

    /// Upload auxiliary data (home positions, packed colors) for image decomposition.
    /// Recreates aux buffer and compute bind groups.
    pub fn upload_aux_data(&mut self, device: &Device, queue: &Queue, data: &[ParticleAux]) {
        let aux_size = (std::mem::size_of::<ParticleAux>() * data.len().max(1)) as u64;
        self.aux_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle-aux"),
            size: aux_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if !data.is_empty() {
            queue.write_buffer(&self.aux_buffer, 0, bytemuck::cast_slice(data));
        }
        self.has_aux_data = !data.is_empty();

        // Recreate compute bind groups with new aux buffer
        self.compute_bind_groups = create_compute_bind_groups(
            device,
            &self.compute_bgl,
            &self.uniform_buffer,
            &self.storage_buffers,
            &self.counter_buffer,
            &self.aux_buffer,
            &self.dead_index_buffer,
            &self.alive_index_buffers,
        );
    }

    /// Reset sprite, aux data, and compute shader to defaults.
    #[allow(dead_code)]
    pub fn clear_customization(&mut self, device: &Device, queue: &Queue) {
        // Reset sprite to placeholder
        let placeholder = super::sprite::create_placeholder_sprite(device, queue);
        self.sprite_bind_group = create_sprite_bind_group(device, &self.sprite_bgl, &placeholder);
        self.sprite = None;
        self.render_uniforms.render_mode = 0;
        self.render_uniforms.sprite_cols = 1;
        self.render_uniforms.sprite_rows = 1;
        self.render_uniforms.sprite_frames = 1;
        self.blend_mode = "additive".to_string();

        // Clear aux data
        self.upload_aux_data(device, queue, &[]);

        // Restore default compute shader
        // NOTE: caller should provide source with libraries prepended via
        // EffectLoader::prepend_compute_libraries(). This fallback uses raw source.
        let default_source =
            include_str!("../../../../../assets/shaders/builtin/particle_sim.wgsl");
        if let Err(e) = self.recompile_compute(device, default_source) {
            log::error!("Failed to restore default compute shader: {e}");
        } else {
            self.current_compute_source = default_source.to_string();
        }

        // Reset emitter shape
        self.def.emitter.shape = "point".to_string();
    }

    /// Enable or upgrade the flow field texture. If enabling for the first time,
    /// bakes the full 64x64x64 curl noise texture.
    pub fn set_flow_field(&mut self, device: &Device, queue: &Queue, enabled: bool) {
        self.def.flow_field = enabled;
        let new_field = if enabled {
            FlowFieldTexture::new(device, queue)
        } else {
            FlowFieldTexture::placeholder(device, queue)
        };
        self.flow_field = new_field;
        self.flow_field_bind_group =
            create_flow_field_bind_group(device, &self.flow_field_bgl, &self.flow_field);
    }

    /// Set up spatial hash grid for particle-particle interaction.
    pub fn setup_spatial_hash(&mut self, device: &Device) {
        let hash = SpatialHashGrid::new(
            device,
            self.max_particles,
            &self.storage_buffers,
            &self.uniform_buffer,
        );
        self.spatial_hash = Some(hash);

        // Recompile pipeline with spatial hash BGL (group 3) now included
        if let Err(e) = self.recompile_compute(device, &self.current_compute_source.clone()) {
            log::error!("Failed to recompile compute pipeline with spatial hash: {e}");
        }
    }

    /// Get the spatial hash query bind group layout, if interaction is enabled.
    pub fn spatial_hash_query_bgl(&self) -> Option<&BindGroupLayout> {
        self.spatial_hash.as_ref().map(|h| &h.query_bgl)
    }

    /// Get the spatial hash query bind group, if interaction is enabled.
    pub fn spatial_hash_query_bg(&self) -> Option<&BindGroup> {
        self.spatial_hash.as_ref().map(|h| &h.query_bind_group)
    }

    /// Set up trail rendering with the given trail length.
    /// Creates trail buffer, compute bind group for trail writes, and render pipeline.
    pub fn setup_trails(
        &mut self,
        device: &Device,
        hdr_format: TextureFormat,
        trail_length: u32,
        trail_width: f32,
    ) {
        if trail_length < 2 {
            self.trail_buffer = None;
            self.trail_length = 0;
            self.trail_render_pipeline = None;
            self.trail_render_bgl = None;
            self.trail_render_bind_groups = None;
            self.trail_compute_bgl = None;
            self.trail_compute_bind_group = None;
            return;
        }

        self.trail_length = trail_length;
        self.trail_width = trail_width;

        // Trail buffer: max_particles * trail_length * 16 bytes (vec4f per point: xy=pos, z=size, w=alpha)
        let trail_buf_size = self.max_particles as u64 * trail_length as u64 * 16;
        let trail_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle-trail-buffer"),
            size: trail_buf_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Compute bind group for trail writes (group 2)
        let trail_compute_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("particle-trail-compute-bgl"),
            entries: &[
                // binding 0: trail buffer (read_write)
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let trail_compute_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("particle-trail-compute-bg"),
            layout: &trail_compute_bgl,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: trail_buffer.as_entire_binding(),
            }],
        });

        // Trail render bind group layout
        let trail_render_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("particle-trail-render-bgl"),
            entries: &[
                // binding 0: particles (read)
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::VERTEX,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 1: render uniforms
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 2: alive_indices (read)
                BindGroupLayoutEntry {
                    binding: 2,
                    visibility: ShaderStages::VERTEX,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 3: trail buffer (read)
                BindGroupLayoutEntry {
                    binding: 3,
                    visibility: ShaderStages::VERTEX,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // Trail render bind groups (ping-pong: read from output side)
        let trail_render_bind_groups = create_trail_render_bind_groups(
            device,
            &trail_render_bgl,
            &self.storage_buffers,
            &self.render_uniform_buffer,
            &self.alive_index_buffers,
            &trail_buffer,
        );

        // Trail render pipeline
        let render_source =
            include_str!("../../../../../assets/shaders/builtin/particle_render_trail.wgsl");
        let render_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particle-trail-render"),
            source: wgpu::ShaderSource::Wgsl(render_source.into()),
        });

        let render_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("particle-trail-render-layout"),
            bind_group_layouts: &[&trail_render_bgl],
            push_constant_ranges: &[],
        });

        let trail_render_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("particle-trail-render"),
                layout: Some(&render_layout),
                vertex: VertexState {
                    module: &render_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: PipelineCompilationOptions::default(),
                },
                fragment: Some(FragmentState {
                    module: &render_shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(ColorTargetState {
                        format: hdr_format,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::SrcAlpha,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: PipelineCompilationOptions::default(),
                }),
                primitive: PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

        // Store trail compute BGL/BG now so recompile can see them
        self.trail_compute_bgl = Some(trail_compute_bgl);
        self.trail_compute_bind_group = Some(trail_compute_bind_group);

        // Recompile compute pipeline with trail bind group (group 2)
        if let Err(e) = self.recompile_compute(device, &self.current_compute_source.clone()) {
            log::error!("Failed to recompile compute pipeline with trails: {e}");
        }

        // Trail indirect args buffer (vertex_count = 6*(trail_length-1), instance_count from alive)
        let trail_indirect_args_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle-trail-indirect-args"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Trail prepare indirect pipeline (reads counters, writes trail-specific DrawIndirectArgs)
        let (trail_prepare_pipeline, trail_prepare_bg) = create_trail_prepare_indirect_pipeline(
            device,
            &self.counter_buffer,
            &trail_indirect_args_buffer,
            trail_length,
        );

        self.trail_buffer = Some(trail_buffer);
        self.trail_render_pipeline = Some(trail_render_pipeline);
        self.trail_render_bgl = Some(trail_render_bgl);
        self.trail_render_bind_groups = Some(trail_render_bind_groups);
        // trail_compute_bgl and trail_compute_bind_group already stored above (before recompile)
        self.trail_indirect_args_buffer = Some(trail_indirect_args_buffer);
        self.trail_prepare_indirect_pipeline = Some(trail_prepare_pipeline);
        self.trail_prepare_indirect_bind_group = Some(trail_prepare_bg);
    }

    /// Flip ping-pong buffers for next frame.
    pub fn flip(&mut self) {
        self.current = 1 - self.current;
        self.frame_index = self.frame_index.wrapping_add(1);
    }
}

/// Create the prepare-indirect compute pipeline and bind groups.
/// This is a simple 1-thread shader that reads counters and writes DrawIndirectArgs.
fn create_prepare_indirect_pipeline(
    device: &Device,
    counter_buffer: &wgpu::Buffer,
    indirect_args_buffer: &wgpu::Buffer,
) -> (ComputePipeline, [BindGroup; 2]) {
    let bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("prepare-indirect-bgl"),
        entries: &[
            // binding 0: counters (read — non-atomic view for prepare shader)
            BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // binding 1: indirect_args (write)
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });

    let source = include_str!("../../../../../assets/shaders/builtin/particle_prepare_indirect.wgsl");
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("prepare-indirect"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });

    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("prepare-indirect-layout"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });

    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("prepare-indirect-pipeline"),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("cs_main"),
        compilation_options: PipelineCompilationOptions::default(),
        cache: None,
    });

    // Same bind group for both ping-pong states (counter and indirect buffers are shared)
    let bg = device.create_bind_group(&BindGroupDescriptor {
        label: Some("prepare-indirect-bg"),
        layout: &bgl,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: counter_buffer.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: indirect_args_buffer.as_entire_binding(),
            },
        ],
    });

    // Clone the bind group reference — both ping-pong states use the same bind group
    let bg2 = device.create_bind_group(&BindGroupDescriptor {
        label: Some("prepare-indirect-bg-1"),
        layout: &bgl,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: counter_buffer.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: indirect_args_buffer.as_entire_binding(),
            },
        ],
    });

    (pipeline, [bg, bg2])
}

fn create_compute_bind_groups(
    device: &Device,
    layout: &BindGroupLayout,
    uniform_buffer: &wgpu::Buffer,
    storage_buffers: &[wgpu::Buffer; 2],
    counter_buffer: &wgpu::Buffer,
    aux_buffer: &wgpu::Buffer,
    dead_index_buffer: &wgpu::Buffer,
    alive_index_buffers: &[wgpu::Buffer; 2],
) -> [BindGroup; 2] {
    // bind_group[0]: read from storage[0], write to storage[1], write to alive_indices[1]
    let bg0 = device.create_bind_group(&BindGroupDescriptor {
        label: Some("particle-compute-bg-0"),
        layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: storage_buffers[0].as_entire_binding(),
            },
            BindGroupEntry {
                binding: 2,
                resource: storage_buffers[1].as_entire_binding(),
            },
            BindGroupEntry {
                binding: 3,
                resource: counter_buffer.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 4,
                resource: aux_buffer.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 5,
                resource: dead_index_buffer.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 6,
                resource: alive_index_buffers[1].as_entire_binding(),
            },
        ],
    });
    // bind_group[1]: read from storage[1], write to storage[0], write to alive_indices[0]
    let bg1 = device.create_bind_group(&BindGroupDescriptor {
        label: Some("particle-compute-bg-1"),
        layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: storage_buffers[1].as_entire_binding(),
            },
            BindGroupEntry {
                binding: 2,
                resource: storage_buffers[0].as_entire_binding(),
            },
            BindGroupEntry {
                binding: 3,
                resource: counter_buffer.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 4,
                resource: aux_buffer.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 5,
                resource: dead_index_buffer.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 6,
                resource: alive_index_buffers[0].as_entire_binding(),
            },
        ],
    });
    [bg0, bg1]
}

fn create_sprite_bind_group(
    device: &Device,
    layout: &BindGroupLayout,
    sprite: &SpriteAtlas,
) -> BindGroup {
    device.create_bind_group(&BindGroupDescriptor {
        label: Some("particle-sprite-bg"),
        layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: BindingResource::TextureView(&sprite.view),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::Sampler(&sprite.sampler),
            },
        ],
    })
}

fn create_flow_field_bind_group(
    device: &Device,
    layout: &BindGroupLayout,
    flow_field: &FlowFieldTexture,
) -> BindGroup {
    device.create_bind_group(&BindGroupDescriptor {
        label: Some("particle-flow-field-bg"),
        layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: BindingResource::TextureView(&flow_field.view),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::Sampler(&flow_field.sampler),
            },
        ],
    })
}

fn create_render_bind_groups(
    device: &Device,
    layout: &BindGroupLayout,
    storage_buffers: &[wgpu::Buffer; 2],
    render_uniform_buffer: &wgpu::Buffer,
    alive_index_buffers: &[wgpu::Buffer; 2],
) -> [BindGroup; 2] {
    // bg[0]: particles from storage[0], alive_indices from alive_index[0]
    let bg0 = device.create_bind_group(&BindGroupDescriptor {
        label: Some("particle-render-bg-0"),
        layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: storage_buffers[0].as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: render_uniform_buffer,
                    offset: 0,
                    size: None,
                }),
            },
            BindGroupEntry {
                binding: 2,
                resource: alive_index_buffers[0].as_entire_binding(),
            },
        ],
    });
    // bg[1]: particles from storage[1], alive_indices from alive_index[1]
    let bg1 = device.create_bind_group(&BindGroupDescriptor {
        label: Some("particle-render-bg-1"),
        layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: storage_buffers[1].as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: render_uniform_buffer,
                    offset: 0,
                    size: None,
                }),
            },
            BindGroupEntry {
                binding: 2,
                resource: alive_index_buffers[1].as_entire_binding(),
            },
        ],
    });
    [bg0, bg1]
}

fn create_trail_render_bind_groups(
    device: &Device,
    layout: &BindGroupLayout,
    storage_buffers: &[wgpu::Buffer; 2],
    render_uniform_buffer: &wgpu::Buffer,
    alive_index_buffers: &[wgpu::Buffer; 2],
    trail_buffer: &wgpu::Buffer,
) -> [BindGroup; 2] {
    let bg0 = device.create_bind_group(&BindGroupDescriptor {
        label: Some("particle-trail-render-bg-0"),
        layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: storage_buffers[0].as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: render_uniform_buffer,
                    offset: 0,
                    size: None,
                }),
            },
            BindGroupEntry {
                binding: 2,
                resource: alive_index_buffers[0].as_entire_binding(),
            },
            BindGroupEntry {
                binding: 3,
                resource: trail_buffer.as_entire_binding(),
            },
        ],
    });
    let bg1 = device.create_bind_group(&BindGroupDescriptor {
        label: Some("particle-trail-render-bg-1"),
        layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: storage_buffers[1].as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: render_uniform_buffer,
                    offset: 0,
                    size: None,
                }),
            },
            BindGroupEntry {
                binding: 2,
                resource: alive_index_buffers[1].as_entire_binding(),
            },
            BindGroupEntry {
                binding: 3,
                resource: trail_buffer.as_entire_binding(),
            },
        ],
    });
    [bg0, bg1]
}

fn create_trail_prepare_indirect_pipeline(
    device: &Device,
    counter_buffer: &wgpu::Buffer,
    trail_indirect_args_buffer: &wgpu::Buffer,
    trail_length: u32,
) -> (ComputePipeline, BindGroup) {
    let bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("trail-prepare-indirect-bgl"),
        entries: &[
            BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });

    // Bake trail_length into shader source for vertex count calculation
    let verts_per_instance = 6 * (trail_length.max(2) - 1);
    let source = format!(
        r#"
@group(0) @binding(0) var<storage, read> counters: array<u32, 4>;
@group(0) @binding(1) var<storage, read_write> indirect_args: array<u32, 4>;

@compute @workgroup_size(1)
fn cs_main() {{
    let alive_count = counters[0];
    indirect_args[0] = {verts_per_instance}u;
    indirect_args[1] = alive_count;
    indirect_args[2] = 0u;
    indirect_args[3] = 0u;
}}
"#
    );

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("trail-prepare-indirect"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });

    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("trail-prepare-indirect-layout"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });

    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("trail-prepare-indirect-pipeline"),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("cs_main"),
        compilation_options: PipelineCompilationOptions::default(),
        cache: None,
    });

    let bg = device.create_bind_group(&BindGroupDescriptor {
        label: Some("trail-prepare-indirect-bg"),
        layout: &bgl,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: counter_buffer.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: trail_indirect_args_buffer.as_entire_binding(),
            },
        ],
    });

    (pipeline, bg)
}
