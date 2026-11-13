//! The depth light (board #5439) on a desktop GPU: the occluder mesh drawn
//! with the light over a synthetic seated view (a person, a vacuum's
//! handle, a box, the floor and a wall, holes without data and texels too
//! near) and compared with the depth-only occluder and with the Rust
//! copies. The light's vertex stages write the occluder's depth bit for
//! bit, with geometry first's drop too; each vertex's keep, light by
//! distance, normal and height match `depth_light::vertex_light`; the
//! fragment over known inputs matches `depth_light::fragment`, its points,
//! octaves and fades included; and over a sky drawn first, the veil leaves
//! what `depth_light::veil` says at each level. `#[ignore]`d like the other
//! GPU tests (they need an adapter); run them with
//! `cargo test -p fosfora-xr --test depth_light_gpu -- --ignored`.
#![cfg(not(target_os = "android"))]

use std::sync::{Mutex, MutexGuard, OnceLock};

use fosfora_xr::depth_light::{self, Drive, ROWS, fragment, veil, vertex_light};
use fosfora_xr::env_depth::{
    DepthView, ENV_DEPTH_WGSL, EyeReprojection, FILTER_SOFT_M, LIGHT_ROW, NEAR_CUT_M, Obb,
    UNIFORM_ROWS, mesh_indices, mesh_side, mesh_texel, ray_obb,
};
use fosfora_xr::geom_first::{mesh_geom_wgsl, set_drop_vertices};
use fosfora_xr::math::{self, Fov};
use fosfora_xr::surfaces::TEAL;
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

/// Both layers of a `MAP` x `MAP` D16 map (the same picture in each) as a
/// texture array view.
fn depth_map(device: &wgpu::Device, queue: &wgpu::Queue, layer: &[u16]) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width: MAP,
        height: MAP,
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
            bytes_per_row: Some(MAP * 2),
            rows_per_image: Some(MAP),
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
fn uniform_with(light: [[f32; 4]; ROWS], drop_vertices: u32) -> [[f32; 4]; UNIFORM_ROWS] {
    let mut rows = eye().uniform();
    rows[LIGHT_ROW..].copy_from_slice(&light);
    set_drop_vertices(&mut rows, drop_vertices);
    rows
}

fn uniform_rows(drop_vertices: u32) -> [[f32; 4]; UNIFORM_ROWS] {
    uniform_with(light_rows(), drop_vertices)
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

/// What a draw drew: each pixel's RGBA (row-major, `W` x `H`) and depth.
struct Drawn {
    color: Vec<[f32; 4]>,
    depth: Vec<f32>,
}

/// One draw from `source`'s `vertex` and `fragment` over the synthetic
/// map, into a cleared float target and depth (compare `LessEqual`,
/// written, as the occluder): the mesh's triangles at `STEP`, its vertices
/// as points (`points`), or 3 vertices with no index buffer (`vertices`).
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
    let map = synthetic_map();
    let view = depth_map(device, queue, &map);
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
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("depth-light"),
        layout: &layout,
        entries: &entries,
    });
    use wgpu::util::DeviceExt as _;
    let indices = mesh_indices([MAP, MAP], STEP);
    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("depth-light-mesh"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    let extent = wgpu::Extent3d {
        width: W,
        height: H,
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
    let color_bytes = (W * texel).next_multiple_of(256);
    let depth_bytes = (W * 4).next_multiple_of(256);
    let read = |row_bytes: u32| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("back"),
            size: u64::from(row_bytes * H),
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
                let side = mesh_side(MAP, STEP);
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
                    rows_per_image: Some(H),
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
                bytemuck::pod_collect_to_vec::<u8, u16>(&row[..(W * 8) as usize])
                    .chunks(4)
                    .map(|c| [0, 1, 2, 3].map(|k| half(c[k])))
                    .collect::<Vec<_>>()
            })
            .collect()
    } else {
        rows_of(&color_back, color_bytes, W * 16)
            .chunks(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect()
    };
    Drawn {
        color,
        depth: rows_of(&depth_back, depth_bytes, W * 4),
    }
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
            let rows = uniform_with(light_rows_at(peak, v), 0);
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
    let rows = uniform_with(light_rows(), 0);
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
    let rows = uniform_rows(0);
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
    let r = eye();
    let light = light_rows();
    let side = mesh_side(MAP, STEP);
    let want = |i: u32| {
        let texel = [
            mesh_texel(i % side, MAP, STEP),
            mesh_texel(i / side, MAP, STEP),
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
        "{compared} vertices compared ({full} keep all, {part} part, {far} dimmed by distance): worst keep {worst_keep:.2e}, light {worst_bright:.2e}, height {worst_y:.2e} m, normal {worst_n:.2e}"
    );
    assert!(compared > 5000, "{compared}");
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
    let rows = uniform_rows(0);
    let light = light_rows();
    let eye_pos = eye().eye_pos;
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
            "fs_mesh_light",
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
                let want = fragment(&light, eye_pos, p, n, 0.5 + 0.5 * ny, 0.6 + 0.4 * nx, fw);
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
        // A pixel on a point's edge where the GPU's division put it in
        // the next cell's window sees the same points; allow a handful of
        // the hash's float edges, nothing more.
        assert!(bad <= (W * H / 20000) as usize, "{bad}");
    }
}
