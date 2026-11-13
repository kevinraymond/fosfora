//! The edge bars' in mode (board #5431) on a desktop GPU: a wall's tunnel
//! drawn by `EDGE_BARS_WGSL`'s `vs_tunnel` / `fs_tunnel` from a camera
//! looking at it off-axis, and compared, pixel by pixel, with the
//! fragment's Rust copy (`edge_bars::tunnel_alpha_at`) at the point of the
//! quad under each pixel. That checks the shader's eye from `view_proj`,
//! its frame, its one intersection, the rung and history lookup, the
//! dimming and the dark end against the tested Rust. `#[ignore]`d like the
//! other GPU tests (they need an adapter); run it with
//! `cargo test -p fosfora-xr --test edge_bars_gpu -- --ignored`.
#![cfg(not(target_os = "android"))]

use fosfora_xr::edge_bars::{
    EDGE_BARS_WGSL, HISTORY_STEP_S, History, LIFT_M, Outline, RUNG_FILL, RUNGS, Tunnel, rows,
    rung_at, tunnel_alpha_at, tunnel_hit,
};
use fosfora_xr::math::{self, Fov};
use glam::{Mat4, Quat, Vec3};

const W: u32 = 1024;
const H: u32 = 1024;

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
        label: Some("xr-edge-bars-test"),
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

/// The tunnel stages over `rows` from a camera `view_proj`, into a float
/// target: each pixel's alpha (the color is white, so rgb is the alpha).
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    view_proj: Mat4,
    rows: &[[f32; 4]],
) -> Vec<f32> {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("edge-bars"),
        source: wgpu::ShaderSource::Wgsl(EDGE_BARS_WGSL.into()),
    });
    let format = wgpu::TextureFormat::Rgba32Float;
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("edge-tunnels"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_tunnel"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_tunnel"),
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
    let eye = uniform(
        device,
        queue,
        bytemuck::cast_slice(&view_proj.to_cols_array()),
    );
    // The whole uniform, as the app binds it; the rows written at its
    // start.
    let mut full = vec![[0.0f32; 4]; fosfora_xr::edge_bars::UNIFORM_ROWS];
    full[..rows.len()].copy_from_slice(rows);
    let edge = uniform(device, queue, bytemuck::cast_slice(&full));
    let group = |i: u32, buffer: &wgpu::Buffer| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("edge-bars"),
            layout: &pipeline.get_bind_group_layout(i),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        })
    };
    let (eye_group, edge_group) = (group(0, &eye), group(1, &edge));
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
            label: Some("edge-tunnels"),
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
        pass.set_bind_group(0, &eye_group, &[]);
        pass.set_bind_group(1, &edge_group, &[]);
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
        .skip(3)
        .step_by(4)
        .collect()
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_tunnel_shader_matches_its_rust_copy() {
    let (device, queue) = gpu();
    // A 4 x 2.5 m wall's room side at z = -1.98, a tunnel 1.5 m deep, seen
    // from a seated head off to the right and above its middle, so all
    // four inner walls and the dark end show.
    let tunnel = Tunnel {
        center: Vec3::new(0.0, 1.25, -1.98),
        axes: [Vec3::X, Vec3::Y],
        half: [2.0, 1.25],
        normal: Vec3::Z,
        depth: 1.5,
        bars: [24, 20],
        low: true,
        color: [1.0, 1.0, 1.0],
        alpha: 0.9,
    };
    // The bars now and over the last steps, different per bar and step,
    // so each rung reads its own row.
    let bars = |step: usize| -> Vec<f32> {
        (0..24)
            .map(|i| ((i * 7 + step * 5) % 11) as f32 / 10.0)
            .collect()
    };
    let mut history = History::default();
    for step in 0..RUNGS {
        history.step(HISTORY_STEP_S, &bars(step));
    }
    let outline = Outline {
        strips: Vec::new(),
        tunnels: vec![tunnel],
    };
    let mut r = Vec::new();
    rows(&bars(RUNGS), &history, &outline, &mut r);
    let eye = Vec3::new(0.5, 1.45, 0.3);
    let fov = Fov {
        left: -0.8,
        right: 0.8,
        up: 0.8,
        down: -0.8,
    };
    let look = Quat::from_rotation_arc(Vec3::NEG_Z, (tunnel.center - eye).normalize());
    let view_proj = math::view_projection(look.to_array(), eye.to_array(), fov, 0.05, 100.0);
    let gpu_alpha = render(&device, &queue, view_proj, &r);
    // The quad's point under a pixel's middle.
    let inverse = view_proj.inverse();
    let plane = tunnel.center.z + LIFT_M;
    let under = |x: f32, y: f32| -> Vec3 {
        let ndc = Vec3::new(x / W as f32 * 2.0 - 1.0, 1.0 - y / H as f32 * 2.0, 0.5);
        let dir = inverse.project_point3(ndc) - eye;
        eye + dir * ((plane - eye.z) / dir.z)
    };
    let local = |p: Vec3| p - tunnel.center;
    let depth_at = |p: Vec3| tunnel_hit(tunnel.half, tunnel.depth, 1.0, local(eye), local(p));
    let spacing = tunnel.depth / RUNGS as f32;
    let w = RUNG_FILL * spacing;
    let (mut compared, mut lit, mut dark_end, mut worst) = (0, 0, 0, 0.0f32);
    let mut skipped = [0usize; 3];
    let mut walls = [false; 4];
    for py in 1..H - 1 {
        for px in 1..W - 1 {
            let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
            let p = under(x, y);
            let q = local(p);
            // On the quad, clear of its border.
            if q.x.abs() > tunnel.half[0] - 0.05 || q.y.abs() > tunnel.half[1] - 0.05 {
                skipped[0] += 1;
                continue;
            }
            let hit = depth_at(p);
            let right = depth_at(under(x + 1.0, y));
            let down = depth_at(under(x, y + 1.0));
            // Clear of the inner corners, where the derivatives jump.
            if right.edge != hit.edge || down.edge != hit.edge {
                skipped[1] += 1;
                continue;
            }
            let fw = (right.depth - hit.depth).abs() + (down.depth - hit.depth).abs();
            // Clear of a rung's edge by a pixel's worth of depth, where the
            // pixel's cover is the derivative's to say: the cover is then 1
            // or 0 for any derivative up to twice this one.
            let (_, dz) = rung_at(hit.depth, tunnel.depth);
            if (dz.abs() - 0.5 * w).abs() < fw {
                skipped[2] += 1;
                continue;
            }
            let want = tunnel_alpha_at(&r, 0, eye, p, fw);
            let got = gpu_alpha[(py * W + px) as usize];
            let err = (got - want).abs();
            worst = worst.max(err);
            // The bars' soft sides turn a few micrometers of the eye's
            // solve into about 1e-3 of alpha.
            assert!(
                err < 4e-3,
                "pixel ({px}, {py}): GPU {got} vs Rust {want} ({hit:?})"
            );
            compared += 1;
            if want > 0.01 {
                lit += 1;
                walls[hit.edge] = true;
            }
            if hit.end {
                dark_end += 1;
            }
        }
    }
    eprintln!(
        "compared {compared} pixels, {lit} lit, {dark_end} at the end, worst {worst:.2e}; skipped off the quad, at a corner, at a rung's edge {skipped:?}"
    );
    assert!(compared > (W * H / 4) as usize, "{compared}");
    assert!(lit > 500, "{lit}");
    assert!(dark_end > 500, "{dark_end}");
    assert_eq!(walls, [true; 4], "every inner wall lit");
    // Outside the quad nothing was drawn: the clear value stays.
    assert!(gpu_alpha[0] < -0.5, "{}", gpu_alpha[0]);
}
