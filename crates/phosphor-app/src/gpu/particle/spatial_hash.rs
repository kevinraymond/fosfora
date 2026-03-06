use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingType, BufferBindingType, CommandEncoder, ComputePipeline, Device,
    PipelineCompilationOptions, PipelineLayoutDescriptor, ShaderStages,
};

const WORKGROUP_SIZE: u32 = 256;

/// Compute grid dimensions that scale with particle count.
/// Target: ~16 particles per cell. Clamped to [40, 256].
/// `grid_max_override`: if > 0, caps the upper bound (for effects with large interaction radii).
pub fn grid_dims(max_particles: u32, grid_max_override: u32) -> (u32, u32) {
    let upper = if grid_max_override > 0 { grid_max_override } else { 256 };
    let lower = upper.min(40);
    let dim = ((max_particles as f64 / 16.0).sqrt() as u32).clamp(lower, upper);
    (dim, dim)
}

/// GPU spatial hash grid for particle-particle interaction.
/// 3-pass compute pipeline: count → prefix sum → scatter.
/// After execution, `sorted_indices` contains particle indices sorted by grid cell,
/// and `cell_offsets` contains the start index for each cell in `sorted_indices`.
pub struct SpatialHashGrid {
    /// Per-cell atomic count buffer (num_cells * 4 bytes)
    cell_counts_buffer: wgpu::Buffer,
    /// Per-cell prefix sum result (num_cells * 4 bytes)
    cell_offsets_buffer: wgpu::Buffer,
    /// Sorted particle indices (max_particles * 4 bytes)
    sorted_indices_buffer: wgpu::Buffer,

    // Pass 1: Count — each particle hashes position → atomicAdd
    count_pipeline: ComputePipeline,
    count_bind_groups: [BindGroup; 2], // ping-pong: read from different storage buffers

    // Pass 2: Prefix sum (Blelloch scan) on cell_counts → cell_offsets
    prefix_sum_pipeline: ComputePipeline,
    prefix_sum_bind_group: BindGroup,

    // Pass 3: Scatter — each particle writes index to sorted_indices
    scatter_pipeline: ComputePipeline,
    scatter_bind_groups: [BindGroup; 2],

    pub max_particles: u32,
    grid_w: u32,
    grid_h: u32,
    num_cells: u32,

    /// Bind group layout for the neighbor query in sim shader (group 3)
    pub query_bgl: BindGroupLayout,
    pub query_bind_group: BindGroup,
}

impl SpatialHashGrid {
    pub fn new(
        device: &Device,
        max_particles: u32,
        grid_max_override: u32,
        pos_life_buffers: &[wgpu::Buffer; 2],
        uniform_buffer: &wgpu::Buffer,
    ) -> Self {
        let (grid_w, grid_h) = grid_dims(max_particles, grid_max_override);
        let num_cells = grid_w * grid_h;
        log::info!(
            "Spatial hash grid: {grid_w}x{grid_h} ({num_cells} cells) for {max_particles} particles"
        );

        // Buffers
        let cell_counts_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spatial-hash-cell-counts"),
            size: (num_cells * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let cell_offsets_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spatial-hash-cell-offsets"),
            size: (num_cells * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sorted_indices_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spatial-hash-sorted-indices"),
            size: (max_particles * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // --- Pass 1: Count pipeline ---
        let count_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("spatial-hash-count-bgl"),
            entries: &[
                // binding 0: pos_life (read)
                bgl_storage_entry(0, true),
                // binding 1: cell_counts (read_write atomic)
                bgl_storage_entry(1, false),
                // binding 2: uniforms (for max_particles)
                bgl_uniform_entry(2),
            ],
        });

        let count_source =
            include_str!("../../../../../assets/shaders/builtin/spatial_hash_count.wgsl");
        let count_source = patch_grid_constants(count_source, grid_w, grid_h);
        let count_pipeline =
            create_compute_pipeline(device, "spatial-hash-count", &count_source, &count_bgl);

        let count_bind_groups = [
            create_count_bind_group(
                device,
                &count_bgl,
                &pos_life_buffers[0],
                &cell_counts_buffer,
                uniform_buffer,
            ),
            create_count_bind_group(
                device,
                &count_bgl,
                &pos_life_buffers[1],
                &cell_counts_buffer,
                uniform_buffer,
            ),
        ];

        // --- Pass 2: Prefix sum pipeline ---
        let prefix_sum_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("spatial-hash-prefix-sum-bgl"),
            entries: &[
                // binding 0: cell_counts (read)
                bgl_storage_entry(0, true),
                // binding 1: cell_offsets (write)
                bgl_storage_entry(1, false),
            ],
        });

        let prefix_sum_source =
            include_str!("../../../../../assets/shaders/builtin/spatial_hash_prefix_sum.wgsl");
        let prefix_sum_source = prefix_sum_source.replace(
            "const NUM_CELLS: u32 = 1600u;",
            &format!("const NUM_CELLS: u32 = {num_cells}u;"),
        );
        let prefix_sum_pipeline = create_compute_pipeline(
            device,
            "spatial-hash-prefix-sum",
            &prefix_sum_source,
            &prefix_sum_bgl,
        );

        let prefix_sum_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("spatial-hash-prefix-sum-bg"),
            layout: &prefix_sum_bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: cell_counts_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: cell_offsets_buffer.as_entire_binding(),
                },
            ],
        });

        // --- Pass 3: Scatter pipeline ---
        let scatter_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("spatial-hash-scatter-bgl"),
            entries: &[
                // binding 0: pos_life (read)
                bgl_storage_entry(0, true),
                // binding 1: cell_offsets (read_write atomic — scatter uses atomicAdd for local offset)
                bgl_storage_entry(1, false),
                // binding 2: sorted_indices (write)
                bgl_storage_entry(2, false),
                // binding 3: uniforms
                bgl_uniform_entry(3),
            ],
        });

        let scatter_source =
            include_str!("../../../../../assets/shaders/builtin/spatial_hash_scatter.wgsl");
        let scatter_source = patch_grid_constants(scatter_source, grid_w, grid_h);
        let scatter_pipeline = create_compute_pipeline(
            device,
            "spatial-hash-scatter",
            &scatter_source,
            &scatter_bgl,
        );

        let scatter_bind_groups = [
            create_scatter_bind_group(
                device,
                &scatter_bgl,
                &pos_life_buffers[0],
                &cell_offsets_buffer,
                &sorted_indices_buffer,
                uniform_buffer,
            ),
            create_scatter_bind_group(
                device,
                &scatter_bgl,
                &pos_life_buffers[1],
                &cell_offsets_buffer,
                &sorted_indices_buffer,
                uniform_buffer,
            ),
        ];

        // --- Query bind group (for sim shader, group 3) ---
        let query_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("spatial-hash-query-bgl"),
            entries: &[
                // binding 0: cell_offsets (read)
                bgl_storage_entry(0, true),
                // binding 1: cell_counts (read)
                bgl_storage_entry(1, true),
                // binding 2: sorted_indices (read)
                bgl_storage_entry(2, true),
            ],
        });

        let query_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("spatial-hash-query-bg"),
            layout: &query_bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: cell_offsets_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: cell_counts_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: sorted_indices_buffer.as_entire_binding(),
                },
            ],
        });

        Self {
            cell_counts_buffer,
            cell_offsets_buffer,
            sorted_indices_buffer,
            count_pipeline,
            count_bind_groups,
            prefix_sum_pipeline,
            prefix_sum_bind_group,
            scatter_pipeline,
            scatter_bind_groups,
            max_particles,
            grid_w,
            grid_h,
            num_cells,
            query_bgl,
            query_bind_group,
        }
    }

    /// Run the 3-pass spatial hash build before particle sim.
    /// `current` is the ping-pong index (which storage buffer has current particle data).
    pub fn dispatch(&self, encoder: &mut CommandEncoder, current: usize) {
        // Clear cell_counts and cell_offsets to zero (GPU-side, no CPU allocation)
        encoder.clear_buffer(&self.cell_counts_buffer, 0, None);
        encoder.clear_buffer(&self.cell_offsets_buffer, 0, None);

        let workgroups = self.max_particles.div_ceil(WORKGROUP_SIZE);

        // Pass 1: Count particles per cell
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("spatial-hash-count"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.count_pipeline);
            // Read from the CURRENT particle buffer (not the output)
            // current=0 → read storage[0], current=1 → read storage[1]
            pass.set_bind_group(0, &self.count_bind_groups[current], &[]);
            pass.dispatch_workgroups(workgroups, 1, 1);
        }

        // Pass 2: Parallel prefix sum (256 threads, single workgroup)
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("spatial-hash-prefix-sum"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.prefix_sum_pipeline);
            pass.set_bind_group(0, &self.prefix_sum_bind_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }

        // Pass 3: Scatter particles into sorted order
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("spatial-hash-scatter"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.scatter_pipeline);
            pass.set_bind_group(0, &self.scatter_bind_groups[current], &[]);
            pass.dispatch_workgroups(workgroups, 1, 1);
        }
    }
}

// --- Helper functions ---

fn bgl_storage_entry(binding: u32, read_only: bool) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::COMPUTE,
        ty: BindingType::Buffer {
            ty: BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn bgl_uniform_entry(binding: u32) -> BindGroupLayoutEntry {
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

fn create_count_bind_group(
    device: &Device,
    layout: &BindGroupLayout,
    pos_life: &wgpu::Buffer,
    cell_counts: &wgpu::Buffer,
    uniforms: &wgpu::Buffer,
) -> BindGroup {
    device.create_bind_group(&BindGroupDescriptor {
        label: Some("spatial-hash-count-bg"),
        layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: pos_life.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: cell_counts.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 2,
                resource: uniforms.as_entire_binding(),
            },
        ],
    })
}

/// Replace hardcoded GRID_W/GRID_H constants in a shader source string.
fn patch_grid_constants(source: &str, grid_w: u32, grid_h: u32) -> String {
    source
        .replace(
            "const GRID_W: u32 = 40u;",
            &format!("const GRID_W: u32 = {grid_w}u;"),
        )
        .replace(
            "const GRID_H: u32 = 40u;",
            &format!("const GRID_H: u32 = {grid_h}u;"),
        )
}

fn create_scatter_bind_group(
    device: &Device,
    layout: &BindGroupLayout,
    pos_life: &wgpu::Buffer,
    cell_offsets: &wgpu::Buffer,
    sorted_indices: &wgpu::Buffer,
    uniforms: &wgpu::Buffer,
) -> BindGroup {
    device.create_bind_group(&BindGroupDescriptor {
        label: Some("spatial-hash-scatter-bg"),
        layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: pos_life.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: cell_offsets.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 2,
                resource: sorted_indices.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 3,
                resource: uniforms.as_entire_binding(),
            },
        ],
    })
}
