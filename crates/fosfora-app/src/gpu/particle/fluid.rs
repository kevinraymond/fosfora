//! Eulerian velocity-grid fluid over the obstacle terrain (#1939). Maintains a
//! divergence-free 2D velocity field in clip space with the obstacle as a solid
//! (no-slip) boundary, so a throughflow prescribed at the top edge is routed
//! around the silhouette — a real bow wave in front, wake behind, and shed
//! eddies. Particles read the field via `fluid_velocity(pos)` (particle_lib) and
//! are advected by it.
//!
//! Structure mirrors `water.rs`: `Rgba16Float` ping-pong textures, neighbours
//! read via `textureLoad`, output written to a `WriteOnly` storage binding, a
//! fixed bind-group layout reused across passes by swapping which textures fill
//! the `tex_a` / `tex_b` roles (`fluid_sim.wgsl`: advect_forces → divergence →
//! pressure_jacobi ×N → project).

use wgpu::{Device, Queue};

/// Tunable fluid parameters (driven from the obstacle panel / audio).
#[derive(Clone, Copy)]
pub struct FluidParams {
    pub dt: f32,
    /// Inflow magnitude at the top edge (clip units/sec) — the flow's driver.
    pub flow_speed: f32,
    /// Inflow lateral tilt (added to the downward inflow).
    pub flow_dx: f32,
    /// Small downward body force (clip units/sec²).
    pub gravity: f32,
    /// Velocity damping per second (0 = inviscid).
    pub viscosity: f32,
    /// Vorticity-confinement strength (0 = off).
    pub vorticity: f32,
    /// Jacobi pressure iterations per frame (the main cost dial).
    pub jacobi_iters: u32,
    /// Solid where effective obstacle height >= threshold.
    pub threshold: f32,
    /// Effective height = terrain.a + water_scale * water.r (couples pooled
    /// water into the solid mask when the water sim is on).
    pub water_scale: f32,
}

impl Default for FluidParams {
    fn default() -> Self {
        Self {
            dt: 1.0 / 60.0,
            flow_speed: 0.9,
            flow_dx: 0.0,
            gravity: 0.4,
            viscosity: 0.02,
            vorticity: 0.18,
            jacobi_iters: 40,
            threshold: 0.5,
            water_scale: 0.0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FluidUniforms {
    grid: u32,
    _pad0: u32,
    dt: f32,
    flow_speed: f32,
    flow_dx: f32,
    gravity: f32,
    viscosity: f32,
    vorticity: f32,
    jacobi_iters: u32,
    _pad1: u32,
    threshold: f32,
    water_scale: f32,
    fit: u32,
    res_x: f32,
    res_y: f32,
    obst_w: f32,
    obst_h: f32,
    _pad2: f32,
    _pad3: f32,
    _pad4: f32,
}

/// A 1×1 zero velocity texture bound into the collision group when no fluid sim
/// is active, so `fluid_vel_tex` samples 0 and contributes no advection.
pub fn placeholder(device: &Device, queue: &Queue) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fluid-placeholder"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[0u8; 8],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(8),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

/// An Eulerian fluid simulation bound to an obstacle terrain texture.
pub struct FluidSim {
    grid: u32,
    /// Kept for test readback; the views hold the GPU textures alive otherwise.
    #[allow(dead_code)]
    velocity: [wgpu::Texture; 2],
    velocity_views: [wgpu::TextureView; 2],
    #[allow(dead_code)]
    pressure: [wgpu::Texture; 2],
    pressure_views: [wgpu::TextureView; 2],
    #[allow(dead_code)]
    divergence: wgpu::Texture,
    divergence_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    advect_pipeline: wgpu::ComputePipeline,
    divergence_pipeline: wgpu::ComputePipeline,
    jacobi_pipeline: wgpu::ComputePipeline,
    project_pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
    /// Per-pass bind groups, rebuilt when the terrain / water views change.
    bgs: Option<FluidBindGroups>,
}

struct FluidBindGroups {
    /// advect_forces: tex_a = velocity[0] (sampled + loaded), out = velocity[1].
    advect: wgpu::BindGroup,
    /// divergence: tex_a = velocity[1], out = divergence.
    divergence: wgpu::BindGroup,
    /// pressure_jacobi phase p: tex_a = pressure[p], tex_b = divergence,
    /// out = pressure[1-p].
    jacobi: [wgpu::BindGroup; 2],
    /// project: tex_a = velocity[1], tex_b = pressure[0] (final), out = velocity[0].
    project: wgpu::BindGroup,
}

impl FluidSim {
    pub fn new(device: &Device, queue: &Queue, grid: u32) -> Self {
        let tex = |label: &str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: grid,
                    height: grid,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let velocity = [tex("fluid-vel-a"), tex("fluid-vel-b")];
        let pressure = [tex("fluid-pressure-a"), tex("fluid-pressure-b")];
        let divergence = tex("fluid-divergence");

        // Zero-init every texture (Rgba16Float all-zero bytes == 0.0).
        let zeros = vec![0u8; (grid * grid * 8) as usize];
        for t in velocity
            .iter()
            .chain(pressure.iter())
            .chain(std::iter::once(&divergence))
        {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: t,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &zeros,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(grid * 8),
                    rows_per_image: Some(grid),
                },
                wgpu::Extent3d {
                    width: grid,
                    height: grid,
                    depth_or_array_layers: 1,
                },
            );
        }

        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let velocity_views = [view(&velocity[0]), view(&velocity[1])];
        let pressure_views = [view(&pressure[0]), view(&pressure[1])];
        let divergence_view = view(&divergence);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("fluid-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fluid-sim-shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../../assets/shaders/fluid_sim.wgsl").into(),
            ),
        });

        let ldr = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fluid-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                ldr(1), // obstacle
                ldr(2), // water
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                ldr(4), // tex_a
                ldr(5), // tex_b
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fluid-pipeline-layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipe = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("fluid-pipeline"),
                layout: Some(&layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let advect_pipeline = pipe("advect_forces");
        let divergence_pipeline = pipe("divergence");
        let jacobi_pipeline = pipe("pressure_jacobi");
        let project_pipeline = pipe("project");

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fluid-uniforms"),
            size: std::mem::size_of::<FluidUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            grid,
            velocity,
            velocity_views,
            pressure,
            pressure_views,
            divergence,
            divergence_view,
            sampler,
            advect_pipeline,
            divergence_pipeline,
            jacobi_pipeline,
            project_pipeline,
            bgl,
            uniform_buffer,
            bgs: None,
        }
    }

    /// (Re)build the per-pass bind groups against a terrain + water view. Call
    /// whenever the obstacle texture or water view is (re)created.
    pub fn rebuild(
        &mut self,
        device: &Device,
        terrain_view: &wgpu::TextureView,
        water_view: &wgpu::TextureView,
    ) {
        let mk = |tex_a: &wgpu::TextureView, tex_b: &wgpu::TextureView, out: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fluid-bg"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(terrain_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(water_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(tex_a),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(tex_b),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(out),
                    },
                ],
            })
        };
        // velocity: v0 --advect--> v1 --project--> v0 (2 writes → back to v0, so
        // `velocity_view()` can point at v0 permanently). pressure warm-starts
        // from p0; an even jacobi count leaves the result in p0.
        let (v0, v1) = (&self.velocity_views[0], &self.velocity_views[1]);
        let (p0, p1) = (&self.pressure_views[0], &self.pressure_views[1]);
        let div = &self.divergence_view;
        self.bgs = Some(FluidBindGroups {
            advect: mk(v0, v0, v1),
            divergence: mk(v1, v1, div),
            jacobi: [mk(p0, div, p1), mk(p1, div, p0)],
            project: mk(v1, p0, v0),
        });
    }

    /// The velocity view (`.rg` = clip-space velocity) the particle sim reads.
    /// Always index 0 — one `step` leaves the freshest velocity back in
    /// `velocity[0]` (advect writes v1, project writes v0), so the particle
    /// bind group can point here once and stay valid.
    pub fn velocity_view(&self) -> &wgpu::TextureView {
        &self.velocity_views[0]
    }

    pub fn grid(&self) -> u32 {
        self.grid
    }

    /// Advance the sim one frame into `encoder`: advect+forces → divergence →
    /// N Jacobi pressure iterations → project. `&self` so it can run inside
    /// `ParticleSystem::dispatch(&self)`.
    pub fn step(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        queue: &Queue,
        params: &FluidParams,
        fit: u32,
        res: [f32; 2],
        obst_dims: (u32, u32),
    ) {
        let Some(bgs) = &self.bgs else {
            return;
        };
        // Even Jacobi count so the converged pressure lands back in pressure[0],
        // which the project bind group reads.
        let iters = params.jacobi_iters.max(1).next_multiple_of(2);
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&FluidUniforms {
                grid: self.grid,
                _pad0: 0,
                dt: params.dt,
                flow_speed: params.flow_speed,
                flow_dx: params.flow_dx,
                gravity: params.gravity,
                viscosity: params.viscosity,
                vorticity: params.vorticity,
                jacobi_iters: iters,
                _pad1: 0,
                threshold: params.threshold,
                water_scale: params.water_scale,
                fit,
                res_x: res[0],
                res_y: res[1],
                obst_w: obst_dims.0 as f32,
                obst_h: obst_dims.1 as f32,
                _pad2: 0.0,
                _pad3: 0.0,
                _pad4: 0.0,
            }),
        );
        let wg = self.grid.div_ceil(8);
        let pass = |encoder: &mut wgpu::CommandEncoder,
                    pipeline: &wgpu::ComputePipeline,
                    bg: &wgpu::BindGroup,
                    label: &str| {
            let mut p = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: None,
            });
            p.set_pipeline(pipeline);
            p.set_bind_group(0, bg, &[]);
            p.dispatch_workgroups(wg, wg, 1);
        };

        pass(encoder, &self.advect_pipeline, &bgs.advect, "fluid-advect");
        pass(
            encoder,
            &self.divergence_pipeline,
            &bgs.divergence,
            "fluid-divergence",
        );
        // Jacobi ping-pong: phase p reads pressure[p], writes pressure[1-p].
        // With an even count the final result is in pressure[0].
        for i in 0..iters {
            let phase = (i % 2) as usize;
            pass(
                encoder,
                &self.jacobi_pipeline,
                &bgs.jacobi[phase],
                "fluid-jacobi",
            );
        }
        pass(
            encoder,
            &self.project_pipeline,
            &bgs.project,
            "fluid-project",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::test_gpu::{gpu_guard, test_gpu};

    // Validates fluid_sim.wgsl compiles standalone (its own uniforms/bindings,
    // no particle_lib prepend). naga runs at shader-module creation.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn fluid_sim_shader_compiles() {
        let _g = gpu_guard();
        let (device, _q) = test_gpu();
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _m = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fluid-validate"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../../assets/shaders/fluid_sim.wgsl").into(),
            ),
        });
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "fluid_sim.wgsl validation error: {err:?}");
    }

    /// Build a terrain texture (Rgba8Unorm, alpha = height) from a height fn.
    fn terrain(
        device: &Device,
        queue: &Queue,
        grid: u32,
        h: impl Fn(f32, f32) -> f32,
    ) -> wgpu::TextureView {
        let mut data = vec![0u8; (grid * grid * 4) as usize];
        for y in 0..grid {
            for x in 0..grid {
                let hv = (h(x as f32 / grid as f32, y as f32 / grid as f32).clamp(0.0, 1.0) * 255.0)
                    as u8;
                let i = ((y * grid + x) * 4) as usize;
                data[i..i + 4].copy_from_slice(&[hv, hv, hv, hv]);
            }
        }
        let t = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test-terrain"),
            size: wgpu::Extent3d {
                width: grid,
                height: grid,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &t,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(grid * 4),
                rows_per_image: Some(grid),
            },
            wgpu::Extent3d {
                width: grid,
                height: grid,
                depth_or_array_layers: 1,
            },
        );
        t.create_view(&wgpu::TextureViewDescriptor::default())
    }

    fn half_to_f32(h: u16) -> f32 {
        let sign = ((h >> 15) & 1) as u32;
        let exp = ((h >> 10) & 0x1f) as u32;
        let man = (h & 0x3ff) as u32;
        let bits = if exp == 0 {
            if man == 0 {
                sign << 31
            } else {
                let mut e = -14i32;
                let mut m = man;
                while m & 0x400 == 0 {
                    m <<= 1;
                    e -= 1;
                }
                m &= 0x3ff;
                (sign << 31) | (((e + 127) as u32) << 23) | (m << 13)
            }
        } else if exp == 0x1f {
            (sign << 31) | (0xff << 23) | (man << 13)
        } else {
            (sign << 31) | ((exp + 112) << 23) | (man << 13)
        };
        f32::from_bits(bits)
    }

    /// Read back a texture's `.rg` (first two f16 of each texel) as (f32, f32).
    fn read_rg(device: &Device, queue: &Queue, t: &wgpu::Texture, grid: u32) -> Vec<(f32, f32)> {
        let bpr = grid * 8;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fluid-readback"),
            size: (bpr * grid) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: t,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr),
                    rows_per_image: Some(grid),
                },
            },
            wgpu::Extent3d {
                width: grid,
                height: grid,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([enc.finish()]);
        buf.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        let bytes = buf.slice(..).get_mapped_range().to_vec();
        buf.unmap();
        (0..(grid * grid) as usize)
            .map(|i| {
                let r = half_to_f32(u16::from_le_bytes([bytes[i * 8], bytes[i * 8 + 1]]));
                let g = half_to_f32(u16::from_le_bytes([bytes[i * 8 + 2], bytes[i * 8 + 3]]));
                (r, g)
            })
            .collect()
    }

    // A disc obstacle in a top-edge downward inflow: the flow must divert AROUND
    // it — near-zero velocity on the solid, tangential speed-up at the sides,
    // and a genuine throughflow reaching past it (not a hydrostatic dead field).
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn flow_diverts_around_a_disc() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let grid = 128u32;

        // Disc of radius 0.18 (clip) centred on screen; alpha = height.
        let terr = terrain(&device, &queue, grid, |x, y| {
            let (dx, dy) = (x - 0.5, y - 0.5);
            if dx * dx + dy * dy < 0.18 * 0.18 {
                1.0
            } else {
                0.0
            }
        });
        let (_wtex, wview) = placeholder(&device, &queue);

        let mut sim = FluidSim::new(&device, &queue, grid);
        sim.rebuild(&device, &terr, &wview);
        let params = FluidParams {
            water_scale: 0.0,
            ..Default::default()
        };

        // Run enough frames for the flow to establish and route around the disc.
        let mut enc =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        for _ in 0..120 {
            sim.step(
                &mut enc,
                &queue,
                &params,
                0,
                [grid as f32, grid as f32],
                (grid, grid),
            );
        }
        queue.submit([enc.finish()]);
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();

        let v = read_rg(&device, &queue, &sim.velocity[0], grid);
        let at = |cx: u32, cy: u32| v[(cy * grid + cx) as usize];
        let g = grid as f32;
        let cell = |clipx: f32, clipy: f32| {
            let cx = (((clipx + 1.0) * 0.5) * g).clamp(0.0, g - 1.0) as u32;
            let cy = (((1.0 - clipy) * 0.5) * g).clamp(0.0, g - 1.0) as u32;
            at(cx, cy)
        };

        // The disc has radius 0.18 in UV = 0.36 in CLIP (UV spans [0,1] over
        // clip [-1,1]). Sample points below are chosen just OUTSIDE r_clip=0.36.
        let free = cell(0.9, 0.0); // far free stream, well clear of the disc

        // (1) Inside the solid: velocity is zero (no-slip).
        let inside = cell(0.0, 0.0);
        assert!(
            inside.0.abs() < 1e-3 && inside.1.abs() < 1e-3,
            "solid interior should be still, got {inside:?}"
        );

        // (2) Far free stream: a real downward throughflow (not a dead
        //     hydrostatic field — the whole point of the inflow boundary).
        assert!(
            free.1 < -0.2,
            "free stream should flow strongly down, got vy={}",
            free.1
        );

        // (3) Just off the disc's flank the flow is present and diverting, not
        //     blocked — comparable in magnitude to the free stream.
        let flank = cell(0.44, 0.0);
        assert!(
            flank.1 < -0.1 && flank.1.abs() > free.1.abs() * 0.5,
            "flank flow should stay strong as it squeezes past: flank={flank:?} free={free:?}"
        );

        // (4) Above the disc's right shoulder (off the symmetry axis, where
        //     deflection is real — on the axis it cancels): straight-down flow
        //     meeting the crown is pushed outward, so vx > 0 (rightward).
        let shoulder = cell(0.2, 0.4);
        assert!(
            shoulder.0 > 0.02,
            "flow should deflect outward over the shoulder, got vx={}",
            shoulder.0
        );

        // (5) Wake below the disc: shadowed, so the downward flow is weaker there
        //     than the free stream (recirculation / stagnation behind the body).
        let wake = cell(0.0, -0.44);
        assert!(
            wake.1 > free.1,
            "wake should be slower than the free stream: wake={wake:?} free={free:?}"
        );
    }

    fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
        let h6 = (h.rem_euclid(1.0)) * 6.0;
        let i = h6.floor() as i32;
        let f = h6 - i as f32;
        let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
        let (r, g, b) = match i.rem_euclid(6) {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
    }

    // Flow-field visualisation (not an assertion): solves the fluid around a
    // head-and-shoulders silhouette and writes a PNG where hue = flow direction
    // and brightness = speed, with the solid drawn dark. Lets a human confirm the
    // flow wraps the form (bow wave above, wake below, eddies at the shoulders).
    // Set FLUID_PNG_DIR to choose the output directory.
    #[test]
    #[ignore = "requires a GPU/software adapter; visual probe, not an assertion"]
    fn fluid_field_preview() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let grid = 256u32;

        // Head-and-shoulders silhouette in UV: a head ellipse + a shoulder band.
        let terr = terrain(&device, &queue, grid, |x, y| {
            let head = {
                let (dx, dy) = ((x - 0.5) / 0.16, (y - 0.4) / 0.2);
                dx * dx + dy * dy < 1.0
            };
            let shoulders =
                y > 0.66 && ((x - 0.5).abs() / 0.34).powi(2) + ((y - 0.9) / 0.24).powi(2) < 1.0;
            if head || shoulders { 1.0 } else { 0.0 }
        });
        let (_wtex, wview) = placeholder(&device, &queue);

        let mut sim = FluidSim::new(&device, &queue, grid);
        sim.rebuild(&device, &terr, &wview);
        let params = FluidParams::default();

        let mut enc =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        for _ in 0..200 {
            sim.step(
                &mut enc,
                &queue,
                &params,
                0,
                [grid as f32, grid as f32],
                (grid, grid),
            );
        }
        queue.submit([enc.finish()]);
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();

        let v = read_rg(&device, &queue, &sim.velocity[0], grid);
        let mut img = vec![0u8; (grid * grid * 3) as usize];
        for i in 0..(grid * grid) as usize {
            let (vx, vy) = v[i];
            let speed = (vx * vx + vy * vy).sqrt();
            let px = if speed < 1e-4 {
                // Solid / stagnant: dark slate so the silhouette reads.
                [24u8, 26, 30]
            } else {
                let dir = vy.atan2(vx) / std::f32::consts::TAU + 0.5;
                hsv_to_rgb(dir, 0.85, (speed / 1.6).clamp(0.12, 1.0))
            };
            img[i * 3..i * 3 + 3].copy_from_slice(&px);
        }

        let dir = std::env::var("FLUID_PNG_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        let path = dir.join("fluid_field_preview.png");
        let path = path.display().to_string();
        image::save_buffer(&path, &img, grid, grid, image::ExtendedColorType::Rgb8).unwrap();
        eprintln!("wrote {path}");
    }
}
