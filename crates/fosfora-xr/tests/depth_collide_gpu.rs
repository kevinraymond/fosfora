//! The live depth map as a collision source (board #3352), on a desktop GPU:
//! the atlas compute pass against its CPU twin. `#[ignore]`d like the core's
//! GPU probes (they need an adapter); run with
//! `cargo test -p fosfora-xr --test depth_collide_gpu -- --ignored`.
#![cfg(not(target_os = "android"))]

use std::sync::{Mutex, MutexGuard, OnceLock};

use fosfora_xr::env_depth::{ATLAS_WGSL, NEAR_CUT_M, atlas_params, encode_atlas};

/// The depth map's side on the Quest 3 (v207).
const MAP: u32 = 320;
const NEAR: f32 = 0.1;

static GPU: OnceLock<(wgpu::Device, wgpu::Queue)> = OnceLock::new();
static LOCK: Mutex<()> = Mutex::new(());

/// One shared device (the core's probes share theirs for the same reason:
/// several devices at once crash some drivers), and a lock so each test's
/// error scope is its own.
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
            label: Some("xr-depth-collide-test"),
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

/// The stored D16 value for a metric distance under the runtime's
/// infinite projection (near `NEAR`): `d = 1 - near / dist`.
fn d16(dist: f32) -> u16 {
    ((1.0 - NEAR / dist) * 65535.0).round() as u16
}

/// Both layers of a map with every case the atlas tells apart: a slanted
/// floor, a person-sized blob at 1.2 m, holes without data, texels nearer
/// than the discard distance and a wall past the atlas's range.
fn synthetic_map() -> Vec<u16> {
    let mut map = Vec::with_capacity((2 * MAP * MAP) as usize);
    for layer in 0..2u32 {
        for row in 0..MAP {
            for x in 0..MAP {
                let (fx, fy) = (x as f32 / MAP as f32, row as f32 / MAP as f32);
                let blob = (fx - 0.5 - 0.1 * layer as f32).powi(2) + (fy - 0.4).powi(2) < 0.02;
                let v = if blob {
                    d16(1.2 + 0.05 * fx)
                } else if (x / 7 + row / 11) % 13 == 0 {
                    u16::MAX
                } else if x < 12 && row > 300 {
                    d16(0.12)
                } else if fy > 0.85 {
                    d16(6.5)
                } else {
                    d16(0.8 + 3.5 * fy + 0.3 * fx)
                };
                map.push(v);
            }
        }
    }
    map
}

/// The atlas pass over `map` (both layers, `MAP` square), read back.
fn run_atlas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    map: &[u16],
    res: u32,
    flip_v: bool,
) -> Vec<u32> {
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
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("env-depth-atlas"),
        source: wgpu::ShaderSource::Wgsl(ATLAS_WGSL.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("env-depth-atlas"),
        layout: None,
        module: &module,
        entry_point: Some("cs_main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    let storage = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("env-depth-atlas"),
        size: u64::from(2 * res * res) * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let params = atlas_params(NEAR, f32::INFINITY, NEAR_CUT_M, res, flip_v);
    let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("env-depth-atlas-params"),
        size: std::mem::size_of_val(&params) as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&params_buf, 0, bytemuck::cast_slice(&params));
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("env-depth-atlas"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: storage.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: params_buf.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(res.div_ceil(8), res.div_ceil(8), 2);
    }
    queue.submit([encoder.finish()]);
    bytemuck::cast_slice(&read_buffer(device, queue, &storage)).to_vec()
}

/// The atlas the GPU builds from a D16 image is its CPU twin's: the same
/// texels with and without data, the same distances to one 2 cm step (the
/// GPU's division may round the other way at a step's edge), at 160 and
/// 320, read upright and flipped.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn the_atlas_pass_matches_its_cpu_twin() {
    let (device, queue, _guard) = gpu();
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let map = synthetic_map();
    let stored: Vec<f32> = map.iter().map(|&v| f32::from(v) / 65535.0).collect();
    for (res, flip_v) in [(160, false), (320, false), (160, true)] {
        let gpu = run_atlas(device, queue, &map, res, flip_v);
        let cpu = encode_atlas(
            &stored,
            [MAP, MAP],
            res,
            NEAR,
            f32::INFINITY,
            NEAR_CUT_M,
            flip_v,
        );
        assert_eq!(gpu.len(), cpu.len());
        let mut valid = 0;
        for (i, (g, c)) in gpu.iter().zip(&cpu).enumerate() {
            let (g, c) = (g.to_le_bytes(), c.to_le_bytes());
            assert_eq!(
                (g[1], g[2], g[3]),
                (c[1], c[2], c[3]),
                "res {res} flip {flip_v} texel {i}"
            );
            assert!(
                g[0].abs_diff(c[0]) <= 1,
                "res {res} flip {flip_v} texel {i}: R {} vs {}",
                g[0],
                c[0]
            );
            valid += usize::from(c[1] == 255);
        }
        // Every case is in the map: data, holes, too near, out of range.
        assert!(
            valid > cpu.len() / 2 && valid < cpu.len(),
            "{valid} of {}",
            cpu.len()
        );
    }
    wait(device);
    let err = pollster::block_on(device.pop_error_scope());
    assert!(err.is_none(), "validation error: {err:?}");
}
