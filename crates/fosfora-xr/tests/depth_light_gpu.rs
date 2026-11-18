//! The depth light (board #5439) on a desktop GPU: the occluder mesh drawn
//! with the light over a synthetic seated view (a person, a vacuum's
//! handle, a box, the floor and a wall, holes without data and texels too
//! near) and compared with the depth-only occluder and with the Rust
//! copies. The light's vertex stages write the occluder's depth bit for
//! bit, with geometry first's drop too; each vertex's keep, light by
//! distance, normal and height match `depth_light::vertex_light`; the
//! fragment over known inputs, reading the same tile of points through the
//! GPU's filter, matches `depth_light::fragment`, its planes, octaves and
//! fades included; and over a sky drawn first, the veil leaves
//! what `depth_light::veil` says at each level. The sweep (board #5511,
//! `sonar.rs`): with its rows at 0 the fragment draws what it drew before
//! the sweep, bit for bit (`depth_light_before_sonar.wgsl`); with a sweep
//! it changes pixels only where the mesh drew and only within the shell's
//! reach, a line sits at its radius from where it began, and the fragment
//! with sweeps and standing lines matches the Rust copy.
//! `the_recorded_room_under_the_sweep` renders a depth recording
//! (`live_rec.rs`) with the sweep for the eye, with
//! `FOSFORA_SONAR_REC=<file.lprec>` and `FOSFORA_SONAR_DUMP=<dir>`.
//! `#[ignore]`d like the other GPU tests (they need an adapter); run them
//! with `cargo test -p fosfora-xr --test depth_light_gpu -- --ignored`.
#![cfg(not(target_os = "android"))]

use std::sync::{Mutex, MutexGuard, OnceLock};

use fosfora_xr::depth_light::{self, Drive, ROWS, fragment, fragment_swept, veil, vertex_light};
use fosfora_xr::env_depth::{
    DepthView, ENV_DEPTH_WGSL, EyeReprojection, FILTER_SOFT_M, LIGHT_ROW, MESH_ROW, NEAR_CUT_M,
    Obb, UNIFORM_ROWS, mesh_indices, mesh_side, mesh_texel, ray_obb,
};
use fosfora_xr::geom_first::{mesh_geom_wgsl, set_drop_vertices};
use fosfora_xr::math::{self, Fov};
use fosfora_xr::sonar::{self, Sonar, SonarOptions};
use fosfora_xr::surfaces::TEAL;
use fosfora_xr::themes::{DepthLook, Theme};
use glam::{Quat, Vec2, Vec3};

const MAP: u32 = 320;
const NEAR: f32 = 0.1;
const STEP: u32 = 2;
/// The target: half the eye's size each way, enough pixels a mesh cell.
const W: u32 = 840;
const H: u32 = 880;

const FOV: Fov = Fov {
    left: -0.942,
    right: 0.698,
    up: 0.768,
    down: -0.960,
};

static GPU: OnceLock<(wgpu::Device, wgpu::Queue)> = OnceLock::new();
static LOCK: Mutex<()> = Mutex::new(());

/// One shared device, and a lock so each test's work is its own.
fn gpu() -> (
    &'static wgpu::Device,
    &'static wgpu::Queue,
    MutexGuard<'static, ()>,
) {
    let guard = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (device, queue) = GPU.get_or_init(|| {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::METAL,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("no wgpu adapter");
        eprintln!("adapter: {:?}", adapter.get_info());
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("xr-depth-light-test"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("no wgpu device")
    });
    (device, queue, guard)
}

fn wait(device: &wgpu::Device) {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("poll");
}

/// The bytes of `buffer` (which has `COPY_SRC`), read back.
fn read_buffer(device: &wgpu::Device, queue: &wgpu::Queue, buffer: &wgpu::Buffer) -> Vec<u8> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("read-back"),
        size: buffer.size(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, buffer.size());
    queue.submit([encoder.finish()]);
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    wait(device);
    let bytes = staging.slice(..).get_mapped_range().to_vec();
    staging.unmap();
    bytes
}

fn buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    bytes: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let b = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("buffer"),
        size: bytes.len() as u64,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&b, 0, bytes);
    b
}

/// Both layers of a `side` x `side` D16 map (the same picture in each) as
/// a texture array view.
fn depth_map(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layer: &[u16],
    side: u32,
) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width: side,
        height: side,
        depth_or_array_layers: 2,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("env-depth-test"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth16Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let both: Vec<u16> = layer.iter().chain(layer).copied().collect();
    queue.write_texture(
        texture.as_image_copy(),
        bytemuck::cast_slice(&both),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(side * 2),
            rows_per_image: Some(side),
        },
        size,
    );
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

/// The seated wearer's depth camera: 1.2 m up, looking ahead and 0.3 rad
/// down, a little turned.
fn view() -> DepthView {
    DepthView {
        orientation: (Quat::from_rotation_y(0.1) * Quat::from_rotation_x(-0.3)).to_array(),
        position: [-0.032, 1.2, 0.0],
        fov: FOV,
    }
}

/// The eye, a centimeter off its depth camera, drawing into the `W` x `H`
/// target with the mesh at `STEP`.
fn eye() -> EyeReprojection {
    let v = view();
    let pos = [-0.032, 1.2, 0.01];
    EyeReprojection {
        view_proj: math::view_projection(v.orientation, pos, FOV, 0.05, 100.0),
        eye_pos: Vec3::from(pos),
        depth_view: v.view_matrix(),
        depth_fov: v.fov,
        near: NEAR,
        far: f32::INFINITY,
        depth_size: [MAP, MAP],
        target_size: [W, H],
        layer: 0,
        near_cut_m: NEAR_CUT_M,
        flip_v: false,
        filter_edge_m: FILTER_SOFT_M,
        mesh_step: STEP,
    }
}

/// The camera-space ray through texel (x, row)'s center at z = -1, row 0
/// the bottom of the view (the map's order, `envdepthflipv 0`).
fn texel_ray(x: u32, row: u32) -> Vec3 {
    let (u, v) = (
        (x as f32 + 0.5) / MAP as f32,
        (row as f32 + 0.5) / MAP as f32,
    );
    let (l, r) = (FOV.left.tan(), FOV.right.tan());
    let (up, down) = (FOV.up.tan(), FOV.down.tan());
    Vec3::new(l + u * (r - l), down + v * (up - down), -1.0)
}

/// Where the ray `o + t d` enters the sphere (c, r), t > 0.
fn ray_sphere(o: Vec3, d: Vec3, c: Vec3, r: f32) -> Option<f32> {
    let oc = o - c;
    let (a, b, k) = (d.dot(d), 2.0 * oc.dot(d), oc.dot(oc) - r * r);
    let disc = b * b - 4.0 * a * k;
    (disc >= 0.0)
        .then(|| (-b - disc.sqrt()) / (2.0 * a))
        .filter(|&t| t > 0.0)
}

/// Where it meets the vertical cylinder through `c` of radius `r`
/// between heights 0 and 1.1 m, t > 0.
fn ray_pole(o: Vec3, d: Vec3, c: Vec3, r: f32) -> Option<f32> {
    let (o2, d2) = (Vec2::new(o.x - c.x, o.z - c.z), Vec2::new(d.x, d.z));
    let (a, b, k) = (d2.dot(d2), 2.0 * o2.dot(d2), o2.dot(o2) - r * r);
    let disc = b * b - 4.0 * a * k;
    let t = (disc >= 0.0).then(|| (-b - disc.sqrt()) / (2.0 * a))?;
    (t > 0.0 && (0.0..1.1).contains(&(o.y + d.y * t))).then_some(t)
}

/// One layer of the map: a person (a sphere 1 m out), a vacuum's handle
/// (a pole 6 cm across, 0.7 m out), a box, the floor and a wall 3 m out;
/// at every silhouette the runtime's blend (a texel next to a jump takes
/// the mean of its 3x3 neighborhood); holes without data and texels
/// nearer than the discard distance. D16 under an infinite projection at
/// `NEAR`, row 0 first.
fn synthetic_map() -> Vec<u16> {
    let v = view();
    let rot = Quat::from_array(v.orientation);
    let o = Vec3::from(v.position);
    let boxes = [
        Obb {
            center: Vec3::new(0.0, -0.05, 0.0),
            rot: Quat::IDENTITY,
            half: Vec3::new(10.0, 0.05, 10.0),
        },
        Obb {
            center: Vec3::new(0.0, 1.5, -3.02),
            rot: Quat::IDENTITY,
            half: Vec3::new(4.0, 1.5, 0.02),
        },
        Obb {
            center: Vec3::new(0.6, 0.3, -1.4),
            rot: Quat::from_rotation_y(0.4),
            half: Vec3::new(0.25, 0.3, 0.2),
        },
    ];
    let mut dist = vec![f32::INFINITY; (MAP * MAP) as usize];
    for row in 0..MAP {
        for x in 0..MAP {
            let d = rot * texel_ray(x, row);
            let hit = boxes
                .iter()
                .filter_map(|b| ray_obb(o, d, b))
                .chain(ray_sphere(o, d, Vec3::new(-0.3, 0.9, -1.0), 0.3))
                .chain(ray_pole(o, d, Vec3::new(0.2, 0.0, -0.7), 0.03))
                .min_by(f32::total_cmp);
            if let Some(t) = hit {
                dist[(row * MAP + x) as usize] = t;
            }
        }
    }
    let at = |x: i32, y: i32| {
        let (x, y) = (x.clamp(0, MAP as i32 - 1), y.clamp(0, MAP as i32 - 1));
        dist[(y as u32 * MAP + x as u32) as usize]
    };
    let mut map = Vec::with_capacity((MAP * MAP) as usize);
    for row in 0..MAP as i32 {
        for x in 0..MAP as i32 {
            let c = at(x, row);
            let near: Vec<f32> = (-1..=1)
                .flat_map(|dy| (-1..=1).map(move |dx| (dx, dy)))
                .map(|(dx, dy)| at(x + dx, row + dy))
                .filter(|d| d.is_finite())
                .collect();
            let jump = near.iter().any(|d| (d - c).abs() > 0.15);
            let mut d = if c.is_finite() && jump {
                near.iter().sum::<f32>() / near.len() as f32
            } else {
                c
            };
            if (x / 9 + row / 13) % 17 == 0 {
                d = f32::INFINITY;
            } else if x < 10 && row < 20 {
                d = 0.15;
            }
            map.push(if d.is_finite() {
                ((1.0 - NEAR / d) * 65535.0).round() as u16
            } else {
                u16::MAX
            });
        }
    }
    map
}

/// The light's rows: the music half way, the wave and the twinkle part
/// way, the peak `peak`, teal sides and a violet key on top, the veil
/// `veil`.
fn light_rows_at(peak: f32, veil: f32) -> [[f32; 4]; ROWS] {
    let mut d = Drive::default();
    d.step(0.37 / 0.1, 0.5, 0.0);
    depth_light::rows(&d, peak, [TEAL, [0.7, 0.3, 1.0]], depth_light::FILL, veil)
}

/// At full immersion: the veil 0.
fn light_rows() -> [[f32; 4]; ROWS] {
    light_rows_at(depth_light::peak_alpha(1.0, 1.0), 0.0)
}

/// The eye's uniform with the light's `light` rows, and the drop's
/// vertices per layer when not 0.
fn uniform_with(light: &[[f32; 4]; ROWS], drop_vertices: u32) -> [[f32; 4]; UNIFORM_ROWS] {
    let mut rows = eye().uniform();
    rows[LIGHT_ROW..].copy_from_slice(light);
    set_drop_vertices(&mut rows, drop_vertices);
    rows
}

fn uniform_rows(drop_vertices: u32) -> [[f32; 4]; UNIFORM_ROWS] {
    uniform_with(&light_rows(), drop_vertices)
}

/// The blend of the light's and the veil's pipelines (`env_depth`'s
/// `LIGHT_BLEND`): the light added, the veil's share of what is behind
/// erased in color and alpha.
const LIGHT_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
};

/// How a draw blends: written as is into a float target, or over a sky of
/// one premultiplied color drawn first, through `LIGHT_BLEND`.
#[derive(Clone, Copy)]
enum Over {
    Nothing,
    Sky([f32; 4]),
}

/// What a draw drew: each pixel's RGBA (row-major, the target's size) and
/// depth.
struct Drawn {
    color: Vec<[f32; 4]>,
    depth: Vec<f32>,
}

/// What a draw draws over: one layer of the depth map (`side` texels a
/// side, D16, row 0 first), the mesh's step over it and the target's size.
struct Scene {
    map: Vec<u16>,
    side: u32,
    step: u32,
    target: [u32; 2],
}

/// The synthetic seated view, into the `W` x `H` target.
fn synthetic() -> Scene {
    Scene {
        map: synthetic_map(),
        side: MAP,
        step: STEP,
        target: [W, H],
    }
}

/// [`draw_scene`] over the synthetic map.
fn draw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    source: &str,
    vertex: &str,
    fragment: &str,
    topology: wgpu::PrimitiveTopology,
    rows: &[[f32; 4]; UNIFORM_ROWS],
    drops: Option<&[u32]>,
    over: Over,
) -> Drawn {
    draw_scene(
        device,
        queue,
        &synthetic(),
        source,
        vertex,
        fragment,
        topology,
        rows,
        drops,
        over,
    )
}

/// One draw from `source`'s `vertex` and `fragment` over `scene`'s map,
/// into a cleared float target and depth (compare `LessEqual`, written, as
/// the occluder): the mesh's triangles at the scene's step, its vertices
/// as points (`points`), or 3 vertices with no index buffer (`vertices`).
fn draw_scene(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: &Scene,
    source: &str,
    vertex: &str,
    fragment: &str,
    topology: wgpu::PrimitiveTopology,
    rows: &[[f32; 4]; UNIFORM_ROWS],
    drops: Option<&[u32]>,
    over: Over,
) -> Drawn {
    let [w, h] = scene.target;
    let view = depth_map(device, queue, &scene.map, scene.side);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("depth-light"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    // A float target that blends where the draw blends.
    let (format, blend) = match over {
        Over::Nothing => (wgpu::TextureFormat::Rgba32Float, None),
        Over::Sky(_) => (wgpu::TextureFormat::Rgba16Float, Some(LIGHT_BLEND)),
    };
    let depth_format = wgpu::TextureFormat::Depth32Float;
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("depth-light"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some(vertex),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology,
            cull_mode: None,
            ..wgpu::PrimitiveState::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth_format,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some(fragment),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    });
    let sky = match over {
        Over::Nothing => None,
        Over::Sky(c) => Some(sky_pipeline(device, format, depth_format, c)),
    };
    let eye = buffer(
        device,
        queue,
        bytemuck::cast_slice(rows),
        wgpu::BufferUsages::UNIFORM,
    );
    let drops = drops.map(|d| {
        buffer(
            device,
            queue,
            bytemuck::cast_slice(d),
            wgpu::BufferUsages::STORAGE,
        )
    });
    let layout = pipeline.get_bind_group_layout(0);
    let mut entries = vec![wgpu::BindGroupEntry {
        binding: 0,
        resource: eye.as_entire_binding(),
    }];
    // A stage that never reads the map (the fragment test's) has no
    // binding 1 in its derived layout.
    if !vertex.starts_with("vs_ramp") {
        entries.push(wgpu::BindGroupEntry {
            binding: 1,
            resource: wgpu::BindingResource::TextureView(&view),
        });
    }
    if let Some(b) = &drops {
        entries.push(wgpu::BindGroupEntry {
            binding: 2,
            resource: b.as_entire_binding(),
        });
    }
    // The light's fragment reads the tile of points at 3 and 4.
    let (points, sampler) = points_tile(device, queue);
    if fragment.starts_with("fs_mesh_light") {
        entries.push(wgpu::BindGroupEntry {
            binding: 3,
            resource: wgpu::BindingResource::TextureView(&points),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 4,
            resource: wgpu::BindingResource::Sampler(&sampler),
        });
    }
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("depth-light"),
        layout: &layout,
        entries: &entries,
    });
    use wgpu::util::DeviceExt as _;
    let indices = mesh_indices([scene.side, scene.side], scene.step);
    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("depth-light-mesh"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    let extent = wgpu::Extent3d {
        width: w,
        height: h,
        depth_or_array_layers: 1,
    };
    let texture = |format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("target"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
    let color = texture(format, usage);
    let depth = texture(depth_format, usage);
    let texel = if blend.is_some() { 8 } else { 16 };
    let color_bytes = (w * texel).next_multiple_of(256);
    let depth_bytes = (w * 4).next_multiple_of(256);
    let read = |row_bytes: u32| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("back"),
            size: u64::from(row_bytes * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    };
    let (color_back, depth_back) = (read(color_bytes), read(depth_bytes));
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("depth-light"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        if let Some(sky) = &sky {
            pass.set_pipeline(sky);
            pass.draw(0..3, 0..1);
        }
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        match topology {
            wgpu::PrimitiveTopology::PointList => {
                // The vertices of the mesh at the uniform's own step.
                let side = rows[MESH_ROW][1] as u32;
                pass.draw(0..side * side, 0..1);
            }
            _ if vertex.starts_with("vs_ramp") => pass.draw(0..3, 0..1),
            _ => {
                pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..indices.len() as u32, 0, 0..1);
            }
        }
    }
    for (texture, aspect, back, row_bytes) in [
        (&color, wgpu::TextureAspect::All, &color_back, color_bytes),
        (
            &depth,
            wgpu::TextureAspect::DepthOnly,
            &depth_back,
            depth_bytes,
        ),
    ] {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: back,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: Some(h),
                },
            },
            extent,
        );
    }
    queue.submit([encoder.finish()]);
    let rows_of = |back: &wgpu::Buffer, row_bytes: u32, used: u32| -> Vec<f32> {
        read_buffer(device, queue, back)
            .chunks(row_bytes as usize)
            .flat_map(|row| bytemuck::pod_collect_to_vec::<u8, f32>(&row[..used as usize]))
            .collect()
    };
    let color = if blend.is_some() {
        read_buffer(device, queue, &color_back)
            .chunks(color_bytes as usize)
            .flat_map(|row| {
                bytemuck::pod_collect_to_vec::<u8, u16>(&row[..(w * 8) as usize])
                    .chunks(4)
                    .map(|c| [0, 1, 2, 3].map(|k| half(c[k])))
                    .collect::<Vec<_>>()
            })
            .collect()
    } else {
        rows_of(&color_back, color_bytes, w * 16)
            .chunks(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect()
    };
    Drawn {
        color,
        depth: rows_of(&depth_back, depth_bytes, w * 4),
    }
}

/// The light's tile of points as the app uploads it (`env_depth`'s
/// `LightPoints`): RGBA8 at level 0, a linear repeating sampler.
fn points_tile(device: &wgpu::Device, queue: &wgpu::Queue) -> (wgpu::TextureView, wgpu::Sampler) {
    let size = wgpu::Extent3d {
        width: depth_light::TILE,
        height: depth_light::TILE,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("points"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let tile = depth_light::points_tile();
    queue.write_texture(
        texture.as_image_copy(),
        bytemuck::cast_slice(&tile),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(depth_light::TILE * 4),
            rows_per_image: Some(depth_light::TILE),
        },
        size,
    );
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("points"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..wgpu::SamplerDescriptor::default()
    });
    (
        texture.create_view(&wgpu::TextureViewDescriptor::default()),
        sampler,
    )
}

/// An f16's value.
fn half(h: u16) -> f32 {
    let sign = if h & 0x8000 == 0 { 1.0 } else { -1.0 };
    let e = i32::from((h >> 10) & 31);
    let m = f32::from(h & 1023);
    sign * if e == 0 {
        m * 2f32.powi(-24)
    } else {
        (1.0 + m / 1024.0) * 2f32.powi(e - 15)
    }
}

/// The sky's depth: just in front of the far plane, as the room's boxes are
/// behind the sky's faces, so the mesh's triangles that lean back to the far
/// plane without data fall behind it, as they fall behind a wall.
const SKY_DEPTH: f32 = 0.999;

/// A full-target triangle of the premultiplied color `c`, drawn first,
/// written as is, at [`SKY_DEPTH`].
fn sky_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    c: [f32; 4],
) -> wgpu::RenderPipeline {
    let [r, g, b, a] = c;
    let source = format!(
        r"
@vertex
fn vs_sky(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {{
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, {SKY_DEPTH:?}, 1.0);
}}

@fragment
fn fs_sky() -> @location(0) vec4<f32> {{
    return vec4<f32>({r:?}, {g:?}, {b:?}, {a:?});
}}
"
    );
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sky"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sky"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_sky"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth_format,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Always,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_sky"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    })
}

/// Every `k`th vertex of each layer's mesh dropped, for the drop's A/B.
fn some_drops() -> Vec<u32> {
    let side = mesh_side(MAP, STEP);
    (0..2 * side * side)
        .map(|i| u32::from(i % 5 == 0))
        .collect()
}

fn differ(a: &[f32], b: &[f32]) -> usize {
    a.iter()
        .zip(b)
        .filter(|(x, y)| x.to_bits() != y.to_bits())
        .count()
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_light_writes_the_occluders_depth_bit_for_bit() {
    let (device, queue, _guard) = gpu();
    let tri = wgpu::PrimitiveTopology::TriangleList;
    let rows = uniform_rows(0);
    let draw_with = |source: &str, stages: [&str; 2], rows, drops| {
        draw(
            device,
            queue,
            source,
            stages[0],
            stages[1],
            tri,
            rows,
            drops,
            Over::Nothing,
        )
    };
    let own = draw_with(ENV_DEPTH_WGSL, ["vs_mesh", "fs_mesh"], &rows, None);
    let lit = draw_with(
        ENV_DEPTH_WGSL,
        ["vs_mesh_light", "fs_mesh_light"],
        &rows,
        None,
    );
    assert_eq!(differ(&own.depth, &lit.depth), 0);
    // Geometry first's drop: the light's vertex stage drops what the
    // occluder's does, and the faces pass's veil is the occluder's own.
    let side = mesh_side(MAP, STEP);
    let drops = some_drops();
    let rows = uniform_rows(side * side);
    let geom = mesh_geom_wgsl();
    let dropped = draw_with(&geom, ["vs_mesh_geom", "fs_mesh"], &rows, Some(&drops));
    let dropped_lit = draw_with(
        &geom,
        ["vs_mesh_geom_light", "fs_mesh_light"],
        &rows,
        Some(&drops),
    );
    let dropped_veil = draw_with(&geom, ["vs_mesh_geom", "fs_mesh_veil"], &rows, Some(&drops));
    assert_eq!(differ(&dropped.depth, &dropped_lit.depth), 0);
    assert_eq!(differ(&dropped.depth, &dropped_veil.depth), 0);
    assert!(differ(&dropped.depth, &own.depth) > 1000);
    // The light lands only where the mesh wrote depth, never past the
    // peak; its alpha out is the veil (0 here).
    let peak = light_rows()[0][2];
    let (mut drawn, mut lit_px) = (0, 0);
    for (c, &d) in lit.color.iter().zip(&lit.depth) {
        if d.to_bits() == 1f32.to_bits() {
            assert!(c.iter().all(|v| v.to_bits() == 0), "{c:?}");
            continue;
        }
        drawn += 1;
        assert!(c[..3].iter().all(|&v| v <= peak + 1e-6), "{c:?}");
        assert_eq!(c[3].to_bits(), 0, "{c:?}");
        lit_px += usize::from(c[1] > 0.02);
    }
    eprintln!(
        "{drawn} of {} pixels drawn by the mesh, {lit_px} of them lit above 0.02",
        lit.depth.len()
    );
    // Points: clearly less lit than drawn, and some.
    assert!(
        lit_px > drawn / 20 && lit_px < drawn / 2,
        "{lit_px} of {drawn}"
    );
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_veil_leaves_the_sky_by_the_level() {
    let (device, queue, _guard) = gpu();
    let tri = wgpu::PrimitiveTopology::TriangleList;
    let sky = [0.1, 0.5, 0.4, 0.6];
    for level in [0.0, 0.5, 1.0] {
        let v = depth_light::veil_share(level, true);
        // The veil alone (the faces pass's, and the light's with no
        // light): the sky where the mesh is not, its kept share where it
        // is, once per fragment there (a fold of the mesh erases twice).
        for (stages, peak) in [
            (["vs_mesh", "fs_mesh_veil"], 1.0),
            (["vs_mesh_light", "fs_mesh_light"], 0.0),
        ] {
            let rows = uniform_with(&light_rows_at(peak, v), 0);
            let got = draw(
                device,
                queue,
                ENV_DEPTH_WGSL,
                stages[0],
                stages[1],
                tri,
                &rows,
                None,
                Over::Sky(sky),
            );
            let (mut once, mut folds, mut clear) = (0, 0, 0);
            for (c, &d) in got.color.iter().zip(&got.depth) {
                let close = |a: [f32; 4], b: [f32; 4]| (0..4).all(|k| (a[k] - b[k]).abs() < 2e-3);
                if d.to_bits() == SKY_DEPTH.to_bits() {
                    assert!(close(*c, sky), "{c:?}");
                    clear += 1;
                    continue;
                }
                let one = veil(sky, [0.0, 0.0, 0.0, v]);
                if close(*c, one) {
                    once += 1;
                } else {
                    // A fold: erased again.
                    assert!(
                        close(*c, veil(one, [0.0, 0.0, 0.0, v]))
                            || close(*c, veil(veil(one, [0.0, 0.0, 0.0, v]), [0.0, 0.0, 0.0, v])),
                        "{level} {c:?}"
                    );
                    folds += 1;
                }
            }
            eprintln!(
                "level {level}, {}: {once} pixels erased once, {folds} on folds, {clear} sky alone",
                stages[1]
            );
            assert!(once > 100 * folds.max(1) / 2, "{once} {folds}");
            assert!(clear > 1000);
        }
    }
    // At full immersion with the light: the sky kept whole, its alpha
    // untouched, the points added over it.
    let rows = uniform_with(&light_rows(), 0);
    let got = draw(
        device,
        queue,
        ENV_DEPTH_WGSL,
        "vs_mesh_light",
        "fs_mesh_light",
        tri,
        &rows,
        None,
        Over::Sky(sky),
    );
    let mut added = 0;
    for c in &got.color {
        assert!((c[3] - sky[3]).abs() < 2e-3, "{c:?}");
        for k in 0..3 {
            assert!(c[k] >= sky[k] - 2e-3, "{c:?}");
        }
        added += usize::from(c[1] > sky[1] + 0.02);
    }
    assert!(added > 1000, "{added}");
}

/// The light's vertex stage, each vertex a point, handing on its index
/// (`vs_test`), and its outputs as they arrive: the keep, the light by
/// distance, the height (`fs_test`), and the normal (`fs_test_normal`).
fn vertex_test_wgsl(source: &str) -> String {
    format!(
        r"{source}
struct TestOut {{
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec4<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) @interpolate(flat) index: u32,
}}

@vertex
fn vs_test(@builtin(vertex_index) i: u32) -> TestOut {{
    let o = mesh_light(i);
    var out: TestOut;
    out.pos = o.pos;
    out.world = o.world;
    out.normal = o.normal;
    out.index = i;
    return out;
}}

@fragment
fn fs_test(in: TestOut) -> @location(0) vec4<f32> {{
    return vec4<f32>(in.world.w, in.normal.w, in.world.y, f32(in.index));
}}

@fragment
fn fs_test_normal(in: TestOut) -> @location(0) vec4<f32> {{
    return vec4<f32>(in.normal.xyz, f32(in.index));
}}
"
    )
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_vertex_stage_matches_its_cpu_mirror() {
    let (device, queue, _guard) = gpu();
    // The eye pass's mesh, and the faces layer's at twice the step, where
    // the points draw with a faces layer.
    for step in [STEP, 2 * STEP] {
        vertex_stage_matches(device, queue, step);
    }
}

fn vertex_stage_matches(device: &wgpu::Device, queue: &wgpu::Queue, step: u32) {
    let mut r = eye();
    r.mesh_step = step;
    let mut rows = r.uniform();
    rows[LIGHT_ROW..].copy_from_slice(&light_rows());
    let source = vertex_test_wgsl(ENV_DEPTH_WGSL);
    let run = |fragment: &str| {
        draw(
            device,
            queue,
            &source,
            "vs_test",
            fragment,
            wgpu::PrimitiveTopology::PointList,
            &rows,
            None,
            Over::Nothing,
        )
    };
    let (shares, normals) = (run("fs_test"), run("fs_test_normal"));
    let map = synthetic_map();
    let stored = |t: [u32; 2]| f32::from(map[(t[1] * MAP + t[0]) as usize]) / 65535.0;
    let light = light_rows();
    let side = mesh_side(MAP, step);
    let want = |i: u32| {
        let texel = [
            mesh_texel(i % side, MAP, step),
            mesh_texel(i / side, MAP, step),
        ];
        vertex_light(&r, &light, texel, stored)
    };
    let drawn = |c: &[f32; 4], d: f32| {
        !(d.to_bits() == 1f32.to_bits() && c.iter().all(|v| v.to_bits() == 0))
    };
    let (mut compared, mut full, mut part, mut far) = (0, 0, 0, 0);
    let (mut worst_keep, mut worst_bright, mut worst_y, mut worst_n) =
        (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for (c, &d) in shares.color.iter().zip(&shares.depth) {
        if !drawn(c, d) {
            continue;
        }
        let w = want(c[3] as u32);
        worst_keep = worst_keep.max((c[0] - w.keep).abs());
        worst_bright = worst_bright.max((c[1] - w.bright).abs());
        if w.keep > 0.0 {
            worst_y = worst_y.max((c[2] - w.world.y).abs());
        }
        compared += 1;
        full += usize::from(w.keep > 0.99);
        part += usize::from(w.keep > 0.01 && w.keep < 0.99);
        far += usize::from(w.bright < 0.99 && w.keep > 0.0);
    }
    for (c, &d) in normals.color.iter().zip(&normals.depth) {
        if drawn(c, d) {
            let w = want(c[3] as u32);
            worst_n = worst_n.max((Vec3::new(c[0], c[1], c[2]) - w.normal).length());
        }
    }
    eprintln!(
        "step {step}: {compared} vertices compared ({full} keep all, {part} part, {far} dimmed by distance): worst keep {worst_keep:.2e}, light {worst_bright:.2e}, height {worst_y:.2e} m, normal {worst_n:.2e}"
    );
    assert!(compared > 5000 / (step * step / 4) as usize, "{compared}");
    assert!(full > 1000 && part > 50 && far > 100, "{full} {part} {far}");
    assert!(worst_keep < 2e-3, "{worst_keep}");
    assert!(worst_bright < 2e-3, "{worst_bright}");
    assert!(worst_y < 1e-4, "{worst_y}");
    assert!(worst_n < 2e-3, "{worst_n}");
}

/// The light's fragment stage over known inputs: a triangle over the
/// whole target whose world point runs linearly across it from `p0` by
/// `px` per unit of NDC x and `py` of NDC y, the keep up NDC y, the light
/// by distance across NDC x, the normal `n` throughout.
fn fragment_test_wgsl(p0: Vec3, px: Vec3, py: Vec3, n: Vec3) -> String {
    let v = |v: Vec3| format!("vec3<f32>({:?}, {:?}, {:?})", v.x, v.y, v.z);
    let (p0, px, py, n) = (v(p0), v(px), v(py), v(n));
    format!(
        r"{ENV_DEPTH_WGSL}
@vertex
fn vs_ramp(@builtin(vertex_index) i: u32) -> LightOut {{
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    var out: LightOut;
    out.pos = vec4<f32>(x, y, 0.5, 1.0);
    out.world = vec4<f32>({p0} + {px} * x + {py} * y, 0.5 + 0.5 * y);
    out.normal = vec4<f32>({n}, 0.6 + 0.4 * x);
    return out;
}}
"
    )
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_fragment_matches_its_cpu_mirror() {
    let (device, queue, _guard) = gpu();
    fragment_matches(device, queue, &light_rows(), 15);
}

/// The fragment over the known surfaces with the light's rows `light`
/// (the light's own, or the light's under the sweep where the rows hold
/// one, as the app picks it) against `depth_light::fragment`: at most
/// `bad_max` pixels of each further than 2e-3 from it.
fn fragment_matches(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    light: &[[f32; 4]; ROWS],
    bad_max: usize,
) {
    let rows = uniform_with(light, 0);
    let eye_pos = eye().eye_pos;
    let tile = depth_light::points_tile();
    // A wall 1.2 m out, a top below the eye, a surface between axes
    // across an octave's boundary, one whose cells shrink to pixels, and
    // a steep one whose wave is too thin to draw.
    let cases = [
        (
            Vec3::new(0.0, 1.2, -1.2),
            Vec3::new(0.5, 0.0, 0.0),
            Vec3::new(0.0, 0.5, 0.0),
            Vec3::Z,
        ),
        (
            Vec3::new(0.0, 0.5, -0.9),
            Vec3::new(0.4, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.4),
            Vec3::Y,
        ),
        (
            Vec3::new(0.3, 1.0, -1.9),
            Vec3::new(0.6, 0.1, -0.3),
            Vec3::new(0.0, 0.5, 0.2),
            Vec3::new(0.6, 0.3, 0.75).normalize(),
        ),
        (
            Vec3::new(0.0, 1.2, -1.2),
            Vec3::new(12.0, 0.0, 0.0),
            Vec3::new(0.0, 0.6, 0.0),
            Vec3::Z,
        ),
        (
            Vec3::new(0.0, 1.2, -1.2),
            Vec3::new(0.4, 0.0, 0.0),
            Vec3::new(0.0, 25.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        ),
    ];
    for (p0, px, py, n) in cases {
        let got = draw(
            device,
            queue,
            &fragment_test_wgsl(p0, px, py, n),
            "vs_ramp",
            light_fragment(light),
            wgpu::PrimitiveTopology::TriangleList,
            &rows,
            None,
            Over::Nothing,
        );
        // The world's change across a pixel: fwidth's sum.
        let fw = (px * (2.0 / W as f32)).abs() + (py * (2.0 / H as f32)).abs();
        let (mut worst, mut bad, mut lit) = (0.0f32, 0, 0);
        for py_ in 0..H {
            for px_ in 0..W {
                let nx = (px_ as f32 + 0.5) / W as f32 * 2.0 - 1.0;
                let ny = 1.0 - (py_ as f32 + 0.5) / H as f32 * 2.0;
                let p = p0 + px * nx + py * ny;
                let (keep, bright) = (0.5 + 0.5 * ny, 0.6 + 0.4 * nx);
                let want = if depth_light::swept(light) {
                    // The world's change across a pixel each way.
                    let across = [px * (2.0 / W as f32), py * (-2.0 / H as f32)];
                    fragment_swept(light, &tile, eye_pos, p, n, keep, bright, across)
                } else {
                    fragment(light, &tile, eye_pos, p, n, keep, bright, fw)
                };
                let c = got.color[(py_ * W + px_) as usize];
                let e = (0..4)
                    .map(|k| (c[k] - want[k]).abs())
                    .fold(0.0f32, f32::max);
                worst = worst.max(e);
                bad += usize::from(e > 2e-3);
                lit += usize::from(want[1] > 0.02);
            }
        }
        eprintln!(
            "p0 {p0} by {px} / {py}, n {n}: {lit} pixels lit, worst {worst:.2e}, {bad} over 2e-3"
        );
        // The GPU weighs the tile's texels with a few bits of sub-texel
        // precision; where two points' channels meet (an overlap, a size
        // between two twinkle rates) a handful of pixels differ.
        assert!(bad <= bad_max, "{bad}");
    }
}

// ---- The sweep (board #5511, sonar.rs) --------------------------------

/// `fs_mesh_light` as it was before the sweep: with the sweep's rows at 0
/// the fragment must draw what this drew, bit for bit.
const BEFORE_SONAR: &str = include_str!("depth_light_before_sonar.wgsl");

/// A frame of the sweep's inputs `dt` after the last, the head at `head`.
fn sonar_input(dt: f32, head: Vec3) -> sonar::Input {
    sonar::Input {
        dt,
        downbeat: false,
        drop: false,
        beat_period_s: Some(0.5),
        buildup: 0.0,
        quiet: false,
        head,
        motion: 1.0,
    }
}

/// One sweep that left `origin` on a downbeat at 120 BPM (2 m/s), now
/// `radius` m out.
fn one_sweep(opts: &SonarOptions, origin: Vec3, radius: f32) -> Sonar {
    let mut s = Sonar::default();
    s.step(
        opts,
        sonar::Input {
            downbeat: true,
            ..sonar_input(0.0, origin)
        },
    );
    s.step(opts, sonar_input(radius / 2.0, origin));
    s
}

/// `sonar` after a build held at `buildup` for `secs`, the head at `head`.
fn built(mut sonar: Sonar, opts: &SonarOptions, head: Vec3, buildup: f32, secs: f32) -> Sonar {
    for _ in 0..(secs * 72.0) as usize {
        sonar.step(
            opts,
            sonar::Input {
                buildup,
                ..sonar_input(1.0 / 72.0, head)
            },
        );
    }
    sonar
}

/// The fragment stage the app draws the light with for the rows `light`:
/// the light's under the sweep on a frame the sweep shows, else the
/// light's own.
fn light_fragment(light: &[[f32; 4]; ROWS]) -> &'static str {
    if depth_light::swept(light) {
        "fs_mesh_light_sonar"
    } else {
        "fs_mesh_light"
    }
}

/// The fragment that hands on what the vertex stage gave it: the world
/// point and the keep.
fn world_wgsl() -> String {
    format!(
        r"{ENV_DEPTH_WGSL}
@fragment
fn fs_world(in: LightOut) -> @location(0) vec4<f32> {{
    return in.world;
}}
"
    )
}

fn bits(pixels: &[[f32; 4]]) -> Vec<f32> {
    pixels.iter().flatten().copied().collect()
}

/// Off is off: with the sweep's rows at 0 (the knob at 0, or nothing
/// alive) the app draws the light with `fs_mesh_light`, and that writes
/// what the fragment from before the sweep writes, bit for bit, over the
/// mesh and over the known surfaces, with the room black and half visible.
#[test]
#[ignore = "needs a GPU adapter"]
fn with_the_sweep_off_the_light_draws_what_it_drew_bit_for_bit() {
    let (device, queue, _guard) = gpu();
    let tri = wgpu::PrimitiveTopology::TriangleList;
    // The knob at 0 with a sweep that would be alive, and the knob at 1
    // with nothing alive: both all 0.
    let off = SonarOptions {
        mode: 0,
        ..SonarOptions::default()
    };
    let none = [[0.0f32; 4]; sonar::ROWS];
    assert_eq!(
        one_sweep(&off, eye().eye_pos, 1.2).rows(&off, 1.0),
        none,
        "the knob at 0"
    );
    assert_eq!(
        Sonar::default().rows(&SonarOptions::default(), 1.0),
        none,
        "nothing alive"
    );
    let source = format!("{ENV_DEPTH_WGSL}\n{BEFORE_SONAR}");
    for level in [1.0, 0.5] {
        let light = depth_light::with_sonar(
            light_rows_at(
                depth_light::peak_alpha(level, 1.0),
                depth_light::veil_share(level, true),
            ),
            &none,
        );
        assert_eq!(light_fragment(&light), "fs_mesh_light");
        let rows = uniform_with(&light, 0);
        let run = |fragment: &str| {
            draw(
                device,
                queue,
                &source,
                "vs_mesh_light",
                fragment,
                tri,
                &rows,
                None,
                Over::Nothing,
            )
        };
        let (now, before) = (run("fs_mesh_light"), run("fs_mesh_light_before"));
        let lit = now.color.iter().filter(|c| c[1] > 0.02).count();
        let differing = differ(&bits(&now.color), &bits(&before.color));
        eprintln!(
            "level {level}: {lit} pixels lit, {differing} values differ from before the sweep"
        );
        assert!(lit > 10_000, "{lit}");
        assert_eq!(differing, 0);
        assert_eq!(differ(&now.depth, &before.depth), 0);
    }
    // The known surfaces, where every plane, octave and fade is reached.
    let rows = uniform_rows(0);
    for (p0, px, py, n) in [
        (
            Vec3::new(0.3, 1.0, -1.9),
            Vec3::new(0.6, 0.1, -0.3),
            Vec3::new(0.0, 0.5, 0.2),
            Vec3::new(0.6, 0.3, 0.75).normalize(),
        ),
        (
            Vec3::new(0.0, 1.2, -1.2),
            Vec3::new(12.0, 0.0, 0.0),
            Vec3::new(0.0, 0.6, 0.0),
            Vec3::Z,
        ),
    ] {
        let source = format!("{}\n{BEFORE_SONAR}", fragment_test_wgsl(p0, px, py, n));
        let run = |fragment: &str| {
            draw(
                device,
                queue,
                &source,
                "vs_ramp",
                fragment,
                tri,
                &rows,
                None,
                Over::Nothing,
            )
        };
        let (now, before) = (run("fs_mesh_light"), run("fs_mesh_light_before"));
        assert_eq!(differ(&bits(&now.color), &bits(&before.color)), 0);
    }
}

/// With a sweep the frame changes only where the mesh drew, and there only
/// within the shell's reach: from the end of its tail to half a line ahead
/// of it. Elsewhere the light's fragment under the sweep writes what the
/// light's own writes, to the float's rounding. The depth and the veil
/// stay as they were.
#[test]
#[ignore = "needs a GPU adapter"]
fn the_sweep_changes_the_lights_things_alone() {
    let (device, queue, _guard) = gpu();
    let tri = wgpu::PrimitiveTopology::TriangleList;
    let opts = SonarOptions::default();
    let origin = eye().eye_pos + Vec3::new(0.15, -0.1, 0.1);
    let radius = 1.4;
    let swept = one_sweep(&opts, origin, radius).rows(&opts, 1.0);
    let run = |sonar: &[[f32; 4]; sonar::ROWS], source: &str, fragment: Option<&str>| {
        let light = depth_light::with_sonar(light_rows(), sonar);
        draw(
            device,
            queue,
            source,
            "vs_mesh_light",
            fragment.unwrap_or(light_fragment(&light)),
            tri,
            &uniform_with(&light, 0),
            None,
            Over::Nothing,
        )
    };
    let none = [[0.0f32; 4]; sonar::ROWS];
    let off = run(&none, ENV_DEPTH_WGSL, None);
    let on = run(&swept, ENV_DEPTH_WGSL, None);
    let world = run(&none, &world_wgsl(), Some("fs_world"));
    assert_eq!(differ(&on.depth, &off.depth), 0);
    let tail_m = 2.0 * opts.tail_s;
    let half = sonar::half_width(sonar::HALF_WIDTH_M, radius, 1.0);
    let (mut changed, mut brighter) = (0, 0);
    let mut rounding = 0.0f32;
    for (k, (a, b)) in on.color.iter().zip(&off.color).enumerate() {
        // The veil in the alpha, untouched.
        assert_eq!(a[3].to_bits(), b[3].to_bits());
        let moved = (0..3).map(|c| (a[c] - b[c]).abs()).fold(0.0f32, f32::max);
        let w = world.color[k];
        let behind = radius - (Vec3::new(w[0], w[1], w[2]) - origin).length();
        let reached = on.depth[k].to_bits() != 1f32.to_bits()
            && w[3] > 0.0
            && behind > -half - 1e-3
            && behind < tail_m + 1e-3;
        if !reached {
            // Off the mesh, where it keeps none of its light, or out of
            // the shell's reach: as without the sweep, within what two
            // compilations of the same arithmetic differ by (a hundredth
            // of an 8-bit step on an RTX 4090).
            rounding = rounding.max(moved);
            assert!(
                moved < 1e-4,
                "pixel {k}: {moved}, {behind} m behind the shell"
            );
            continue;
        }
        changed += usize::from(moved > 1e-4);
        brighter += usize::from(a[1] > b[1] + 0.02);
    }
    eprintln!(
        "{changed} pixels changed by a sweep {radius} m out, {brighter} of them brighter by 0.02; out of its reach the most any moved is {rounding:.1e}"
    );
    assert!(changed > 20_000 && brighter > 2000, "{changed} {brighter}");
}

/// A line sits at its radius from where the sweep began: with no points
/// and no tail, every lit pixel's world point is within the line's
/// half-width of the radius, the brightest in its middle, and most of the
/// mesh there is lit.
#[test]
#[ignore = "needs a GPU adapter"]
fn a_line_sits_at_its_radius_from_where_the_sweep_began() {
    let (device, queue, _guard) = gpu();
    let tri = wgpu::PrimitiveTopology::TriangleList;
    let opts = SonarOptions {
        tail_s: 0.0,
        ..SonarOptions::default()
    };
    // Not the eye: the sweep's own origin.
    let origin = eye().eye_pos + Vec3::new(0.2, -0.15, 0.1);
    let run = |sonar: &[[f32; 4]; sonar::ROWS], source: &str, fragment: Option<&str>| {
        let light = depth_light::with_sonar(light_rows_at(0.0, 0.0), sonar);
        draw(
            device,
            queue,
            source,
            "vs_mesh_light",
            fragment.unwrap_or(light_fragment(&light)),
            tri,
            &uniform_with(&light, 0),
            None,
            Over::Nothing,
        )
    };
    let world = run(&[[0.0; 4]; sonar::ROWS], &world_wgsl(), Some("fs_world"));
    let light = light_rows();
    for radius in [0.9f32, 1.6, 2.8] {
        let rows = one_sweep(&opts, origin, radius).rows(&opts, 1.0);
        let got = run(&rows, ENV_DEPTH_WGSL, None);
        let half = sonar::half_width(sonar::HALF_WIDTH_M, radius, 1.0);
        // The crest's light in a mass: the line's alpha, the side's color
        // tinted toward the top's by the crest.
        let line = rows[3][0];
        let peak = (0..3)
            .map(|k| {
                let (side, top) = (light[1][k], light[2][k]);
                (side + (top - side) * rows[0][3] * line) * rows[0][0] * line
            })
            .fold(0.0f32, f32::max);
        let (mut lit, mut bright, mut middle, mut middle_lit) = (0, 0, 0, 0);
        let mut worst = 0.0f32;
        for (c, w) in got.color.iter().zip(&world.color) {
            let off = ((Vec3::new(w[0], w[1], w[2]) - origin).length() - radius).abs();
            let light = c[0].max(c[1]).max(c[2]);
            if light > 0.02 * peak {
                lit += 1;
                worst = worst.max(off);
            }
            if light > 0.9 * peak {
                bright += 1;
                assert!(off < 0.4 * half, "{off} m off at {radius} m");
            }
            // The mesh inside a mass, in the line's middle.
            if w[3] > 0.999 && off < 0.25 * half {
                middle += 1;
                middle_lit += usize::from(light > 0.25 * peak);
            }
        }
        eprintln!(
            "a sweep {radius} m out, the line {:.1} cm to each side: {lit} pixels lit, the farthest {:.1} cm off the radius, {bright} at full light, {middle_lit} of {middle} in its middle lit",
            100.0 * half,
            100.0 * worst
        );
        assert!(lit > 300 && bright > 20, "{lit} {bright}");
        assert!(worst <= half + 1e-3, "{worst}");
        assert!(middle_lit * 10 >= middle * 8, "{middle_lit} of {middle}");
    }
}

/// The fragment with two sweeps crossing the known surfaces and the
/// standing lines at a full build matches `depth_light::fragment`.
/// The light's rows drawing `look` in `theme`'s colors (a violet key), at
/// immersion `level`, the band at `music` and the loudness at `gust`,
/// `secs` in: the travel, the sway and the twinkle part way.
fn look_rows(
    theme: Theme,
    look: DepthLook,
    level: f32,
    music: f32,
    gust: f32,
    secs: f32,
) -> [[f32; 4]; ROWS] {
    let mut d = Drive::default();
    for _ in 0..(secs * 72.0) as usize {
        d.step(1.0 / 72.0, music, 0.0);
        d.step_wind(1.0 / 72.0, gust);
    }
    // What the look reads of the room: the wash's front for a theme that
    // has one, a third of a bar in, 1 m ahead of the eye, over the known
    // surfaces.
    // The glints turn about the eye's place, the orbit a little on with
    // the time.
    let at = eye().eye_pos;
    let around = depth_light::Around {
        front: depth_light::bar_front(0.35, at),
        orbit: 0.3 + 0.15 * secs,
        center: [at.x, at.z],
        bands: [0.95 * music, 0.6 * music, 0.35 * music],
    };
    depth_light::with_around(
        depth_light::with_look(
            depth_light::rows(
                &d,
                depth_light::peak_alpha(level, 1.0),
                depth_light::colors(Some(theme), [0.7, 0.3, 1.0]),
                depth_light::FILL,
                0.0,
            ),
            look,
        ),
        &around,
    )
}

/// The video's three themes, each with the look its depth light draws.
const LOOKS: [(Theme, DepthLook); 7] = [
    (Theme::Aurora, DepthLook::Beams),
    (Theme::Flint, DepthLook::Coals),
    (Theme::Contour, DepthLook::Grass),
    (Theme::Drift, DepthLook::Bubbles),
    (Theme::Tide, DepthLook::Sparkle),
    (Theme::Mirror, DepthLook::Glints),
    (Theme::Classic, DepthLook::Meters),
];

/// Board #5543: the beams, the coals and the grass over the known
/// surfaces match `depth_light::fragment`, quiet and loud, and under a
/// sweep `depth_light::fragment_swept`. Each theme's row names the look,
/// and the look is built.
#[test]
#[ignore = "needs a GPU adapter"]
fn each_look_matches_its_cpu_mirror() {
    let (device, queue, _guard) = gpu();
    let opts = SonarOptions::default();
    let sweep = one_sweep(&opts, eye().eye_pos, 1.25).rows(&opts, 1.0);
    for (theme, look) in LOOKS {
        assert_eq!(theme.depth().look, look);
        assert_eq!(
            depth_light::look(Some(theme)),
            look,
            "{} is built",
            look.name()
        );
        for (music, gust) in [(0.1, 0.0), (0.9, 0.7)] {
            let rows = look_rows(theme, look, 1.0, music, gust, 3.7);
            assert_eq!(depth_light::look_of(&rows), look);
            eprintln!("{} at music {music}, gust {gust}", look.name());
            // Of 739 200 pixels a case. More than the points' handful:
            // where two points' channels meet, a streak's phase and rate
            // or a blade's height step, not only a twinkle. A bubble's
            // lane is wider than a point, so two lanes meet more often
            // and the seam between them, where the phase is two points'
            // mixed, crosses lit bubbles.
            let seams = if look == DepthLook::Bubbles { 2 } else { 1 };
            fragment_matches(device, queue, &rows, 200 * seams);
        }
        // The points on a top, as the looks of upright things drew them
        // until `debug.fosfora.depthlighttops` (off by default: nothing).
        if depth_light::upright_look(look) {
            eprintln!("{} with the points on its tops", look.name());
            let rows = depth_light::with_tops(look_rows(theme, look, 1.0, 0.9, 0.7, 3.7), true);
            let seams = if look == DepthLook::Bubbles { 2 } else { 1 };
            fragment_matches(device, queue, &rows, 200 * seams);
        }
        eprintln!("{} under a sweep", look.name());
        let rows = depth_light::with_sonar(look_rows(theme, look, 1.0, 0.5, 0.3, 3.7), &sweep);
        assert!(depth_light::swept(&rows));
        let seams = if look == DepthLook::Bubbles { 2 } else { 1 };
        fragment_matches(device, queue, &rows, 300 * seams);
    }
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_swept_fragment_matches_its_cpu_mirror() {
    let (device, queue, _guard) = gpu();
    let opts = SonarOptions::default();
    let origin = eye().eye_pos;
    // A sweep 1.25 m out, a build to full, then a second sweep 0.75 s
    // behind the first.
    let mut s = one_sweep(&opts, origin, 1.25);
    s = built(s, &opts, origin, 1.0, 0.75 - 1.0 / 72.0);
    s.step(
        &opts,
        sonar::Input {
            downbeat: true,
            buildup: 1.0,
            ..sonar_input(1.0 / 72.0, origin)
        },
    );
    let s = built(s, &opts, origin, 1.0, 0.5);
    assert_eq!(s.sweeps().count(), 2);
    assert!(s.contours().strength > 0.5);
    let rows = s.rows(&opts, 1.0);
    eprintln!(
        "sweeps {:.2} and {:.2} m out, standing lines at {:.2}",
        rows[2][3],
        rows[4][3],
        s.contours().strength
    );
    fragment_matches(
        device,
        queue,
        &depth_light::with_sonar(light_rows(), &rows),
        40,
    );
}

/// The sRGB byte of a linear value.
fn srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let v = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (v * 255.0).round() as u8
}

/// `light` (premultiplied, linear) over `room` at `room_share` of its
/// light, as a PPM.
fn write_ppm(
    path: &std::path::Path,
    light: &[[f32; 4]],
    room: &[[f32; 4]],
    room_share: f32,
    w: u32,
) {
    let mut out = format!("P6\n{w} {}\n255\n", light.len() / w as usize).into_bytes();
    for (l, r) in light.iter().zip(room) {
        for k in 0..3 {
            out.push(srgb(l[k] + room_share * r[k]));
        }
    }
    std::fs::write(path, out).expect("writing the picture");
}

/// A stand-in for the room behind the light, for the pictures with the
/// room half visible: the mesh shaded gray by its normal. A recording has
/// the depth and no camera image.
fn room_wgsl() -> String {
    format!(
        r"{ENV_DEPTH_WGSL}
@fragment
fn fs_room(in: LightOut) -> @location(0) vec4<f32> {{
    let nn = in.normal.xyz;
    let n = select(vec3<f32>(0.0, 0.0, 1.0), normalize(nn), dot(nn, nn) > 1e-12);
    let lit = 0.1 + 0.4 * clamp(dot(n, normalize(vec3<f32>(0.3, 0.8, 0.5))), 0.0, 1.0);
    return vec4<f32>(vec3<f32>(lit), 1.0);
}}
"
    )
}

/// The recorded room under the sweep, for the eye and for the line's
/// stability on the headset's own depth. `FOSFORA_SONAR_REC` names a
/// recording (`live_rec.rs`; they stay out of the repository) and
/// `FOSFORA_SONAR_DUMP` a folder for the pictures (PPM): the sweep at four
/// moments of one bar with the room black and half visible, and the
/// standing lines at a mid build. The view is the recording's own (the
/// read-back's 64 x 64 grid as the mesh, coarser than the headset's), the
/// target the faces layer's size, the theme Aurora's colors. Without a
/// room scan every surface carries the light, as in roam.
///
/// The stability: the line alone (no points, no tail) drawn over two
/// read-backs a quarter of a second apart with the sweep held still, and
/// how much of its light moved; beside it, how much moves when the shell
/// itself moves by half the line's half-width.
#[test]
#[ignore = "needs a GPU adapter and a depth recording"]
fn the_recorded_room_under_the_sweep() {
    let Some(path) = std::env::var_os("FOSFORA_SONAR_REC") else {
        eprintln!("FOSFORA_SONAR_REC is not set: nothing rendered");
        return;
    };
    let dump = std::env::var_os("FOSFORA_SONAR_DUMP").map(std::path::PathBuf::from);
    let bytes = std::fs::read(&path).expect("the recording reads");
    let rec = fosfora_xr::live_rec::decode(&bytes).expect("the recording decodes");
    let (device, queue, _guard) = gpu();
    let tri = wgpu::PrimitiveTopology::TriangleList;
    let target = [W, H];
    let side = rec.cols as u32;
    let scene_of = |k: usize| Scene {
        map: rec.records[k]
            .grid
            .depth
            .iter()
            .map(|&d| (d.clamp(0.0, 1.0) * 65535.0).round() as u16)
            .collect(),
        side,
        step: 1,
        target,
    };
    let eye_of = |k: usize| {
        let g = &rec.records[k].grid;
        EyeReprojection {
            view_proj: math::view_projection(
                g.view.orientation,
                g.view.position,
                g.view.fov,
                0.05,
                100.0,
            ),
            eye_pos: Vec3::from(g.view.position),
            depth_view: g.view.view_matrix(),
            depth_fov: g.view.fov,
            near: g.near,
            far: g.far,
            depth_size: [side, side],
            target_size: target,
            layer: 0,
            near_cut_m: NEAR_CUT_M,
            flip_v: rec.reading == fosfora_xr::env_depth::Reading::RowTop,
            filter_edge_m: FILTER_SOFT_M,
            mesh_step: 1,
        }
    };
    let render = |k: usize, light: &[[f32; 4]; ROWS], source: &str, fragment: &str| {
        let mut rows = eye_of(k).uniform();
        // The faces layer's pixels are twice the eye's.
        rows[LIGHT_ROW..].copy_from_slice(&depth_light::scale_fades(*light, 2.0));
        draw_scene(
            device,
            queue,
            &scene_of(k),
            source,
            "vs_mesh_light",
            fragment,
            tri,
            &rows,
            None,
            Over::Nothing,
        )
        .color
    };
    // A settled read-back, three quarters of the way in.
    let k = rec.records.len() * 3 / 4;
    let head = Vec3::from(rec.records[k].grid.view.position);
    eprintln!(
        "{}: {} read-backs of {side} x {side}, drawing number {k}, the head at {head}",
        std::path::Path::new(&path).display(),
        rec.records.len()
    );
    let colors = depth_light::colors(Some(Theme::Aurora), [0.7, 0.3, 1.0]);
    let light_at = |level: f32, points: bool, sonar: &[[f32; 4]; sonar::ROWS]| {
        let mut d = Drive::default();
        d.step(0.37 / 0.1, 0.5, 0.0);
        let peak = if points {
            depth_light::peak_alpha(level, 1.0)
        } else {
            0.0
        };
        depth_light::with_sonar(
            depth_light::rows(
                &d,
                peak,
                colors,
                depth_light::FILL,
                depth_light::veil_share(level, true),
            ),
            sonar,
        )
    };
    let opts = SonarOptions::default();
    let none = [[0.0f32; 4]; sonar::ROWS];
    let room = render(k, &light_at(1.0, true, &none), &room_wgsl(), "fs_room");
    let lit_px = |pixels: &[[f32; 4]]| pixels.iter().filter(|c| c[1] > 0.02).count();
    let shot = |name: &str, level: f32, sonar: &[[f32; 4]; sonar::ROWS]| {
        let light = light_at(level, true, sonar);
        let light = render(k, &light, ENV_DEPTH_WGSL, light_fragment(&light));
        eprintln!("{name}: {} pixels lit", lit_px(&light));
        if let Some(dir) = &dump {
            std::fs::create_dir_all(dir).expect("the folder");
            write_ppm(
                &dir.join(format!("{name}.ppm")),
                &light,
                &room,
                1.0 - level,
                W,
            );
        }
        light
    };
    // One bar at 120 BPM, a quarter of it apart: the sweep 0.75, 1.75,
    // 2.75 and 3.75 m out.
    let plain = shot("black-no-sweep", 1.0, &none);
    for (level, room_name) in [(1.0, "black"), (0.5, "half")] {
        for (n, radius) in [0.75f32, 1.75, 2.75, 3.75].into_iter().enumerate() {
            let rows = one_sweep(&opts, head, radius).rows(&opts, level);
            let got = shot(
                &format!("{room_name}-sweep-{}-{radius}m", n + 1),
                level,
                &rows,
            );
            if level > 0.9 {
                assert!(differ(&bits(&got), &bits(&plain)) > 1000);
            }
        }
        // A mid build: `buildup` 0.7, the second set in, lines 0.3 m apart.
        let lines = built(Sonar::default(), &opts, head, 0.7, 8.0);
        shot(
            &format!("{room_name}-contours-mid-build"),
            level,
            &lines.rows(&opts, level),
        );
        let mut both = lines;
        both.step(
            &opts,
            sonar::Input {
                downbeat: true,
                buildup: 0.7,
                ..sonar_input(1.0 / 72.0, head)
            },
        );
        let both = built(both, &opts, head, 0.7, 0.75);
        shot(
            &format!("{room_name}-contours-and-a-sweep-1.5m"),
            level,
            &both.rows(&opts, level),
        );
    }
    let half_off = shot("half-no-sweep", 0.5, &none);
    assert!(lit_px(&half_off) > 0);
    // A full build, for the tightest spacing.
    let tight = built(Sonar::default(), &opts, head, 1.0, 10.0);
    shot("black-contours-full-build", 1.0, &tight.rows(&opts, 1.0));

    // The stability: the lines alone over consecutive read-backs.
    let line_only = SonarOptions {
        tail_s: 0.0,
        ..opts
    };
    let alone = |k: usize, rows: &[[f32; 4]; sonar::ROWS]| {
        let light = light_at(1.0, false, rows);
        render(k, &light, ENV_DEPTH_WGSL, light_fragment(&light))
    };
    // The share of two pictures' light that differs, and their light.
    let moved = |a: &[[f32; 4]], b: &[[f32; 4]]| {
        let (mut diff, mut most) = (0.0f64, 0.0f64);
        for (x, y) in a.iter().zip(b) {
            diff += f64::from((x[1] - y[1]).abs());
            most += f64::from(x[1].max(y[1]));
        }
        (if most > 0.0 { diff / most } else { 0.0 }, most)
    };
    let first = rec.records.len() / 2;
    let pairs: Vec<usize> = (0..8).map(|n| first + n * 8).collect();
    eprintln!(
        "| lines | half-width | their light (green, summed over the picture) | share of it that moved between two read-backs (mean, worst of {}) | moved by a shift of a quarter, a half and one half-width |",
        pairs.len()
    );
    eprintln!("|---|---|---|---|---|");
    // `rows(shift)`: the lines to draw, `shift` meters further out. The
    // worst share moved between two read-backs, and the shares a shift of
    // a quarter, a half and one half-width moves.
    let measure = |what: &str, half: f32, rows: &dyn Fn(f32) -> [[f32; 4]; sonar::ROWS]| {
        let still = rows(0.0);
        let (mut sum, mut worst, mut light) = (0.0f64, 0.0f64, 0.0f64);
        for &k in &pairs {
            let (m, most) = moved(&alone(k, &still), &alone(k + 1, &still));
            sum += m;
            worst = worst.max(m);
            light += most;
        }
        let base = alone(first, &still);
        let shifted =
            [0.25, 0.5, 1.0].map(|share| moved(&base, &alone(first, &rows(share * half))).0);
        let n = pairs.len() as f64;
        eprintln!(
            "| {what} | {:.1} cm | {:.0} | {:.3}, {:.3} | {:.3}, {:.3}, {:.3} |",
            100.0 * half,
            light / n,
            sum / n,
            worst,
            shifted[0],
            shifted[1],
            shifted[2]
        );
        (worst, shifted)
    };
    for radius in [0.75f32, 1.75, 2.75, 3.75] {
        let half = sonar::half_width(sonar::HALF_WIDTH_M, radius, 1.0);
        measure(&format!("a sweep {radius} m out"), half, &|shift| {
            one_sweep(&line_only, head, radius + shift).rows(&line_only, 1.0)
        });
    }
    for (what, buildup) in [("standing, mid build", 0.7), ("standing, full build", 1.0)] {
        let lines = built(Sonar::default(), &opts, head, buildup, 10.0);
        let (worst, shifted) = measure(what, sonar::CONTOUR_HALF_WIDTH_M, &|shift| {
            let mut rows = lines.rows(&opts, 1.0);
            rows[8][3] += shift;
            rows
        });
        // The map's noise moves a standing line less than a shift of half
        // its half-width would.
        assert!(worst < shifted[1], "{what}: {worst} against {shifted:?}");
        // For the eye: the lines alone over two read-backs, and where
        // their light differs.
        if let Some(dir) = &dump {
            let rows = lines.rows(&opts, 1.0);
            let (a, b) = (alone(first, &rows), alone(first + 1, &rows));
            let differs: Vec<[f32; 4]> = a
                .iter()
                .zip(&b)
                .map(|(x, y)| [0, 1, 2, 3].map(|k| (x[k] - y[k]).abs()))
                .collect();
            let black = vec![[0.0f32; 4]; a.len()];
            let name = what.replace([',', ' '], "-").replace("--", "-");
            for (tag, pixels) in [("a", &a), ("b", &b), ("moved", &differs)] {
                write_ppm(
                    &dir.join(format!("lines-alone-{name}-{tag}.ppm")),
                    pixels,
                    &black,
                    0.0,
                    W,
                );
            }
        }
    }
}

/// The recorded room in each look, for the eye (board #5543).
/// `FOSFORA_LOOK_REC` names a depth recording (`live_rec.rs`; they stay
/// out of the repository) and `FOSFORA_LOOK_DUMP` a folder for the
/// pictures (PPM): the points and each look in its theme's
/// colors, quiet and loud, with the room black and with it visible (the
/// light at its lowest over the stand-in room), and each look a third of
/// a second on, to see what moved. The view and the target are the
/// sweep's pictures' (`the_recorded_room_under_the_sweep`).
#[test]
#[ignore = "needs a GPU adapter and a depth recording"]
fn the_recorded_room_in_each_look() {
    let Some(path) = std::env::var_os("FOSFORA_LOOK_REC") else {
        eprintln!("FOSFORA_LOOK_REC is not set: nothing rendered");
        return;
    };
    let dump = std::env::var_os("FOSFORA_LOOK_DUMP").map(std::path::PathBuf::from);
    let bytes = std::fs::read(&path).expect("the recording reads");
    let rec = fosfora_xr::live_rec::decode(&bytes).expect("the recording decodes");
    let (device, queue, _guard) = gpu();
    let tri = wgpu::PrimitiveTopology::TriangleList;
    let target = [W, H];
    let side = rec.cols as u32;
    let k = rec.records.len() * 3 / 4;
    let g = &rec.records[k].grid;
    let scene = Scene {
        map: g
            .depth
            .iter()
            .map(|&d| (d.clamp(0.0, 1.0) * 65535.0).round() as u16)
            .collect(),
        side,
        step: 1,
        target,
    };
    let eye = EyeReprojection {
        view_proj: math::view_projection(
            g.view.orientation,
            g.view.position,
            g.view.fov,
            0.05,
            100.0,
        ),
        eye_pos: Vec3::from(g.view.position),
        depth_view: g.view.view_matrix(),
        depth_fov: g.view.fov,
        near: g.near,
        far: g.far,
        depth_size: [side, side],
        target_size: target,
        layer: 0,
        near_cut_m: NEAR_CUT_M,
        flip_v: rec.reading == fosfora_xr::env_depth::Reading::RowTop,
        filter_edge_m: FILTER_SOFT_M,
        mesh_step: 1,
    };
    let render = |light: &[[f32; 4]; ROWS], source: &str, fragment: &str| {
        let mut rows = eye.uniform();
        // The faces layer's pixels are twice the eye's.
        rows[LIGHT_ROW..].copy_from_slice(&depth_light::scale_fades(*light, 2.0));
        draw_scene(
            device,
            queue,
            &scene,
            source,
            "vs_mesh_light",
            fragment,
            tri,
            &rows,
            None,
            Over::Nothing,
        )
        .color
    };
    let room = render(&light_rows(), &room_wgsl(), "fs_room");
    let sum =
        |pixels: &[[f32; 4]]| -> f64 { pixels.iter().map(|c| f64::from(c[0] + c[1] + c[2])).sum() };
    let shot = |name: String, light: &[[f32; 4]; ROWS], level: f32| {
        let got = render(light, ENV_DEPTH_WGSL, light_fragment(light));
        let lit = got
            .iter()
            .filter(|c| c[0].max(c[1]).max(c[2]) > 0.02)
            .count();
        eprintln!("{name}: {lit} pixels lit, light {:.0}", sum(&got));
        if let Some(dir) = &dump {
            std::fs::create_dir_all(dir).expect("the folder");
            write_ppm(
                &dir.join(format!("{name}.ppm")),
                &got,
                &room,
                1.0 - level,
                W,
            );
        }
        got
    };
    let mut cases = vec![(Theme::Aurora, DepthLook::Points)];
    cases.extend(LOOKS);
    for (theme, look) in cases {
        let name = |what: &str| format!("{}-{}-{what}", theme.name(), look.name());
        let quiet = shot(
            name("black-quiet"),
            &look_rows(theme, look, 1.0, 0.1, 0.1, 3.7),
            1.0,
        );
        let loud_rows = look_rows(theme, look, 1.0, 0.9, 0.8, 3.7);
        let loud = shot(name("black-loud"), &loud_rows, 1.0);
        let later = shot(
            name("black-loud-later"),
            &look_rows(theme, look, 1.0, 0.9, 0.8, 3.7 + 1.0 / 3.0),
            1.0,
        );
        let plain = shot(
            name("visible-loud"),
            &look_rows(theme, look, 0.0, 0.9, 0.8, 3.7),
            0.0,
        );
        // Every look draws, louder with the music, and still draws with
        // the room visible, at the lowest peak.
        assert!(
            sum(&quiet) > 0.0 && sum(&loud) > sum(&quiet),
            "{}",
            look.name()
        );
        assert!(
            sum(&plain) > 0.0 && sum(&plain) < sum(&loud),
            "{}",
            look.name()
        );
        // A third of a second on the look has moved, but for the meters:
        // they stand where their bands' levels are, and those are held.
        let moved = differ(&bits(&loud), &bits(&later));
        if look == DepthLook::Meters {
            assert_eq!(moved, 0, "{}", look.name());
        } else {
            assert!(moved > 1000, "{}", look.name());
        }
    }
}
