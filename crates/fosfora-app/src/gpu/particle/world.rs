//! World-space particle rendering for the XR build.
//!
//! The desktop renderer draws screen-space NDC with one instance per alive
//! particle and no camera. A headset needs particles that live in meters in 3D,
//! drawn once per eye through that eye's view and projection, depth tested
//! against other geometry. This module holds the public camera/target types and
//! the lazily built GPU state behind [`ParticleSystem::render_world`]; nothing
//! here exists until that entry is first called, so desktop never builds it.
//!
//! [`ParticleSystem::render_world`]: super::ParticleSystem::render_world

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::{
    BindGroup, BindGroupLayout, BlendComponent, BlendFactor, BlendOperation, BlendState,
    CommandEncoder, ComputePipeline, Device, PipelineLayout, Queue, RenderPipeline, ShaderModule,
    TextureFormat,
};

/// One eye's camera for [`ParticleSystem::render_world`].
///
/// [`ParticleSystem::render_world`]: super::ParticleSystem::render_world
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WorldCamera {
    /// World to view transform (the inverse of the eye pose).
    pub view: Mat4,
    /// View to clip projection, wgpu depth convention (z in 0..1).
    pub proj: Mat4,
    /// Effect anchor in world space: added to every particle position, which a
    /// world-layout sim writes relative to it.
    pub anchor: Vec3,
}

/// Where [`ParticleSystem::render_world`] draws.
///
/// [`ParticleSystem::render_world`]: super::ParticleSystem::render_world
#[derive(Debug, Clone, Copy)]
pub struct WorldTarget<'a> {
    pub color: &'a wgpu::TextureView,
    pub color_format: TextureFormat,
    /// Depth to test against (`Less`, loaded, never written). `None` draws
    /// without a depth test.
    pub depth: Option<&'a wgpu::TextureView>,
    /// Format of `depth`. Ignored when `depth` is `None`.
    pub depth_format: TextureFormat,
}

/// GPU side of [`WorldCamera`], matching `WorldCamera` in
/// `particle_render_world.wgsl`: 160 bytes.
#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
pub(super) struct WorldCameraUniforms {
    view: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
    anchor: [f32; 4],
    gain: f32,
    _pad: [f32; 3],
}

impl WorldCameraUniforms {
    pub(super) fn new(camera: &WorldCamera, gain: f32) -> Self {
        Self {
            view: camera.view.to_cols_array_2d(),
            proj: camera.proj.to_cols_array_2d(),
            anchor: camera.anchor.extend(0.0).to_array(),
            gain,
            _pad: [0.0; 3],
        }
    }
}

/// Camera slots in the uniform ring. Each `render_world` call writes its own
/// slot, so up to this many calls between two queue submissions (both eyes, a
/// spectator view, ...) each see their own camera. A single slot would not do:
/// `Queue::write_buffer` lands at the next submit, before every pass in it, so
/// two eyes recorded into one encoder would both draw with the last camera.
const CAMERA_SLOTS: u64 = 16;

/// Render pipeline cache key: color format, depth format (`None` = no depth
/// test), alpha blend (else additive).
type PipelineKey = (TextureFormat, Option<TextureFormat>, bool);

/// Lazily built state of the world-space path.
pub(super) struct WorldRender {
    shader: ShaderModule,
    layout: PipelineLayout,
    pipelines: HashMap<PipelineKey, RenderPipeline>,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: BindGroup,
    camera_stride: u64,
    camera_cursor: u64,
    /// `[3 * alive, 1, 0, 0]`, written on the GPU by `record_prepare`.
    pub(super) indirect_buffer: wgpu::Buffer,
    prepare_pipeline: ComputePipeline,
    prepare_bind_group: BindGroup,
}

impl WorldRender {
    /// `render_bgl` is the 2D renderer's group-0 layout (SoA buffers, render
    /// uniforms, alive indices), reused unchanged as group 0 here.
    pub(super) fn new(
        device: &Device,
        render_bgl: &BindGroupLayout,
        counter_buffer: &wgpu::Buffer,
    ) -> Self {
        let size = std::mem::size_of::<WorldCameraUniforms>() as u64;
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let camera_stride = size.div_ceil(align) * align;

        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particle-world-camera-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(size),
                },
                count: None,
            }],
        });
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle-world-camera"),
            size: camera_stride * CAMERA_SLOTS,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particle-world-camera-bg"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &camera_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(size),
                }),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particle-render-world"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../../assets/shaders/builtin/particle_render_world.wgsl")
                    .into(),
            ),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particle-render-world-layout"),
            bind_group_layouts: &[render_bgl, &camera_bgl],
            push_constant_ranges: &[],
        });

        let indirect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle-world-indirect-args"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_SRC,
            // Zeroed: draws nothing until the first prepare.
            mapped_at_creation: false,
        });
        let (prepare_pipeline, prepare_bind_group) =
            create_prepare_pipeline(device, counter_buffer, &indirect_buffer);

        Self {
            shader,
            layout,
            pipelines: HashMap::new(),
            camera_buffer,
            camera_bind_group,
            camera_stride,
            camera_cursor: 0,
            indirect_buffer,
            prepare_pipeline,
            prepare_bind_group,
        }
    }

    /// Record the pass that turns `counters[0]` into `[3 * alive, 1, 0, 0]`.
    pub(super) fn record_prepare(&self, encoder: &mut CommandEncoder) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("particle-world-prepare-indirect"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.prepare_pipeline);
        pass.set_bind_group(0, &self.prepare_bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }

    /// Write `uniforms` into the next ring slot; returns its dynamic offset.
    pub(super) fn write_camera(&mut self, queue: &Queue, uniforms: &WorldCameraUniforms) -> u32 {
        let offset = self.camera_cursor * self.camera_stride;
        self.camera_cursor = (self.camera_cursor + 1) % CAMERA_SLOTS;
        queue.write_buffer(&self.camera_buffer, offset, bytemuck::bytes_of(uniforms));
        offset as u32
    }

    pub(super) fn camera_bind_group(&self) -> &BindGroup {
        &self.camera_bind_group
    }

    /// Build the pipeline for `key` if this is its first use.
    pub(super) fn ensure_pipeline(&mut self, device: &Device, key: PipelineKey) {
        if !self.pipelines.contains_key(&key) {
            let pipeline = create_render_pipeline(device, &self.shader, &self.layout, key);
            self.pipelines.insert(key, pipeline);
        }
    }

    /// The pipeline for `key`; `ensure_pipeline` must have built it.
    pub(super) fn pipeline(&self, key: PipelineKey) -> &RenderPipeline {
        &self.pipelines[&key]
    }
}

/// Same blend states as the 2D renderer's additive and alpha pipelines.
fn blend_state(alpha: bool) -> BlendState {
    let dst = if alpha {
        BlendFactor::OneMinusSrcAlpha
    } else {
        BlendFactor::One
    };
    BlendState {
        color: BlendComponent {
            src_factor: BlendFactor::SrcAlpha,
            dst_factor: dst,
            operation: BlendOperation::Add,
        },
        alpha: BlendComponent {
            src_factor: BlendFactor::One,
            dst_factor: dst,
            operation: BlendOperation::Add,
        },
    }
}

fn create_render_pipeline(
    device: &Device,
    shader: &ShaderModule,
    layout: &PipelineLayout,
    (color_format, depth_format, alpha): PipelineKey,
) -> RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(if alpha {
            "particle-render-world-alpha"
        } else {
            "particle-render-world-additive"
        }),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: Some(blend_state(alpha)),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            // Billboards always face the eye, but a mirrored projection would
            // flip their winding; culling would buy nothing.
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: depth_format.map(|format| wgpu::DepthStencilState {
            format,
            // Test against the scene, never occlude other sprites: they are
            // blended, not sorted.
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

fn create_prepare_pipeline(
    device: &Device,
    counter_buffer: &wgpu::Buffer,
    indirect_buffer: &wgpu::Buffer,
) -> (ComputePipeline, BindGroup) {
    let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("particle-world-prepare-indirect-bgl"),
        entries: &[storage(0, true), storage(1, false)],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("particle-world-prepare-indirect"),
        source: wgpu::ShaderSource::Wgsl(
            include_str!(
                "../../../../../assets/shaders/builtin/particle_prepare_indirect_world.wgsl"
            )
            .into(),
        ),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("particle-world-prepare-indirect-layout"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("particle-world-prepare-indirect"),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("cs_main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("particle-world-prepare-indirect-bg"),
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: counter_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: indirect_buffer.as_entire_binding(),
            },
        ],
    });
    (pipeline, bind_group)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Rust uniform must match the WGSL struct byte for byte; naga reports
    /// the WGSL side's size, so this runs without a GPU.
    #[test]
    fn camera_uniform_matches_wgsl_layout() {
        let src = include_str!("../../../../../assets/shaders/builtin/particle_render_world.wgsl");
        crate::trama::effect::validate_wgsl(src).expect("world render shader validates");
        let module = naga::front::wgsl::parse_str(src).unwrap();
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("WorldCamera"))
            .expect("WorldCamera struct");
        let naga::TypeInner::Struct { span, .. } = ty.inner else {
            panic!("WorldCamera is not a struct");
        };
        assert_eq!(span as usize, std::mem::size_of::<WorldCameraUniforms>());
        assert_eq!(std::mem::size_of::<WorldCameraUniforms>(), 160);
    }

    #[test]
    fn prepare_shader_validates() {
        crate::trama::effect::validate_wgsl(include_str!(
            "../../../../../assets/shaders/builtin/particle_prepare_indirect_world.wgsl"
        ))
        .expect("world prepare-indirect shader validates");
    }

    /// `sample_flow_field_3d` compiles as part of the particle library at
    /// baseline WebGPU capabilities, called the way a world-layout sim would.
    #[test]
    fn particle_lib_flow_3d_validates() {
        let noise = include_str!("../../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../../assets/shaders/lib/palette.wgsl");
        let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
        let sim = "
@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    var p = read_particle(gid.x);
    p.vel_size = vec4f(sample_flow_field_3d(p.pos_life.xyz, 1.0), p.vel_size.w);
    write_particle(gid.x, p);
}";
        crate::trama::effect::validate_wgsl(&format!("{noise}\n{palette}\n{plib}\n{sim}"))
            .expect("particle_lib with sample_flow_field_3d validates");
    }

    #[test]
    fn uniforms_carry_camera_columns_and_anchor() {
        let camera = WorldCamera {
            view: Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0)),
            proj: Mat4::IDENTITY,
            anchor: Vec3::new(0.0, 1.5, -1.5),
        };
        let u = WorldCameraUniforms::new(&camera, 0.5);
        // Column-major, as WGSL's mat4x4f: translation is the fourth column.
        assert_eq!(u.view[3], [1.0, 2.0, 3.0, 1.0]);
        assert_eq!(u.anchor, [0.0, 1.5, -1.5, 0.0]);
        assert_eq!(u.gain, 0.5);
    }
}
