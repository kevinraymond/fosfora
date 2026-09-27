//! S5: world-space particles on the billboard path (`particles3d_*.wgsl`).
//!
//! A small curl-noise test sim in a cube ahead of the user, every slot always
//! alive, drawn once per eye as camera-facing quads through that eye's view
//! and projection. It exists to answer the S5 gate (stereo depth, particle
//! ceiling per display rate), not to be a product effect; existing core sims
//! stay screen-space until the layout decision in `docs/xr/MEASURED.md`.

use std::cell::Cell;

use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use log::info;

use crate::gfx::SWAPCHAIN_FORMAT;

/// Bytes per particle: `pos: vec3` + `life` + `vel: vec3` + `seed`.
const PARTICLE_BYTES: u64 = 32;
/// With the sim disabled it still runs this many frames so the particles
/// spread through the cube before freezing (otherwise they all sit at the
/// origin and get clipped, and the draw measures nothing).
const WARMUP_FRAMES: u32 = 144;
const WORKGROUP: u32 = 256;
/// Depth attachment the eye passes share (quad writes, particles test).
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Uniform block for the sim and draw, matching `struct Sim` in the WGSL.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SimUniform {
    cube_center: [f32; 3],
    cube_half: f32,
    time: f32,
    dt: f32,
    count: u32,
    speed: f32,
    size: f32,
    base_size: f32,
    flow_scale: f32,
    lifetime: f32,
    verts_per_sprite: u32,
    pull: u32,
    gravity: f32,
    near_cull: f32,
}

/// Obstacle capacities, matching `MAX_SPHERES` / `MAX_BOXES` in the sim WGSL.
pub const MAX_SPHERES: usize = 64;
pub const MAX_BOXES: usize = 32;

/// An oriented box obstacle in the reference space: a scene plane (thin
/// box), a scene volume or the floor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObstacleBox {
    pub center: [f32; 3],
    /// Rotation box -> world as a quaternion (x, y, z, w).
    pub rot: [f32; 4],
    pub half: [f32; 3],
}

/// Uniform block for the S7 obstacles, matching `struct Obstacles` in the
/// sim WGSL. Fixed-capacity so the uniform buffer never resizes.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct ObstacleSet {
    sphere_count: u32,
    box_count: u32,
    restitution: f32,
    margin: f32,
    spheres: [[f32; 4]; MAX_SPHERES],
    box_center: [[f32; 4]; MAX_BOXES],
    box_rot: [[f32; 4]; MAX_BOXES],
    box_half: [[f32; 4]; MAX_BOXES],
}

impl ObstacleSet {
    /// `restitution` is the fraction of normal velocity kept on a bounce,
    /// `margin` the clearance around every obstacle (meters, roughly the
    /// sprite radius).
    pub fn new(restitution: f32, margin: f32) -> Self {
        Self {
            restitution,
            margin,
            ..Self::zeroed()
        }
    }

    /// Add a sphere (`[x, y, z, radius]`). Beyond the capacity the sphere is
    /// dropped and `false` returned.
    pub fn push_sphere(&mut self, sphere: [f32; 4]) -> bool {
        let i = self.sphere_count as usize;
        if i >= MAX_SPHERES {
            return false;
        }
        self.spheres[i] = sphere;
        self.sphere_count += 1;
        true
    }

    pub fn push_box(&mut self, b: &ObstacleBox) -> bool {
        let i = self.box_count as usize;
        if i >= MAX_BOXES {
            return false;
        }
        self.box_center[i] = [b.center[0], b.center[1], b.center[2], 0.0];
        self.box_rot[i] = b.rot;
        self.box_half[i] = [b.half[0], b.half[1], b.half[2], 0.0];
        self.box_count += 1;
        true
    }

    pub fn sphere_count(&self) -> u32 {
        self.sphere_count
    }

    pub fn box_count(&self) -> u32 {
        self.box_count
    }
}

/// Per-eye camera block, matching `struct Eye` in the WGSL.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct EyeUniform {
    view_proj: [f32; 16],
    view: [f32; 16],
    proj: [f32; 16],
}

/// Static placement and tuning of the test sim.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    /// Draw each sprite as one triangle (3 vertices) instead of a quad (6):
    /// half the vertex work for about 1.3x the covered area.
    pub triangles: bool,
    /// One non-instanced draw of `verts * count` vertices instead of `count`
    /// instances: tilers pay per instance, so this is the mobile idiom.
    pub pull: bool,
    /// Cube center in the reference space (meters).
    pub cube_center: [f32; 3],
    /// Half the cube's edge (meters).
    pub cube_half: f32,
    /// Sprite radius in meters before the audio size multiplier.
    pub base_size: f32,
    /// Spatial frequency of the flow field (1/m).
    pub flow_scale: f32,
    /// Particle lifetime in seconds (the sim randomizes 30..100 % of it).
    pub lifetime: f32,
    /// Downward drift speed in m/s (0 = the pure S5 flow sim).
    pub gravity: f32,
    /// Cull sprites nearer than this to the eye (meters; 0 = off).
    pub near_cull: f32,
    /// Depth-test the sprites against the quad (S5 behavior). Off is a
    /// diagnostic: the same draw with `CompareFunction::Always`.
    pub depth_test: bool,
}

pub struct Particles3d {
    /// False freezes the sim after `WARMUP_FRAMES` (no compute pass) so a
    /// measurement isolates the draw cost.
    pub sim_enabled: bool,
    frames: Cell<u32>,
    /// Cube placement, changeable per frame (`set_cube`): mixed reality
    /// centers it on the wearer once tracking is valid.
    cube: Cell<([f32; 3], f32)>,
    count: u32,
    triangles: bool,
    params: Params,
    sim_uniform: wgpu::Buffer,
    obstacles: wgpu::Buffer,
    _particles: wgpu::Buffer,
    sim_bind_group: wgpu::BindGroup,
    read_bind_group: wgpu::BindGroup,
    eye_uniforms: Vec<wgpu::Buffer>,
    eye_bind_groups: Vec<wgpu::BindGroup>,
    step_pipeline: wgpu::ComputePipeline,
    draw_pipeline: wgpu::RenderPipeline,
}

impl Particles3d {
    pub fn new(device: &wgpu::Device, count: u32, eyes: usize, params: Params) -> Self {
        // Shared declarations plus one body per module. The draw module
        // declares the particle buffer read-only, matching its bind group
        // layout (a writable storage binding in the vertex stage would need
        // VERTEX_WRITABLE_STORAGE).
        const COMMON: &str = include_str!("particles3d_common.wgsl");
        const RW: &str = "var<storage, read_write> particles";
        assert!(
            COMMON.contains(RW),
            "particles3d_common.wgsl: particle buffer declaration changed"
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xr-particles3d-sim"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{COMMON}\n{}", include_str!("particles3d_sim.wgsl")).into(),
            ),
        });
        let draw_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xr-particles3d-draw"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}",
                    COMMON.replace(RW, "var<storage, read> particles"),
                    include_str!("particles3d_draw.wgsl")
                )
                .into(),
            ),
        });
        let sim_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xr-particles3d-sim"),
            size: std::mem::size_of::<SimUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Zeroed: no obstacles until `set_obstacles`.
        let obstacles = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xr-particles3d-obstacles"),
            size: std::mem::size_of::<ObstacleSet>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Zeroed: life 0 makes every slot spawn on the first step.
        let particles = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xr-particles3d-buffer"),
            size: u64::from(count) * PARTICLE_BYTES,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        // The same two buffers under two layouts: the sim writes the
        // particles, the vertex stage only reads them (a writable storage
        // binding in the vertex stage would need VERTEX_WRITABLE_STORAGE).
        // The sim group also carries the obstacle block (binding 2); the
        // draw only reads particles.
        let group_layout = |label: &str, stage: wgpu::ShaderStages, read_only: bool| {
            let mut entries = vec![
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: stage,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<SimUniform>() as u64
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: stage,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(PARTICLE_BYTES),
                    },
                    count: None,
                },
            ];
            if !read_only {
                entries.push(wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: stage,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<ObstacleSet>() as u64
                        ),
                    },
                    count: None,
                });
            }
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: &entries,
            })
        };
        let bind = |label: &str, layout: &wgpu::BindGroupLayout, with_obstacles: bool| {
            let mut entries = vec![
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: sim_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: particles.as_entire_binding(),
                },
            ];
            if with_obstacles {
                entries.push(wgpu::BindGroupEntry {
                    binding: 2,
                    resource: obstacles.as_entire_binding(),
                });
            }
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &entries,
            })
        };
        let sim_layout = group_layout("xr-particles3d-sim", wgpu::ShaderStages::COMPUTE, false);
        let sim_bind_group = bind("xr-particles3d-sim", &sim_layout, true);
        let read_layout = group_layout("xr-particles3d-read", wgpu::ShaderStages::VERTEX, true);
        let read_bind_group = bind("xr-particles3d-read", &read_layout, false);
        let eye_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xr-particles3d-eye"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<EyeUniform>() as u64
                    ),
                },
                count: None,
            }],
        });
        let (eye_uniforms, eye_bind_groups): (Vec<_>, Vec<_>) = (0..eyes)
            .map(|_| {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("xr-particles3d-eye"),
                    size: std::mem::size_of::<EyeUniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("xr-particles3d-eye"),
                    layout: &eye_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    }],
                });
                (buffer, bind_group)
            })
            .unzip();

        let step_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xr-particles3d-step"),
            bind_group_layouts: &[&sim_layout],
            push_constant_ranges: &[],
        });
        let step_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("xr-particles3d-step"),
            layout: Some(&step_layout),
            module: &shader,
            entry_point: Some("cs_step"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let draw_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xr-particles3d-draw"),
            bind_group_layouts: &[&read_layout, &eye_layout],
            push_constant_ranges: &[],
        });
        let draw_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("xr-particles3d-draw"),
            layout: Some(&draw_layout),
            vertex: wgpu::VertexState {
                module: &draw_shader,
                entry_point: Some("vs_particle"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..wgpu::PrimitiveState::default()
            },
            // Test against the quad's depth, never write: additive sprites
            // have no order among themselves.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: if params.depth_test {
                    wgpu::CompareFunction::Less
                } else {
                    wgpu::CompareFunction::Always
                },
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &draw_shader,
                entry_point: Some("fs_particle"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: SWAPCHAIN_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::OVER,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        info!(
            "particles3d: {count} particles ({} MB), {} per sprite, {}, cube {:.1} m at ({:.2}, {:.2}, {:.2})",
            u64::from(count) * PARTICLE_BYTES / (1024 * 1024),
            if params.triangles { "triangle" } else { "quad" },
            if params.pull {
                "vertex pulling"
            } else {
                "instanced"
            },
            params.cube_half * 2.0,
            params.cube_center[0],
            params.cube_center[1],
            params.cube_center[2]
        );
        Self {
            sim_enabled: true,
            frames: Cell::new(0),
            cube: Cell::new((params.cube_center, params.cube_half)),
            count,
            triangles: params.triangles,
            params,
            sim_uniform,
            obstacles,
            _particles: particles,
            sim_bind_group,
            read_bind_group,
            eye_uniforms,
            eye_bind_groups,
            step_pipeline,
            draw_pipeline,
        }
    }

    /// Upload this frame's sim inputs. `speed` and `size` are the audio
    /// multipliers (1.0 = neutral).
    pub fn update(&self, queue: &wgpu::Queue, time: f32, dt: f32, speed: f32, size: f32) {
        let (cube_center, cube_half) = self.cube.get();
        let u = SimUniform {
            cube_center,
            cube_half,
            time,
            dt,
            count: self.count,
            speed,
            size,
            base_size: self.params.base_size,
            flow_scale: self.params.flow_scale,
            lifetime: self.params.lifetime,
            verts_per_sprite: self.verts_per_sprite(),
            pull: u32::from(self.params.pull),
            gravity: self.params.gravity,
            near_cull: self.params.near_cull,
        };
        queue.write_buffer(&self.sim_uniform, 0, bytemuck::bytes_of(&u));
    }

    /// Move and resize the cube (meters). Particles outside it respawn
    /// inside on their next step.
    pub fn set_cube(&self, center: [f32; 3], half: f32) {
        self.cube.set((center, half));
    }

    /// Upload this frame's obstacles (S7: hand joints, scene boxes, floor).
    pub fn set_obstacles(&self, queue: &wgpu::Queue, set: &ObstacleSet) {
        queue.write_buffer(&self.obstacles, 0, bytemuck::bytes_of(set));
    }

    /// Upload one eye's camera. `view_proj` must equal `proj * view`.
    pub fn set_eye(&self, queue: &wgpu::Queue, eye: usize, view: Mat4, proj: Mat4) {
        let u = EyeUniform {
            view_proj: (proj * view).to_cols_array(),
            view: view.to_cols_array(),
            proj: proj.to_cols_array(),
        };
        queue.write_buffer(&self.eye_uniforms[eye], 0, bytemuck::bytes_of(&u));
    }

    /// Advance the sim by one frame (once per frame, before the eye passes).
    pub fn step(&self, encoder: &mut wgpu::CommandEncoder) {
        let frame = self.frames.get();
        self.frames.set(frame.saturating_add(1));
        if !self.sim_enabled && frame >= WARMUP_FRAMES {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("xr-particles3d-step"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.step_pipeline);
        pass.set_bind_group(0, &self.sim_bind_group, &[]);
        pass.dispatch_workgroups(self.count.div_ceil(WORKGROUP), 1, 1);
    }

    fn verts_per_sprite(&self) -> u32 {
        if self.triangles { 3 } else { 6 }
    }

    /// Draw every particle into the current eye pass.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, eye: usize) {
        pass.set_pipeline(&self.draw_pipeline);
        pass.set_bind_group(0, &self.read_bind_group, &[]);
        pass.set_bind_group(1, &self.eye_bind_groups[eye], &[]);
        let verts = self.verts_per_sprite();
        if self.params.pull {
            pass.draw(0..verts * self.count, 0..1);
        } else {
            pass.draw(0..verts, 0..self.count);
        }
    }
}
