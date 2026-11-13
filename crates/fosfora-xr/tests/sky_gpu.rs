//! The sky (board #5436) on a desktop GPU: `sky::SKY_WGSL`'s `vs_sky` and
//! `fs_sky` over a probe effect that returns its frame coordinate, drawn
//! on a scanned room's faces from a seated eye off the room's center, and
//! compared pixel by pixel with the fragment's Rust copy (`sky::sample`)
//! at the point of the face under each pixel. That checks what the
//! desktop unit tests cannot: the world point and the eye as the stages
//! hand them on, the mapping, the seam's cross-fade, the zenith fade, the
//! reflection's ray and its fades, the rim per edge and the early out, as
//! the shader runs them; and that `fs_sky` mapping per face writes what
//! `fs_main` writes. `#[ignore]`d like the other GPU tests (they need an
//! adapter); run it with `cargo test -p fosfora-xr --test sky_gpu --
//! --ignored`.
#![cfg(not(target_os = "android"))]

use std::f32::consts::FRAC_PI_2;

use fosfora_xr::math::{self, Fov};
use fosfora_xr::sky::{self, Map, RESOLUTION, Room, Sky};
use fosfora_xr::surface_fx::LIFT_M;
use fosfora_xr::surface_port::{self, FIRST_ID, UNIFORM_ROWS, WRAPPER_WGSL, port_face};
use fosfora_xr::surfaces::{
    Face, KIND_CEILING, KIND_FLOOR, KIND_WALL, PLANE_HALF_THICKNESS_M, acting_face,
};
use glam::{Mat4, Quat, Vec2, Vec3};

const W: u32 = 512;
const H: u32 = 512;

/// A stand-in for an effect: the uniform's resolution and an
/// `effect_main` that returns its frame coordinate over the resolution,
/// scaled into 0..1 for a frame from -2 to 3 (the seam's other end and the
/// floor under the horizon lie outside 0..1), at full alpha.
const PROBE: &str = "
struct U { resolution: vec2<f32> }
@group(0) @binding(0) var<uniform> u: U;
fn effect_main(frag_coord: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(frag_coord.xy / u.resolution * 0.2 + 0.4, 0.0, 1.0);
}
";

/// The probe's color at frame coordinate `at` (the WGSL above).
fn probe(at: Vec2, res: [f32; 2]) -> Vec2 {
    at / Vec2::from(res) * 0.2 + 0.4
}

fn gpu() -> (wgpu::Device, wgpu::Queue) {
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
        label: Some("xr-sky-test"),
        required_features: wgpu::Features::empty(),
        required_limits: adapter.limits(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("no wgpu device")
}

fn uniform(device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("uniform"),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, bytes);
    buffer
}

fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

/// The probe through the port's `entries` over `rows` at `res`, from a
/// camera `view_proj`, into a float target cleared to -1: each pixel's
/// premultiplied RGBA.
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    entries: (&str, &str),
    view_proj: Mat4,
    rows: &[[f32; 4]; UNIFORM_ROWS],
    res: [f32; 2],
) -> Vec<[f32; 4]> {
    let source = format!("{PROBE}{WRAPPER_WGSL}{}", sky::SKY_WGSL);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sky"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let format = wgpu::TextureFormat::Rgba32Float;
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sky"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some(entries.0),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some(entries.1),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    });
    let u = uniform(device, queue, bytemuck::cast_slice(&res));
    let eye = uniform(
        device,
        queue,
        bytemuck::cast_slice(&view_proj.to_cols_array()),
    );
    let port = uniform(device, queue, bytemuck::cast_slice(rows));
    let group0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sky-effect"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[entry(0, &u)],
    });
    let group1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sky-port"),
        layout: &pipeline.get_bind_group_layout(1),
        entries: &[entry(0, &eye), entry(1, &port)],
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
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let row_bytes = (W * 16).next_multiple_of(256);
    let back = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("color-back"),
        size: u64::from(row_bytes * H),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sky"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: -1.0,
                        g: -1.0,
                        b: -1.0,
                        a: -1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group0, &[]);
        pass.set_bind_group(1, &group1, &[]);
        pass.draw(0..6, 0..1);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
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
    back.slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("poll");
    let bytes = back.slice(..).get_mapped_range().to_vec();
    bytes
        .chunks(row_bytes as usize)
        .flat_map(|row| bytemuck::pod_collect_to_vec::<u8, f32>(&row[..(W * 16) as usize]))
        .collect::<Vec<f32>>()
        .chunks_exact(4)
        .map(|p| [p[0], p[1], p[2], p[3]])
        .collect()
}

/// A 4.6 by 3.8 m room turned 20°, its center at (0.4, -0.3), 2.45 m
/// high, every plane 2 cm thick: the walls, the floor, the ceiling.
fn room() -> Vec<(u32, Vec3, Quat, Vec3)> {
    let (w, d, h) = (4.6, 3.8, 2.45);
    let r = Quat::from_rotation_y(0.35);
    let c = Vec3::new(0.4, 0.0, -0.3);
    let t = PLANE_HALF_THICKNESS_M;
    let wall = |out: Vec3, turn: f32, len: f32| {
        (
            KIND_WALL,
            c + r * out + Vec3::Y * 0.5 * h,
            r * Quat::from_rotation_y(turn),
            Vec3::new(0.5 * len, 0.5 * h, t),
        )
    };
    vec![
        wall(Vec3::X * 0.5 * w, -FRAC_PI_2, d),
        wall(Vec3::NEG_X * 0.5 * w, FRAC_PI_2, d),
        wall(Vec3::Z * 0.5 * d, std::f32::consts::PI, w),
        wall(Vec3::NEG_Z * 0.5 * d, 0.0, w),
        (
            KIND_FLOOR,
            c,
            r * Quat::from_rotation_x(-FRAC_PI_2),
            Vec3::new(0.5 * w, 0.5 * d, t),
        ),
        (
            KIND_CEILING,
            c + Vec3::Y * h,
            r * Quat::from_rotation_x(FRAC_PI_2),
            Vec3::new(0.5 * w, 0.5 * d, t),
        ),
    ]
}

/// What one view compared.
#[derive(Debug, Default)]
struct Tally {
    compared: usize,
    lit: usize,
    mixed: usize,
    faded: usize,
    dark: usize,
    worst: f32,
}

/// `face` drawn with `rows` from `eye` looking at `target`, compared with
/// `sky::sample` under every pixel clear of the face's border.
fn compare(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    face: &Face,
    rows: &[[f32; 4]; UNIFORM_ROWS],
    eye: Vec3,
    target: Vec3,
) -> Tally {
    let fov = Fov {
        left: -0.7,
        right: 0.7,
        up: 0.7,
        down: -0.7,
    };
    let look = Quat::from_rotation_arc(Vec3::NEG_Z, (target - eye).normalize());
    let view_proj = math::view_projection(look.to_array(), eye.to_array(), fov, 0.05, 100.0);
    let got = render(
        device,
        queue,
        ("vs_sky", "fs_sky"),
        view_proj,
        rows,
        RESOLUTION,
    );
    let inverse = view_proj.inverse();
    // The eye as the vertex stage solves it from the camera.
    let solved = fosfora_xr::edge_bars::eye_position(view_proj);
    assert!(solved.distance(eye) < 1e-4, "{solved} {eye}");
    let lifted = face.center + face.normal * LIFT_M;
    let mut t = Tally::default();
    for py in 0..H {
        for px in 0..W {
            let ndc = Vec3::new(
                (px as f32 + 0.5) / W as f32 * 2.0 - 1.0,
                1.0 - (py as f32 + 0.5) / H as f32 * 2.0,
                0.5,
            );
            let dir = inverse.project_point3(ndc) - eye;
            let along = face.normal.dot(dir);
            if along.abs() < 1e-6 {
                continue;
            }
            let p = eye + dir * (face.normal.dot(lifted - eye) / along);
            let uv = Vec2::new(
                (p - face.center).dot(face.axes[0]),
                (p - face.center).dot(face.axes[1]),
            );
            // On the quad, a centimeter clear of its border, in front of
            // the eye.
            if uv.x.abs() > face.half[0] - 0.01
                || uv.y.abs() > face.half[1] - 0.01
                || (p - eye).dot(dir) <= 0.0
            {
                continue;
            }
            let want = match sky::sample(rows, RESOLUTION, uv, p, solved) {
                None => {
                    t.dark += 1;
                    [0.0; 4]
                }
                Some(s) => {
                    let c = match s.other {
                        Some((o, w)) => {
                            t.mixed += 1;
                            probe(s.at, RESOLUTION).lerp(probe(o, RESOLUTION), w)
                        }
                        None => probe(s.at, RESOLUTION),
                    };
                    let full = rows[6][3] * rows[5][0] * sky::rim(uv, face.half, rows[8]);
                    if s.k < 0.99 * full {
                        t.faded += 1;
                    }
                    let c = c.clamp(Vec2::ZERO, Vec2::ONE);
                    [c.x * s.k, c.y * s.k, 0.0, s.k]
                }
            };
            let g = got[(py * W + px) as usize];
            let err = g
                .iter()
                .zip(want)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            // The rim's 5 cm smoothstep turns the micrometers between the
            // interpolated and the unprojected point, seen at a slant,
            // into about 2e-4 of light; elsewhere they agree to 1e-5.
            assert!(
                err < 5e-4,
                "pixel ({px}, {py}) at {p}: GPU {g:?} vs Rust {want:?}"
            );
            t.worst = t.worst.max(err);
            t.compared += 1;
            if want[3] > 0.01 {
                t.lit += 1;
            }
        }
    }
    t
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_sky_shader_matches_its_rust_copy() {
    let (device, queue) = gpu();
    let boxes = room();
    let sky = Sky::new(
        Room::fit(boxes.iter().copied()).expect("walls"),
        sky::facing(Quat::from_rotation_y(0.35)),
    );
    // A seated eye half a meter off the point.
    let eye = sky.point + Vec3::new(0.45, -0.05, 0.3);
    let face_of = |b: &(u32, Vec3, Quat, Vec3)| port_face(acting_face(b.1, b.2, b.3, eye));
    let faces: Vec<Face> = boxes
        .iter()
        .filter(|b| b.0 != KIND_FLOOR)
        .map(face_of)
        .collect();
    let joined = sky::join(&faces);
    let rows_of = |face: &Face, map: Map, fade: [bool; 4]| {
        let base = surface_port::rows(face, 0.9, true, FIRST_ID, [1.0; 3]);
        sky::rows(base, map, &sky, fade)
    };
    let ahead = Vec3::new(sky.facing.x, 0.0, sky.facing.y);
    let sky_face = |dir: Vec3| {
        joined
            .iter()
            .find(|(f, _)| f.normal.dot(dir) < -0.9)
            .copied()
            .expect("a face that way")
    };
    let mut views = Vec::new();
    // The wall ahead, the wall behind with the seam on it, the ceiling
    // overhead with the zenith fade and the seam's wedge.
    for (dir, target) in [
        (ahead, eye + ahead * 2.0 + Vec3::Y * 0.3),
        (-ahead, eye - ahead * 2.0 + Vec3::Y * 0.4),
        (Vec3::Y, eye + Vec3::Y * 2.0 - ahead * 0.3),
    ] {
        let (face, fade) = sky_face(dir);
        let rows = rows_of(&face, Map::Sky, fade);
        views.push(("sky", compare(&device, &queue, &face, &rows, eye, target)));
    }
    // The floor's reflection, ahead and behind over the seam.
    let floor = face_of(&boxes[4]);
    let rows = rows_of(&floor, Map::Reflection, [true; 4]);
    for target in [eye + ahead * 2.0 - Vec3::Y, eye - ahead * 1.5 - Vec3::Y] {
        views.push((
            "reflection",
            compare(&device, &queue, &floor, &rows, eye, target),
        ));
    }
    for (what, t) in &views {
        eprintln!("{what}: {t:?}");
        assert!(t.compared > (W * H / 8) as usize, "{what}: {t:?}");
    }
    let sum = |f: fn(&Tally) -> usize| views.iter().map(|(_, t)| f(t)).collect::<Vec<_>>();
    // Every view lit; the seam mixed on the back wall and the ceiling;
    // the zenith faded overhead; the reflection dark toward the feet.
    assert!(sum(|t| t.lit).iter().all(|&n| n > 1000), "{views:?}");
    assert!(
        views[1].1.mixed > 1000 && views[2].1.mixed > 100,
        "{views:?}"
    );
    assert!(views[0].1.mixed == 0 && views[3].1.mixed == 0, "{views:?}");
    assert!(
        views[2].1.faded > 1000 && views[2].1.dark > 100,
        "{views:?}"
    );
    assert!(
        views[3].1.faded > 1000 && views[4].1.dark > 100,
        "{views:?}"
    );

    // Per face, fs_sky writes what fs_main writes.
    let (face, _) = sky_face(ahead);
    let base = surface_port::rows(&face, 0.9, true, FIRST_ID, [1.0; 3]);
    let res = surface_port::resolution(face.half);
    let look = Quat::from_rotation_arc(Vec3::NEG_Z, (face.center - eye).normalize());
    let fov = Fov {
        left: -0.9,
        right: 0.9,
        up: 0.9,
        down: -0.9,
    };
    let view_proj = math::view_projection(look.to_array(), eye.to_array(), fov, 0.05, 100.0);
    let main = render(
        &device,
        &queue,
        ("vs_main", "fs_main"),
        view_proj,
        &base,
        res,
    );
    let per_face = render(&device, &queue, ("vs_sky", "fs_sky"), view_proj, &base, res);
    let drawn = main.iter().filter(|p| p[3] >= 0.0).count();
    assert!(drawn > (W * H / 4) as usize, "{drawn}");
    let worst = main
        .iter()
        .zip(&per_face)
        .flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| (x - y).abs()))
        .fold(0.0, f32::max);
    eprintln!("per face: {drawn} pixels drawn, fs_sky against fs_main worst {worst:.2e}");
    assert!(worst < 1e-6, "{worst}");
}

/// An IEEE half as an f32 (the dump's texels).
fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 == 0 { 1.0 } else { -1.0 };
    let (e, m) = (i32::from((h >> 10) & 0x1f), f32::from(h & 0x3ff));
    sign * match e {
        0 => m * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + m / 1024.0) * 2f32.powi(e - 15),
    }
}

/// The real Aurora, composed as the app composes it: the core's loader's
/// preamble, the entry renamed, the wrapper and the sky's stages.
fn aurora_source() -> String {
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let _ = fosfora_app::effect::loader::set_assets_dir(assets);
    let loader = fosfora_app::effect::EffectLoader::new();
    let port = &surface_port::PORTS[0];
    assert_eq!(port.name, "aurora");
    let renamed = surface_port::rename_entry(port.shader).expect("the entry renames");
    format!(
        "{}\n{WRAPPER_WGSL}{}",
        loader.prepend_library(&renamed),
        sky::SKY_WGSL
    )
}

/// Aurora's uniform at `frame` of the synthetic groove, its `.pfx`
/// defaults with the curtain speed's travel, at `res`, its glow at `glow`.
/// The groove moves the two lowest bands alone, so the five above them
/// are set to a full mix's, falling with the band, so all seven curtains
/// show.
fn aurora_uniform(frame: u32, res: [f32; 2], glow: f32) -> Vec<u8> {
    use fosfora_app::gpu::uniforms::{ShaderUniforms, mirror_audio_features, shader_time};
    let mut f = fosfora_app::headless::loop_driver::synth_features_accented(frame, 72, 120.0, 4);
    f.low_mid = 0.55;
    f.mid = 0.5;
    f.upper_mid = 0.45;
    f.presence = 0.4;
    f.brilliance = 0.35;
    let mut u: ShaderUniforms = bytemuck::Zeroable::zeroed();
    mirror_audio_features(&mut u, &f);
    let t = frame as f32 / 72.0;
    u.time = shader_time(f64::from(t));
    u.delta_time = 1.0 / 72.0;
    u.frame_index = frame as f32;
    u.resolution = res;
    u.params[..4].copy_from_slice(&[0.5, 0.5, glow, 0.5 * t]);
    bytemuck::bytes_of(&u).to_vec()
}

/// Every face of `faces` (its port block and its effect's uniform) drawn
/// with Aurora's sky stages from `view_proj`, premultiplied over clear, at
/// `size` square: the light, row 0 the top.
fn render_room(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    source: &str,
    faces: &[([[f32; 4]; UNIFORM_ROWS], Vec<u8>)],
    view_proj: Mat4,
    size: u32,
) -> Vec<[f32; 4]> {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("aurora-sky"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let format = wgpu::TextureFormat::Rgba16Float;
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("aurora-sky"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_sky"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_sky"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    });
    let eye = uniform(
        device,
        queue,
        bytemuck::cast_slice(&view_proj.to_cols_array()),
    );
    let groups: Vec<_> = faces
        .iter()
        .map(|(rows, u)| {
            let u = uniform(device, queue, u);
            let port = uniform(device, queue, bytemuck::cast_slice(rows));
            let g0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("aurora-effect"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[entry(0, &u)],
            });
            let g1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("aurora-port"),
                layout: &pipeline.get_bind_group_layout(1),
                entries: &[entry(0, &eye), entry(1, &port)],
            });
            (g0, g1)
        })
        .collect();
    let extent = wgpu::Extent3d {
        width: size,
        height: size,
        depth_or_array_layers: 1,
    };
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("color"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let row_bytes = (size * 8).next_multiple_of(256);
    let back = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("color-back"),
        size: u64::from(row_bytes * size),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("aurora-sky"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline);
        for (g0, g1) in &groups {
            pass.set_bind_group(0, g0, &[]);
            pass.set_bind_group(1, g1, &[]);
            pass.draw(0..6, 0..1);
        }
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &back,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(size),
            },
        },
        extent,
    );
    queue.submit([encoder.finish()]);
    back.slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("poll");
    let bytes = back.slice(..).get_mapped_range().to_vec();
    bytes
        .chunks(row_bytes as usize)
        .flat_map(|row| bytemuck::pod_collect_to_vec::<u8, u16>(&row[..(size * 8) as usize]))
        .collect::<Vec<u16>>()
        .chunks_exact(4)
        .map(|p| [p[0], p[1], p[2], p[3]].map(half_to_f32))
        .collect()
}

/// `pixels` (premultiplied light) over a dim gray room, as a PPM.
fn write_ppm(path: &std::path::Path, pixels: &[[f32; 4]], w: usize) {
    let mut out = format!("P6\n{w} {}\n255\n", pixels.len() / w).into_bytes();
    for p in pixels {
        for c in &p[..3] {
            let lin = (c + (1.0 - p[3]) * 0.03).clamp(0.0, 1.0);
            out.push((lin.powf(1.0 / 2.2) * 255.0) as u8);
        }
    }
    std::fs::write(path, out).expect("writing the dump");
}

/// The real Aurora on the room's walls, ceiling and floor from a seated
/// eye, the sky on and per face, over a bar of the synthetic groove.
/// Checks the look's coarse facts (light near the horizon all around,
/// rings on the ceiling and a dark center straight overhead, a fainter
/// reflection) and, with `FOSFORA_SKY_DUMP=<dir>`, writes each view as a
/// PPM for looking at.
#[test]
#[ignore = "needs a GPU adapter"]
fn aurora_renders_as_one_sky_with_a_dark_zenith_and_a_fainter_floor() {
    let (device, queue) = gpu();
    let source = aurora_source();
    let dump = std::env::var_os("FOSFORA_SKY_DUMP").map(std::path::PathBuf::from);
    let boxes = room();
    let sky = Sky::new(
        Room::fit(boxes.iter().copied()).expect("walls"),
        sky::facing(Quat::from_rotation_y(0.35)),
    );
    let eye = sky.point + Vec3::new(0.2, -0.05, 0.15);
    let faces: Vec<(u32, Face)> = boxes
        .iter()
        .map(|b| (b.0, port_face(acting_face(b.1, b.2, b.3, eye))))
        .collect();
    let sky_faces: Vec<Face> = faces
        .iter()
        .filter(|(k, _)| *k != KIND_FLOOR)
        .map(|(_, f)| *f)
        .collect();
    let joined = sky::join(&sky_faces);
    let ahead = Vec3::new(sky.facing.x, 0.0, sky.facing.y);
    let size = 640;
    let fov = Fov {
        left: -0.9,
        right: 0.9,
        up: 0.9,
        down: -0.9,
    };
    let views = [
        ("ahead", ahead + Vec3::Y * 0.15),
        ("behind", -ahead + Vec3::Y * 0.15),
        ("lying-back", ahead * 0.3 + Vec3::Y),
        ("overhead", Vec3::Y - ahead * 0.02),
        ("floor", ahead - Vec3::Y * 0.8),
    ];
    let mut mean_at = std::collections::BTreeMap::new();
    for frame in [40u32, 150] {
        for on in [true, false] {
            let mut joined_i = joined.iter();
            let drawn: Vec<_> = faces
                .iter()
                .map(|&(kind, face)| {
                    let map = Map::of(on, fosfora_xr::surfaces::SurfaceBehavior::Aurora, kind);
                    let (face, fade) = match map {
                        Map::Sky => *joined_i.next().expect("a joined face"),
                        _ => (face, [true; 4]),
                    };
                    if map == Map::Face && kind != KIND_FLOOR {
                        joined_i.next();
                    }
                    let base = surface_port::rows(&face, 1.0, false, FIRST_ID, [1.0; 3]);
                    let rows = sky::rows(base, map, &sky, fade);
                    let (res, glow) = match map {
                        Map::Face => (surface_port::resolution(face.half), 0.3),
                        Map::Sky => (RESOLUTION, 0.3),
                        Map::Reflection => (RESOLUTION, 0.8),
                    };
                    (rows, aurora_uniform(frame, res, glow))
                })
                .collect();
            for (name, dir) in views {
                let look = if dir.y > 0.99 * dir.length() {
                    // Straight up: the top of the view toward the facing.
                    Quat::from_rotation_arc(Vec3::NEG_Z, Vec3::Y) * Quat::from_rotation_z(0.0)
                } else {
                    let fwd = dir.normalize();
                    let right = fwd.cross(Vec3::Y).normalize();
                    Quat::from_mat3(&glam::Mat3::from_cols(right, right.cross(fwd), -fwd))
                };
                let view_proj =
                    math::view_projection(look.to_array(), eye.to_array(), fov, 0.05, 100.0);
                let pixels = render_room(&device, &queue, &source, &drawn, view_proj, size);
                assert!(pixels.iter().all(|p| p.iter().all(|x| x.is_finite())));
                let s = size as usize;
                let mean = |x0: f32, x1: f32, y0: f32, y1: f32| {
                    let (xs, ys) = (
                        (x0 * s as f32) as usize..(x1 * s as f32) as usize,
                        (y0 * s as f32) as usize..(y1 * s as f32) as usize,
                    );
                    let n = (xs.len() * ys.len()) as f32;
                    ys.flat_map(|y| xs.clone().map(move |x| (x, y)))
                        .map(|(x, y)| pixels[y * s + x][3])
                        .sum::<f32>()
                        / n
                };
                // Around the sky's zenith: the point's straight up, on the
                // ceiling, where it lands in this view.
                let zenith = view_proj.project_point3(Vec3::new(
                    sky.point.x,
                    sky.room.ceiling_y,
                    sky.point.z,
                ));
                let (zx, zy) = (0.5 + 0.5 * zenith.x, 0.5 - 0.5 * zenith.y);
                let in_view = |v: f32| (0.1..0.9).contains(&v);
                let at_zenith = if zenith.z > 0.0 && in_view(zx) && in_view(zy) {
                    mean(zx - 0.03, zx + 0.03, zy - 0.03, zy + 0.03)
                } else {
                    f32::NAN
                };
                mean_at.insert((frame, on, name), [mean(0.0, 1.0, 0.0, 1.0), at_zenith]);
                if let Some(dir) = &dump {
                    let sky_on = if on { "sky" } else { "per-face" };
                    write_ppm(
                        &dir.join(format!("{sky_on}_{name}_{frame}.ppm")),
                        &pixels,
                        s,
                    );
                }
            }
        }
    }
    for (k, v) in &mean_at {
        eprintln!(
            "{k:?}: mean alpha {:.3}, around the zenith {:.3}",
            v[0], v[1]
        );
    }
    for frame in [40u32, 150] {
        let at = |on: bool, name: &str| mean_at[&(frame, on, name)];
        // The sky lights every way the seated wearer looks.
        for name in ["ahead", "behind", "lying-back", "overhead"] {
            assert!(at(true, name)[0] > 0.01, "{frame} {name}");
        }
        // Around the sky's zenith it is dark: the fade.
        assert!(at(true, "overhead")[1] < 1e-3, "{frame}");
        // The floor's reflection is lit and fainter than the sky ahead.
        let floor = at(true, "floor")[0];
        assert!(floor > 1e-3 && floor < at(true, "ahead")[0], "{frame}");
    }
}
