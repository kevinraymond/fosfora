use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingType, BufferBindingType, CommandEncoder, ComputePipeline, Device,
    PipelineCompilationOptions, PipelineLayoutDescriptor, ShaderStages,
};

const WORKGROUP_SIZE: u32 = 256;

/// Probes read the grid back; production buffers carry no extra usage.
const PROBE_USAGE: wgpu::BufferUsages = if cfg!(test) {
    wgpu::BufferUsages::COPY_SRC
} else {
    wgpu::BufferUsages::empty()
};

const COUNT_2D: &str =
    include_str!("../../../../../assets/shaders/builtin/spatial_hash_count.wgsl");
const SCATTER_2D: &str =
    include_str!("../../../../../assets/shaders/builtin/spatial_hash_scatter.wgsl");
const PREFIX_SUM: &str =
    include_str!("../../../../../assets/shaders/builtin/spatial_hash_prefix_sum.wgsl");
const COUNT_3D: &str =
    include_str!("../../../../../assets/shaders/builtin/spatial_hash_count_3d.wgsl");
const SCATTER_3D: &str =
    include_str!("../../../../../assets/shaders/builtin/spatial_hash_scatter_3d.wgsl");

/// Compute grid dimensions that scale with particle count.
/// Target: ~16 particles per cell. Clamped to [40, 256].
/// `grid_max_override`: if > 0, caps the upper bound (for effects with large interaction radii).
pub fn grid_dims(max_particles: u32, grid_max_override: u32) -> (u32, u32) {
    let upper = if grid_max_override > 0 {
        grid_max_override
    } else {
        256
    };
    let lower = upper.min(40);
    let dim = ((max_particles as f64 / 16.0).sqrt() as u32).clamp(lower, upper);
    (dim, dim)
}

/// Edge length of the 3D grid (`"interaction_3d"`): `num_cells` is its cube.
/// Target: ~16 particles per cell. Clamped to [8, 64]; 200K particles give 23.
/// `grid_max_override`: if > 0, caps the upper bound, as for `grid_dims`.
pub fn grid_dims_3d(max_particles: u32, grid_max_override: u32) -> u32 {
    let upper = if grid_max_override > 0 {
        grid_max_override
    } else {
        64
    };
    let lower = upper.min(8);
    ((max_particles as f64 / 16.0).cbrt() as u32).clamp(lower, upper)
}

/// How a `SpatialHashGrid` maps particles to cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpatialHashMode {
    /// `pos_life.xy` in clip space [-1,1]² over a `grid_dims` grid
    /// (`"interaction": true`; every shipped interaction effect).
    Planar,
    /// `pos_life.xyz` in meters within ±`emitter_radius` of the anchor over a
    /// `grid_dims_3d` cube (`"interaction_3d": true`).
    Volume,
}

/// The 3D count/scatter shaders read `emitter_radius` through a `Uniforms`
/// prefix of the live `ParticleUniforms` buffer, at this byte offset.
const _: () = assert!(std::mem::offset_of!(super::types::ParticleUniforms, emitter_radius) == 24);

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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    grid_w: u32,
    #[allow(dead_code)]
    grid_h: u32,
    #[allow(dead_code)]
    num_cells: u32,
    /// Edge of the 3D grid, patched into the sim's `SH_GRID_D`; 1 in `Planar` mode.
    grid_d: u32,
    mode: SpatialHashMode,

    /// Bind group layout for the neighbor query in sim shader (group 3)
    pub query_bgl: BindGroupLayout,
    pub query_bind_group: BindGroup,
}

impl SpatialHashGrid {
    pub fn new(
        device: &Device,
        max_particles: u32,
        grid_max_override: u32,
        mode: SpatialHashMode,
        pos_life_buffers: &[wgpu::Buffer; 2],
        uniform_buffer: &wgpu::Buffer,
    ) -> Self {
        let (grid_w, grid_h, grid_d) = match mode {
            SpatialHashMode::Planar => {
                let (w, h) = grid_dims(max_particles, grid_max_override);
                (w, h, 1)
            }
            SpatialHashMode::Volume => {
                let d = grid_dims_3d(max_particles, grid_max_override);
                (d, d, d)
            }
        };
        let num_cells = grid_w * grid_h * grid_d;
        match mode {
            SpatialHashMode::Planar => log::info!(
                "Spatial hash grid: {grid_w}x{grid_h} ({num_cells} cells) for {max_particles} particles"
            ),
            SpatialHashMode::Volume => log::info!(
                "Spatial hash grid (3D): {grid_d}^3 ({num_cells} cells) for {max_particles} particles"
            ),
        }

        // Buffers
        let cell_counts_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spatial-hash-cell-counts"),
            size: (num_cells * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | PROBE_USAGE,
            mapped_at_creation: false,
        });

        let cell_offsets_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spatial-hash-cell-offsets"),
            size: (num_cells * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | PROBE_USAGE,
            mapped_at_creation: false,
        });

        let sorted_indices_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spatial-hash-sorted-indices"),
            size: (max_particles * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | PROBE_USAGE,
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

        let count_source = match mode {
            SpatialHashMode::Planar => patch_grid_constants(COUNT_2D, grid_w, grid_h),
            SpatialHashMode::Volume => patch_grid_d(COUNT_3D, grid_d),
        };
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

        let prefix_sum_source = PREFIX_SUM.replace(
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

        let scatter_source = match mode {
            SpatialHashMode::Planar => patch_grid_constants(SCATTER_2D, grid_w, grid_h),
            SpatialHashMode::Volume => patch_grid_d(SCATTER_3D, grid_d),
        };
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
            grid_d,
            mode,
            query_bgl,
            query_bind_group,
        }
    }

    /// How this grid maps particles to cells.
    pub fn mode(&self) -> SpatialHashMode {
        self.mode
    }

    /// Edge of the 3D grid, the value the sim's `SH_GRID_D` must carry; 1 in
    /// `Planar` mode.
    pub fn grid_d(&self) -> u32 {
        self.grid_d
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

#[cfg(test)]
impl SpatialHashGrid {
    /// `[cell_counts, cell_offsets, sorted_indices]`, for readback in probes.
    pub(super) fn buffers(&self) -> [&wgpu::Buffer; 3] {
        [
            &self.cell_counts_buffer,
            &self.cell_offsets_buffer,
            &self.sorted_indices_buffer,
        ]
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

/// Replace the hardcoded GRID_D constant in a 3D hash shader source string.
fn patch_grid_d(source: &str, grid_d: u32) -> String {
    source.replace(
        "const GRID_D: u32 = 8u;",
        &format!("const GRID_D: u32 = {grid_d}u;"),
    )
}

/// Set a sim source's `SH_GRID_D` to `grid_d`, whatever value it carries.
///
/// The loader patches it from the effect's unscaled `max_count`, but the grid
/// is sized from the count `ParticleSystem` actually allocates (after quality
/// scaling and device clamps), and the initial-effect path in `app.rs` never
/// sets it. The grid owns the buffers, so its edge is the one the sim must use.
/// A source without the declaration comes back unchanged.
pub(super) fn patch_sim_grid_d(source: &str, grid_d: u32) -> String {
    const DECL: &str = "const SH_GRID_D: u32 = ";
    let Some(start) = source.find(DECL) else {
        return source.to_string();
    };
    let value = start + DECL.len();
    let Some(len) = source[value..].find(';') else {
        return source.to_string();
    };
    format!("{}{grid_d}u{}", &source[..value], &source[value + len..])
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The 2D path sizes exactly as it did before the 3D mode existed.
    #[test]
    fn grid_dims_2d_unchanged() {
        assert_eq!(grid_dims(40_000, 0), (50, 50));
        assert_eq!(grid_dims(200_000, 0), (111, 111));
        assert_eq!(grid_dims(1_200_000, 0), (256, 256));
        assert_eq!(grid_dims(1_000, 0), (40, 40));
        assert_eq!(grid_dims(200_000, 32), (32, 32));
    }

    #[test]
    fn grid_dims_3d_targets_16_per_cell() {
        let d = grid_dims_3d(200_000, 0);
        assert_eq!(d, 23);
        assert_eq!(d * d * d, 12_167);
        assert_eq!(grid_dims_3d(1_000, 0), 8, "clamped up to 8");
        assert_eq!(grid_dims_3d(20_000_000, 0), 64, "clamped down to 64");
        assert_eq!(grid_dims_3d(200_000, 4), 4, "grid_max caps the edge");
        assert_eq!(grid_dims_3d(200_000, 100), 23, "grid_max only caps");
    }

    /// The 2D shaders keep the anchors `patch_grid_constants` and the
    /// NUM_CELLS patch rely on; a missed anchor would silently run a 40x40 grid.
    #[test]
    fn shaders_2d_keep_their_anchors() {
        for (name, src) in [("count", COUNT_2D), ("scatter", SCATTER_2D)] {
            assert!(src.contains("const GRID_W: u32 = 40u;"), "{name}: GRID_W");
            assert!(src.contains("const GRID_H: u32 = 40u;"), "{name}: GRID_H");
            assert!(
                !src.contains("GRID_D"),
                "{name}: 2D shader grew a 3D constant"
            );
        }
        assert!(PREFIX_SUM.contains("const NUM_CELLS: u32 = 1600u;"));
        let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
        for anchor in [
            "const SH_GRID_W: u32 = 40u;",
            "const SH_GRID_H: u32 = 40u;",
            "const SH_GRID_D: u32 = 1u;",
        ] {
            assert_eq!(plib.matches(anchor).count(), 1, "particle_lib: {anchor}");
        }
    }

    #[test]
    fn shaders_3d_patch_and_validate() {
        for (name, src) in [("count_3d", COUNT_3D), ("scatter_3d", SCATTER_3D)] {
            assert!(src.contains("const GRID_D: u32 = 8u;"), "{name}: anchor");
            let patched = patch_grid_d(src, 23);
            assert!(patched.contains("const GRID_D: u32 = 23u;"), "{name}");
            crate::trama::effect::validate_wgsl(&patched)
                .unwrap_or_else(|e| panic!("{name} does not validate: {e:?}"));
        }
    }

    /// The 3D shaders' `Uniforms` is a prefix of `ParticleUniforms` bound to the
    /// same buffer, so `emitter_radius` must sit where the Rust struct puts it.
    #[test]
    fn shaders_3d_read_emitter_radius_at_its_rust_offset() {
        let rust = std::mem::offset_of!(super::super::types::ParticleUniforms, emitter_radius);
        for (name, src) in [("count_3d", COUNT_3D), ("scatter_3d", SCATTER_3D)] {
            let module = naga::front::wgsl::parse_str(src).unwrap();
            let (_, ty) = module
                .types
                .iter()
                .find(|(_, t)| t.name.as_deref() == Some("Uniforms"))
                .expect("Uniforms struct");
            let naga::TypeInner::Struct { members, .. } = &ty.inner else {
                panic!("{name}: Uniforms is not a struct");
            };
            let m = members
                .iter()
                .find(|m| m.name.as_deref() == Some("emitter_radius"))
                .expect("emitter_radius member");
            assert_eq!(m.offset as usize, rust, "{name}");
            let max = members
                .iter()
                .find(|m| m.name.as_deref() == Some("max_particles"))
                .expect("max_particles member");
            assert_eq!(
                max.offset as usize,
                std::mem::offset_of!(super::super::types::ParticleUniforms, max_particles),
                "{name}"
            );
        }
    }

    #[test]
    fn interaction_3d_is_opt_in_and_implies_interaction() {
        use super::super::types::ParticleDef;
        let parse = |json: &str| serde_json::from_str::<ParticleDef>(json).unwrap();
        let plain = parse("{}");
        assert!(!plain.interaction_3d);
        assert_eq!(plain.spatial_hash_mode(), None);
        assert_eq!(
            parse(r#"{"interaction": true}"#).spatial_hash_mode(),
            Some(SpatialHashMode::Planar)
        );
        assert_eq!(
            parse(r#"{"interaction_3d": true}"#).spatial_hash_mode(),
            Some(SpatialHashMode::Volume)
        );
        assert_eq!(
            parse(r#"{"interaction": true, "interaction_3d": true}"#).spatial_hash_mode(),
            Some(SpatialHashMode::Volume)
        );
        // Existing effects serialize exactly as before.
        let json = serde_json::to_string(&plain).unwrap();
        assert!(!json.contains("interaction_3d"), "{json}");
    }

    #[test]
    fn patch_sim_grid_d_rewrites_any_value() {
        let src = "a\nconst SH_GRID_D: u32 = 1u;\nb";
        assert_eq!(
            patch_sim_grid_d(src, 23),
            "a\nconst SH_GRID_D: u32 = 23u;\nb"
        );
        // Stale value from the loader: the grid's edge still wins.
        let stale = "const SH_GRID_D: u32 = 17u;";
        assert_eq!(patch_sim_grid_d(stale, 4), "const SH_GRID_D: u32 = 4u;");
        assert_eq!(patch_sim_grid_d("no decl", 4), "no decl");
    }

    /// The 3D query helpers compile inside particle_lib at baseline WebGPU
    /// capabilities, called the way a world-layout sim would.
    #[test]
    fn particle_lib_3d_query_validates() {
        let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
        let sim = "
@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    var p = read_particle(gid.x);
    let c = sh_pos_to_cell_3d(p.pos_life.xyz, u.emitter_radius);
    let r = sh_cell_range_3d(c + vec3i(-1, 0, 1));
    if r.y > 0u {
        p.vel_size.x = f32(sh_sorted_indices[r.x]);
    }
    write_particle(gid.x, p);
}";
        crate::trama::effect::validate_wgsl(&format!(
            "{}\n{plib}\n{sim}",
            crate::effect::loader::probe_libs()
        ))
        .expect("particle_lib with the 3D hash helpers validates");
    }

    fn read_u32s(device: &Device, queue: &wgpu::Queue, buf: &wgpu::Buffer, len: u32) -> Vec<u32> {
        let bytes = u64::from(len) * 4;
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prefix-sum-readback"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("prefix-sum-readback"),
        });
        enc.copy_buffer_to_buffer(buf, 0, &staging, 0, bytes);
        queue.submit([enc.finish()]);
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        let out = bytemuck::cast_slice(&staging.slice(..).get_mapped_range()).to_vec();
        staging.unmap();
        out
    }

    /// The prefix sum is one workgroup that chunks NUM_CELLS across its 256
    /// threads, so it serves the 3D cell counts too: 23^3 (200K particles) and
    /// 64^3 (the default cap), both past the 65,536 its header comment names.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn prefix_sum_scans_3d_cell_counts() {
        use wgpu::util::DeviceExt;
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, queue, _) = crate::headless::gpu::create().expect("headless device");
        for d in [23u32, 64] {
            let num_cells = d * d * d;
            let counts: Vec<u32> = (0..num_cells)
                .map(|i| i.wrapping_mul(2_654_435_761) >> 29)
                .collect();
            let counts_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("prefix-sum-counts"),
                contents: bytemuck::cast_slice(&counts),
                usage: wgpu::BufferUsages::STORAGE,
            });
            let offsets_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("prefix-sum-offsets"),
                size: u64::from(num_cells) * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("prefix-sum-probe-bgl"),
                entries: &[bgl_storage_entry(0, true), bgl_storage_entry(1, false)],
            });
            let src = PREFIX_SUM.replace(
                "const NUM_CELLS: u32 = 1600u;",
                &format!("const NUM_CELLS: u32 = {num_cells}u;"),
            );
            let pipeline = create_compute_pipeline(&device, "prefix-sum-probe", &src, &bgl);
            let bg = device.create_bind_group(&BindGroupDescriptor {
                label: Some("prefix-sum-probe-bg"),
                layout: &bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: counts_buf.as_entire_binding(),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: offsets_buf.as_entire_binding(),
                    },
                ],
            });
            let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("prefix-sum-probe"),
            });
            {
                let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("prefix-sum-probe"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bg, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
            queue.submit([enc.finish()]);

            let got = read_u32s(&device, &queue, &offsets_buf, num_cells);
            let mut want = Vec::with_capacity(counts.len());
            let mut acc = 0u32;
            for c in &counts {
                want.push(acc);
                acc += c;
            }
            let first_bad = got.iter().zip(&want).position(|(g, w)| g != w);
            assert_eq!(first_bad, None, "{d}^3: exclusive scan diverges");
        }
    }
}
