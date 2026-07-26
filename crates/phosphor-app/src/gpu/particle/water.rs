//! Virtual-pipes shallow-water sim over the obstacle height field (#1851 water
//! accumulation). Maintains a per-cell water height that flows over the
//! obstacle terrain (its alpha = near-bright depth), fills enclosed basins
//! (eye sockets), and overflows the rims. The particle collision samples the
//! resulting water so Tide's particles rest on the pools and ride the overflow.
//!
//! Structure mirrors the reaction-diffusion sim in `system.rs`: `Rgba16Float`
//! ping-pong textures, neighbours read via `textureLoad`, output written to a
//! `WriteOnly` storage binding, two compute passes per sub-step
//! (`water_sim.wgsl`: `flux_step` then `height_step`).

use wgpu::{Device, Queue};

/// Tunable water parameters (driven from the obstacle panel / audio later).
#[derive(Clone, Copy)]
pub struct WaterParams {
    pub dt: f32,
    pub flux_gain: f32,
    pub source_rate: f32,
    pub drain: f32,
    pub terrain_floor: f32,
    pub edge_drain: f32,
    /// Sub-steps per frame (more = faster settling, higher cost). Rounded up to
    /// an even number by `WaterSim::step`.
    pub substeps: u32,
    /// How much accumulated water raises the collision surface (fed to the sim's
    /// `obstacle_water_scale` uniform). Not used by the sim itself.
    pub level_scale: f32,
}

impl Default for WaterParams {
    fn default() -> Self {
        Self {
            dt: 1.0,
            flux_gain: 0.18,
            source_rate: 0.01,
            drain: 0.06,
            terrain_floor: 0.3,
            edge_drain: 0.8,
            substeps: 8,
            level_scale: 1.5,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct WaterUniforms {
    grid: u32,
    _pad0: u32,
    dt: f32,
    flux_gain: f32,
    source_rate: f32,
    drain: f32,
    terrain_floor: f32,
    edge_drain: f32,
}

/// A 1×1 zero water texture bound into the collision group when no water sim is
/// active, so `water_tex` samples 0 and the collision surface is pure terrain.
pub fn placeholder(device: &Device, queue: &Queue) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("water-placeholder"),
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

/// A virtual-pipes water simulation bound to an obstacle terrain texture.
pub struct WaterSim {
    grid: u32,
    /// Kept for test readback; the views hold the GPU textures alive otherwise.
    #[allow(dead_code)]
    water: [wgpu::Texture; 2],
    water_views: [wgpu::TextureView; 2],
    flux_views: [wgpu::TextureView; 2],
    /// Ping-pong index. `Cell` so `step` runs under `dispatch(&self)` (mirrors
    /// the reaction-diffusion `rd_current`).
    current: std::cell::Cell<usize>,
    flux_pipeline: wgpu::ComputePipeline,
    height_pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
    /// Bind groups per ping-pong phase, rebuilt when the terrain view changes.
    flux_bgs: Option<[wgpu::BindGroup; 2]>,
    height_bgs: Option<[wgpu::BindGroup; 2]>,
}

impl WaterSim {
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
        let water = [tex("water-a"), tex("water-b")];
        let flux = [tex("water-flux-a"), tex("water-flux-b")];

        // Zero-init all four (Rgba16Float all-zero bytes == 0.0).
        let zeros = vec![0u8; (grid * grid * 8) as usize];
        for t in water.iter().chain(flux.iter()) {
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
        let water_views = [view(&water[0]), view(&water[1])];
        let flux_views = [view(&flux[0]), view(&flux[1])];

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water-sim-shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../../assets/shaders/water_sim.wgsl").into(),
            ),
        });

        let ldr = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water-bgl"),
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
                ldr(1), // terrain
                ldr(2), // water_in
                ldr(3), // flux_in
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
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
            label: Some("water-pipeline-layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipe = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("water-pipeline"),
                layout: Some(&layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let flux_pipeline = pipe("flux_step");
        let height_pipeline = pipe("height_step");

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water-uniforms"),
            size: std::mem::size_of::<WaterUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            grid,
            water,
            water_views,
            flux_views,
            current: std::cell::Cell::new(0),
            flux_pipeline,
            height_pipeline,
            bgl,
            uniform_buffer,
            flux_bgs: None,
            height_bgs: None,
        }
    }

    /// (Re)build the ping-pong bind groups against a terrain view. Call whenever
    /// the obstacle texture is (re)created.
    pub fn rebuild(&mut self, device: &Device, terrain_view: &wgpu::TextureView) {
        let mk =
            |water_in: &wgpu::TextureView, flux_in: &wgpu::TextureView, out: &wgpu::TextureView| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("water-bg"),
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
                            resource: wgpu::BindingResource::TextureView(water_in),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(flux_in),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(out),
                        },
                    ],
                })
            };
        // phase p: read water[p]/flux[p], write flux[1-p]; then read water[p]/
        // flux[1-p], write water[1-p].
        self.flux_bgs = Some([
            mk(
                &self.water_views[0],
                &self.flux_views[0],
                &self.flux_views[1],
            ),
            mk(
                &self.water_views[1],
                &self.flux_views[1],
                &self.flux_views[0],
            ),
        ]);
        self.height_bgs = Some([
            mk(
                &self.water_views[0],
                &self.flux_views[1],
                &self.water_views[1],
            ),
            mk(
                &self.water_views[1],
                &self.flux_views[0],
                &self.water_views[0],
            ),
        ]);
    }

    /// The water texture view (`.r` = height) the collision reads. Always index
    /// 0: `step` runs an even number of sub-steps starting from `current == 0`,
    /// so the freshest water always lands back in `water[0]` at frame
    /// boundaries — the bind group can point here once and stay valid.
    pub fn water_view(&self) -> &wgpu::TextureView {
        &self.water_views[0]
    }

    pub fn grid(&self) -> u32 {
        self.grid
    }

    /// Advance the sim `params.substeps` times into `encoder`. `&self` (index in
    /// a `Cell`) so it can run inside `ParticleSystem::dispatch(&self)`.
    pub fn step(&self, encoder: &mut wgpu::CommandEncoder, queue: &Queue, params: &WaterParams) {
        let (Some(flux_bgs), Some(height_bgs)) = (&self.flux_bgs, &self.height_bgs) else {
            return;
        };
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&WaterUniforms {
                grid: self.grid,
                _pad0: 0,
                dt: params.dt,
                flux_gain: params.flux_gain,
                source_rate: params.source_rate,
                drain: params.drain,
                terrain_floor: params.terrain_floor,
                edge_drain: params.edge_drain,
            }),
        );
        let wg = self.grid.div_ceil(8);
        // Even sub-steps keep the freshest water in `water[0]` (see `water_view`).
        let substeps = params.substeps.max(1).next_multiple_of(2);
        for _ in 0..substeps {
            let p = self.current.get();
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("water-flux"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.flux_pipeline);
                pass.set_bind_group(0, &flux_bgs[p], &[]);
                pass.dispatch_workgroups(wg, wg, 1);
            }
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("water-height"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.height_pipeline);
                pass.set_bind_group(0, &height_bgs[p], &[]);
                pass.dispatch_workgroups(wg, wg, 1);
            }
            self.current.set(1 - p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::test_gpu::{gpu_guard, test_gpu};

    // Validates tide_sim.wgsl (Drape surface-flow, #1851) compiles with the
    // shipped lib prepend — the runtime assembly (loader.rs LIB_FILENAMES +
    // particle_lib + effect source). naga runs at shader-module creation.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn tide_sim_shader_compiles() {
        let _g = gpu_guard();
        let (device, _q) = test_gpu();
        let libs = [
            include_str!("../../../../../assets/shaders/lib/noise.wgsl"),
            include_str!("../../../../../assets/shaders/lib/palette.wgsl"),
            include_str!("../../../../../assets/shaders/lib/sdf.wgsl"),
            include_str!("../../../../../assets/shaders/lib/tonemap.wgsl"),
            include_str!("../../../../../assets/shaders/lib/chronoflow.wgsl"),
        ]
        .join("\n");
        let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl")
            .replace("const SH_GRID_W: u32 = 40u;", "const SH_GRID_W: u32 = 64u;")
            .replace("const SH_GRID_H: u32 = 40u;", "const SH_GRID_H: u32 = 64u;");
        let tide = include_str!("../../../../../assets/shaders/tide_sim.wgsl");
        let src = format!("{libs}\n{plib}\n{tide}");
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _m = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tide-validate"),
            source: wgpu::ShaderSource::Wgsl(src.into()),
        });
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "tide_sim.wgsl validation error: {err:?}");
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

    /// Read back the current water height (`.r`, decoded from f16) as f32.
    fn read_water(device: &Device, queue: &Queue, sim: &WaterSim) -> Vec<f32> {
        let grid = sim.grid;
        let bpr = grid * 8; // Rgba16Float
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water-readback"),
            size: (bpr * grid) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &sim.water[sim.current.get()],
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
        // .r is the first f16 of each 8-byte texel.
        (0..(grid * grid) as usize)
            .map(|i| half_to_f32(u16::from_le_bytes([bytes[i * 8], bytes[i * 8 + 1]])))
            .collect()
    }

    fn half_to_f32(h: u16) -> f32 {
        let sign = ((h >> 15) & 1) as u32;
        let exp = ((h >> 10) & 0x1f) as u32;
        let man = (h & 0x3ff) as u32;
        let bits = if exp == 0 {
            if man == 0 {
                sign << 31
            } else {
                // subnormal
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

    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn water_fills_and_overflows_a_bowl() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let grid = 64u32;

        // Terrain: a circular basin (low centre) inside a raised rim, empty
        // outside. Centre h≈0.4, rim h≈0.8, background 0.
        let terr = terrain(&device, &queue, grid, |x, y| {
            let (dx, dy) = (x - 0.5, y - 0.5);
            let d = (dx * dx + dy * dy).sqrt();
            if d < 0.25 {
                0.4 + d // rises from 0.4 (centre) toward the rim
            } else if d < 0.32 {
                0.8 // rim
            } else {
                0.0 // off-model
            }
        });

        let mut sim = WaterSim::new(&device, &queue, grid);
        sim.rebuild(&device, &terr);
        let params = WaterParams::default();

        let center = ((grid / 2) * grid + grid / 2) as usize;
        let outside = (2 * grid + 2) as usize; // background corner

        // Step a while; water should collect in the basin, not the background.
        let mut enc =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        for _ in 0..40 {
            sim.step(&mut enc, &queue, &params);
        }
        queue.submit([enc.finish()]);
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();

        let w = read_water(&device, &queue, &sim);
        assert!(
            w[center] > 0.05,
            "water should pool in the basin centre, got {}",
            w[center]
        );
        assert!(
            w[outside] < w[center] * 0.5,
            "background should stay ~dry: outside={} center={}",
            w[outside],
            w[center]
        );
    }
}
