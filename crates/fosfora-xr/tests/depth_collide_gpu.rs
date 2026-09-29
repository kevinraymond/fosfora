//! The live depth map as a collision source (board #3352), on a desktop GPU:
//! the atlas compute pass and its copy into an RGBA8 texture against the
//! pass's CPU twin, and `flux_xr_sim.wgsl`'s
//! collide against a depth atlas, run headless behind the loader's compute
//! preamble; and the particle pitcher's pour (board #3402) through the same
//! sim onto a floor box. The GPU tests are `#[ignore]`d like the core's probes (they
//! need an adapter); run them with
//! `cargo test -p fosfora-xr --test depth_collide_gpu -- --ignored`.
#![cfg(not(target_os = "android"))]

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use fosfora_xr::env_depth::{
    ATLAS_WGSL, COLLIDE_ROWS, DepthCollide, DepthView, NEAR_CUT_M, atlas_params, atlas_row_texels,
    encode_atlas,
};
use fosfora_xr::instruments::{
    PITCHER_SPEED_M_S, POUR_NOZZLE_M, POUR_SPREAD_DEG, Pour, pour_row, rows as instrument_rows,
};
use fosfora_xr::math::Fov;
use glam::Vec3;

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

/// The atlas pass over `map` (both layers, `MAP` square) as the XR app
/// records it: the pass into the padded storage buffer, then in the same
/// encoder the copy into an RGBA8 texture of the atlas's size (the
/// obstacle texture on the device); the texture read back, unpadded.
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
        size: u64::from(2 * res * atlas_row_texels(res)) * 4,
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
    let extent = wgpu::Extent3d {
        width: res,
        height: 2 * res,
        depth_or_array_layers: 1,
    };
    let layout = wgpu::TexelCopyBufferLayout {
        offset: 0,
        bytes_per_row: Some(atlas_row_texels(res) * 4),
        rows_per_image: Some(2 * res),
    };
    let obstacle = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("obstacle"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    encoder.copy_buffer_to_texture(
        wgpu::TexelCopyBufferInfo {
            buffer: &storage,
            layout,
        },
        obstacle.as_image_copy(),
        extent,
    );
    // Read the texture back through a second padded buffer.
    let back = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("obstacle-back"),
        size: storage.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        obstacle.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &back,
            layout,
        },
        extent,
    );
    queue.submit([encoder.finish()]);
    let padded: Vec<u32> = bytemuck::cast_slice(&read_buffer(device, queue, &back)).to_vec();
    padded
        .chunks(atlas_row_texels(res) as usize)
        .flat_map(|row| row[..res as usize].iter().copied())
        .collect()
}

/// The atlas the GPU builds from a D16 image is its CPU twin's: the same
/// texels with and without data, the same 16-bit distances to a step or
/// two (the GPU's division may round the other way), at 160 and 320, read
/// upright and flipped.
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
            let (gb, cb) = (g.to_le_bytes(), c.to_le_bytes());
            assert_eq!(
                (gb[1], gb[3]),
                (cb[1], cb[3]),
                "res {res} flip {flip_v} texel {i}"
            );
            let value = |b: [u8; 4]| i32::from(b[0]) << 8 | i32::from(b[2]);
            assert!(
                (value(gb) - value(cb)).abs() <= 2,
                "res {res} flip {flip_v} texel {i}: {} vs {}",
                value(gb),
                value(cb)
            );
            valid += usize::from(cb[1] == 255);
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

// ---- The sim's collide -------------------------------------------------------

/// What `EffectLoader::prepend_compute_libraries` puts in front of a
/// compute shader: the shared libraries, then the particle library.
const PREAMBLE: [&str; 7] = [
    include_str!("../../../assets/shaders/lib/noise.wgsl"),
    include_str!("../../../assets/shaders/lib/palette.wgsl"),
    include_str!("../../../assets/shaders/lib/sdf.wgsl"),
    include_str!("../../../assets/shaders/lib/tonemap.wgsl"),
    include_str!("../../../assets/shaders/lib/chronoflow.wgsl"),
    include_str!("../../../assets/shaders/lib/overlay_lib.wgsl"),
    include_str!("../../../assets/shaders/lib/particle_lib.wgsl"),
];
const FLUX_SIM: &str = include_str!("../../../assets/xr/shaders/flux_xr_sim.wgsl");
/// Rows the XR app uploads (`WORLD_AUX_ROWS` in `scene.rs`).
const WORLD_AUX_ROWS: usize = 181;
const FPS: f32 = 60.0;

fn sim_source() -> String {
    format!("{}\n{FLUX_SIM}", PREAMBLE.join("\n"))
}

fn sim_literal(module: &naga::Module, name: &str) -> naga::Literal {
    let (_, c) = module
        .constants
        .iter()
        .find(|(_, c)| c.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("const {name}"));
    match module.global_expressions[c.init] {
        naga::Expression::Literal(v) => v,
        ref e => panic!("const {name} is {e:?}"),
    }
}

fn sim_const(module: &naga::Module, name: &str) -> u32 {
    match sim_literal(module, name) {
        naga::Literal::U32(v) => v,
        v => panic!("const {name} is {v:?}"),
    }
}

/// The depth rows sit right after the instrument rows, the pour row after
/// them, ending where the XR app's upload does; the sim validates with
/// them, and its pour cone is the pitcher's.
#[test]
fn the_sim_reads_the_depth_rows_after_the_instruments() {
    let module = naga::front::wgsl::parse_str(&sim_source()).expect("flux_xr_sim.wgsl parses");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .expect("flux_xr_sim.wgsl validates");
    let get = |name| sim_const(&module, name);
    assert_eq!(
        get("XR_AUX_DEPTH"),
        get("XR_AUX_INSTRUMENTS") + get("XR_AUX_INSTRUMENT_ROWS")
    );
    assert_eq!(get("XR_AUX_DEPTH"), 173);
    assert_eq!(get("XR_AUX_DEPTH_ROWS") as usize, COLLIDE_ROWS);
    assert_eq!(
        get("XR_AUX_DEPTH") + get("XR_AUX_DEPTH_ROWS"),
        get("XR_AUX_POUR")
    );
    assert_eq!(get("XR_AUX_POUR") as usize + 1, WORLD_AUX_ROWS);
    let naga::Literal::F32(spread) = sim_literal(&module, "XR_POUR_SPREAD") else {
        panic!("XR_POUR_SPREAD is not an f32");
    };
    assert!(
        (spread - POUR_SPREAD_DEG.to_radians()).abs() < 1e-6,
        "{spread}"
    );
}

/// `struct ParticleUniforms`, filled by member name at naga's offsets.
struct Uniforms {
    bytes: Vec<u8>,
    offsets: HashMap<String, usize>,
}

impl Uniforms {
    fn new(module: &naga::Module) -> Self {
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("ParticleUniforms"))
            .expect("struct ParticleUniforms");
        let naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!("ParticleUniforms is not a struct");
        };
        Self {
            bytes: vec![0; *span as usize],
            offsets: members
                .iter()
                .map(|m| (m.name.clone().unwrap_or_default(), m.offset as usize))
                .collect(),
        }
    }

    fn set(&mut self, name: &str, bits: u32) {
        let at = self.offsets[name];
        self.bytes[at..at + 4].copy_from_slice(&bits.to_le_bytes());
    }

    fn f32(&mut self, name: &str, v: f32) {
        self.set(name, v.to_bits());
    }
}

/// The scene of one sim test: particles, the aux block and the obstacle
/// texture's contents.
struct SimSetup {
    /// Per particle: position and velocity.
    particles: Vec<(Vec3, Vec3)>,
    aux: Vec<[f32; 4]>,
    /// The atlas and its side, or `None` for the core's 1x1 placeholder.
    atlas: Option<(Vec<u32>, u32)>,
    /// Test each particle every this many frames: the depth header's
    /// stride is rewritten each frame with the phase, as the XR app does
    /// (`DepthCollide::rows`). 1 leaves the rows as given.
    every: u32,
}

/// Every particle's position and velocity after each frame in `capture`.
type Captures = Vec<Vec<(Vec3, Vec3)>>;

/// What [`run_sim_with`] changes from [`run_sim`]'s defaults.
struct SimOptions {
    /// The preset's drag (1: none).
    drag: f32,
    /// Lifetime (s): a burst lives `XR_BURST_LIFE` of it, a pour all of it.
    lifetime: f32,
    /// Every particle starts dead, a slot for a burst (else alive).
    dead: bool,
    /// From this frame on, this aux block.
    aux_from: Option<(u32, Vec<[f32; 4]>)>,
}

impl Default for SimOptions {
    fn default() -> Self {
        Self {
            drag: 1.0,
            lifetime: 100.0,
            dead: false,
            aux_from: None,
        }
    }
}

/// One particle after a frame.
#[derive(Debug, Clone, Copy)]
struct Sample {
    pos: Vec3,
    vel: Vec3,
    /// The life lane: 0 dead, 1 alive in the volume, 2 free (a burst's).
    life: f32,
    /// Its lifetime (s).
    max_life: f32,
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_entry(binding: u32, dim: wgpu::TextureViewDimension) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dim,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

/// An RGBA8 texture holding `texels` (little-endian RGBA per `u32`).
fn rgba_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: [u32; 3],
    dimension: wgpu::TextureDimension,
    texels: &[u32],
) -> wgpu::TextureView {
    let extent = wgpu::Extent3d {
        width: size[0],
        height: size[1],
        depth_or_array_layers: size[2],
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sim-test-texture"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        bytemuck::cast_slice(texels),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size[0] * 4),
            rows_per_image: Some(size[1]),
        },
        extent,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// `flux_xr_sim.wgsl` behind the loader's preamble, with the bindings the
/// core's `ParticleSystem` gives it (group 0: uniforms, the particle
/// arrays, counters, aux; group 1: flow field, obstacle texture, water,
/// fluid), stepped at 60 fps with no flow, no drag, no emission and no
/// drift: only the particles' own velocity and the collide move them.
fn run_sim(setup: &SimSetup, capture: &[u32]) -> Captures {
    run_sim_with(setup, &SimOptions::default(), capture)
        .into_iter()
        .map(|frame| frame.iter().map(|s| (s.pos, s.vel)).collect())
        .collect()
}

/// [`run_sim`] with `opts`, reading every particle's lanes back.
fn run_sim_with(setup: &SimSetup, opts: &SimOptions, capture: &[u32]) -> Vec<Vec<Sample>> {
    let (device, queue, _guard) = gpu();
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let source = sim_source();
    let module = naga::front::wgsl::parse_str(&source).expect("sim parses");
    let count = setup.particles.len() as u32;

    let mut u = Uniforms::new(&module);
    u.f32("delta_time", 1.0 / FPS);
    u.set("max_particles", count);
    u.set("emit_count", 0);
    // A volume big enough that nothing respawns at its bounds.
    u.f32("emitter_radius", 5.0);
    u.f32("lifetime", opts.lifetime);
    u.f32("initial_size", 0.004);
    u.f32("size_end", 0.004);
    u.f32("drag", opts.drag);
    u.f32("flow_enabled", 0.0);
    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sim-test-uniforms"),
        size: u.bytes.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let lanes: [Vec<[f32; 4]>; 4] = [
        setup
            .particles
            .iter()
            .map(|(p, _)| [p.x, p.y, p.z, if opts.dead { 0.0 } else { 1.0 }])
            .collect(),
        setup
            .particles
            .iter()
            .map(|(_, v)| [v.x, v.y, v.z, 0.004])
            .collect(),
        vec![[0.5, 0.5, 0.5, 0.1]; count as usize],
        vec![[0.0, opts.lifetime, 0.004, 0.1]; count as usize],
    ];
    let storage = |label, bytes: &[u8]| {
        let b = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        queue.write_buffer(&b, 0, bytes);
        b
    };
    let ins: Vec<wgpu::Buffer> = lanes
        .iter()
        .map(|l| storage("sim-test-in", bytemuck::cast_slice(l)))
        .collect();
    let outs: Vec<wgpu::Buffer> = lanes
        .iter()
        .map(|l| storage("sim-test-out", bytemuck::cast_slice(l)))
        .collect();
    let counters = storage("sim-test-counters", &[0u8; 16]);
    let mut aux = setup.aux.clone();
    aux.resize(WORLD_AUX_ROWS, [0.0; 4]);
    let aux_buf = storage("sim-test-aux", bytemuck::cast_slice(&aux));
    let dead = storage("sim-test-dead", &vec![0u8; count as usize * 4]);
    let alive = storage("sim-test-alive", &vec![0u8; count as usize * 4]);

    let flow = rgba_texture(device, queue, [1, 1, 1], wgpu::TextureDimension::D3, &[0]);
    let obstacle = match &setup.atlas {
        Some((texels, side)) => rgba_texture(
            device,
            queue,
            [*side, 2 * side, 1],
            wgpu::TextureDimension::D2,
            texels,
        ),
        None => rgba_texture(device, queue, [1, 1, 1], wgpu::TextureDimension::D2, &[0]),
    };
    let water = rgba_texture(device, queue, [1, 1, 1], wgpu::TextureDimension::D2, &[0]);
    let fluid = rgba_texture(device, queue, [1, 1, 1], wgpu::TextureDimension::D2, &[0]);
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    let mut group0_entries = vec![wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }];
    group0_entries.extend((1..=4).map(|b| storage_entry(b, true)));
    group0_entries.extend((5..=9).map(|b| storage_entry(b, false)));
    group0_entries.extend([
        storage_entry(10, true),
        storage_entry(11, true),
        storage_entry(12, false),
    ]);
    let layout0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sim-test-0"),
        entries: &group0_entries,
    });
    let layout1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sim-test-1"),
        entries: &[
            texture_entry(0, wgpu::TextureViewDimension::D3),
            sampler_entry(1),
            texture_entry(2, wgpu::TextureViewDimension::D2),
            sampler_entry(3),
            texture_entry(4, wgpu::TextureViewDimension::D2),
            texture_entry(5, wgpu::TextureViewDimension::D2),
        ],
    });
    let mut entries0 = vec![wgpu::BindGroupEntry {
        binding: 0,
        resource: uniforms.as_entire_binding(),
    }];
    for (k, b) in ins.iter().chain(&outs).enumerate() {
        entries0.push(wgpu::BindGroupEntry {
            binding: 1 + k as u32,
            resource: b.as_entire_binding(),
        });
    }
    for (binding, b) in [(9, &counters), (10, &aux_buf), (11, &dead), (12, &alive)] {
        entries0.push(wgpu::BindGroupEntry {
            binding,
            resource: b.as_entire_binding(),
        });
    }
    let group0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sim-test-0"),
        layout: &layout0,
        entries: &entries0,
    });
    let view = |binding, v| wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(v),
    };
    let samp = |binding| wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::Sampler(&sampler),
    };
    let group1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sim-test-1"),
        layout: &layout1,
        entries: &[
            view(0, &flow),
            samp(1),
            view(2, &obstacle),
            samp(3),
            view(4, &water),
            view(5, &fluid),
        ],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("flux-xr-sim"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sim-test"),
        bind_group_layouts: &[&layout0, &layout1],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("flux-xr-sim"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("cs_main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });

    let frames = capture.iter().copied().max().unwrap_or(0);
    let mut out = Vec::new();
    for frame in 0..frames {
        u.f32("time", frame as f32 / FPS);
        queue.write_buffer(&uniforms, 0, &u.bytes);
        queue.write_buffer(&counters, 0, &[0u8; 16]);
        if let Some((from, next)) = &opts.aux_from
            && frame == *from
        {
            aux.clone_from(next);
            aux.resize(WORLD_AUX_ROWS, [0.0; 4]);
            queue.write_buffer(&aux_buf, 0, bytemuck::cast_slice(&aux));
        }
        if setup.every > 1 && bytemuck::cast::<f32, u32>(aux[173][0]) != 0 {
            let every = setup.every;
            aux[173][0] = f32::from_bits(every | (frame % every) << 16);
            queue.write_buffer(&aux_buf, 173 * 16, bytemuck::cast_slice(&aux[173..174]));
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group0, &[]);
            pass.set_bind_group(1, &group1, &[]);
            pass.dispatch_workgroups(count.div_ceil(256), 1, 1);
        }
        // Ping-pong by copy: this frame's output is the next one's input.
        for (i, o) in ins.iter().zip(&outs) {
            encoder.copy_buffer_to_buffer(o, 0, i, 0, i.size());
        }
        queue.submit([encoder.finish()]);
        if capture.contains(&(frame + 1)) {
            let pos: Vec<[f32; 4]> =
                bytemuck::cast_slice(&read_buffer(device, queue, &ins[0])).to_vec();
            let vel: Vec<[f32; 4]> =
                bytemuck::cast_slice(&read_buffer(device, queue, &ins[1])).to_vec();
            let flags: Vec<[f32; 4]> =
                bytemuck::cast_slice(&read_buffer(device, queue, &ins[3])).to_vec();
            out.push(
                pos.iter()
                    .zip(&vel)
                    .zip(&flags)
                    .map(|((p, v), f)| Sample {
                        pos: Vec3::new(p[0], p[1], p[2]),
                        vel: Vec3::new(v[0], v[1], v[2]),
                        life: p[3],
                        max_life: f[1],
                    })
                    .collect(),
            );
        }
    }
    wait(device);
    let err = pollster::block_on(device.pop_error_scope());
    assert!(err.is_none(), "validation error: {err:?}");
    out
}

/// The collide's obstacle header: no spheres, no boxes, restitution and
/// margin (the XR app's 0.4 and the sprite radius), then no drift.
const RESTITUTION: f32 = 0.4;
const MARGIN: f32 = 0.005;
const THICKNESS: f32 = 0.15;
const WALL_M: f32 = 1.0;

/// The aux block: the header rows, zeros, and the depth rows for a depth
/// camera at the anchor looking down -Z with a 90 degree fov in both
/// layers (`on` false: the rows with x = 0), every particle every frame.
fn aux_rows(on: bool, res: u32) -> Vec<[f32; 4]> {
    aux_rows_turned(on, res, [[0.0, 0.0, 0.0, 1.0]; 2])
}

/// `aux_rows` with each layer's camera turned by its own quaternion.
fn aux_rows_turned(on: bool, res: u32, turn: [[f32; 4]; 2]) -> Vec<[f32; 4]> {
    let mut aux = vec![[0.0; 4]; WORLD_AUX_ROWS];
    aux[1] = [0.0, 0.0, RESTITUTION, MARGIN];
    let fov = Fov {
        left: -std::f32::consts::FRAC_PI_4,
        right: std::f32::consts::FRAC_PI_4,
        up: std::f32::consts::FRAC_PI_4,
        down: -std::f32::consts::FRAC_PI_4,
    };
    let view = |orientation| DepthView {
        orientation,
        position: [0.0, 0.0, 0.0],
        fov,
    };
    let rows = DepthCollide {
        views: turn.map(view),
        near: NEAR,
        res,
        thickness_m: THICKNESS,
        every: 1,
    }
    .rows(Vec3::ZERO, 0);
    let mut rows = rows.to_vec();
    if !on {
        rows[0][0] = f32::from_bits(0);
    }
    aux[173..180].copy_from_slice(&rows);
    aux
}

/// The atlas at 160 per side of a 320 map whose texel (layer, row, x) holds
/// `dist` (`None`: no data), built by the CPU twin of the atlas pass.
fn atlas_of(dist: impl Fn(u32, u32, u32) -> Option<f32>) -> (Vec<u32>, u32) {
    let mut map = Vec::with_capacity((2 * MAP * MAP) as usize);
    for layer in 0..2 {
        for row in 0..MAP {
            for x in 0..MAP {
                map.push(dist(layer, row, x).map_or(1.0, |m| f32::from(d16(m)) / 65535.0));
            }
        }
    }
    let res = 160;
    (
        encode_atlas(
            &map,
            [MAP, MAP],
            res,
            NEAR,
            f32::INFINITY,
            NEAR_CUT_M,
            false,
        ),
        res,
    )
}

/// 20K particles on the plane 0.9 m ahead of the depth camera, spread over
/// a square of its view but kept 2 cm off the horizon (the row where the
/// half-view wall below ends), flying at the wall at 1 m/s.
fn particles_at_the_wall() -> Vec<(Vec3, Vec3)> {
    let mut seed = 0x2545_f491_u32;
    let mut rand = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    (0..20_000)
        .map(|_| {
            let x = rand() - 0.5;
            let y = (0.02 + 0.48 * rand()) * if rand() < 0.5 { -1.0 } else { 1.0 };
            (Vec3::new(x, y, -0.9), Vec3::new(0.0, 0.0, -1.0))
        })
        .collect()
}

/// A wall 1 m ahead of the depth camera over the lower half of its view
/// (rows 0..160 of each layer: row 0 is the bottom), no data above. Six
/// frames in (1.0 m reached) every particle in the lower half sits the
/// margin in front of the wall, bouncing back; forty frames in none is
/// behind it and every one flies away from it, while the upper half,
/// where the map has no data, flew through. With the rows' switch off the
/// lower half flies through too.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn particles_bounce_off_a_wall_in_the_depth_map() {
    let atlas = atlas_of(|_, row, _| (row < MAP / 2).then_some(WALL_M));
    let particles = particles_at_the_wall();
    let lower = |p: &Vec3| p.y < 0.0;
    let on = run_sim(
        &SimSetup {
            particles: particles.clone(),
            aux: aux_rows(true, atlas.1),
            atlas: Some(atlas.clone()),
            every: 1,
        },
        &[6, 40],
    );
    let (hit, later) = (&on[0], &on[1]);
    let starts: Vec<bool> = particles.iter().map(|(p, _)| lower(p)).collect();
    let n_lower = starts.iter().filter(|&&l| l).count();
    assert!(n_lower > 9000 && n_lower < 11000, "{n_lower}");
    // At the wall: depth (along -Z) the margin short of it, speed reversed.
    let at_margin = hit
        .iter()
        .zip(&starts)
        .filter(|&(&(p, _), &l)| l && (-p.z - (WALL_M - MARGIN)).abs() < 2e-3)
        .count();
    assert!(
        at_margin * 100 >= n_lower * 99,
        "{at_margin} of {n_lower} at the wall"
    );
    for (&(p, v), &l) in hit.iter().zip(&starts) {
        if l {
            assert!(v.z > 0.3, "at {p}: velocity {v} not bouncing back");
            assert!((v.z - RESTITUTION).abs() < 0.02, "at {p}: {v}");
        }
    }
    for (&(p, v), &l) in later.iter().zip(&starts) {
        if l {
            assert!(-p.z <= WALL_M, "at {p}: behind the wall");
            assert!(v.z > 0.0, "at {p}: velocity {v} toward the wall");
        } else {
            assert!(
                -p.z > WALL_M + THICKNESS,
                "at {p}: stopped where there is no data"
            );
        }
    }
    // The switch off: nothing collides.
    let off = run_sim(
        &SimSetup {
            particles,
            aux: aux_rows(false, atlas.1),
            atlas: Some(atlas),
            every: 1,
        },
        &[40],
    );
    assert!(
        off[0].iter().all(|(p, _)| -p.z > WALL_M + THICKNESS),
        "a particle stopped with the collide off"
    );
}

/// Layer 0 turned away (looking down +Z), layer 1 holding the wall over
/// the whole view: the particles, outside layer 0's view, fall through to
/// the right eye's layer (the lower half of the atlas) and every one
/// bounces. Layer 0 facing them over a texel without data decides alone:
/// they fly through (the other eye's layer, 6 cm over, has its holes in
/// the same places, so it is not asked).
#[test]
#[ignore = "requires a GPU/software adapter"]
fn the_right_layer_answers_outside_the_left_view() {
    let atlas = atlas_of(|layer, _, _| (layer == 1).then_some(WALL_M));
    let away = glam::Quat::from_rotation_y(std::f32::consts::PI).to_array();
    let identity = [0.0, 0.0, 0.0, 1.0];
    let run = |turn| {
        run_sim(
            &SimSetup {
                particles: particles_at_the_wall(),
                aux: aux_rows_turned(true, atlas.1, turn),
                atlas: Some(atlas.clone()),
                every: 1,
            },
            &[40],
        )
        .pop()
        .expect("one capture")
    };
    for (p, v) in run([away, identity]) {
        assert!(-p.z <= WALL_M && v.z > 0.0, "at {p}: {v}");
    }
    for (p, _) in run([identity, identity]) {
        assert!(-p.z > WALL_M + THICKNESS, "at {p}: stopped by layer 1");
    }
}

/// `depthcollideevery 2`: each particle is tested every other frame, on
/// its own phase. The wall still stops every one in the lower half, at
/// most a frame late (1.7 cm at 1 m/s, inside the thickness band): on the
/// frame the wall is reached about half have bounced, two frames later all
/// have and none is behind the wall; forty in all
/// fly away from it, and the upper half still flies through.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn every_other_frame_still_stops_every_particle() {
    let atlas = atlas_of(|_, row, _| (row < MAP / 2).then_some(WALL_M));
    let particles = particles_at_the_wall();
    let caps = run_sim(
        &SimSetup {
            particles: particles.clone(),
            aux: aux_rows(true, atlas.1),
            atlas: Some(atlas),
            every: 2,
        },
        &[6, 8, 40],
    );
    // Six frames in (the wall reached), only the particles whose turn it
    // was have bounced: about half.
    let lower = particles.iter().filter(|(p, _)| p.y < 0.0).count();
    let bounced = caps[0]
        .iter()
        .zip(&particles)
        .filter(|((_, v), (p, _))| p.y < 0.0 && v.z > 0.0)
        .count();
    assert!(
        bounced * 10 > lower * 4 && bounced * 10 < lower * 6,
        "{bounced} of {lower} bounced on the first frame at the wall"
    );
    for (cap, frame) in caps[1..].iter().zip([8, 40]) {
        for (&(p, v), (start, _)) in cap.iter().zip(&particles) {
            if start.y < 0.0 {
                assert!(-p.z <= WALL_M && v.z > 0.0, "frame {frame} at {p}: {v}");
            } else if frame == 40 {
                assert!(-p.z > WALL_M + THICKNESS, "at {p}: stopped without data");
            }
        }
    }
}

/// Rows that claim an atlas over the core's 1x1 placeholder (which the
/// XR app sizes before the atlas's first copy, except with
/// `depthcollideupload 0`) still collide with
/// nothing: the sim takes the side from the rows, not the texture, and
/// the placeholder's zeros, however the load is bounded, read as no data.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn the_placeholder_texture_collides_with_nothing() {
    let later = run_sim(
        &SimSetup {
            particles: particles_at_the_wall(),
            aux: aux_rows(true, 160),
            atlas: None,
            every: 1,
        },
        &[40],
    )
    .pop()
    .expect("one capture");
    assert!(later.iter().all(|(p, _)| -p.z > WALL_M + THICKNESS));
}

/// A horizontal surface (a seat 0.4 m below the depth camera, seen over
/// the lower part of its view, 0.8 to 2 m out) and particles falling onto
/// it: the normal comes from the map's depth gradient, not the ray, so
/// every one bounces up, not toward the camera, and none falls through.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn particles_bounce_up_off_a_horizontal_surface() {
    const SEAT_Y: f32 = -0.4;
    // Along each row's ray (tangent t_y below the axis) the plane is
    // SEAT_Y / t_y ahead.
    let atlas = atlas_of(|_, row, _| {
        let t_y = -1.0 + 2.0 * (row as f32 + 0.5) / MAP as f32;
        (t_y < -0.1).then(|| SEAT_Y / t_y)
    });
    let mut seed = 0x1234_5678_u32;
    let mut rand = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    let particles: Vec<(Vec3, Vec3)> = (0..20_000)
        .map(|_| {
            (
                Vec3::new(0.6 * rand() - 0.3, -0.3, -0.8 - 1.2 * rand()),
                Vec3::new(0.0, -1.0, 0.0),
            )
        })
        .collect();
    let caps = run_sim(
        &SimSetup {
            particles,
            aux: aux_rows(true, atlas.1),
            atlas: Some(atlas),
            every: 1,
        },
        &[8, 40],
    );
    // Eight frames in (0.1 m fallen at 6), all have bounced straight up
    // (the gradient normal within 3 degrees of vertical: with the distance
    // in 8 bits it tilted up to 28), at the seat. At 160 texels a side the
    // surface is each texel's nearest depth, so where the seat is seen at
    // a grazing angle a particle lands a few cm early along its ray.
    for (p, v) in &caps[0] {
        let tilt = v.z.atan2(v.y).to_degrees().abs();
        assert!(
            v.y > 0.3 && tilt < 3.0,
            "at {p}: velocity {v}, {tilt} degrees"
        );
        assert!(
            p.y > SEAT_Y - 0.005 && p.y < SEAT_Y + 0.06,
            "at {p}: not at the seat"
        );
    }
    for (p, v) in &caps[1] {
        assert!(p.y > SEAT_Y && v.y > 0.0, "at {p}: velocity {v}");
    }
}

// ---- The pitcher's pour (board #3402) ------------------------------------------

/// The settle drift the XR app runs world mode with (m/s, `gravity`), and
/// the Flux world presets' drag and lifetime.
const SETTLE: f32 = 0.5;
const PRESET_DRAG: f32 = 0.98;
const PRESET_LIFETIME: f32 = 12.0;
/// The floor box's top, 1 m under the nozzle at the anchor.
const FLOOR_TOP: f32 = -1.0;
const POUR_PER_FRAME: u32 = 200;
const POUR_FRAMES: u32 = 30;

/// The aux block for a pour at the anchor, straight down at the pitcher's
/// speed, over a floor box (the obstacle header with one box, the settle
/// drift), packed by the XR app's own row builders. `pour` false: the same
/// burst rows with the pour row zero, as on a frame a throw has them.
fn pour_aux(pour: bool, count: u32) -> Vec<[f32; 4]> {
    let mut aux = vec![[0.0; 4]; WORLD_AUX_ROWS];
    aux[1] = [f32::from_bits(0), f32::from_bits(1), RESTITUTION, MARGIN];
    aux[2] = [0.0, 0.0, SETTLE, 0.0];
    aux[67] = [0.0, FLOOR_TOP - 0.05, 0.0, 0.0];
    aux[99] = [0.0, 0.0, 0.0, 1.0];
    aux[131] = [2.0, 0.05, 2.0, 0.0];
    let p = Pour {
        center: Vec3::ZERO,
        dir: Vec3::NEG_Y,
        count,
        speed: PITCHER_SPEED_M_S,
        radius: POUR_NOZZLE_M,
    };
    aux[170..173].copy_from_slice(&instrument_rows(Some(p.as_burst()), None, Vec3::ZERO));
    if pour {
        aux[180] = pour_row(Some(p));
    }
    aux
}

/// 20K dead slots, parked well away from the pour.
fn dead_slots() -> SimSetup {
    SimSetup {
        particles: vec![(Vec3::new(0.0, 10.0, 0.0), Vec3::ZERO); 20_000],
        aux: pour_aux(true, POUR_PER_FRAME),
        atlas: None,
        every: 1,
    }
}

/// A pour of 200 a frame for 30 frames from the anchor, straight down at
/// 1.5 m/s, onto a floor box 1 m below, with the presets' drag and
/// lifetime and the app's settle drift. The newborns fly within the 6
/// degree cone at the pitcher's speed and live the full lifetime; each
/// lands no wider than the cone's geometric width at the floor (the nozzle
/// plus 1 m x tan 6 degrees); two and a half seconds in all 6000 rest on
/// it, the margin above its top, none below. The same burst rows with the
/// pour row zero are a throw's: the ball, flying outward, with half the
/// lifetime.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn a_pour_lands_on_the_floor_within_its_cone_and_rests_there() {
    let spread = POUR_SPREAD_DEG.to_radians();
    let opts = SimOptions {
        drag: PRESET_DRAG,
        lifetime: PRESET_LIFETIME,
        dead: true,
        aux_from: Some((POUR_FRAMES, pour_aux(true, 0))),
    };
    let mut frames: Vec<u32> = (1..=90).collect();
    frames.push(150);
    let caps = run_sim_with(&dead_slots(), &opts, &frames);
    // The first frame's newborns: at the nozzle, in the cone, full life.
    let born: Vec<&Sample> = caps[0].iter().filter(|s| s.life > 0.0).collect();
    assert_eq!(born.len(), POUR_PER_FRAME as usize);
    let mut widest = 0.0f32;
    for s in &born {
        assert!(s.pos.length() <= POUR_NOZZLE_M + 1e-4, "{s:?}");
        assert!(
            (s.vel.length() - PITCHER_SPEED_M_S).abs() < 1e-3,
            "{s:?}: not at the pitcher's speed"
        );
        let angle = s.vel.angle_between(Vec3::NEG_Y);
        assert!(angle <= spread + 1e-4, "{s:?}: {angle} rad off the pour");
        widest = widest.max(angle);
        assert!(
            (s.max_life - PRESET_LIFETIME).abs() < 1e-6,
            "{s:?}: not the full lifetime"
        );
        assert!(
            (s.life - 2.0).abs() < 1e-6,
            "free of the volume, like a burst"
        );
    }
    assert!(widest > spread * 0.7, "a cone, not a line: {widest}");
    // Where each lands: its first frame at the floor, within the cone's
    // width there (and a frame's slide).
    let width = POUR_NOZZLE_M + spread.tan() * (0.0 - FLOOR_TOP);
    let landed = |s: &Sample| s.life > 0.0 && s.pos.y <= FLOOR_TOP + MARGIN + 1e-3;
    let mut first: Vec<Option<Vec3>> = vec![None; 20_000];
    for cap in &caps[..90] {
        for (i, s) in cap.iter().enumerate() {
            if first[i].is_none() && landed(s) {
                first[i] = Some(s.pos);
            }
        }
    }
    let lands: Vec<Vec3> = first.iter().flatten().copied().collect();
    assert_eq!(lands.len(), (POUR_PER_FRAME * POUR_FRAMES) as usize);
    let across = lands.iter().map(|p| p.x.hypot(p.z)).fold(0.0f32, f32::max);
    assert!(
        across <= width + 0.003,
        "landed {across} m out, cone {width} m"
    );
    assert!(across > width * 0.5, "{across}: the cone's spread lost");
    // Resting: every poured particle alive at the floor, none below it.
    let rest = caps.last().expect("frame 150");
    let poured: Vec<&Sample> = rest.iter().filter(|s| s.life > 0.0).collect();
    assert_eq!(poured.len(), (POUR_PER_FRAME * POUR_FRAMES) as usize);
    for s in poured {
        assert!(
            (s.pos.y - (FLOOR_TOP + MARGIN)).abs() < 1e-3,
            "{s:?}: not resting on the floor"
        );
    }
    // The pour row zero: the throw's ball, unchanged.
    let ball = run_sim_with(
        &SimSetup {
            aux: pour_aux(false, POUR_PER_FRAME),
            ..dead_slots()
        },
        &SimOptions {
            aux_from: None,
            ..opts
        },
        &[1],
    );
    let born: Vec<&Sample> = ball[0].iter().filter(|s| s.life > 0.0).collect();
    assert_eq!(born.len(), POUR_PER_FRAME as usize);
    let scale = POUR_NOZZLE_M / 0.12;
    for s in born {
        assert!(s.pos.length() <= POUR_NOZZLE_M + 1e-4, "{s:?}");
        let speed = s.vel.length();
        assert!(
            speed >= 0.3 * scale - 1e-4 && speed <= 1.2 * scale + 1e-4,
            "{s:?}: not a throw's speed"
        );
        if s.pos.length() > 1e-3 {
            assert!(
                s.vel.normalize().dot(s.pos.normalize()) > 0.999,
                "{s:?}: not flying outward"
            );
        }
        assert!(
            (s.max_life - PRESET_LIFETIME * 0.5).abs() < 1e-6,
            "{s:?}: not a burst's life"
        );
    }
}
