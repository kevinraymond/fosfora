//! Volumetric Mode (R3) — particle density ray marching.
//!
//! Renders any particle effect as continuous fog/nebula instead of discrete
//! dots. Three GPU steps per frame, run inside `ParticleSystem` (where the
//! private SoA particle buffers are reachable, like `ComputeRasterizer`):
//!
//! 1. **scatter** — each alive particle deposits fixed-point density into an
//!    atomic `u32` voxel grid. Particles are 2D, so a stable Z is synthesized
//!    from a per-particle hash (see `volumetric_scatter.wgsl`).
//! 2. **resolve** — normalise + 3x3x3-blur the voxel grid into a samplable
//!    `r32float` 3D density texture.
//! 3. **ray march** — a fullscreen fragment pass builds an orbiting camera ray
//!    inline and marches the density texture with Beer-Lambert absorption.
//!
//! The density texture + ray marcher are the reusable core: the later `Lattice`
//! effect will write [`VolumetricRenderer::density_storage_view`] directly and
//! reuse the same marcher, so particle scatter is just one density producer.
//!
//! `r32float` is required: `r16float` has no storage caps and `r32float` is not
//! hardware-filterable without `FLOAT32_FILTERABLE` (unrequested), so the marcher
//! samples via manual trilinear `textureLoad` (no sampler).

use bytemuck::{Pod, Zeroable};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingType, BufferBindingType, ColorTargetState, CommandEncoder,
    ComputePipeline, Device, FragmentState, PipelineCompilationOptions, PipelineLayoutDescriptor,
    PrimitiveState, Queue, RenderPipeline, ShaderStages, StorageTextureAccess, TextureFormat,
    TextureSampleType, TextureView, TextureViewDimension, VertexState,
};

/// Fixed voxel grid resolution (64^3). Kept constant so the buffers/texture are
/// allocated once; higher resolutions are a follow-up.
pub const GRID_RES: u32 = 64;

const SCATTER_WORKGROUP: u32 = 256;
const RESOLVE_WORKGROUP: u32 = 4;

/// GPU-side uniform block. Mirrored byte-for-byte by `VolUniforms` in all three
/// `volumetric_*.wgsl` shaders. All scalars (4-byte aligned); size padded to a
/// multiple of 16 for the uniform address space.
#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
pub struct VolumetricUniforms {
    pub grid_res: u32,
    pub march_steps: u32,
    pub res_x: f32,
    pub res_y: f32,
    pub time: f32,
    pub absorption: f32,
    pub detail_scale: f32,
    pub detail_strength: f32,
    pub density_threshold: f32,
    pub volume_depth: f32,
    pub density_scale: f32,
    pub cam_yaw: f32,
    pub cam_pitch: f32,
    pub cam_distance: f32,
    pub cam_orbit_speed: f32,
    pub fov: f32,
    pub palette_hue: f32,
    pub emission_gain: f32,
    pub beat: f32,
    pub kick: f32,
    pub rms: f32,
    pub beat_phase: f32,
    pub dominant_chroma: f32,
    pub density_gain: f32,
}

/// Tunable volumetric parameters (host-side). Owned globally by the app and
/// pushed onto the active particle layer each frame.
#[derive(Debug, Copy, Clone)]
pub struct VolumetricParams {
    pub march_steps: u32,
    pub absorption: f32,
    pub detail_scale: f32,
    pub detail_strength: f32,
    pub density_threshold: f32,
    pub volume_depth: f32,
    pub density_scale: f32,
    /// Saturation gain applied in `resolve` (`density = 1 - exp(-gain * occupancy)`),
    /// mapping mean particles-per-voxel to a bounded, count-robust `[0,1)` density.
    pub density_gain: f32,
    pub cam_pitch: f32,
    pub cam_distance: f32,
    pub cam_orbit_speed: f32,
    pub fov: f32,
    pub palette_hue: f32,
    pub emission_gain: f32,
}

impl Default for VolumetricParams {
    fn default() -> Self {
        Self {
            march_steps: 64,
            absorption: 1.2,
            detail_scale: 3.0,
            detail_strength: 0.5,
            density_threshold: 0.001,
            volume_depth: 0.8,
            density_scale: 256.0,
            density_gain: 0.15,
            cam_pitch: 0.35,
            cam_distance: 3.2,
            cam_orbit_speed: 0.15,
            fov: 1.5,
            palette_hue: 0.6,
            emission_gain: 1.5,
        }
    }
}

impl VolumetricParams {
    /// Set a named parameter (from OSC `/phosphor/volumetric/{name}` or UI).
    pub fn set_param(&mut self, name: &str, value: f32) {
        match name {
            "march_steps" => self.march_steps = (value.max(1.0)) as u32,
            "absorption" => self.absorption = value,
            "detail_scale" => self.detail_scale = value,
            "detail_strength" => self.detail_strength = value,
            "density_threshold" => self.density_threshold = value,
            "volume_depth" => self.volume_depth = value,
            "density_scale" => self.density_scale = value,
            "density_gain" => self.density_gain = value,
            "cam_pitch" => self.cam_pitch = value,
            "cam_distance" => self.cam_distance = value,
            "cam_orbit_speed" => self.cam_orbit_speed = value,
            "fov" => self.fov = value,
            "palette_hue" => self.palette_hue = value,
            "emission_gain" => self.emission_gain = value,
            _ => log::warn!("unknown volumetric param: {name}"),
        }
    }

    /// Pack params + per-frame audio into the GPU uniform block. `grid_res` is
    /// filled by the renderer at dispatch time.
    #[allow(clippy::too_many_arguments)]
    pub fn build_uniforms(
        &self,
        resolution: [f32; 2],
        time: f32,
        beat: f32,
        kick: f32,
        rms: f32,
        beat_phase: f32,
        dominant_chroma: f32,
    ) -> VolumetricUniforms {
        VolumetricUniforms {
            grid_res: GRID_RES,
            march_steps: self.march_steps.max(1),
            res_x: resolution[0],
            res_y: resolution[1],
            time,
            absorption: self.absorption,
            detail_scale: self.detail_scale,
            detail_strength: self.detail_strength,
            density_threshold: self.density_threshold,
            volume_depth: self.volume_depth,
            density_scale: self.density_scale.max(1.0),
            cam_yaw: 0.0,
            cam_pitch: self.cam_pitch,
            cam_distance: self.cam_distance,
            cam_orbit_speed: self.cam_orbit_speed,
            fov: self.fov,
            palette_hue: self.palette_hue,
            emission_gain: self.emission_gain,
            beat,
            kick,
            rms,
            beat_phase,
            dominant_chroma,
            density_gain: self.density_gain.max(0.0),
        }
    }
}

/// Owns the density volume, the scatter/resolve/raymarch pipelines, and the
/// per-frame uniform buffer for one particle system.
pub struct VolumetricRenderer {
    grid_res: u32,

    // Density volume: one r32float 3D texture, written by resolve (storage view),
    // sampled by the ray marcher (same view — the texture carries both usages).
    density_view: TextureView,
    // Kept alive for the bind groups; `density_texture` + the storage view are the
    // reusable interface a future Lattice effect will write into directly.
    #[allow(dead_code)]
    density_texture: wgpu::Texture,

    // Atomic u32 voxel grid (additive scatter target).
    voxel_buffer: wgpu::Buffer,
    uniform_buffer: wgpu::Buffer,

    scatter_pipeline: ComputePipeline,
    scatter_bind_groups: [BindGroup; 2],

    resolve_pipeline: ComputePipeline,
    resolve_bind_group: BindGroup,

    raymarch_pipeline: RenderPipeline,
    raymarch_bind_group: BindGroup,
}

impl VolumetricRenderer {
    pub fn new(
        device: &Device,
        hdr_format: TextureFormat,
        pos_life_buffers: &[wgpu::Buffer; 2],
        alive_index_buffers: &[wgpu::Buffer; 2],
        counter_buffer: &wgpu::Buffer,
    ) -> Self {
        let grid_res = GRID_RES;
        let voxel_count = (grid_res * grid_res * grid_res) as u64;

        // --- Density 3D texture (r32float, storage + sampled) ---
        let density_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("volumetric-density"),
            size: wgpu::Extent3d {
                width: grid_res,
                height: grid_res,
                depth_or_array_layers: grid_res,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: TextureFormat::R32Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let density_view = density_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // --- Buffers ---
        let voxel_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("volumetric-voxel"),
            size: voxel_count * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("volumetric-uniforms"),
            size: std::mem::size_of::<VolumetricUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // --- Scatter pipeline (compute) ---
        let scatter_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("vol-scatter-bgl"),
            entries: &[
                storage_ro_entry(0), // pos_life
                storage_ro_entry(1), // alive_indices
                storage_ro_entry(2), // counters
                uniform_entry(3, ShaderStages::COMPUTE),
                storage_rw_entry(4), // voxel
            ],
        });
        let scatter_pipeline = create_compute_pipeline(
            device,
            "vol-scatter",
            include_str!("../../../../assets/shaders/builtin/volumetric_scatter.wgsl"),
            "cs_scatter",
            &scatter_bgl,
        );
        let scatter_bind_groups = std::array::from_fn(|idx| {
            device.create_bind_group(&BindGroupDescriptor {
                label: Some("vol-scatter-bg"),
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
                        resource: uniform_buffer.as_entire_binding(),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: voxel_buffer.as_entire_binding(),
                    },
                ],
            })
        });

        // --- Resolve pipeline (compute) ---
        let resolve_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("vol-resolve-bgl"),
            entries: &[
                storage_ro_entry(0), // voxel
                uniform_entry(1, ShaderStages::COMPUTE),
                storage_texture_3d_entry(2), // density_out
            ],
        });
        let resolve_pipeline = create_compute_pipeline(
            device,
            "vol-resolve",
            include_str!("../../../../assets/shaders/builtin/volumetric_resolve.wgsl"),
            "cs_resolve",
            &resolve_bgl,
        );
        let resolve_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("vol-resolve-bg"),
            layout: &resolve_bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: voxel_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: uniform_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&density_view),
                },
            ],
        });

        // --- Ray march pipeline (render, premultiplied over) ---
        let raymarch_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("vol-raymarch-bgl"),
            entries: &[
                uniform_entry(0, ShaderStages::FRAGMENT),
                sampled_texture_3d_entry(1),
            ],
        });
        let raymarch_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vol-raymarch"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../assets/shaders/builtin/volumetric_raymarch.wgsl").into(),
            ),
        });
        let raymarch_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("vol-raymarch-layout"),
            bind_group_layouts: &[&raymarch_bgl],
            push_constant_ranges: &[],
        });
        let raymarch_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vol-raymarch-pipeline"),
            layout: Some(&raymarch_layout),
            vertex: VertexState {
                module: &raymarch_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: PipelineCompilationOptions::default(),
            },
            fragment: Some(FragmentState {
                module: &raymarch_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(ColorTargetState {
                    format: hdr_format,
                    // Premultiplied "over": the shader outputs (transmittance-
                    // weighted color, 1 - transmittance).
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
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
        let raymarch_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("vol-raymarch-bg"),
            layout: &raymarch_bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&density_view),
                },
            ],
        });

        Self {
            grid_res,
            density_view,
            density_texture,
            voxel_buffer,
            uniform_buffer,
            scatter_pipeline,
            scatter_bind_groups,
            resolve_pipeline,
            resolve_bind_group,
            raymarch_pipeline,
            raymarch_bind_group,
        }
    }

    /// Write-only 3D density view — the reusable interface for other producers
    /// (e.g. Lattice writes density here directly and reuses the ray marcher).
    #[allow(dead_code)] // consumed by the future Lattice effect (R3 → Lattice arc)
    pub fn density_storage_view(&self) -> &TextureView {
        &self.density_view
    }

    /// Clear → scatter → resolve. `output_idx` is the ping-pong index of the
    /// particle output buffers (1 - current). `max_particles` sets the dispatch
    /// size (the shader exits early past `alive_count`).
    pub fn dispatch(
        &self,
        encoder: &mut CommandEncoder,
        queue: &Queue,
        output_idx: usize,
        max_particles: u32,
        uniforms: &VolumetricUniforms,
    ) {
        let mut u = *uniforms;
        u.grid_res = self.grid_res;
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&u));

        // Clear the voxel grid (DMA fill; own transfer→compute barrier).
        encoder.clear_buffer(&self.voxel_buffer, 0, None);

        // Scatter: one thread per particle.
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("vol-scatter"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.scatter_pipeline);
            pass.set_bind_group(0, &self.scatter_bind_groups[output_idx], &[]);
            pass.dispatch_workgroups(max_particles.div_ceil(SCATTER_WORKGROUP), 1, 1);
        }

        // Resolve: voxel grid → density texture (distinct pass = write visible to
        // the raymarch read via the pass-boundary barrier).
        {
            let groups = self.grid_res.div_ceil(RESOLVE_WORKGROUP);
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("vol-resolve"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.resolve_pipeline);
            pass.set_bind_group(0, &self.resolve_bind_group, &[]);
            pass.dispatch_workgroups(groups, groups, groups);
        }
    }

    /// Ray march the density texture, compositing over `target` (LoadOp::Load).
    /// The uniform buffer is written in [`dispatch`], which runs earlier the
    /// same frame.
    pub fn render_raymarch(&self, encoder: &mut CommandEncoder, target: &TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("vol-raymarch"),
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
        pass.set_pipeline(&self.raymarch_pipeline);
        pass.set_bind_group(0, &self.raymarch_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

// --- BGL entry helpers ---

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

fn uniform_entry(binding: u32, visibility: ShaderStages) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility,
        ty: BindingType::Buffer {
            ty: BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn storage_texture_3d_entry(binding: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::COMPUTE,
        ty: BindingType::StorageTexture {
            access: StorageTextureAccess::WriteOnly,
            format: TextureFormat::R32Float,
            view_dimension: TextureViewDimension::D3,
        },
        count: None,
    }
}

fn sampled_texture_3d_entry(binding: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Texture {
            sample_type: TextureSampleType::Float { filterable: false },
            view_dimension: TextureViewDimension::D3,
            multisampled: false,
        },
        count: None,
    }
}

fn create_compute_pipeline(
    device: &Device,
    label: &str,
    source: &str,
    entry_point: &str,
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
        entry_point: Some(entry_point),
        compilation_options: PipelineCompilationOptions::default(),
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volumetric_uniforms_size() {
        // 24 x 4-byte scalars, 16-byte-aligned for the uniform address space.
        assert_eq!(std::mem::size_of::<VolumetricUniforms>(), 96);
    }
}
