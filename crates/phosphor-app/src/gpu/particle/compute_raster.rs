use bytemuck::{Pod, Zeroable};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingType, BufferBindingType, ColorTargetState, CommandEncoder,
    ComputePipeline, Device, FragmentState, PipelineCompilationOptions, PipelineLayoutDescriptor,
    PrimitiveState, Queue, RenderPipeline, ShaderStages, TextureFormat, TextureView, VertexState,
};

const WORKGROUP_SIZE: u32 = 256;
const TILE_SIZE: u32 = 16;
const TILED_THRESHOLD: u32 = 50_000;
const MAX_PREFIX_SUM_CELLS: u32 = 65_536;

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
struct DrawUniforms {
    width: u32,
    height: u32,
    /// 1 = anisotropic gaussian footprints from the per-splat screen conic
    /// packed in flags.zw (Splat #1800); 0 = classic isotropic glow discs.
    splat_mode: u32,
    /// 1 = accumulate NDC velocity into the fb_vx/fb_vy channels (#1482).
    velocity_mode: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
struct ResolveUniforms {
    width: u32,
    height: u32,
    mode: u32, // 0 = additive, 1 = alpha, 2 = weighted-average OIT (Splat #1800)
    _pad: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
struct TileUniforms {
    width: u32,
    height: u32,
    num_tiles_x: u32,
    num_tiles_y: u32,
    max_particles: u32,
    /// See `DrawUniforms::splat_mode`.
    splat_mode: u32,
    /// See `DrawUniforms::velocity_mode`.
    velocity_mode: u32,
    _pad2: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
struct VelResolveUniforms {
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
}

/// Resources for resolving the velocity channels into the texture the pass
/// graph exposes as `@particles.velocity` (#1482). Present only when the
/// effect opted in via `velocity_field`.
struct VelocityResolve {
    texture: wgpu::Texture,
    view: TextureView,
    sampler: wgpu::Sampler,
    pipeline: RenderPipeline,
    bind_group: BindGroup,
}

/// Compute rasterizer: atomic framebuffer for sub-pixel particles.
///
/// Two rasterization paths:
/// - **Direct**: 1-thread-per-particle `atomicAdd` to global framebuffer (low particle counts)
/// - **Tiled**: 4-pass bin→prefix-sum→scatter→tile-raster with shared-memory accumulation (high counts)
///
/// Both paths share the same resolve pass (fullscreen triangle → render target).
pub struct ComputeRasterizer {
    width: u32,
    height: u32,
    max_particles: u32,
    /// Anisotropic-conic footprints (Splat #1800). Set once at creation from
    /// `def.splat.is_some() && blend == "oit"`; every classic effect runs
    /// with 0 and is bit-identical to the pre-splat rasterizer.
    splat_mode: bool,
    /// Per-pixel velocity accumulation (#1482 Chronoflow). Set once at creation
    /// from `def.velocity_field`; when off, the vel buffers are 16-byte dummies
    /// and the shaders' uniform-flag guards never touch them.
    velocity_mode: bool,

    // 4 atomic storage buffers (one per channel)
    fb_buffers: [wgpu::Buffer; 4], // R, G, B, A
    // Velocity channels (full-size when velocity_mode, else 16-byte dummies)
    vel_buffers: [wgpu::Buffer; 2], // VX, VY
    velocity: Option<VelocityResolve>,
    vel_resolve_uniform_buffer: wgpu::Buffer,

    // Uniform buffers
    draw_uniform_buffer: wgpu::Buffer,
    resolve_uniform_buffer: wgpu::Buffer,
    tile_uniform_buffer: wgpu::Buffer,

    // Direct draw pass
    draw_pipeline: ComputePipeline,
    draw_bind_groups: [BindGroup; 2], // ping-pong for particle data
    draw_bgl: BindGroupLayout,

    // Tiled path: tile geometry
    num_tiles_x: u32,
    num_tiles_y: u32,
    num_tiles: u32,

    // Tiled path: buffers
    tile_counts_buffer: wgpu::Buffer,
    tile_offsets_buffer: wgpu::Buffer,
    tile_scatter_offsets_buffer: wgpu::Buffer,
    sorted_particles_buffer: wgpu::Buffer,

    // Tiled path: bin pass
    bin_pipeline: ComputePipeline,
    bin_bgl: BindGroupLayout,
    bin_bind_groups: [BindGroup; 2],

    // Tiled path: prefix sum pass (reuses spatial_hash_prefix_sum.wgsl)
    prefix_sum_pipeline: ComputePipeline,
    prefix_sum_bgl: BindGroupLayout,
    prefix_sum_bind_group: BindGroup,

    // Tiled path: scatter pass
    scatter_pipeline: ComputePipeline,
    scatter_bgl: BindGroupLayout,
    scatter_bind_groups: [BindGroup; 2],

    // Tiled path: tiled raster pass
    tiled_pipeline: ComputePipeline,
    tiled_bgl: BindGroupLayout,
    tiled_bind_groups: [BindGroup; 2],

    // Resolve pass (render pipelines with hardware blend)
    resolve_pipeline_additive: RenderPipeline,
    resolve_pipeline_alpha: RenderPipeline,
    resolve_bind_group: BindGroup,
    resolve_bgl: BindGroupLayout,
}

impl ComputeRasterizer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &Device,
        hdr_format: TextureFormat,
        width: u32,
        height: u32,
        pos_life_buffers: &[wgpu::Buffer; 2],
        vel_size_buffers: &[wgpu::Buffer; 2],
        color_buffers: &[wgpu::Buffer; 2],
        flags_buffers: &[wgpu::Buffer; 2],
        alive_index_buffers: &[wgpu::Buffer; 2],
        counter_buffer: &wgpu::Buffer,
        max_particles: u32,
        splat_mode: bool,
        velocity_mode: bool,
    ) -> Self {
        let pixel_count = (width * height) as u64;
        let fb_size = pixel_count * 4; // 4 bytes per i32

        let fb_buffers = std::array::from_fn(|i| {
            let label = ["fb-r", "fb-g", "fb-b", "fb-a"][i];
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: fb_size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        let vel_buffers = create_vel_buffers(device, fb_size, velocity_mode);
        let vel_resolve_uniform_buffer = create_vel_resolve_uniforms(device, width, height);

        let draw_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cr-draw-uniforms"),
            size: std::mem::size_of::<DrawUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let resolve_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cr-resolve-uniforms"),
            size: std::mem::size_of::<ResolveUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let tile_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cr-tile-uniforms"),
            size: std::mem::size_of::<TileUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // --- Direct draw pipeline ---
        let draw_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("cr-draw-bgl"),
            entries: &[
                storage_ro_entry(0),  // pos_life
                storage_ro_entry(1),  // vel_size
                storage_ro_entry(2),  // color
                storage_ro_entry(3),  // alive_indices
                storage_ro_entry(4),  // counters
                uniform_entry(5),     // draw uniforms
                storage_rw_entry(6),  // fb_r
                storage_rw_entry(7),  // fb_g
                storage_rw_entry(8),  // fb_b
                storage_rw_entry(9),  // fb_a
                storage_ro_entry(10), // flags (splat_mode conic in .zw, #1800)
                storage_rw_entry(11), // fb_vx (velocity_mode, #1482)
                storage_rw_entry(12), // fb_vy
            ],
        });

        let draw_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cr-draw"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../../assets/shaders/builtin/compute_raster_draw.wgsl")
                    .into(),
            ),
        });

        let draw_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("cr-draw-layout"),
            bind_group_layouts: &[&draw_bgl],
            push_constant_ranges: &[],
        });

        let draw_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("cr-draw-pipeline"),
            layout: Some(&draw_layout),
            module: &draw_shader,
            entry_point: Some("cs_draw"),
            compilation_options: PipelineCompilationOptions::default(),
            cache: None,
        });

        let draw_bind_groups = create_draw_bind_groups(
            device,
            &draw_bgl,
            pos_life_buffers,
            vel_size_buffers,
            color_buffers,
            flags_buffers,
            alive_index_buffers,
            counter_buffer,
            &draw_uniform_buffer,
            &fb_buffers,
            &vel_buffers,
        );

        // --- Tiled path setup ---
        let num_tiles_x = width.div_ceil(TILE_SIZE);
        let num_tiles_y = height.div_ceil(TILE_SIZE);
        let num_tiles = num_tiles_x * num_tiles_y;

        let tile_counts_buffer = create_tile_buffer(device, "cr-tile-counts", num_tiles, true);
        let tile_offsets_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cr-tile-offsets"),
            size: (num_tiles as u64) * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let tile_scatter_offsets_buffer =
            create_tile_buffer(device, "cr-tile-scatter-offsets", num_tiles, true);

        // 9× max_particles worst-case for Gaussian splat particles (up to 3×3 tiles)
        let sorted_particles_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cr-sorted-particles"),
            size: (max_particles as u64) * 9 * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        // --- Bin pipeline ---
        let bin_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("cr-bin-bgl"),
            entries: &[
                storage_ro_entry(0), // pos_life
                storage_ro_entry(1), // vel_size
                storage_ro_entry(2), // alive_indices
                storage_ro_entry(3), // counters
                uniform_entry(4),    // tile uniforms
                storage_rw_entry(5), // tile_counts
            ],
        });

        let bin_pipeline = create_compute_pipeline(
            device,
            "cr-bin",
            include_str!("../../../../../assets/shaders/builtin/compute_raster_bin.wgsl"),
            &bin_bgl,
        );

        let bin_bind_groups = create_bin_bind_groups(
            device,
            &bin_bgl,
            pos_life_buffers,
            vel_size_buffers,
            alive_index_buffers,
            counter_buffer,
            &tile_uniform_buffer,
            &tile_counts_buffer,
        );

        // --- Prefix sum pipeline (reuses spatial hash prefix sum with patched constant) ---
        let prefix_sum_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("cr-prefix-sum-bgl"),
            entries: &[
                storage_ro_entry(0), // tile_counts (cell_counts in shader)
                storage_rw_entry(1), // tile_offsets (cell_offsets in shader)
            ],
        });

        let prefix_sum_pipeline = create_prefix_sum_pipeline(device, &prefix_sum_bgl, num_tiles);

        let prefix_sum_bind_group = create_prefix_sum_bind_group(
            device,
            &prefix_sum_bgl,
            &tile_counts_buffer,
            &tile_offsets_buffer,
        );

        // --- Scatter pipeline ---
        let scatter_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("cr-scatter-bgl"),
            entries: &[
                storage_ro_entry(0), // pos_life
                storage_ro_entry(1), // vel_size
                storage_ro_entry(2), // alive_indices
                storage_ro_entry(3), // counters
                uniform_entry(4),    // tile uniforms
                storage_rw_entry(5), // tile_scatter_offsets
                storage_rw_entry(6), // sorted_particles
            ],
        });

        let scatter_pipeline = create_compute_pipeline(
            device,
            "cr-scatter",
            include_str!("../../../../../assets/shaders/builtin/compute_raster_scatter.wgsl"),
            &scatter_bgl,
        );

        let scatter_bind_groups = create_scatter_bind_groups(
            device,
            &scatter_bgl,
            pos_life_buffers,
            vel_size_buffers,
            alive_index_buffers,
            counter_buffer,
            &tile_uniform_buffer,
            &tile_scatter_offsets_buffer,
            &sorted_particles_buffer,
        );

        // --- Tiled raster pipeline ---
        let tiled_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("cr-tiled-bgl"),
            entries: &[
                storage_ro_entry(0),  // pos_life
                storage_ro_entry(1),  // vel_size
                storage_ro_entry(2),  // color
                storage_ro_entry(3),  // tile_offsets
                storage_ro_entry(4),  // tile_counts
                uniform_entry(5),     // tile uniforms
                storage_ro_entry(6),  // sorted_particles
                storage_rw_entry(7),  // fb_r
                storage_rw_entry(8),  // fb_g
                storage_rw_entry(9),  // fb_b
                storage_rw_entry(10), // fb_a
                storage_ro_entry(11), // flags (splat_mode conic in .zw, #1800)
                storage_rw_entry(12), // fb_vx (velocity_mode, #1482)
                storage_rw_entry(13), // fb_vy
            ],
        });

        let tiled_pipeline = create_compute_pipeline(
            device,
            "cr-tiled",
            include_str!("../../../../../assets/shaders/builtin/compute_raster_tiled.wgsl"),
            &tiled_bgl,
        );

        let tiled_bind_groups = create_tiled_bind_groups(
            device,
            &tiled_bgl,
            pos_life_buffers,
            vel_size_buffers,
            color_buffers,
            flags_buffers,
            &tile_offsets_buffer,
            &tile_counts_buffer,
            &tile_uniform_buffer,
            &sorted_particles_buffer,
            &fb_buffers,
            &vel_buffers,
        );

        // --- Velocity resolve (#1482) ---
        let velocity = velocity_mode.then(|| {
            create_velocity_resolve(
                device,
                width,
                height,
                &vel_buffers,
                &fb_buffers,
                &vel_resolve_uniform_buffer,
            )
        });

        // --- Resolve pipeline (render) ---
        let resolve_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("cr-resolve-bgl"),
            entries: &[
                fragment_storage_ro_entry(0),
                fragment_storage_ro_entry(1),
                fragment_storage_ro_entry(2),
                fragment_storage_ro_entry(3),
                BindGroupLayoutEntry {
                    binding: 4,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let resolve_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cr-resolve"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../../assets/shaders/builtin/compute_raster_resolve.wgsl")
                    .into(),
            ),
        });

        let resolve_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("cr-resolve-layout"),
            bind_group_layouts: &[&resolve_bgl],
            push_constant_ranges: &[],
        });

        let resolve_pipeline_additive = create_resolve_render_pipeline(
            device,
            &resolve_layout,
            &resolve_shader,
            hdr_format,
            wgpu::BlendState {
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
            },
            "cr-resolve-additive",
        );

        let resolve_pipeline_alpha = create_resolve_render_pipeline(
            device,
            &resolve_layout,
            &resolve_shader,
            hdr_format,
            wgpu::BlendState {
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
            },
            "cr-resolve-alpha",
        );

        let resolve_bind_group =
            create_resolve_bind_group(device, &resolve_bgl, &fb_buffers, &resolve_uniform_buffer);

        Self {
            width,
            height,
            max_particles,
            splat_mode,
            velocity_mode,
            fb_buffers,
            vel_buffers,
            velocity,
            vel_resolve_uniform_buffer,
            draw_uniform_buffer,
            resolve_uniform_buffer,
            tile_uniform_buffer,
            draw_pipeline,
            draw_bind_groups,
            draw_bgl,
            num_tiles_x,
            num_tiles_y,
            num_tiles,
            tile_counts_buffer,
            tile_offsets_buffer,
            tile_scatter_offsets_buffer,
            sorted_particles_buffer,
            bin_pipeline,
            bin_bgl,
            bin_bind_groups,
            prefix_sum_pipeline,
            prefix_sum_bgl,
            prefix_sum_bind_group,
            scatter_pipeline,
            scatter_bgl,
            scatter_bind_groups,
            tiled_pipeline,
            tiled_bgl,
            tiled_bind_groups,
            resolve_pipeline_additive,
            resolve_pipeline_alpha,
            resolve_bind_group,
            resolve_bgl,
        }
    }

    /// Returns true if the tiled path should be used for the given alive count.
    pub fn should_use_tiled(&self, alive_count: u32) -> bool {
        alive_count >= TILED_THRESHOLD && self.num_tiles <= MAX_PREFIX_SUM_CELLS
    }

    /// Clear all framebuffer channels via DMA fill (no compute shader overhead).
    pub fn dispatch_clear(&self, encoder: &mut CommandEncoder) {
        for fb in &self.fb_buffers {
            encoder.clear_buffer(fb, 0, None);
        }
        if self.velocity_mode {
            for fb in &self.vel_buffers {
                encoder.clear_buffer(fb, 0, None);
            }
        }
    }

    /// Dispatch the direct draw compute pass (particles write to atomic framebuffer).
    /// `output_idx` is the ping-pong index of the particle output buffers (1 - current).
    /// `max_particles` sets the dispatch size (shader exits early for thread_idx >= alive_count).
    pub fn dispatch_draw(
        &self,
        encoder: &mut CommandEncoder,
        queue: &Queue,
        output_idx: usize,
        max_particles: u32,
    ) {
        let uniforms = DrawUniforms {
            width: self.width,
            height: self.height,
            splat_mode: u32::from(self.splat_mode),
            velocity_mode: u32::from(self.velocity_mode),
        };
        queue.write_buffer(&self.draw_uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let workgroups = max_particles.div_ceil(WORKGROUP_SIZE);

        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("cr-draw"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.draw_pipeline);
        pass.set_bind_group(0, &self.draw_bind_groups[output_idx], &[]);
        pass.dispatch_workgroups(workgroups, 1, 1);
    }

    /// Dispatch the 4-pass tiled rasterization pipeline.
    /// Handles clearing, binning, prefix sum, scatter, and tiled raster internally.
    pub fn dispatch_tiled(
        &self,
        encoder: &mut CommandEncoder,
        queue: &Queue,
        output_idx: usize,
        max_particles: u32,
    ) {
        // Write tile uniforms
        let tile_uniforms = TileUniforms {
            width: self.width,
            height: self.height,
            num_tiles_x: self.num_tiles_x,
            num_tiles_y: self.num_tiles_y,
            max_particles: self.max_particles,
            splat_mode: u32::from(self.splat_mode),
            velocity_mode: u32::from(self.velocity_mode),
            _pad2: 0,
        };
        queue.write_buffer(
            &self.tile_uniform_buffer,
            0,
            bytemuck::bytes_of(&tile_uniforms),
        );

        // Clear framebuffer + tile counts
        for fb in &self.fb_buffers {
            encoder.clear_buffer(fb, 0, None);
        }
        if self.velocity_mode {
            for fb in &self.vel_buffers {
                encoder.clear_buffer(fb, 0, None);
            }
        }
        encoder.clear_buffer(&self.tile_counts_buffer, 0, None);

        let particle_workgroups = max_particles.div_ceil(WORKGROUP_SIZE);

        // Pass 1: Bin count
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cr-tile-bin"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.bin_pipeline);
            pass.set_bind_group(0, &self.bin_bind_groups[output_idx], &[]);
            pass.dispatch_workgroups(particle_workgroups, 1, 1);
        }

        // Pass 2: Prefix sum (tile_counts → tile_offsets)
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cr-tile-prefix-sum"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.prefix_sum_pipeline);
            pass.set_bind_group(0, &self.prefix_sum_bind_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }

        // Copy tile_offsets → tile_scatter_offsets (scatter destructively modifies offsets)
        encoder.copy_buffer_to_buffer(
            &self.tile_offsets_buffer,
            0,
            &self.tile_scatter_offsets_buffer,
            0,
            (self.num_tiles as u64) * 4,
        );

        // Pass 3: Scatter (write particle indices into sorted_particles)
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cr-tile-scatter"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.scatter_pipeline);
            pass.set_bind_group(0, &self.scatter_bind_groups[output_idx], &[]);
            pass.dispatch_workgroups(particle_workgroups, 1, 1);
        }

        // Pass 4: Tiled raster (shared-memory accumulation per tile)
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cr-tile-raster"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.tiled_pipeline);
            pass.set_bind_group(0, &self.tiled_bind_groups[output_idx], &[]);
            pass.dispatch_workgroups(self.num_tiles_x, self.num_tiles_y, 1);
        }
    }

    /// Render the resolve pass: reads atomic framebuffer, outputs to target.
    pub fn render_resolve(
        &self,
        encoder: &mut CommandEncoder,
        queue: &Queue,
        target: &TextureView,
        blend_mode: &str,
    ) {
        let mode = match blend_mode {
            "alpha" => 1u32,
            "oit" => 2, // weighted-average OIT: rgb/Σw + coverage (Splat #1800)
            _ => 0,
        };
        let uniforms = ResolveUniforms {
            width: self.width,
            height: self.height,
            mode,
            _pad: 0,
        };
        queue.write_buffer(
            &self.resolve_uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );

        // OIT composites through the alpha pipeline: (SrcAlpha, 1−SrcAlpha)
        // color / (One, 1−SrcAlpha) alpha is exactly weighted-average
        // compositing over the background pass.
        let pipeline = if blend_mode == "alpha" || blend_mode == "oit" {
            &self.resolve_pipeline_alpha
        } else {
            &self.resolve_pipeline_additive
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("cr-resolve"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.resolve_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Resolve the velocity channels into the `@particles.velocity` texture
    /// (#1482). No-op unless the effect opted in via `velocity_field`. Called
    /// at the end of `ParticleSystem::dispatch`, i.e. before the fragment
    /// passes, so they read same-frame velocity.
    pub fn resolve_velocity(&self, encoder: &mut CommandEncoder) {
        let Some(ref vel) = self.velocity else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("cr-resolve-velocity"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &vel.view,
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
        pass.set_pipeline(&vel.pipeline);
        pass.set_bind_group(0, &vel.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// The resolved per-pixel particle-velocity texture, when `velocity_field`
    /// is on — what the pass graph binds for `@particles.velocity`.
    pub fn velocity_view_sampler(&self) -> Option<(&TextureView, &wgpu::Sampler)> {
        self.velocity.as_ref().map(|v| (&v.view, &v.sampler))
    }

    /// Raw velocity texture, for test readback.
    #[allow(dead_code)]
    pub(crate) fn velocity_texture(&self) -> Option<&wgpu::Texture> {
        self.velocity.as_ref().map(|v| &v.texture)
    }

    /// Resize framebuffer if dimensions changed. Returns true if resized.
    #[allow(clippy::too_many_arguments)]
    pub fn ensure_size(
        &mut self,
        device: &Device,
        width: u32,
        height: u32,
        pos_life_buffers: &[wgpu::Buffer; 2],
        vel_size_buffers: &[wgpu::Buffer; 2],
        color_buffers: &[wgpu::Buffer; 2],
        flags_buffers: &[wgpu::Buffer; 2],
        alive_index_buffers: &[wgpu::Buffer; 2],
        counter_buffer: &wgpu::Buffer,
    ) -> bool {
        if self.width == width && self.height == height {
            return false;
        }

        self.width = width;
        self.height = height;

        let pixel_count = (width * height) as u64;
        let fb_size = pixel_count * 4;

        self.fb_buffers = std::array::from_fn(|i| {
            let label = ["fb-r", "fb-g", "fb-b", "fb-a"][i];
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: fb_size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        self.vel_buffers = create_vel_buffers(device, fb_size, self.velocity_mode);
        self.vel_resolve_uniform_buffer = create_vel_resolve_uniforms(device, width, height);
        if self.velocity_mode {
            self.velocity = Some(create_velocity_resolve(
                device,
                width,
                height,
                &self.vel_buffers,
                &self.fb_buffers,
                &self.vel_resolve_uniform_buffer,
            ));
        }

        // Recompute tile geometry
        self.num_tiles_x = width.div_ceil(TILE_SIZE);
        self.num_tiles_y = height.div_ceil(TILE_SIZE);
        self.num_tiles = self.num_tiles_x * self.num_tiles_y;

        // Recreate tile buffers (sized by num_tiles)
        self.tile_counts_buffer =
            create_tile_buffer(device, "cr-tile-counts", self.num_tiles, true);
        self.tile_offsets_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cr-tile-offsets"),
            size: (self.num_tiles as u64) * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        self.tile_scatter_offsets_buffer =
            create_tile_buffer(device, "cr-tile-scatter-offsets", self.num_tiles, true);

        // Recreate prefix sum pipeline with new tile count
        self.prefix_sum_pipeline =
            create_prefix_sum_pipeline(device, &self.prefix_sum_bgl, self.num_tiles);

        // Recreate all bind groups
        self.draw_bind_groups = create_draw_bind_groups(
            device,
            &self.draw_bgl,
            pos_life_buffers,
            vel_size_buffers,
            color_buffers,
            flags_buffers,
            alive_index_buffers,
            counter_buffer,
            &self.draw_uniform_buffer,
            &self.fb_buffers,
            &self.vel_buffers,
        );

        self.bin_bind_groups = create_bin_bind_groups(
            device,
            &self.bin_bgl,
            pos_life_buffers,
            vel_size_buffers,
            alive_index_buffers,
            counter_buffer,
            &self.tile_uniform_buffer,
            &self.tile_counts_buffer,
        );

        self.prefix_sum_bind_group = create_prefix_sum_bind_group(
            device,
            &self.prefix_sum_bgl,
            &self.tile_counts_buffer,
            &self.tile_offsets_buffer,
        );

        self.scatter_bind_groups = create_scatter_bind_groups(
            device,
            &self.scatter_bgl,
            pos_life_buffers,
            vel_size_buffers,
            alive_index_buffers,
            counter_buffer,
            &self.tile_uniform_buffer,
            &self.tile_scatter_offsets_buffer,
            &self.sorted_particles_buffer,
        );

        self.tiled_bind_groups = create_tiled_bind_groups(
            device,
            &self.tiled_bgl,
            pos_life_buffers,
            vel_size_buffers,
            color_buffers,
            flags_buffers,
            &self.tile_offsets_buffer,
            &self.tile_counts_buffer,
            &self.tile_uniform_buffer,
            &self.sorted_particles_buffer,
            &self.fb_buffers,
            &self.vel_buffers,
        );

        self.resolve_bind_group = create_resolve_bind_group(
            device,
            &self.resolve_bgl,
            &self.fb_buffers,
            &self.resolve_uniform_buffer,
        );

        true
    }
}

// --- Helper functions ---

fn storage_rw_entry(binding: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::COMPUTE,
        ty: BindingType::Buffer {
            ty: BufferBindingType::Storage { read_only: false },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn storage_ro_entry(binding: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::COMPUTE,
        ty: BindingType::Buffer {
            ty: BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::COMPUTE,
        ty: BindingType::Buffer {
            ty: BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn fragment_storage_ro_entry(binding: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
        ty: BindingType::Buffer {
            ty: BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// Velocity channel buffers (#1482): full framebuffer size when the effect
/// opted in, 16-byte stand-ins otherwise (bound but never touched — the
/// shaders' `velocity_mode` uniform guard gates every access).
fn create_vel_buffers(device: &Device, fb_size: u64, enabled: bool) -> [wgpu::Buffer; 2] {
    let size = if enabled { fb_size } else { 16 };
    std::array::from_fn(|i| {
        let label = ["fb-vx", "fb-vy"][i];
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    })
}

/// Uniforms for the velocity resolve, written at creation (no queue in
/// `new()`/`ensure_size`, so the contents go in via `mapped_at_creation`).
fn create_vel_resolve_uniforms(device: &Device, width: u32, height: u32) -> wgpu::Buffer {
    let uniforms = VelResolveUniforms {
        width,
        height,
        _pad0: 0,
        _pad1: 0,
    };
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("cr-vel-resolve-uniforms"),
        size: std::mem::size_of::<VelResolveUniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM,
        mapped_at_creation: true,
    });
    buffer
        .slice(..)
        .get_mapped_range_mut()
        .copy_from_slice(bytemuck::bytes_of(&uniforms));
    buffer.unmap();
    buffer
}

/// Build the velocity resolve target + pipeline + bind group (#1482).
fn create_velocity_resolve(
    device: &Device,
    width: u32,
    height: u32,
    vel_buffers: &[wgpu::Buffer; 2],
    fb_buffers: &[wgpu::Buffer; 4],
    uniform_buffer: &wgpu::Buffer,
) -> VelocityResolve {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cr-velocity-tex"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("cr-velocity-sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        ..Default::default()
    });

    let bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("cr-vel-resolve-bgl"),
        entries: &[
            fragment_storage_ro_entry(0), // fb_vx
            fragment_storage_ro_entry(1), // fb_vy
            fragment_storage_ro_entry(2), // fb_a
            BindGroupLayoutEntry {
                binding: 3,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("cr-vel-resolve"),
        source: wgpu::ShaderSource::Wgsl(
            include_str!(
                "../../../../../assets/shaders/builtin/compute_raster_resolve_velocity.wgsl"
            )
            .into(),
        ),
    });

    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("cr-vel-resolve-layout"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });

    let pipeline = create_resolve_render_pipeline(
        device,
        &layout,
        &shader,
        TextureFormat::Rgba16Float,
        wgpu::BlendState::REPLACE,
        "cr-vel-resolve",
    );

    let bind_group = device.create_bind_group(&BindGroupDescriptor {
        label: Some("cr-vel-resolve-bg"),
        layout: &bgl,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: vel_buffers[0].as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: vel_buffers[1].as_entire_binding(),
            },
            BindGroupEntry {
                binding: 2,
                resource: fb_buffers[3].as_entire_binding(),
            },
            BindGroupEntry {
                binding: 3,
                resource: uniform_buffer.as_entire_binding(),
            },
        ],
    });

    VelocityResolve {
        texture,
        view,
        sampler,
        pipeline,
        bind_group,
    }
}

fn create_tile_buffer(
    device: &Device,
    label: &str,
    num_tiles: u32,
    copy_dst: bool,
) -> wgpu::Buffer {
    let mut usage = wgpu::BufferUsages::STORAGE;
    if copy_dst {
        usage |= wgpu::BufferUsages::COPY_DST;
    }
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (num_tiles as u64) * 4,
        usage,
        mapped_at_creation: false,
    })
}

fn create_compute_pipeline(
    device: &Device,
    label: &str,
    source: &str,
    bgl: &BindGroupLayout,
) -> ComputePipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some(&format!("{label}-layout")),
        bind_group_layouts: &[bgl],
        push_constant_ranges: &[],
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(&format!("{label}-pipeline")),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("cs_main"),
        compilation_options: PipelineCompilationOptions::default(),
        cache: None,
    })
}

fn create_prefix_sum_pipeline(
    device: &Device,
    bgl: &BindGroupLayout,
    num_tiles: u32,
) -> ComputePipeline {
    let source = include_str!("../../../../../assets/shaders/builtin/spatial_hash_prefix_sum.wgsl");
    let patched = source.replace(
        "const NUM_CELLS: u32 = 1600u;",
        &format!("const NUM_CELLS: u32 = {num_tiles}u;"),
    );
    create_compute_pipeline(device, "cr-prefix-sum", &patched, bgl)
}

#[allow(clippy::too_many_arguments)]
fn create_draw_bind_groups(
    device: &Device,
    layout: &BindGroupLayout,
    pos_life_buffers: &[wgpu::Buffer; 2],
    vel_size_buffers: &[wgpu::Buffer; 2],
    color_buffers: &[wgpu::Buffer; 2],
    flags_buffers: &[wgpu::Buffer; 2],
    alive_index_buffers: &[wgpu::Buffer; 2],
    counter_buffer: &wgpu::Buffer,
    draw_uniform_buffer: &wgpu::Buffer,
    fb_buffers: &[wgpu::Buffer; 4],
    vel_buffers: &[wgpu::Buffer; 2],
) -> [BindGroup; 2] {
    let make_bg = |idx: usize, label: &str| {
        device.create_bind_group(&BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: pos_life_buffers[idx].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: vel_size_buffers[idx].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: color_buffers[idx].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: alive_index_buffers[idx].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: counter_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 5,
                    resource: draw_uniform_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 6,
                    resource: fb_buffers[0].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 7,
                    resource: fb_buffers[1].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 8,
                    resource: fb_buffers[2].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 9,
                    resource: fb_buffers[3].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 10,
                    resource: flags_buffers[idx].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 11,
                    resource: vel_buffers[0].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 12,
                    resource: vel_buffers[1].as_entire_binding(),
                },
            ],
        })
    };

    [make_bg(0, "cr-draw-bg-0"), make_bg(1, "cr-draw-bg-1")]
}

fn create_bin_bind_groups(
    device: &Device,
    layout: &BindGroupLayout,
    pos_life_buffers: &[wgpu::Buffer; 2],
    vel_size_buffers: &[wgpu::Buffer; 2],
    alive_index_buffers: &[wgpu::Buffer; 2],
    counter_buffer: &wgpu::Buffer,
    tile_uniform_buffer: &wgpu::Buffer,
    tile_counts_buffer: &wgpu::Buffer,
) -> [BindGroup; 2] {
    let make_bg = |idx: usize, label: &str| {
        device.create_bind_group(&BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: pos_life_buffers[idx].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: vel_size_buffers[idx].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: alive_index_buffers[idx].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: counter_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: tile_uniform_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 5,
                    resource: tile_counts_buffer.as_entire_binding(),
                },
            ],
        })
    };

    [make_bg(0, "cr-bin-bg-0"), make_bg(1, "cr-bin-bg-1")]
}

fn create_prefix_sum_bind_group(
    device: &Device,
    layout: &BindGroupLayout,
    tile_counts_buffer: &wgpu::Buffer,
    tile_offsets_buffer: &wgpu::Buffer,
) -> BindGroup {
    device.create_bind_group(&BindGroupDescriptor {
        label: Some("cr-prefix-sum-bg"),
        layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: tile_counts_buffer.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: tile_offsets_buffer.as_entire_binding(),
            },
        ],
    })
}

fn create_scatter_bind_groups(
    device: &Device,
    layout: &BindGroupLayout,
    pos_life_buffers: &[wgpu::Buffer; 2],
    vel_size_buffers: &[wgpu::Buffer; 2],
    alive_index_buffers: &[wgpu::Buffer; 2],
    counter_buffer: &wgpu::Buffer,
    tile_uniform_buffer: &wgpu::Buffer,
    tile_scatter_offsets_buffer: &wgpu::Buffer,
    sorted_particles_buffer: &wgpu::Buffer,
) -> [BindGroup; 2] {
    let make_bg = |idx: usize, label: &str| {
        device.create_bind_group(&BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: pos_life_buffers[idx].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: vel_size_buffers[idx].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: alive_index_buffers[idx].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: counter_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: tile_uniform_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 5,
                    resource: tile_scatter_offsets_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 6,
                    resource: sorted_particles_buffer.as_entire_binding(),
                },
            ],
        })
    };

    [make_bg(0, "cr-scatter-bg-0"), make_bg(1, "cr-scatter-bg-1")]
}

#[allow(clippy::too_many_arguments)]
fn create_tiled_bind_groups(
    device: &Device,
    layout: &BindGroupLayout,
    pos_life_buffers: &[wgpu::Buffer; 2],
    vel_size_buffers: &[wgpu::Buffer; 2],
    color_buffers: &[wgpu::Buffer; 2],
    flags_buffers: &[wgpu::Buffer; 2],
    tile_offsets_buffer: &wgpu::Buffer,
    tile_counts_buffer: &wgpu::Buffer,
    tile_uniform_buffer: &wgpu::Buffer,
    sorted_particles_buffer: &wgpu::Buffer,
    fb_buffers: &[wgpu::Buffer; 4],
    vel_buffers: &[wgpu::Buffer; 2],
) -> [BindGroup; 2] {
    let make_bg = |idx: usize, label: &str| {
        device.create_bind_group(&BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: pos_life_buffers[idx].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: vel_size_buffers[idx].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: color_buffers[idx].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: tile_offsets_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: tile_counts_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 5,
                    resource: tile_uniform_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 6,
                    resource: sorted_particles_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 7,
                    resource: fb_buffers[0].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 8,
                    resource: fb_buffers[1].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 9,
                    resource: fb_buffers[2].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 10,
                    resource: fb_buffers[3].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 11,
                    resource: flags_buffers[idx].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 12,
                    resource: vel_buffers[0].as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 13,
                    resource: vel_buffers[1].as_entire_binding(),
                },
            ],
        })
    };

    [make_bg(0, "cr-tiled-bg-0"), make_bg(1, "cr-tiled-bg-1")]
}

fn create_resolve_bind_group(
    device: &Device,
    layout: &BindGroupLayout,
    fb_buffers: &[wgpu::Buffer; 4],
    uniform_buffer: &wgpu::Buffer,
) -> BindGroup {
    device.create_bind_group(&BindGroupDescriptor {
        label: Some("cr-resolve-bg"),
        layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: fb_buffers[0].as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: fb_buffers[1].as_entire_binding(),
            },
            BindGroupEntry {
                binding: 2,
                resource: fb_buffers[2].as_entire_binding(),
            },
            BindGroupEntry {
                binding: 3,
                resource: fb_buffers[3].as_entire_binding(),
            },
            BindGroupEntry {
                binding: 4,
                resource: uniform_buffer.as_entire_binding(),
            },
        ],
    })
}

fn create_resolve_render_pipeline(
    device: &Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: TextureFormat,
    blend: wgpu::BlendState,
    label: &str,
) -> RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: PipelineCompilationOptions::default(),
        },
        fragment: Some(FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(ColorTargetState {
                format,
                blend: Some(blend),
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_uniforms_size() {
        assert_eq!(std::mem::size_of::<DrawUniforms>(), 16);
    }

    #[test]
    fn resolve_uniforms_size() {
        assert_eq!(std::mem::size_of::<ResolveUniforms>(), 16);
    }

    #[test]
    fn tile_uniforms_size() {
        assert_eq!(std::mem::size_of::<TileUniforms>(), 32);
    }

    #[test]
    fn tile_geometry_1080p() {
        let ntx = 1920u32.div_ceil(16);
        let nty = 1080u32.div_ceil(16);
        assert_eq!(ntx, 120);
        assert_eq!(nty, 68); // 1080/16 = 67.5 → 68
        assert_eq!(ntx * nty, 8160);
    }

    #[test]
    fn tile_geometry_4k() {
        let ntx = 3840u32.div_ceil(16);
        let nty = 2160u32.div_ceil(16);
        assert_eq!(ntx, 240);
        assert_eq!(nty, 135);
        assert!(ntx * nty <= MAX_PREFIX_SUM_CELLS);
    }

    #[test]
    fn vel_resolve_uniforms_size() {
        assert_eq!(std::mem::size_of::<VelResolveUniforms>(), 16);
    }

    /// IEEE 754 half → f32 (no `half` dependency; enough for finite values).
    fn f16_to_f32(bits: u16) -> f32 {
        let sign = ((bits >> 15) & 1) as u32;
        let exp = ((bits >> 10) & 0x1f) as u32;
        let frac = (bits & 0x3ff) as u32;
        let f = match exp {
            0 => {
                // Subnormal (or zero): value = frac / 2^10 * 2^-14
                return if sign == 1 { -1.0 } else { 1.0 } * (frac as f32) * 2.0f32.powi(-24);
            }
            0x1f => (sign << 31) | 0x7f80_0000 | (frac << 13), // inf/NaN
            _ => (sign << 31) | ((exp + 112) << 23) | (frac << 13),
        };
        f32::from_bits(f)
    }

    // Signed velocity resolve (#1482): one alive particle at NDC center moving
    // (+0.5, −0.25) NDC/s must land in the velocity texture as exactly that
    // signed value with nonzero coverage — through BOTH raster paths. Catches
    // fixed-point sign errors, channel swaps, and the tiled shared-memory
    // flush being missed.
    #[test]
    #[ignore = "requires a wgpu adapter; renders offscreen"]
    fn velocity_resolve_signed_direct_and_tiled() {
        use crate::gpu::test_gpu::{gpu_guard, test_gpu};
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let (w, h) = (64u32, 64u32);

        let storage_buf = |label: &str, bytes: &[u8]| {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes.len() as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: true,
            });
            buf.slice(..).get_mapped_range_mut().copy_from_slice(bytes);
            buf.unmap();
            buf
        };
        let pair = |label: &str, bytes: &[u8]| {
            [
                storage_buf(&format!("{label}-0"), bytes),
                storage_buf(&format!("{label}-1"), bytes),
            ]
        };

        // Size 0.004 → radius_px 0.128 → single-pixel path at (32, 32).
        let pos_life = pair("t-pos", bytemuck::cast_slice(&[0.0f32, 0.0, 0.0, 1.0]));
        let vel_size = pair("t-vel", bytemuck::cast_slice(&[0.5f32, -0.25, 0.0, 0.004]));
        let color = pair("t-col", bytemuck::cast_slice(&[1.0f32, 1.0, 1.0, 1.0]));
        let flags = pair("t-flg", bytemuck::cast_slice(&[0.0f32; 4]));
        let alive = pair("t-alv", bytemuck::cast_slice(&[0u32]));
        let counters = storage_buf("t-cnt", bytemuck::cast_slice(&[1u32, 0, 0, 0]));

        let cr = ComputeRasterizer::new(
            &device,
            TextureFormat::Rgba16Float,
            w,
            h,
            &pos_life,
            &vel_size,
            &color,
            &flags,
            &alive,
            &counters,
            1,
            false,
            true, // velocity_mode
        );

        let bpr = (w * 8).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT); // 8 B/px
        let read_texel = |tiled: bool, tx: u32, ty: u32| -> [f32; 4] {
            let mut enc = device.create_command_encoder(&Default::default());
            if tiled {
                cr.dispatch_tiled(&mut enc, &queue, 0, 1);
            } else {
                cr.dispatch_clear(&mut enc);
                cr.dispatch_draw(&mut enc, &queue, 0, 1);
            }
            cr.resolve_velocity(&mut enc);

            let staging = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("t-vel-staging"),
                size: (bpr * h) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: cr.velocity_texture().expect("velocity texture exists"),
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &staging,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bpr),
                        rows_per_image: Some(h),
                    },
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([enc.finish()]);

            staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            let data = staging.slice(..).get_mapped_range().to_vec();
            let off = (ty * bpr + tx * 8) as usize;
            std::array::from_fn(|c| {
                f16_to_f32(u16::from_le_bytes([
                    data[off + c * 2],
                    data[off + c * 2 + 1],
                ]))
            })
        };

        for tiled in [false, true] {
            let path = if tiled { "tiled" } else { "direct" };
            let v = read_texel(tiled, 32, 32);
            assert!(
                (v[0] - 0.5).abs() < 0.02,
                "{path}: vx should be +0.5 NDC/s, got {}",
                v[0]
            );
            assert!(
                (v[1] + 0.25).abs() < 0.02,
                "{path}: vy should be −0.25 NDC/s, got {}",
                v[1]
            );
            assert!(v[2] > 0.5, "{path}: coverage should be ~1, got {}", v[2]);

            // Far corner: no particle → zero velocity, zero coverage.
            let z = read_texel(tiled, 5, 5);
            assert!(
                z[0].abs() < 1e-3 && z[1].abs() < 1e-3 && z[2].abs() < 1e-3,
                "{path}: empty texel should be zero, got {z:?}"
            );
        }
    }
}
