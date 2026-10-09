//! The live depth map as a collision source (board #3352), on a desktop GPU:
//! the atlas compute pass and its copy into an RGBA8 texture against the
//! pass's CPU twin, and `flux_xr_sim.wgsl`'s
//! collide against a depth atlas, run headless behind the loader's compute
//! preamble; the particle pitcher's pour (board #3402) through the same
//! sim onto a floor box; and the surface behavior lanes (board #3326),
//! which move the room's emission from box to box; the box cell list
//! (board #3808) against the full box loop; and the flow samples lane in
//! its header (board #3810), which picks the flow's diagonals. The GPU tests are
//! `#[ignore]`d like the core's probes (they need an adapter); run them
//! with `cargo test -p fosfora-xr --test depth_collide_gpu -- --ignored`.
#![cfg(not(target_os = "android"))]

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use fosfora_xr::box_cells;
use fosfora_xr::env_depth::{
    ATLAS_WGSL, COLLIDE_ROWS, DepthCollide, DepthView, NEAR_CUT_M, atlas_params, atlas_row_texels,
    encode_atlas,
};
use fosfora_xr::instruments::{
    PITCHER_SPEED_M_S, POUR_NOZZLE_M, POUR_SPREAD_DEG, Pour, pour_row, rows as instrument_rows,
};
use fosfora_xr::math::Fov;
use fosfora_xr::surfaces::{
    KIND_FLOOR, KIND_TABLE, KIND_WALL, SURFACE_LANE_ROWS, SurfaceBehavior, lane_row,
};
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
const WORLD_AUX_ROWS: usize = 1238;
/// The first surface lane (`XR_AUX_SURFACE`).
const AUX_SURFACE: usize = 181;
/// The box cell list's header (`XR_AUX_CELLS`), its masks after it.
const AUX_CELLS: usize = 213;
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
/// them, then the surface lanes, one per box, then the box cell list,
/// ending where the XR app's upload does; the sim validates with them, its pour cone is the
/// pitcher's and its behavior ids are the catalogue's.
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
    assert_eq!(get("XR_AUX_POUR") + 1, get("XR_AUX_SURFACE"));
    assert_eq!(get("XR_AUX_SURFACE") as usize, AUX_SURFACE);
    assert_eq!(get("XR_AUX_SURFACE_ROWS"), get("XR_MAX_BOXES"));
    assert_eq!(get("XR_AUX_SURFACE_ROWS") as usize, SURFACE_LANE_ROWS);
    assert_eq!(
        get("XR_AUX_CELLS"),
        get("XR_AUX_SURFACE") + get("XR_AUX_SURFACE_ROWS")
    );
    assert_eq!(get("XR_AUX_CELLS") as usize, AUX_CELLS);
    assert_eq!(get("XR_AUX_CELL_ROWS"), 1024);
    assert_eq!(get("XR_AUX_CELL_ROWS") as usize, box_cells::CELL_ROWS);
    assert_eq!(get("XR_CELLS_MAX"), box_cells::CELLS_MAX);
    assert_eq!(
        get("XR_CELLS_MAX").pow(3),
        4 * get("XR_AUX_CELL_ROWS"),
        "four cells per row"
    );
    assert_eq!(
        (get("XR_AUX_CELLS") + 1 + get("XR_AUX_CELL_ROWS")) as usize,
        WORLD_AUX_ROWS
    );
    assert_eq!(AUX_CELLS + box_cells::ROWS, WORLD_AUX_ROWS);
    for (name, b) in [
        ("XR_BEHAVIOR_NONE", SurfaceBehavior::None),
        ("XR_BEHAVIOR_EMBERS", SurfaceBehavior::Embers),
        ("XR_BEHAVIOR_SPARKS", SurfaceBehavior::Sparks),
        ("XR_BEHAVIOR_SPECTRUM", SurfaceBehavior::Spectrum),
        // The sim keeps the rings' old name (board #3472 left the sim
        // alone); the id is what it reads.
        ("XR_BEHAVIOR_RIPPLE", SurfaceBehavior::Rings),
    ] {
        assert_eq!(get(name), b.id(), "{name}");
    }
    for (name, kind) in [
        ("XR_KIND_TABLE", KIND_TABLE),
        ("XR_KIND_FLOOR", KIND_FLOOR),
        ("XR_KIND_WALL", KIND_WALL),
    ] {
        assert_eq!(get(name), kind, "{name}");
    }
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

    /// `param(i)` for i in 4..8: `effect_params_1[i - 4]`.
    fn param(&mut self, i: usize, v: f32) {
        assert!((4..8).contains(&i), "param({i}) is not in effect_params_1");
        let at = self.offsets["effect_params_1"] + 4 * (i - 4);
        self.bytes[at..at + 4].copy_from_slice(&v.to_bits().to_le_bytes());
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
    /// Dead slots the emitter may fill each frame (`emit_count`).
    emit: u32,
    /// `param(6)` on: spawns go to the weighted boxes' top faces (Flux XR
    /// Room's `surface_emit`), with the bass at 1 and the beat phase at 0,
    /// so both the embers' and the sparks' gates are fully open.
    surface: bool,
    /// A flow field (on, at unit strength): the side of the cubic RGBA8
    /// texture and its texels (little-endian RGBA per `u32`), sampled
    /// with repeat as the core's is, at `flow_scale` (else no flow).
    flow: Option<(u32, Vec<u32>, f32)>,
    /// The clock at the first frame (s); each frame adds 1 / [`FPS`].
    time0: f32,
    /// `u.frame_index` at the first frame; each frame adds 1, as
    /// `ParticleSystem::flip` does.
    frame0: u32,
}

impl Default for SimOptions {
    fn default() -> Self {
        Self {
            drag: 1.0,
            lifetime: 100.0,
            dead: false,
            aux_from: None,
            emit: 0,
            surface: false,
            flow: None,
            time0: 0.0,
            frame0: 0,
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
    run_sim_raw(setup, opts, capture)
        .iter()
        .map(|[pos, vel, _, flags]| {
            let pos: &[[f32; 4]] = bytemuck::cast_slice(pos);
            let vel: &[[f32; 4]] = bytemuck::cast_slice(vel);
            let flags: &[[f32; 4]] = bytemuck::cast_slice(flags);
            pos.iter()
                .zip(vel)
                .zip(flags)
                .map(|((p, v), f)| Sample {
                    pos: Vec3::new(p[0], p[1], p[2]),
                    vel: Vec3::new(v[0], v[1], v[2]),
                    life: p[3],
                    max_life: f[1],
                })
                .collect()
        })
        .collect()
}

/// [`run_sim_with`]'s frames as the bytes of the four particle lanes
/// (position and life, velocity and size, color, flags).
fn run_sim_raw(setup: &SimSetup, opts: &SimOptions, capture: &[u32]) -> Vec<[Vec<u8>; 4]> {
    let (device, queue, _guard) = gpu();
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let source = sim_source();
    let module = naga::front::wgsl::parse_str(&source).expect("sim parses");
    let count = setup.particles.len() as u32;

    let mut u = Uniforms::new(&module);
    u.f32("delta_time", 1.0 / FPS);
    u.set("max_particles", count);
    u.set("emit_count", opts.emit);
    if opts.surface {
        u.param(6, 1.0);
        u.f32("bass", 1.0);
        u.f32("beat_phase", 0.0);
    }
    // A volume big enough that nothing respawns at its bounds.
    u.f32("emitter_radius", 5.0);
    u.f32("lifetime", opts.lifetime);
    u.f32("initial_size", 0.004);
    u.f32("size_end", 0.004);
    u.f32("drag", opts.drag);
    u.f32("flow_enabled", 0.0);
    if let Some((_, _, scale)) = &opts.flow {
        u.f32("flow_enabled", 1.0);
        u.f32("flow_strength", 1.0);
        u.f32("flow_scale", *scale);
        u.f32("flow_speed", 1.0);
    }
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

    let flow = match &opts.flow {
        Some((side, texels, _)) => rgba_texture(
            device,
            queue,
            [*side; 3],
            wgpu::TextureDimension::D3,
            texels,
        ),
        None => rgba_texture(device, queue, [1, 1, 1], wgpu::TextureDimension::D3, &[0]),
    };
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
    // The flow field repeats (the core's flow sampler); unused without one.
    let flow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
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
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&flow_sampler),
            },
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
        u.f32("time", opts.time0 + frame as f32 / FPS);
        u.set("frame_index", opts.frame0 + frame);
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
            out.push([0, 1, 2, 3].map(|i| read_buffer(device, queue, &ins[i])));
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
        ..SimOptions::default()
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

// ---- The surface behavior lanes (board #3326) ----------------------------------

/// The lane tests' boxes: a table centered at x = -1 and a floor at x = +1,
/// both 0.8 m square with their top faces at y = 0, 1.2 m apart, and a
/// wall 1.5 m behind them, its top edge at y = 1; all inside the volume.
const TABLE_X: f32 = -1.0;
const FLOOR_X: f32 = 1.0;
const FACE_HALF: f32 = 0.4;
const WALL_TOP: f32 = 1.0;
/// `XR_SURFACE_LIFT`: a surface newborn sits this far above its face.
const SURFACE_LIFT: f32 = 0.01;
const SLOTS: usize = 20_000;

/// A box as the lane tests place it: kind, center, half extents (axis
/// aligned, so the upward face is local +Y).
type TestBox = (u32, Vec3, Vec3);

fn table_box() -> TestBox {
    (
        KIND_TABLE,
        Vec3::new(TABLE_X, -0.3, 0.0),
        Vec3::new(FACE_HALF, 0.3, FACE_HALF),
    )
}

fn floor_box() -> TestBox {
    (
        KIND_FLOOR,
        Vec3::new(FLOOR_X, -0.05, 0.0),
        Vec3::new(FACE_HALF, 0.05, FACE_HALF),
    )
}

fn wall_box() -> TestBox {
    (
        KIND_WALL,
        Vec3::new(0.0, WALL_TOP * 0.5, -1.5),
        Vec3::new(0.8, WALL_TOP * 0.5, 0.02),
    )
}

/// The aux block for `boxes`, each weighted 1 (as the XR app weighs them
/// is `surfaces.rs`'s business; here the sim's gates alone decide), with
/// box k's lane set to `lanes[k]` (`None`, or past the list: unset).
fn room_aux(boxes: &[TestBox], lanes: &[Option<(SurfaceBehavior, f32)>]) -> Vec<[f32; 4]> {
    let mut aux = vec![[0.0; 4]; WORLD_AUX_ROWS];
    aux[1] = [
        f32::from_bits(0),
        f32::from_bits(boxes.len() as u32),
        RESTITUTION,
        MARGIN,
    ];
    for (k, &(kind, center, half)) in boxes.iter().enumerate() {
        aux[67 + k] = [center.x, center.y, center.z, kind as f32];
        aux[99 + k] = [0.0, 0.0, 0.0, 1.0];
        aux[131 + k] = [half.x, half.y, half.z, 1.0];
        if let Some(&Some((behavior, strength))) = lanes.get(k) {
            aux[AUX_SURFACE + k] = lane_row(behavior, strength, [0.0; 2]);
        }
    }
    aux
}

/// One frame of Embers over 20K dead slots, every one free to
/// emit: the newborns, as spawned (a slot born this frame is written as
/// the emitter made it, before any integration).
fn newborns(aux: Vec<[f32; 4]>) -> Vec<Sample> {
    let setup = SimSetup {
        particles: vec![(Vec3::new(0.0, 10.0, 0.0), Vec3::ZERO); SLOTS],
        aux,
        atlas: None,
        every: 1,
    };
    let opts = SimOptions {
        dead: true,
        emit: SLOTS as u32,
        surface: true,
        ..SimOptions::default()
    };
    run_sim_with(&setup, &opts, &[1])
        .pop()
        .expect("one capture")
        .into_iter()
        .filter(|s| s.life > 0.0)
        .collect()
}

/// Where a newborn was born.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Face {
    Table,
    Floor,
    /// The wall's top edge (its upward face).
    WallTop,
    Elsewhere,
}

fn face_of(p: Vec3) -> Face {
    let on = |x: f32, half_x: f32, z: f32, top: f32| {
        (p.y - (top + SURFACE_LIFT)).abs() < 1e-3
            && (p.x - x).abs() <= half_x + 1e-4
            && (p.z - z).abs() <= FACE_HALF + 1e-4
    };
    let (_, wall, wall_half) = wall_box();
    if on(TABLE_X, FACE_HALF, 0.0, 0.0) {
        Face::Table
    } else if on(FLOOR_X, FACE_HALF, 0.0, 0.0) {
        Face::Floor
    } else if (p.y - (WALL_TOP + SURFACE_LIFT)).abs() < 1e-3
        && (p.x - wall.x).abs() <= wall_half.x + 1e-4
        && (p.z - wall.z).abs() <= wall_half.z + 1e-4
    {
        Face::WallTop
    } else {
        Face::Elsewhere
    }
}

fn count(born: &[Sample], face: Face) -> usize {
    born.iter().filter(|s| face_of(s.pos) == face).count()
}

/// The sparks' velocity: up at 0.3 + 1.2 x bass (1.5 m/s at bass 1), a
/// little sideways.
fn is_spark(s: &Sample) -> bool {
    s.vel.y > 1.0 && s.vel.x.abs() <= 0.1 && s.vel.z.abs() <= 0.1
}

/// The embers' velocity: a slow slide across the face, nothing upward.
fn is_ember(s: &Sample) -> bool {
    s.vel.y.abs() < 1e-6 && s.vel.length() <= 0.03 * std::f32::consts::SQRT_2 + 1e-6
}

/// Unset lanes run the kinds' defaults (step 2d, decision #3459): the
/// table sheds embers as before the lanes, the floor's default is the
/// ripple, which spawns nothing, so every newborn is an ember on the
/// table's face and the floor's half of the draws spawns nothing. The
/// floor waits for a lane.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn unset_lanes_emit_from_the_table_alone_the_floor_waits_for_a_lane() {
    let born = newborns(room_aux(&[table_box(), floor_box()], &[]));
    assert!(born.len() > SLOTS / 3, "{} born", born.len());
    assert!(born.len() < SLOTS * 2 / 3, "{} born", born.len());
    for s in &born {
        assert_eq!(face_of(s.pos), Face::Table, "{s:?}");
        assert!(is_ember(s), "{s:?}: not an ember");
    }
}

/// The floor's lane set to sparks, the table's unset: both faces emit as
/// before step 2d, the table embers and the floor sparks, each over half
/// the draws with both gates open, and nothing is born anywhere else.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn a_floor_on_sparks_emits_beside_an_unset_table() {
    let born = newborns(room_aux(
        &[table_box(), floor_box()],
        &[None, Some((SurfaceBehavior::Sparks, 1.0))],
    ));
    let (table, floor) = (count(&born, Face::Table), count(&born, Face::Floor));
    assert_eq!(table + floor, born.len(), "a newborn off both faces");
    assert!(born.len() > SLOTS * 9 / 10, "{} born", born.len());
    for n in [table, floor] {
        assert!(n * 10 > born.len() * 4, "table {table} floor {floor}");
    }
    for s in &born {
        match face_of(s.pos) {
            Face::Table => assert!(is_ember(s), "{s:?}: not an ember"),
            _ => assert!(is_spark(s), "{s:?}: not a spark"),
        }
    }
}

/// The table on embers and the floor on none: every newborn on the
/// table's face, as an ember; the floor's draws spawn nothing.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn a_lane_of_none_turns_a_floor_off() {
    let born = newborns(room_aux(
        &[table_box(), floor_box()],
        &[
            Some((SurfaceBehavior::Embers, 1.0)),
            Some((SurfaceBehavior::None, 1.0)),
        ],
    ));
    assert!(born.len() > SLOTS / 3, "{} born", born.len());
    assert!(born.len() < SLOTS * 2 / 3, "{} born", born.len());
    for s in &born {
        assert_eq!(face_of(s.pos), Face::Table, "{s:?}");
        assert!(is_ember(s), "{s:?}: not an ember");
    }
}

/// The table on none and the floor on sparks: every newborn on the
/// floor's face, flying up.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn a_lane_of_none_turns_a_table_off() {
    let born = newborns(room_aux(
        &[table_box(), floor_box()],
        &[
            Some((SurfaceBehavior::None, 1.0)),
            Some((SurfaceBehavior::Sparks, 1.0)),
        ],
    ));
    assert!(born.len() > SLOTS / 3, "{} born", born.len());
    for s in &born {
        assert_eq!(face_of(s.pos), Face::Floor, "{s:?}");
        assert!(is_spark(s), "{s:?}: not a spark");
    }
}

/// The table on embers at half strength, the floor on embers at full: the
/// table's share of the newborns is its gated weight's share, a third
/// (0.5 against 1), and the floor sheds embers, not sparks.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn the_strength_scales_a_surface_share() {
    let born = newborns(room_aux(
        &[table_box(), floor_box()],
        &[
            Some((SurfaceBehavior::Embers, 0.5)),
            Some((SurfaceBehavior::Embers, 1.0)),
        ],
    ));
    let (table, floor) = (count(&born, Face::Table), count(&born, Face::Floor));
    assert_eq!(table + floor, born.len());
    // About 15K newborns: one standard deviation of the share is 0.4 %.
    assert!(born.len() > SLOTS * 2 / 3, "{} born", born.len());
    let share = table as f32 / born.len() as f32;
    assert!((share - 1.0 / 3.0).abs() < 0.03, "table share {share}");
    assert!(born.iter().all(is_ember), "a floor spark on embers");
}

/// A wall weighted 1: on embers it sheds from its top edge; on the
/// spectrum, or with its lane unset (its default, the spectrum, through
/// the same gate since step 2d; a fixed 0.3 gate before), nothing is born
/// from it, and the table beside it keeps emitting.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn a_wall_on_the_spectrum_emits_nothing() {
    let boxes = [table_box(), wall_box()];
    let embers = newborns(room_aux(
        &boxes,
        &[None, Some((SurfaceBehavior::Embers, 1.0))],
    ));
    assert!(
        count(&embers, Face::WallTop) > SLOTS / 20,
        "the wall's face"
    );
    for lanes in [&[None, Some((SurfaceBehavior::Spectrum, 1.0))][..], &[]] {
        let born = newborns(room_aux(&boxes, lanes));
        assert_eq!(count(&born, Face::WallTop), 0, "{lanes:?}");
        assert!(count(&born, Face::Table) > SLOTS / 3, "{} born", born.len());
        assert_eq!(count(&born, Face::Table), born.len());
    }
}

/// `aux` with every box's emitter weight set to 0: the weights the XR app
/// gives boxes none of which emits (`surfaces::emitter_weights` weighs
/// only embers and sparks).
fn unweighted(mut aux: Vec<[f32; 4]>) -> Vec<[f32; 4]> {
    for row in &mut aux[131..131 + SURFACE_LANE_ROWS] {
        row[3] = 0.0;
    }
    aux
}

/// The living particles after each frame in `capture` of Embers
/// over 20K dead slots, every one free to emit each frame, with `aux`
/// throughout.
fn alive_over(aux: Vec<[f32; 4]>, capture: &[u32]) -> Vec<usize> {
    let setup = SimSetup {
        particles: vec![(Vec3::new(0.0, 10.0, 0.0), Vec3::ZERO); SLOTS],
        aux,
        atlas: None,
        every: 1,
    };
    let opts = SimOptions {
        dead: true,
        emit: SLOTS as u32,
        surface: true,
        ..SimOptions::default()
    };
    run_sim_with(&setup, &opts, capture)
        .iter()
        .map(|frame| frame.iter().filter(|s| s.life > 0.0).count())
        .collect()
}

/// Every lane `none` with boxes present (Kevin's "All: none", step 2e):
/// nothing is born over two seconds, whether the boxes carry the app's
/// weights for it (all 0, which used to send every spawn into the volume)
/// or a weight each (the gates alone close them). The stage floor alone on
/// its default, the ripple, is as silent (a device run with no anchors);
/// on sparks (the sweeps' `#0=sparks`) it spawns.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn every_lane_none_with_boxes_spawns_nothing() {
    use SurfaceBehavior as B;
    let capture = [1, 30, 60, 120];
    let room = [table_box(), floor_box(), wall_box()];
    let none = [Some((B::None, 1.0)); 3];
    for aux in [
        unweighted(room_aux(&room, &none)),
        room_aux(&room, &none),
        unweighted(room_aux(&[floor_box()], &[])),
    ] {
        assert_eq!(alive_over(aux, &capture), [0; 4]);
    }
    let sparks = newborns(room_aux(&[floor_box()], &[Some((B::Sparks, 1.0))]));
    assert!(sparks.len() > SLOTS * 9 / 10, "{} born", sparks.len());
    assert_eq!(count(&sparks, Face::Floor), sparks.len());
}

/// No boxes at all with surface emit on (a run with no room: no anchors,
/// no stage floor): the volume path, as before step 2e. Every free slot is
/// born, spread through the volume.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn no_boxes_spawns_in_the_volume() {
    let born = newborns(room_aux(&[], &[]));
    assert!(born.len() > SLOTS * 9 / 10, "{} born", born.len());
    let (above, below) = (
        born.iter().filter(|s| s.pos.y > 1.0).count(),
        born.iter().filter(|s| s.pos.y < -1.0).count(),
    );
    for n in [above, below] {
        assert!(n * 4 > born.len(), "above {above} below {below}");
    }
}

// ---- The box cell list (board #3808) -----------------------------------------

/// The cell list test's room, as (center, rotation box -> world, half
/// extents): a floor reaching past the grid, a turned table standing on
/// it, a book on the table, a tilted lamp overlapping the table, two
/// turned walls (one tilted as a scan never is), a wall 3 m out, past
/// the grid, that only the full loop meets, and a shelf just past two
/// cell borders. The book's cells start at
/// y = -0.375 (8 cells over +-1.5 m): a particle inside the table's top
/// below that, pushed up out of it, lands in the book, a box its first
/// cell does not list.
fn cell_room() -> Vec<([f32; 3], [f32; 4], [f32; 3])> {
    let q = |axis: Vec3, deg: f32| {
        glam::Quat::from_axis_angle(axis.normalize(), deg.to_radians()).to_array()
    };
    vec![
        ([0.0, -1.05, 0.0], q(Vec3::Y, 0.0), [2.5, 0.05, 2.5]),
        ([0.4, -0.65, -0.3], q(Vec3::Y, 20.0), [0.6, 0.35, 0.4]),
        BOOK,
        (
            [0.7, -0.25, -0.2],
            q(Vec3::new(1.0, 2.0, 0.5), 35.0),
            [0.15, 0.12, 0.15],
        ),
        ([0.0, 0.2, -1.4], q(Vec3::Y, 10.0), [2.0, 1.3, 0.05]),
        (
            [-1.3, 0.2, 0.1],
            (glam::Quat::from_rotation_y(80f32.to_radians())
                * glam::Quat::from_rotation_x(5f32.to_radians()))
            .to_array(),
            [2.0, 1.3, 0.05],
        ),
        ([3.0, 0.0, 0.0], q(Vec3::Y, 0.0), [0.05, 2.0, 2.0]),
        SHELF,
    ]
}

/// The cell list's grid half extent in the test (the presets' 1.5 m).
const CELL_HALF: f32 = 1.5;
/// A shelf whose dilated extent ends 0.8 mm past the cell borders at
/// x = 0.375 and z = 0.75 (8 and 16 cells over +-1.5 m): its corners there
/// sit in the next cells, which only bounds that reach past the extent by
/// the pad list it in.
const SHELF: ([f32; 3], [f32; 4], [f32; 3]) = (
    [0.375 + 0.0008 - 0.1, 0.5, 0.75 + 0.0008 - 0.1],
    [0.0, 0.0, 0.0, 1.0],
    [0.1 - MARGIN, 0.05, 0.1 - MARGIN],
);
/// The book on the table (its top at y = -0.3): turned with it, on it.
const BOOK: ([f32; 3], [f32; 4], [f32; 3]) = (
    [0.4, -0.2, -0.3],
    // 20 degrees about +Y.
    [0.0, 0.173_648_18, 0.0, 0.984_807_8],
    [0.2, 0.1, 0.15],
);

/// The aux block for [`cell_room`] with the box cell list at `cells` per
/// axis (0: no grid, the full loop), packed by the XR app's own builder;
/// `boxes` false uploads no boxes at all (what the room changes).
fn cell_aux(cells: u32, boxes: bool) -> Vec<[f32; 4]> {
    let room = if boxes { cell_room() } else { Vec::new() };
    let mut aux = vec![[0.0; 4]; WORLD_AUX_ROWS];
    aux[1] = [
        f32::from_bits(0),
        f32::from_bits(room.len() as u32),
        RESTITUTION,
        MARGIN,
    ];
    aux[2] = [0.0, 0.0, SETTLE, 0.0];
    for (k, &(c, q, h)) in room.iter().enumerate() {
        aux[67 + k] = [c[0], c[1], c[2], 0.0];
        aux[99 + k] = q;
        aux[131 + k] = [h[0], h[1], h[2], 0.0];
    }
    aux[AUX_CELLS..WORLD_AUX_ROWS]
        .copy_from_slice(&box_cells::rows(&room, MARGIN, cells, CELL_HALF));
    aux
}

/// Particles through and beyond the grid: 4000 uniform over +-2.5 m; 500
/// in and around each box (its dilated extent times 1.2, in its own
/// frame), all at random speeds up to 1 m/s per axis; at rest, each box's
/// corners and 20 points along each edge a hair inside its dilated
/// extent (where its bounds are reached); and 300 inside the table's top
/// under the book, between y = -0.45 and -0.38, at rest. Deterministic.
fn cell_particles() -> Vec<(Vec3, Vec3)> {
    let mut s = 0x2545_f491_u32;
    let mut next = move || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        (s >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    };
    let mut out = Vec::new();
    for _ in 0..4000 {
        let p = Vec3::new(next(), next(), next()) * 2.5;
        out.push((p, Vec3::new(next(), next(), next())));
    }
    for (c, q, h) in cell_room() {
        let (c, q, h) = (Vec3::from(c), glam::Quat::from_array(q), Vec3::from(h));
        for _ in 0..500 {
            let l = Vec3::new(next(), next(), next()) * (h + Vec3::splat(MARGIN)) * 1.2;
            out.push((c + q * l, Vec3::new(next(), next(), next())));
        }
        let e = (h + Vec3::splat(MARGIN)) * 0.999;
        for corner in 0..8u32 {
            let sign = |b: u32| if corner & b == 0 { 1.0 } else { -1.0 };
            let l = Vec3::new(sign(1), sign(2), sign(4)) * e;
            out.push((c + q * l, Vec3::ZERO));
            // The three edges along the axes from this corner, each once
            // (from the corners with that axis's bit clear).
            for axis in 0..3 {
                if corner & (1 << axis) != 0 {
                    continue;
                }
                for t in 0..20 {
                    let mut l = l;
                    l[axis] = e[axis] * (1.0 - 2.0 * (t as f32 + 0.5) / 20.0);
                    out.push((c + q * l, Vec3::ZERO));
                }
            }
        }
    }
    let (c, q, h) = (
        Vec3::from(BOOK.0),
        glam::Quat::from_array(BOOK.1),
        Vec3::from(BOOK.2),
    );
    for _ in 0..300 {
        let l = Vec3::new(next() * h.x, 0.0, next() * h.z);
        let mut p = c + q * l;
        p.y = -0.415 + 0.035 * next();
        out.push((p, Vec3::ZERO));
    }
    out
}

/// The box loop through the cell list is the full loop, bit for bit: the
/// same particles over the same room, stepped three frames with no grid,
/// with 8 cells per axis (the default), 16 (the most) and 2, give
/// byte-identical particle buffers (all four lanes) after every frame.
/// The room changes the result for over a thousand particles (against the
/// same run with no boxes), some outside the grid, so the boxes, and the
/// full loop past the grid, are exercised; and at 8 cells most cells skip
/// most boxes, so the lists are not trivially full. The particles under
/// the book need the mask re-read after a push, the ones at the boxes'
/// corners and edges (the shelf's within a millimeter of a cell border)
/// the bounds to reach them.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn the_cell_list_moves_every_particle_as_the_full_loop_does() {
    let particles = cell_particles();
    let opts = SimOptions {
        drag: PRESET_DRAG,
        ..SimOptions::default()
    };
    let run = |cells, boxes| {
        let setup = SimSetup {
            particles: particles.clone(),
            aux: cell_aux(cells, boxes),
            atlas: None,
            every: 1,
        };
        run_sim_raw(&setup, &opts, &[1, 2, 3])
    };
    let full = run(0, true);
    assert_eq!(full.len(), 3);
    for cells in [8, 16, 2] {
        let listed = run(cells, true);
        for (frame, (a, b)) in full.iter().zip(&listed).enumerate() {
            for lane in 0..4 {
                let differ = a[lane]
                    .chunks_exact(16)
                    .zip(b[lane].chunks_exact(16))
                    .filter(|(x, y)| x != y)
                    .count();
                assert_eq!(
                    differ,
                    0,
                    "{cells} cells, frame {}, lane {lane}: {differ} particles differ from the full loop",
                    frame + 1
                );
            }
        }
    }

    // What the boxes did: particles whose frame-1 position or velocity
    // differs from the same run in an empty room.
    let empty = run(0, false);
    let touched: Vec<usize> = (0..particles.len())
        .filter(|&i| {
            let at = |bytes: &[u8]| bytes[i * 16..i * 16 + 16].to_vec();
            at(&full[0][0]) != at(&empty[0][0]) || at(&full[0][1]) != at(&empty[0][1])
        })
        .collect();
    let outside = touched
        .iter()
        .filter(|&&i| particles[i].0.abs().max_element() > CELL_HALF)
        .count();
    assert!(touched.len() > 1000, "only {} touched", touched.len());
    assert!(outside > 50, "only {outside} touched outside the grid");

    // The lists are short: at 8 cells, the share of cells listing at most
    // two of the eight boxes.
    let rows = &cell_aux(8, true)[AUX_CELLS + 1..AUX_CELLS + 1 + 128];
    let short = rows
        .iter()
        .flatten()
        .filter(|m| m.to_bits().count_ones() <= 2)
        .count();
    assert!(short > 400, "{short} of 512 cells list two boxes or fewer");
    eprintln!(
        "cell list: {} particles, {} touched by the room ({outside} outside the grid), {short} of 512 cells list <= 2 boxes",
        particles.len(),
        touched.len()
    );
}

// ---- The flow samples lane ----------------------------------------------------

/// The sim's flow samples modes are [`box_cells::FlowSamples`]'s ids.
#[test]
fn the_flow_samples_modes_match_the_sim() {
    let module = naga::front::wgsl::parse_str(&sim_source()).expect("flux_xr_sim.wgsl parses");
    for (name, mode) in [
        ("XR_FLOW_TWO", box_cells::FlowSamples::Two),
        ("XR_FLOW_ONE", box_cells::FlowSamples::One),
        ("XR_FLOW_ALT", box_cells::FlowSamples::Alternate),
        ("XR_FLOW_ALT_FRAME", box_cells::FlowSamples::AlternateFrame),
    ] {
        assert_eq!(sim_const(&module, name), mode.id(), "{name}");
    }
    assert_eq!(
        box_cells::header(8, 1.5)[box_cells::FLOW_SAMPLES_LANE].to_bits(),
        0
    );
}

/// Each particle's flow (m/s) on one frame at `frame_index` `frame0`,
/// with the flow samples lane at `mode` (`None`: unwritten, the 0 a
/// hand-built row has): at rest, no drag, no features and the turbulence
/// cut, the velocity after the frame is the flow times dt.
fn flow_of(particles: &[Vec3], mode: Option<box_cells::FlowSamples>, frame0: u32) -> Vec<Vec3> {
    // 8^3 texels of a fixed hash: a field that changes from texel to texel.
    let texels: Vec<u32> = (0..512u32)
        .map(|i| {
            let mut h = i.wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b;
            h ^= h >> 15;
            h = h.wrapping_mul(0x2c1b_3c6d);
            h ^= h >> 12;
            h | 0xff00_0000
        })
        .collect();
    let mut aux = vec![[0.0; 4]; WORLD_AUX_ROWS];
    // The sim-cut mask: turbulence (XR_CUT_TURB) only.
    aux[2][0] = f32::from_bits(2);
    if let Some(mode) = mode {
        mode.write(&mut aux[AUX_CELLS]);
    }
    let setup = SimSetup {
        particles: particles.iter().map(|&p| (p, Vec3::ZERO)).collect(),
        aux,
        atlas: None,
        every: 1,
    };
    // A scale of 4 puts the particles' +-1 m over about six texels per
    // axis; at 2 s the drift (0.5 m) moves each diagonal's sample over a
    // texel or more, so the two diagonals read different texels.
    let opts = SimOptions {
        flow: Some((8, texels, 4.0)),
        time0: 2.0,
        frame0,
        ..SimOptions::default()
    };
    run_sim_with(&setup, &opts, &[1])[0]
        .iter()
        .map(|s| s.vel * FPS)
        .collect()
}

/// The flow samples lane picks the diagonals (board #3810): an unwritten
/// lane and a lane of 0 take the two samples' average; 1 takes the first
/// diagonal (a) alone, as is; 2 takes a when (frame + idx) is even and the
/// second (b) otherwise, as is, so neighboring particles take different
/// diagonals on the same frame and each takes the other on the next frame;
/// and the pair is (a + b) / sqrt(2) of the single samples.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn the_flow_samples_lane_picks_the_diagonals() {
    use box_cells::FlowSamples;
    let mut s = 0x1234_5679_u32;
    let mut next = move || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        (s >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    };
    let particles: Vec<Vec3> = (0..512)
        .map(|_| Vec3::new(next(), next(), next()))
        .collect();
    let unwritten = flow_of(&particles, None, 0);
    let two = flow_of(&particles, Some(FlowSamples::Two), 0);
    let one = flow_of(&particles, Some(FlowSamples::One), 0);
    let alt = [0, 1].map(|f| flow_of(&particles, Some(FlowSamples::Alternate), f));
    let by_frame = [0, 1].map(|f| flow_of(&particles, Some(FlowSamples::AlternateFrame), f));
    let close = |x: Vec3, y: Vec3| (x - y).abs().max_element() < 1e-4;
    let mut differ = 0;
    for i in 0..particles.len() {
        assert_eq!(
            unwritten[i], two[i],
            "particle {i}: a 0 lane is the unwritten one"
        );
        // On the frame where (frame + i) is even, the alternate is a, times
        // sqrt(2) so two frames of it move the particle as the pair does.
        let (even, odd) = if i % 2 == 0 { (0, 1) } else { (1, 0) };
        let a = alt[even][i] * std::f32::consts::FRAC_1_SQRT_2;
        let b = alt[odd][i] * std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            close(a, one[i]),
            "particle {i}: {a} is not the first diagonal {}",
            one[i]
        );
        assert!(
            close(two[i], (a + b) * std::f32::consts::FRAC_1_SQRT_2),
            "particle {i}: the pair {} is not (a + b) / sqrt(2) of {a} and {b}",
            two[i]
        );
        if !close(a, b) {
            differ += 1;
        }
        // By the frame alone: every particle takes a on frame 0, b on 1.
        assert!(
            close(by_frame[0][i] * std::f32::consts::FRAC_1_SQRT_2, a)
                && close(by_frame[1][i] * std::f32::consts::FRAC_1_SQRT_2, b),
            "particle {i}: the by-frame mode does not take a then b"
        );
    }
    // The diagonals read different texels for nearly every particle, so the
    // checks above tell a from b; and on one frame, neighbors alternate.
    assert!(differ > 450, "only {differ} of 512 particles see a != b");
    let mixed = (0..particles.len() - 1)
        .filter(|&i| {
            let first = |j: usize| close(alt[0][j] * std::f32::consts::FRAC_1_SQRT_2, one[j]);
            first(i) != first(i + 1)
        })
        .count();
    assert!(
        mixed > 400,
        "only {mixed} neighbors take different diagonals"
    );
    let max = one
        .iter()
        .map(|v| v.abs().max_element())
        .fold(0.0, f32::max);
    assert!(max > 0.1, "the flow is too weak to tell apart: {max}");
}
