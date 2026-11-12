//! Geometry first (board #5416) on a desktop GPU: the explain pass
//! (`EXPLAIN_WGSL`) and the keep pass (`KEEP_WGSL`) run headless on a
//! synthetic room's depth map and compared, vertex by vertex, with their
//! CPU mirrors (`Explain::values`, `Keep::drops`); and the occluder mesh
//! with the drop (`mesh_geom_wgsl`) rendered against the occluder's own
//! (`ENV_DEPTH_WGSL`'s `vs_mesh`). `#[ignore]`d like the other GPU tests
//! (they need an adapter); run them with
//! `cargo test -p fosfora-xr --test geom_first_gpu -- --ignored`.
#![cfg(not(target_os = "android"))]

use std::sync::{Mutex, MutexGuard, OnceLock};

use fosfora_xr::env_depth::{
    DepthView, ENV_DEPTH_WGSL, EyeReprojection, FILTER_SOFT_M, NEAR_CUT_M, Obb, UNIFORM_ROWS,
    faces_mesh_step, mesh_indices, mesh_side, ray_obb,
};
use fosfora_xr::geom_first::{
    EXPLAIN_WGSL, Explain, ExplainLayer, KEEP_WGSL, Keep, MARGIN_TEXELS, TOL_M, mesh_geom_wgsl,
    obstacle_rows, set_drop_vertices,
};
use fosfora_xr::math::{self, Fov};
use glam::{Quat, Vec3};

/// The depth map's side on the Quest 3 (v207).
const MAP: u32 = 320;
const NEAR: f32 = 0.1;

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
            label: Some("xr-geom-first-test"),
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

fn uniform(device: &wgpu::Device, queue: &wgpu::Queue, rows: &[[f32; 4]]) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("uniform"),
        size: (rows.len() * 16) as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, bytemuck::cast_slice(rows));
    buffer
}

fn storage(device: &wgpu::Device, bytes: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("storage"),
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Both layers of a `MAP` x `MAP` D16 map as a texture array view.
fn depth_map(device: &wgpu::Device, queue: &wgpu::Queue, map: &[u16]) -> wgpu::TextureView {
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
    queue.write_texture(
        texture.as_image_copy(),
        bytemuck::cast_slice(map),
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

fn compute(device: &wgpu::Device, source: &str) -> wgpu::ComputePipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("geom-first"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("geom-first"),
        layout: None,
        module: &module,
        entry_point: Some("cs_main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    })
}

fn group(
    device: &wgpu::Device,
    pipeline: &wgpu::ComputePipeline,
    resources: &[wgpu::BindingResource<'_>],
) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry<'_>> = resources
        .iter()
        .enumerate()
        .map(|(i, r)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: r.clone(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("geom-first"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    })
}

fn dispatch(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    group: &wgpu::BindGroup,
    grid: [u32; 2],
) {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.dispatch_workgroups(grid[0].div_ceil(8), grid[1].div_ceil(8), 2);
    }
    queue.submit([encoder.finish()]);
    wait(device);
}

const FOV: Fov = Fov {
    left: -0.9,
    right: 0.8,
    up: 0.85,
    down: -0.9,
};

/// The two depth cameras: a head 1.5 m up looking ahead and 20 degrees
/// down, the layers 6.4 cm apart, the right one's fov mirrored.
fn views() -> [DepthView; 2] {
    let rot = Quat::from_rotation_y(0.1) * Quat::from_rotation_x(-0.35);
    let right = Fov {
        left: -0.8,
        right: 0.9,
        ..FOV
    };
    [
        DepthView {
            orientation: rot.to_array(),
            position: [-0.032, 1.5, 0.0],
            fov: FOV,
        },
        DepthView {
            orientation: rot.to_array(),
            position: [0.032, 1.5, 0.0],
            fov: right,
        },
    ]
}

/// The room's boxes as the app has them: a wall, the stage floor, a
/// turned table, a turned side wall and a shelf.
fn boxes() -> Vec<Obb> {
    vec![
        Obb {
            center: Vec3::new(0.0, 1.5, -3.02),
            rot: Quat::IDENTITY,
            half: Vec3::new(3.0, 1.5, 0.02),
        },
        Obb {
            center: Vec3::new(0.0, -0.05, 0.0),
            rot: Quat::IDENTITY,
            half: Vec3::new(10.0, 0.05, 10.0),
        },
        Obb {
            center: Vec3::new(0.4, 0.375, -1.6),
            rot: Quat::from_rotation_y(0.3),
            half: Vec3::new(0.5, 0.375, 0.3),
        },
        Obb {
            center: Vec3::new(-1.8, 1.2, -1.5),
            rot: Quat::from_rotation_y(1.2),
            half: Vec3::new(1.0, 1.2, 0.02),
        },
        Obb {
            center: Vec3::new(1.2, 1.1, -2.85),
            rot: Quat::from_rotation_y(-0.2),
            half: Vec3::new(0.4, 0.02, 0.12),
        },
    ]
}

/// The camera-space ray through texel (x, row)'s center at z = -1, row 0
/// the bottom of the view (the map's order, `envdepthflipv 0`).
fn texel_ray(fov: Fov, x: u32, row: u32) -> Vec3 {
    let (u, v) = (
        (x as f32 + 0.5) / MAP as f32,
        (row as f32 + 0.5) / MAP as f32,
    );
    let (l, r) = (fov.left.tan(), fov.right.tan());
    let (up, down) = (fov.up.tan(), fov.down.tan());
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

/// The room the depth camera sees: the boxes as they really are (the
/// table 2 cm off its scan and 1 cm bigger), plus what no box explains (a
/// person, a laptop on the table), holes without data, texels nearer than
/// the discard distance, and at every silhouette the runtime's blend (a
/// texel next to a jump takes the mean of its 3x3 neighborhood). Stored
/// as D16 under an infinite projection at `NEAR`, layer-major, row 0
/// first.
fn synthetic_map() -> Vec<u16> {
    let mut real = boxes();
    real[2].center += Vec3::new(0.02, 0.0, 0.01);
    real[2].half += Vec3::splat(0.01);
    let laptop = Obb {
        center: Vec3::new(0.3, 0.86, -1.5),
        rot: Quat::from_rotation_y(0.3),
        half: Vec3::new(0.17, 0.11, 0.02),
    };
    let mut map = Vec::with_capacity((2 * MAP * MAP) as usize);
    for view in views() {
        let rot = Quat::from_array(view.orientation);
        let o = Vec3::from(view.position);
        let mut dist = vec![f32::INFINITY; (MAP * MAP) as usize];
        for row in 0..MAP {
            for x in 0..MAP {
                let d = rot * texel_ray(view.fov, x, row);
                let hit = real
                    .iter()
                    .chain(std::iter::once(&laptop))
                    .filter_map(|b| ray_obb(o, d, b))
                    .chain(ray_sphere(o, d, Vec3::new(-0.3, 1.0, -1.2), 0.25))
                    .min_by(f32::total_cmp);
                if let Some(t) = hit {
                    dist[(row * MAP + x) as usize] = t;
                }
            }
        }
        // The blend at silhouettes.
        let at = |x: i32, y: i32| {
            let (x, y) = (x.clamp(0, MAP as i32 - 1), y.clamp(0, MAP as i32 - 1));
            dist[(y as u32 * MAP + x as u32) as usize]
        };
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
                // Holes without data and texels too near.
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
    }
    map
}

fn explain(step: u32, flip_v: bool) -> Explain {
    Explain {
        layers: views().map(|v| ExplainLayer::of(&v)),
        near: NEAR,
        far: f32::INFINITY,
        near_cut_m: NEAR_CUT_M,
        size: [MAP, MAP],
        flip_v,
        step,
        boxes_on: true,
        margin_texels: MARGIN_TEXELS,
        tol_m: TOL_M,
    }
}

/// The explain pass over `map` on the GPU: the values buffer and the
/// values read back.
fn run_explain(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    map: &[u16],
    e: &Explain,
    boxes: &[Obb],
) -> (wgpu::Buffer, Vec<f32>) {
    let view = depth_map(device, queue, map);
    let pipeline = compute(device, EXPLAIN_WGSL);
    let obstacles = uniform(device, queue, &obstacle_rows(boxes));
    let params = uniform(device, queue, &e.rows());
    let [nx, ny] = e.grid();
    let values = storage(device, u64::from(2 * nx * ny) * 4);
    let g = group(
        device,
        &pipeline,
        &[
            wgpu::BindingResource::TextureView(&view),
            obstacles.as_entire_binding(),
            params.as_entire_binding(),
            values.as_entire_binding(),
        ],
    );
    dispatch(device, queue, &pipeline, &g, [nx, ny]);
    let back = read_buffer(device, queue, &values);
    (values, bytemuck::pod_collect_to_vec::<u8, f32>(&back))
}

/// The keep pass for `k` on the GPU over `values`, read back.
fn run_keep(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    values: &wgpu::Buffer,
    k: &Keep,
) -> Vec<bool> {
    let pipeline = compute(device, KEEP_WGSL);
    let params = uniform(device, queue, &k.rows());
    let drops = storage(device, u64::from(2 * k.grid[0] * k.grid[1]) * 4);
    let g = group(
        device,
        &pipeline,
        &[
            values.as_entire_binding(),
            drops.as_entire_binding(),
            params.as_entire_binding(),
        ],
    );
    dispatch(device, queue, &pipeline, &g, k.grid);
    let back = read_buffer(device, queue, &drops);
    bytemuck::pod_collect_to_vec::<u8, u32>(&back)
        .into_iter()
        .map(|d| d != 0)
        .collect()
}

/// Whether the CPU's verdict on vertex `i` flips when the margins move
/// by a hair: a vertex on a decision's edge, where the GPU's rounding may
/// fall either way.
fn on_an_edge(e: &Explain, boxes: &[Obb], map: &[u16], i: usize) -> bool {
    let [nx, ny] = e.grid();
    let layer = i / (nx * ny) as usize;
    let k = [
        (i % nx as usize) as u32,
        ((i / nx as usize) % ny as usize) as u32,
    ];
    let t = [
        fosfora_xr::env_depth::mesh_texel(k[0], MAP, e.step),
        fosfora_xr::env_depth::mesh_texel(k[1], MAP, e.step),
    ];
    let d = f32::from(map[(layer as u32 * MAP * MAP + t[1] * MAP + t[0]) as usize]) / 65535.0;
    let nudged = |s: f32| Explain {
        margin_texels: e.margin_texels + s * 1e-3,
        tol_m: e.tol_m + s * 1e-4,
        ..*e
    };
    let (lo, hi) = (nudged(-1.0), nudged(1.0));
    (lo.value(layer, k, d, boxes) < 0.0) != (hi.value(layer, k, d, boxes) < 0.0)
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_explain_pass_matches_its_cpu_mirror() {
    let (device, queue, _guard) = gpu();
    let map = synthetic_map();
    let boxes = boxes();
    for (step, flip_v) in [(2, false), (1, false), (4, true)] {
        let e = explain(step, flip_v);
        let (_, gpu) = run_explain(device, queue, &map, &e, &boxes);
        let cpu = e.values(&boxes, |layer, t| {
            f32::from(map[(layer as u32 * MAP * MAP + t[1] * MAP + t[0]) as usize]) / 65535.0
        });
        assert_eq!(gpu.len(), cpu.len());
        let (mut explained, mut unexplained, mut empty, mut edges) = (0, 0, 0, 0);
        for (i, (&g, &c)) in gpu.iter().zip(&cpu).enumerate() {
            // No data agrees exactly; the range within float noise.
            assert_eq!(g == 0.0, c == 0.0, "step {step} vertex {i}: {g} vs {c}");
            assert!(
                (g.abs() - c.abs()).abs() <= 1e-4 * c.abs().max(1.0),
                "step {step} vertex {i}: {g} vs {c}"
            );
            if (g < 0.0) != (c < 0.0) {
                assert!(
                    on_an_edge(&e, &boxes, &map, i),
                    "step {step} vertex {i}: the GPU says {g}, the CPU {c}, not on a decision's edge"
                );
                edges += 1;
            }
            match c {
                c if c < 0.0 => explained += 1,
                c if c > 0.0 => unexplained += 1,
                _ => empty += 1,
            }
        }
        eprintln!(
            "step {step}: {explained} explained, {unexplained} not, {empty} without data, {edges} on an edge"
        );
        // Every case is there in numbers, and the GPU and the CPU part on
        // a handful at most.
        assert!(explained > cpu.len() / 3, "{explained}");
        assert!(unexplained > cpu.len() / 50, "{unexplained}");
        assert!(empty > cpu.len() / 50, "{empty}");
        assert!(edges * 1000 <= cpu.len(), "{edges}");
    }
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_keep_pass_matches_its_cpu_mirror() {
    let (device, queue, _guard) = gpu();
    let map = synthetic_map();
    let boxes = boxes();
    for eye in [1, 2, 4, 8] {
        let e = explain(eye, false);
        let (values, gpu_values) = run_explain(device, queue, &map, &e, &boxes);
        for step in [eye, faces_mesh_step(eye)] {
            for keep in [true, false] {
                let k = Keep::of([MAP, MAP], eye, step, keep).expect("a multiple");
                let gpu = run_keep(device, queue, &values, &k);
                // From the GPU's own values: the comparisons are exact.
                let cpu = k.drops(&gpu_values);
                assert_eq!(gpu.len(), cpu.len());
                let differ = gpu.iter().zip(&cpu).filter(|(g, c)| g != c).count();
                assert_eq!(differ, 0, "eye {eye} step {step} keep {keep}");
                let dropped = cpu.iter().filter(|&&d| d).count();
                eprintln!(
                    "eye step {eye}, mesh step {step}, keep rule {keep}: {dropped} of {} dropped",
                    cpu.len()
                );
                assert!(dropped > 0);
            }
        }
    }
}

/// The eye pass's depth for the occluder mesh drawn from `module`'s
/// `entry` with `drops` (one per vertex of the layer's mesh, both layers)
/// bound when the module takes them, through layer 0's uniform with the
/// drop's vertices per layer set to `drop_vertices`.
fn render_mesh(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    map: &[u16],
    source: &str,
    entry: &str,
    drops: Option<&[u32]>,
    drop_vertices: u32,
) -> Vec<f32> {
    const W: u32 = 420;
    const H: u32 = 440;
    let view = depth_map(device, queue, map);
    let v = views()[0];
    let step = 2;
    let r = EyeReprojection {
        view_proj: math::view_projection(v.orientation, [-0.032, 1.5, 0.01], FOV, 0.05, 100.0),
        eye_pos: Vec3::new(-0.032, 1.5, 0.01),
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
        mesh_step: step,
    };
    let mut rows: [[f32; 4]; UNIFORM_ROWS] = r.uniform();
    set_drop_vertices(&mut rows, drop_vertices);
    let eye = uniform(device, queue, &rows);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("geom-first-mesh"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let depth_format = wgpu::TextureFormat::Depth32Float;
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("geom-first-mesh"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
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
            entry_point: Some("fs_mesh"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::empty(),
            })],
        }),
        multiview: None,
        cache: None,
    });
    let drops_buffer = drops.map(|d| {
        let b = storage(device, (d.len() * 4) as u64);
        queue.write_buffer(&b, 0, bytemuck::cast_slice(d));
        b
    });
    let mut entries = vec![
        wgpu::BindGroupEntry {
            binding: 0,
            resource: eye.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 1,
            resource: wgpu::BindingResource::TextureView(&view),
        },
    ];
    if let Some(b) = &drops_buffer {
        entries.push(wgpu::BindGroupEntry {
            binding: 2,
            resource: b.as_entire_binding(),
        });
    }
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("geom-first-mesh"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    });
    use wgpu::util::DeviceExt as _;
    let indices = mesh_indices([MAP, MAP], step);
    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("geom-first-mesh"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    let extent = wgpu::Extent3d {
        width: W,
        height: H,
        depth_or_array_layers: 1,
    };
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("color"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: depth_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let row_bytes = (W * 4).next_multiple_of(256);
    let back = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("depth-back"),
        size: u64::from(row_bytes * H),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("geom-first-mesh"),
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
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..indices.len() as u32, 0, 0..1);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &depth,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::DepthOnly,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &back,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(H),
            },
        },
        extent,
    );
    queue.submit([encoder.finish()]);
    let bytes = read_buffer(device, queue, &back);
    bytes
        .chunks(row_bytes as usize)
        .flat_map(|row| bytemuck::pod_collect_to_vec::<u8, f32>(&row[..(W * 4) as usize]))
        .collect()
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_mesh_with_the_drop_off_is_the_occluder_and_on_hides_less() {
    let (device, queue, _guard) = gpu();
    let map = synthetic_map();
    let boxes = boxes();
    let side = mesh_side(MAP, 2);
    let none = vec![0u32; (2 * side * side) as usize];
    let own = render_mesh(device, queue, &map, ENV_DEPTH_WGSL, "vs_mesh", None, 0);
    let geom = mesh_geom_wgsl();
    // The drop off (mesh.z 0), or on with nothing dropped: bit for bit the
    // occluder's own depth.
    for (drops, per_layer) in [(&none, 0), (&none, side * side)] {
        let got = render_mesh(
            device,
            queue,
            &map,
            &geom,
            "vs_mesh_geom",
            Some(drops),
            per_layer,
        );
        let differ = got
            .iter()
            .zip(&own)
            .filter(|(a, b)| a.to_bits() != b.to_bits())
            .count();
        assert_eq!(differ, 0, "mesh.z {per_layer}");
    }
    // The real drops: the eye pass's keep over the explain pass's values.
    let e = explain(2, false);
    let (values, _) = run_explain(device, queue, &map, &e, &boxes);
    let k = Keep::of([MAP, MAP], 2, 2, true).expect("same step");
    let drops: Vec<u32> = run_keep(device, queue, &values, &k)
        .into_iter()
        .map(u32::from)
        .collect();
    let got = render_mesh(
        device,
        queue,
        &map,
        &geom,
        "vs_mesh_geom",
        Some(&drops),
        side * side,
    );
    // Only ever less depth written: a pixel is the occluder's own or the
    // cleared 1, never nearer.
    let (mut same, mut cleared, mut farther) = (0, 0, 0);
    for (&g, &o) in got.iter().zip(&own) {
        if g.to_bits() == o.to_bits() {
            same += 1;
        } else if g.to_bits() == 1f32.to_bits() {
            cleared += 1;
        } else {
            assert!(g > o, "the drop wrote a nearer depth: {g} under {o}");
            farther += 1;
        }
    }
    eprintln!(
        "{same} pixels the occluder's own, {cleared} cleared by the drop, {farther} farther (a fold now shows what was behind it)"
    );
    assert!(cleared > got.len() / 4, "{cleared}");
    assert!(same > got.len() / 20, "{same}");
}
