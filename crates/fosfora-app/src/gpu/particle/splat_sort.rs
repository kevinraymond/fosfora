//! GPU depth counting sort for the sorted 3DGS renderer (Splat #1800).
//!
//! Produces a back-to-front ordered index list of the alive splats so the
//! billboard renderer (`splat_render.wgsl`) can composite them front-to-back
//! with hardware alpha-over — real occlusion, matching SuperSplat, which the
//! order-independent weighted-average OIT resolve cannot do.
//!
//! A 16-bit counting sort: each alive splat's view depth (`pos_life.z`) is
//! quantized to one of 65536 far→near buckets (histogram), the bucket counts
//! are exclusive-scanned (reusing `spatial_hash_prefix_sum.wgsl` patched to
//! 65536 — exactly its single-workgroup maximum), and each splat scatters its
//! index into the scanned slot. ~4 cheap dispatches regardless of splat count,
//! versus the bitonic sort's O(n log²n) dispatches (unusable past 65536).

use bytemuck::{Pod, Zeroable};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingType, BufferBindingType, CommandEncoder, ComputePipeline, Device,
    PipelineCompilationOptions, PipelineLayoutDescriptor, Queue, ShaderStages,
};

const WORKGROUP_SIZE: u32 = 256;
/// 65536 = 256×256, the exact single-dispatch ceiling of the reused Blelloch
/// scan, and the number of depth buckets (16-bit key).
const NUM_BUCKETS: u32 = 65_536;

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
struct SplatSortUniforms {
    depth_near: f32,
    depth_far: f32,
    _pad0: f32,
    _pad1: f32,
}

/// Reusable bind-group-layout entry helpers (all compute-visible).
fn storage_entry(binding: u32, read_only: bool) -> BindGroupLayoutEntry {
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

/// GPU depth sort. All buffers are resolution- and count-independent except
/// `sorted_indices` (sized to `max_particles`), so nothing here resizes.
pub struct SplatSorter {
    max_particles: u32,

    histogram_buffer: wgpu::Buffer,
    offsets_buffer: wgpu::Buffer,
    scatter_offsets_buffer: wgpu::Buffer,
    sorted_indices_buffer: wgpu::Buffer,
    sort_uniform_buffer: wgpu::Buffer,

    histogram_pipeline: ComputePipeline,
    histogram_bind_groups: [BindGroup; 2],
    scan_pipeline: ComputePipeline,
    scan_bind_group: BindGroup,
    scatter_pipeline: ComputePipeline,
    scatter_bind_groups: [BindGroup; 2],
}

impl SplatSorter {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &Device,
        max_particles: u32,
        pos_life_buffers: &[wgpu::Buffer; 2],
        alive_index_buffers: &[wgpu::Buffer; 2],
        counter_buffer: &wgpu::Buffer,
    ) -> Self {
        let bucket_bytes = (NUM_BUCKETS as u64) * 4;

        let histogram_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("splat-sort-histogram"),
            size: bucket_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let offsets_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("splat-sort-offsets"),
            size: bucket_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let scatter_offsets_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("splat-sort-scatter-offsets"),
            size: bucket_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sorted_indices_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("splat-sorted-indices"),
            size: (max_particles as u64) * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let sort_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("splat-sort-uniforms"),
            size: std::mem::size_of::<SplatSortUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // --- Histogram pass ---
        let histogram_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("splat-sort-histogram-bgl"),
            entries: &[
                storage_entry(0, true),  // pos_life
                storage_entry(1, true),  // alive_indices
                storage_entry(2, true),  // counters
                uniform_entry(3),        // sort uniforms
                storage_entry(4, false), // histogram
            ],
        });
        let histogram_pipeline = create_compute_pipeline(
            device,
            "splat-sort-histogram",
            include_str!("../../../../../assets/shaders/builtin/splat_sort_histogram.wgsl"),
            &histogram_bgl,
        );
        let histogram_bind_groups: [BindGroup; 2] = std::array::from_fn(|idx| {
            device.create_bind_group(&BindGroupDescriptor {
                label: Some("splat-sort-histogram-bg"),
                layout: &histogram_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: pos_life_buffers[idx].as_entire_binding(),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: alive_index_buffers[idx].as_entire_binding(),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: counter_buffer.as_entire_binding(),
                    },
                    BindGroupEntry {
                        binding: 3,
                        resource: sort_uniform_buffer.as_entire_binding(),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: histogram_buffer.as_entire_binding(),
                    },
                ],
            })
        });

        // --- Scan pass (reuse the spatial-hash Blelloch scan, patched to 65536) ---
        let scan_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("splat-sort-scan-bgl"),
            entries: &[
                storage_entry(0, true),  // cell_counts = histogram
                storage_entry(1, false), // cell_offsets = offsets
            ],
        });
        let scan_source =
            include_str!("../../../../../assets/shaders/builtin/spatial_hash_prefix_sum.wgsl")
                .replace(
                    "const NUM_CELLS: u32 = 1600u;",
                    &format!("const NUM_CELLS: u32 = {NUM_BUCKETS}u;"),
                );
        let scan_pipeline =
            create_compute_pipeline(device, "splat-sort-scan", &scan_source, &scan_bgl);
        let scan_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("splat-sort-scan-bg"),
            layout: &scan_bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: histogram_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: offsets_buffer.as_entire_binding(),
                },
            ],
        });

        // --- Scatter pass ---
        let scatter_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("splat-sort-scatter-bgl"),
            entries: &[
                storage_entry(0, true),  // pos_life
                storage_entry(1, true),  // alive_indices
                storage_entry(2, true),  // counters
                uniform_entry(3),        // sort uniforms
                storage_entry(4, false), // scatter_offsets
                storage_entry(5, false), // sorted_indices
            ],
        });
        let scatter_pipeline = create_compute_pipeline(
            device,
            "splat-sort-scatter",
            include_str!("../../../../../assets/shaders/builtin/splat_sort_scatter.wgsl"),
            &scatter_bgl,
        );
        let scatter_bind_groups: [BindGroup; 2] = std::array::from_fn(|idx| {
            device.create_bind_group(&BindGroupDescriptor {
                label: Some("splat-sort-scatter-bg"),
                layout: &scatter_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: pos_life_buffers[idx].as_entire_binding(),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: alive_index_buffers[idx].as_entire_binding(),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: counter_buffer.as_entire_binding(),
                    },
                    BindGroupEntry {
                        binding: 3,
                        resource: sort_uniform_buffer.as_entire_binding(),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: scatter_offsets_buffer.as_entire_binding(),
                    },
                    BindGroupEntry {
                        binding: 5,
                        resource: sorted_indices_buffer.as_entire_binding(),
                    },
                ],
            })
        });

        Self {
            max_particles,
            histogram_buffer,
            offsets_buffer,
            scatter_offsets_buffer,
            sorted_indices_buffer,
            sort_uniform_buffer,
            histogram_pipeline,
            histogram_bind_groups,
            scan_pipeline,
            scan_bind_group,
            scatter_pipeline,
            scatter_bind_groups,
        }
    }

    /// The back-to-front ordered alive-index buffer, consumed by the sorted
    /// billboard render as its instance indirection (binding 5).
    pub fn sorted_indices(&self) -> &wgpu::Buffer {
        &self.sorted_indices_buffer
    }

    /// Encode the four sort passes. `output_idx` selects the ping-pong side the
    /// sim just wrote (`1 - current`); `near`/`far` bracket the scene's view
    /// depth (far→bucket 0). Runs after the sim + prepare_indirect in dispatch().
    pub fn dispatch(
        &self,
        encoder: &mut CommandEncoder,
        queue: &Queue,
        output_idx: usize,
        near: f32,
        far: f32,
    ) {
        queue.write_buffer(
            &self.sort_uniform_buffer,
            0,
            bytemuck::bytes_of(&SplatSortUniforms {
                depth_near: near,
                depth_far: far,
                _pad0: 0.0,
                _pad1: 0.0,
            }),
        );

        // Histogram is atomic-accumulated, so zero it first. offsets/scatter
        // are fully rewritten (scan then copy), sorted_indices is rewritten over
        // [0, alive_count) by scatter — none need clearing.
        encoder.clear_buffer(&self.histogram_buffer, 0, None);

        let particle_workgroups = self.max_particles.div_ceil(WORKGROUP_SIZE);

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("splat-sort-histogram"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.histogram_pipeline);
            pass.set_bind_group(0, &self.histogram_bind_groups[output_idx], &[]);
            pass.dispatch_workgroups(particle_workgroups, 1, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("splat-sort-scan"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.scan_pipeline);
            pass.set_bind_group(0, &self.scan_bind_group, &[]);
            pass.dispatch_workgroups(1, 1, 1); // single-workgroup scan
        }
        // Scatter bumps its offsets destructively → give it a fresh copy.
        encoder.copy_buffer_to_buffer(
            &self.offsets_buffer,
            0,
            &self.scatter_offsets_buffer,
            0,
            (NUM_BUCKETS as u64) * 4,
        );
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("splat-sort-scatter"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.scatter_pipeline);
            pass.set_bind_group(0, &self.scatter_bind_groups[output_idx], &[]);
            pass.dispatch_workgroups(particle_workgroups, 1, 1);
        }
    }
}
