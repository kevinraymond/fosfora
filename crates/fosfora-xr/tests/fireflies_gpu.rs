//! The fireflies sim (board #5470) on a desktop GPU:
//! `fireflies_xr_sim.wgsl` run headless behind the loader's compute
//! preamble, with the rows the XR app uploads. The caps under a pool that
//! is hammered with bursts, the count knob, the fireflies' calm motion and
//! their answer to the music, and the hand particles: the arc, the landing
//! without a bounce, and the edge flow along a wall's foot, round a table's
//! rim and down a corner. The air a theme makes of the fireflies (board
//! #5543): the stars, the rise, the drift, the orbit, an air by band, and
//! a change of theme. One test runs the preset itself through the
//! core: its loader, its particle system and its world draw, with the aux
//! block the XR app uploads. The tests are `#[ignore]`d like the core's probes
//! (they need an adapter); run them with
//! `cargo test -p fosfora-xr --test fireflies_gpu -- --ignored`.
#![cfg(not(target_os = "android"))]

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use fosfora_xr::fireflies::{
    BAND_COLORS, EDGE_BAND_M, FIREFLY_CAP, Knobs, Look, POOL_CAP, hand_steal, orbit_step,
};
use fosfora_xr::instruments::{Burst, MAX_STEAL, aim_row, rows as instrument_rows};
use fosfora_xr::themes::{Motion, Theme};
use glam::Vec3;

static GPU: OnceLock<(wgpu::Device, wgpu::Queue)> = OnceLock::new();
static LOCK: Mutex<()> = Mutex::new(());

/// One shared device and a lock, so each test's error scope is its own.
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
            label: Some("xr-fireflies-test"),
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
const SIM: &str = include_str!("../../../assets/xr/shaders/fireflies_xr_sim.wgsl");
/// Rows the XR app uploads (`WORLD_AUX_ROWS` in `scene.rs`).
const WORLD_AUX_ROWS: usize = 1242;
/// The instrument rows, the box rows and the fireflies' rows.
const AUX_INSTRUMENTS: usize = 170;
const AUX_POUR: usize = 180;
const AUX_BOX_CENTER: usize = 67;
const AUX_BOX_ROT: usize = 99;
const AUX_BOX_HALF: usize = 131;
const AUX_FIREFLIES: usize = 1238;
const FPS: f32 = 72.0;
const DT: f32 = 1.0 / FPS;
/// The preset's numbers the sim reads from its uniforms.
const HALF_M: f32 = 3.0;
const LIFETIME_S: f32 = 6.0;
const SPRITE_M: f32 = 0.01;
const DRAG: f32 = 0.99;
/// The collide's margin, as the XR app sets it (the test sim's sprite).
const MARGIN: f32 = 0.004;
/// The sim's life lane.
const FREE: f32 = 2.0;

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

/// A 1x1 RGBA8 texture of zeros: the core's placeholder for the flow
/// field, the obstacle texture (no depth atlas), water and fluid.
fn blank(device: &wgpu::Device, dimension: wgpu::TextureDimension) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("sim-test-blank"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// One particle as the sim's lanes hold it.
#[derive(Debug, Clone, Copy)]
struct Sample {
    pos: Vec3,
    vel: Vec3,
    /// 0 dead, 1 a firefly, 2 and above a hand particle.
    life: f32,
    size: f32,
    alpha: f32,
    /// A hand particle's age (s); a firefly's presence.
    age: f32,
    /// Its color, and for a particle of the air the stamp and the motion
    /// it was born with (board #5543).
    rgb: Vec3,
    stamp: f32,
    motion: f32,
    /// A rising particle's age (s).
    clock: f32,
}

impl Sample {
    fn firefly(&self) -> bool {
        self.life > 0.5 && self.life < 1.5
    }

    fn hand(&self) -> bool {
        self.life > 1.5
    }
}

/// A living hand particle to start a run with.
#[derive(Debug, Clone, Copy)]
struct Seed {
    pos: Vec3,
    vel: Vec3,
    /// Its age (s): past the grace, so a test's spheres would push it.
    age: f32,
}

fn seed(pos: Vec3, vel: Vec3) -> Seed {
    Seed { pos, vel, age: 1.0 }
}

/// The sim behind the loader's preamble, with the bindings the core's
/// `ParticleSystem` gives it (group 0: uniforms, the particle lanes in and
/// out, counters, aux, the dead and alive lists; group 1: the flow field,
/// the obstacle texture, water and fluid, all placeholders), stepped at
/// [`FPS`] with the preset's uniforms.
struct Sim {
    device: &'static wgpu::Device,
    queue: &'static wgpu::Queue,
    _guard: MutexGuard<'static, ()>,
    u: Uniforms,
    uniforms: wgpu::Buffer,
    ins: Vec<wgpu::Buffer>,
    outs: Vec<wgpu::Buffer>,
    counters: wgpu::Buffer,
    aux_buf: wgpu::Buffer,
    pipeline: wgpu::ComputePipeline,
    groups: [wgpu::BindGroup; 2],
    count: u32,
    frame: u32,
    /// `u.rms`, the loudness as the flash limit lets it through.
    rms: f32,
    /// `u.bass`, `u.mid` and `u.presence`, the same way: the bands a
    /// theme's air follows (board #5543).
    bands: [f32; 3],
}

impl Sim {
    /// A pool of `count` slots: `seeds` alive as hand particles in the
    /// last slots, the rest dead.
    fn new(count: u32, seeds: &[Seed], aux: &[[f32; 4]]) -> Self {
        let (device, queue, guard) = gpu();
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let source = format!("{}\n{SIM}", PREAMBLE.join("\n"));
        let module = naga::front::wgsl::parse_str(&source).expect("sim parses");
        let mut u = Uniforms::new(&module);
        u.f32("delta_time", DT);
        u.set("max_particles", count);
        u.f32("emitter_radius", HALF_M);
        u.f32("lifetime", LIFETIME_S);
        u.f32("initial_size", SPRITE_M);
        u.f32("size_end", 0.002);
        u.f32("drag", DRAG);
        u.f32("flow_enabled", 0.0);
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sim-test-uniforms"),
            size: u.bytes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let n = count as usize;
        let mut lanes = [
            vec![[0.0f32; 4]; n],
            vec![[0.0f32; 4]; n],
            vec![[0.0f32; 4]; n],
            vec![[0.0f32; 4]; n],
        ];
        for (k, s) in seeds.iter().enumerate() {
            let i = n - seeds.len() + k;
            lanes[0][i] = [s.pos.x, s.pos.y, s.pos.z, FREE];
            lanes[1][i] = [s.vel.x, s.vel.y, s.vel.z, SPRITE_M];
            lanes[2][i] = [1.0, 0.9, 0.8, 0.35];
            lanes[3][i] = [s.age, 100.0, SPRITE_M, 0.35];
        }
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
        let mut rows = aux.to_vec();
        rows.resize(WORLD_AUX_ROWS, [0.0; 4]);
        let aux_buf = storage("sim-test-aux", bytemuck::cast_slice(&rows));
        let dead = storage("sim-test-dead", &vec![0u8; n * 4]);
        let alive = storage("sim-test-alive", &vec![0u8; n * 4]);

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
        let flow = blank(device, wgpu::TextureDimension::D3);
        let flat = blank(device, wgpu::TextureDimension::D2);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
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
                view(2, &flat),
                samp(3),
                view(4, &flat),
                view(5, &flat),
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fireflies-xr-sim"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sim-test"),
            bind_group_layouts: &[&layout0, &layout1],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("fireflies-xr-sim"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("cs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        Self {
            device,
            queue,
            _guard: guard,
            u,
            uniforms,
            ins,
            outs,
            counters,
            aux_buf,
            pipeline,
            groups: [group0, group1],
            count,
            frame: 0,
            rms: 0.0,
            bands: [0.0; 3],
        }
    }

    /// Replace rows `at..` of the aux block.
    fn write_aux(&self, at: usize, rows: &[[f32; 4]]) {
        self.queue
            .write_buffer(&self.aux_buf, at as u64 * 16, bytemuck::cast_slice(rows));
    }

    /// One frame: the dispatch, then this frame's output becomes the next
    /// one's input, as the core's ping-pong does.
    fn step(&mut self) {
        let t = self.frame as f32 * DT;
        self.u.f32("time", t);
        // The core's seed: it moves with time.
        self.u.f32("seed", t * 1000.0 % 65536.0);
        self.u.set("frame_index", self.frame);
        self.u.f32("rms", self.rms);
        self.u.f32("bass", self.bands[0]);
        self.u.f32("mid", self.bands[1]);
        self.u.f32("presence", self.bands[2]);
        self.queue.write_buffer(&self.uniforms, 0, &self.u.bytes);
        self.queue.write_buffer(&self.counters, 0, &[0u8; 16]);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.groups[0], &[]);
            pass.set_bind_group(1, &self.groups[1], &[]);
            pass.dispatch_workgroups(self.count.div_ceil(256), 1, 1);
        }
        for (i, o) in self.ins.iter().zip(&self.outs) {
            encoder.copy_buffer_to_buffer(o, 0, i, 0, i.size());
        }
        self.queue.submit([encoder.finish()]);
        self.frame += 1;
    }

    fn run(&mut self, seconds: f32) {
        for _ in 0..(seconds * FPS).round() as u32 {
            self.step();
        }
    }

    /// Every slot as the last frame left it.
    fn read(&self) -> Vec<Sample> {
        let lanes: Vec<Vec<u8>> = self
            .ins
            .iter()
            .map(|b| read_buffer(self.device, self.queue, b))
            .collect();
        let lane = |i: usize| -> &[[f32; 4]] { bytemuck::cast_slice(&lanes[i]) };
        (0..self.count as usize)
            .map(|i| Sample {
                pos: Vec3::new(lane(0)[i][0], lane(0)[i][1], lane(0)[i][2]),
                vel: Vec3::new(lane(1)[i][0], lane(1)[i][1], lane(1)[i][2]),
                life: lane(0)[i][3],
                size: lane(1)[i][3],
                alpha: lane(2)[i][3],
                age: lane(3)[i][0],
                rgb: Vec3::new(lane(2)[i][0], lane(2)[i][1], lane(2)[i][2]),
                stamp: lane(3)[i][1],
                motion: lane(3)[i][2],
                clock: lane(3)[i][3],
            })
            .collect()
    }

    /// The alive count the sim's `mark_alive` built on the last frame,
    /// which bounds the draw.
    fn alive_counter(&self) -> u32 {
        let bytes = read_buffer(self.device, self.queue, &self.counters);
        bytemuck::cast_slice::<u8, u32>(&bytes)[0]
    }

    /// End the run: no validation error on the way.
    fn finish(self) {
        wait(self.device);
        let err = pollster::block_on(self.device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }
}

/// A box for the collide: center, half extents, upright.
#[derive(Debug, Clone, Copy)]
struct TestBox {
    center: Vec3,
    half: Vec3,
}

/// The floor: its top face at y = 0, 20 m across, as the stage floor is.
fn floor() -> TestBox {
    TestBox {
        center: Vec3::new(0.0, -0.05, 0.0),
        half: Vec3::new(10.0, 0.05, 10.0),
    }
}

/// A wall across the room at z = `z`, its face toward +Z, 2.5 m high.
fn wall_at_z(z: f32) -> TestBox {
    TestBox {
        center: Vec3::new(0.0, 1.25, z - 0.02),
        half: Vec3::new(3.0, 1.25, 0.02),
    }
}

/// A wall along the room at x = `x`, its face toward +X.
fn wall_at_x(x: f32) -> TestBox {
    TestBox {
        center: Vec3::new(x - 0.02, 1.25, 0.0),
        half: Vec3::new(0.02, 1.25, 3.0),
    }
}

/// A table 0.8 m by 0.6 m, its top at 0.7 m, standing on the floor with
/// its middle at (`x`, `z`).
fn table_at(x: f32, z: f32) -> TestBox {
    TestBox {
        center: Vec3::new(x, 0.35, z),
        half: Vec3::new(0.4, 0.35, 0.3),
    }
}

/// The fireflies' rows for `knobs` at `density` and `swell` in a room with
/// no theme: the air's two rows all zero.
fn fly_rows(knobs: &Knobs, density: f32, swell: f32) -> [[f32; 4]; 4] {
    knobs.rows(density, swell, &Look::default(), 0.0)
}

/// The aux block for `boxes` and `knobs` (density 1, the swell at 0): the
/// header (no spheres, the boxes, the margin), the box rows, no cell list
/// (every particle against every box), and the fireflies' rows.
fn aux_for(boxes: &[TestBox], knobs: &Knobs) -> Vec<[f32; 4]> {
    let mut aux = vec![[0.0f32; 4]; WORLD_AUX_ROWS];
    aux[1] = [0.0, f32::from_bits(boxes.len() as u32), 0.4, MARGIN];
    for (k, b) in boxes.iter().enumerate() {
        aux[AUX_BOX_CENTER + k] = [b.center.x, b.center.y, b.center.z, 0.0];
        aux[AUX_BOX_ROT + k] = [0.0, 0.0, 0.0, 1.0];
        aux[AUX_BOX_HALF + k] = [b.half.x, b.half.y, b.half.z, 0.0];
    }
    aux[AUX_FIREFLIES..].copy_from_slice(&fly_rows(knobs, 1.0, 0.0));
    aux
}

/// The knobs with no fireflies: a run of hand particles alone.
fn hands_only() -> Knobs {
    Knobs {
        count: 0,
        ..Knobs::default()
    }
}

/// The angle between two velocities (degrees); 0 when either is still.
fn turn_deg(a: Vec3, b: Vec3) -> f32 {
    if a.length() < 1e-6 || b.length() < 1e-6 {
        return 0.0;
    }
    a.angle_between(b).to_degrees()
}

/// Neither cap gives, whatever is asked: the fireflies' rows ask for more
/// than a u32 can count, and every frame a burst asks for more slots than
/// the pool has, with the steal at its most. The fireflies settle at their
/// cap exactly, the pool fills to its last slot, and the alive list, which
/// bounds the draw, never holds more than the pool.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn neither_cap_gives_under_a_pool_hammered_with_bursts() {
    let mut aux = aux_for(&[floor()], &Knobs::default());
    aux[AUX_FIREFLIES][0] = f32::from_bits(u32::MAX);
    let burst = Burst {
        center: Vec3::new(0.0, 1.0, -1.0),
        radius: 0.12,
        count: 50_000,
        hand: 1,
        impact: true,
        brightness: 1.0,
        back: Vec3::ZERO,
    };
    let mut rows = instrument_rows(Some(burst), None, Vec3::ZERO);
    rows[0][3] = MAX_STEAL;
    aux[AUX_INSTRUMENTS..AUX_INSTRUMENTS + 3].copy_from_slice(&rows);
    let mut sim = Sim::new(POOL_CAP, &[], &aux);
    let mut most = (0usize, 0usize, 0u32);
    for second in 0..12 {
        sim.run(1.0);
        let all = sim.read();
        let flies = all.iter().filter(|s| s.firefly()).count();
        let hands = all.iter().filter(|s| s.hand()).count();
        let listed = sim.alive_counter();
        assert!(
            flies <= FIREFLY_CAP as usize,
            "{flies} fireflies at {second} s"
        );
        assert!(
            flies + hands <= POOL_CAP as usize,
            "{} alive",
            flies + hands
        );
        assert!(listed <= POOL_CAP, "{listed} on the alive list");
        // A firefly only ever lives in a slot under the cap.
        for (i, s) in all.iter().enumerate() {
            assert!(
                !s.firefly() || i < FIREFLY_CAP as usize,
                "a firefly in slot {i}"
            );
        }
        most = (most.0.max(flies), most.1.max(hands), most.2.max(listed));
    }
    eprintln!(
        "most alive: {} fireflies, {} hand particles, {} listed",
        most.0, most.1, most.2
    );
    // The caps are reached, so the test did push against them.
    assert_eq!(most.0, FIREFLY_CAP as usize);
    assert_eq!(most.1, (POOL_CAP - FIREFLY_CAP) as usize);
    assert_eq!(most.2, POOL_CAP);
    sim.finish();
}

/// The count knob, live: the fireflies fill to it, thin to a lower one by
/// fading out, and fill again, and never one more than it asks.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn the_count_knob_fills_and_thins_the_fireflies() {
    let with = |count: u32| Knobs {
        count,
        ..Knobs::default()
    };
    let mut sim = Sim::new(POOL_CAP, &[], &aux_for(&[], &with(2_000)));
    let flies = |sim: &Sim| sim.read().iter().filter(|s| s.firefly()).count();
    sim.run(8.0);
    assert_eq!(flies(&sim), 2_000);
    // Lit: on average well off zero and under the peak.
    let all = sim.read();
    let mean: f32 = all
        .iter()
        .filter(|s| s.firefly())
        .map(|s| s.alpha)
        .sum::<f32>()
        / 2_000.0;
    assert!((0.2..0.6).contains(&mean), "mean opacity {mean}");
    sim.write_aux(AUX_FIREFLIES, &fly_rows(&with(500), 1.0, 0.0));
    // Fading, not gone at once: none has died a frame later.
    sim.step();
    assert_eq!(flies(&sim), 2_000);
    sim.run(3.0);
    assert_eq!(flies(&sim), 500);
    assert!(sim.read().iter().skip(500).all(|s| !s.firefly()));
    sim.write_aux(AUX_FIREFLIES, &fly_rows(&with(1_500), 1.0, 0.0));
    sim.run(8.0);
    assert_eq!(flies(&sim), 1_500);
    // The density, the debug panel's and the governor's, is a share of it.
    sim.write_aux(AUX_FIREFLIES, &fly_rows(&with(1_500), 0.3, 0.0));
    sim.run(3.0);
    assert_eq!(flies(&sim), 450);
    sim.finish();
}

/// A firefly's motion is slow and calm: never faster than the drift
/// speed's top, its direction turning a fraction of a degree a frame, its
/// light moving a little a frame, and it stays where it was born, within
/// the loop its sines draw.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn a_firefly_drifts_slowly_and_never_turns_on_a_frame() {
    let knobs = Knobs {
        count: 300,
        ..Knobs::default()
    };
    let mut sim = Sim::new(2_048, &[], &aux_for(&[], &knobs));
    sim.run(6.0);
    let start = sim.read();
    assert_eq!(start.iter().filter(|s| s.firefly()).count(), 300);
    let mut before = start.clone();
    let (mut fastest, mut sharpest, mut jump) = (0.0f32, 0.0f32, 0.0f32);
    for _ in 0..(30.0 * FPS) as u32 {
        sim.step();
        let now = sim.read();
        for (a, b) in before.iter().zip(&now).take(300) {
            assert!(b.firefly(), "a firefly died or was born again");
            fastest = fastest.max(b.vel.length());
            sharpest = sharpest.max(turn_deg(a.vel, b.vel));
            jump = jump.max((b.alpha - a.alpha).abs());
            // It moved by its velocity and nothing else.
            assert!((b.pos - a.pos).distance(b.vel * DT) < 1e-5);
        }
        before = now;
    }
    eprintln!(
        "fastest {fastest:.3} m/s, sharpest turn {sharpest:.3} deg a frame, largest step of opacity {jump:.4}"
    );
    // 1.4 of the drift speed at the path's fastest, 1.31 of its own.
    assert!(fastest <= knobs.drift * 1.4 * 1.32, "{fastest}");
    assert!(sharpest < 0.7, "{sharpest} degrees in one frame");
    assert!(jump < 0.02, "{jump} of opacity in one frame");
    let wandered = start
        .iter()
        .zip(&before)
        .take(300)
        .map(|(a, b)| a.pos.distance(b.pos))
        .fold(0.0f32, f32::max);
    assert!(wandered < 1.5, "{wandered} m in 30 s");
    for s in before.iter().take(300) {
        // Past the half extent it is unseen, and not far past it.
        let across = s.pos.x.abs().max(s.pos.z.abs());
        assert!(across <= HALF_M * 1.4, "{:?}", s.pos);
        assert!(
            across <= HALF_M || s.alpha == 0.0,
            "{:?} lit at {}",
            s.pos,
            s.alpha
        );
        assert!((-2.25..=2.85).contains(&s.pos.y), "{:?}", s.pos);
        assert!(
            (-1.5..=2.1).contains(&s.pos.y) || s.alpha == 0.0,
            "{:?}",
            s.pos
        );
    }
    sim.finish();
}

/// The music lifts the light and the size a little and no more: full
/// loudness against silence at the same moment, firefly by firefly. With
/// the music knob at 0 nothing answers.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn the_music_lifts_the_light_and_the_size_a_little() {
    let lit = |music: f32, rms: f32| {
        let knobs = Knobs {
            count: 300,
            brightness: 0.5,
            music,
            ..Knobs::default()
        };
        let mut aux = aux_for(&[], &knobs);
        aux[AUX_FIREFLIES..].copy_from_slice(&fly_rows(&knobs, 1.0, rms));
        let mut sim = Sim::new(2_048, &[], &aux);
        sim.rms = rms;
        sim.run(8.0);
        let all = sim.read();
        sim.finish();
        all
    };
    let (quiet, loud, deaf) = (lit(1.0, 0.0), lit(1.0, 1.0), lit(0.0, 1.0));
    let mut seen = 0;
    for ((q, l), d) in quiet.iter().zip(&loud).zip(&deaf).take(300) {
        if !q.firefly() || q.alpha < 1e-3 {
            continue;
        }
        seen += 1;
        // The same firefly at the same place, brighter and larger by the
        // sim's shares at most: 60% of light, 35% of radius.
        assert!(q.pos.distance(l.pos) < 1e-4);
        let light = l.alpha / q.alpha;
        let size = l.size / q.size;
        assert!((light - 1.6).abs() < 0.01, "light x{light}");
        assert!((size - 1.35).abs() < 0.01, "size x{size}");
        assert!((d.alpha - q.alpha).abs() < 1e-4 && (d.size - q.size).abs() < 1e-6);
    }
    assert!(seen > 250, "{seen} fireflies lit");
}

/// The aux block with no boxes for `knobs` in a room whose saved theme is
/// `theme` (board #5543), the orbit at `phase`.
fn air_aux(theme: Option<Theme>, knobs: &Knobs, phase: f32) -> Vec<[f32; 4]> {
    let mut aux = aux_for(&[], knobs);
    aux[AUX_FIREFLIES..].copy_from_slice(&knobs.rows(1.0, 0.0, &Look::of(theme), phase));
    aux
}

/// 300 particles of the air, the count's knob set.
fn three_hundred() -> Knobs {
    Knobs {
        count: 300,
        count_forced: true,
        ..Knobs::default()
    }
}

/// The slab's floor and top and where the stars' part starts, in meters
/// from the anchor at the preset's half extent (the sim's `XR_FLY_Y_LOW`,
/// `XR_FLY_Y_HIGH` and `XR_STAR_Y_LOW`).
const SLAB_LOW_M: f32 = -0.35 * HALF_M;
const SLAB_HIGH_M: f32 = 0.55 * HALF_M;
const STAR_LOW_M: f32 = 0.1 * HALF_M;

/// Aurora's air: stars. Born in the upper part of the slab, warm white,
/// nearly still. Each flickers on its own period: shallow with the highs
/// silent, deep with them full, and never by much in one frame, while
/// the light of all of them together stays about level.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn a_star_stays_high_and_nearly_still_and_flickers_with_the_highs() {
    let knobs = three_hundred();
    let look = Look::of(Some(Theme::Aurora));
    assert_eq!(look.motion, Motion::Twinkle);
    // What 4 s of one level of the highs shows: each star's dimmest
    // against its brightest, the mean opacity, and the largest step of
    // opacity in a frame.
    let run = |high: f32| {
        let mut sim = Sim::new(2_048, &[], &air_aux(Some(Theme::Aurora), &knobs, 0.0));
        sim.bands[2] = high;
        sim.run(6.0);
        let start = sim.read();
        assert_eq!(start.iter().filter(|s| s.firefly()).count(), 300);
        let (mut low, mut top) = (vec![f32::MAX; 300], vec![0.0f32; 300]);
        let (mut sum, mut jump, mut fastest) = (0.0f32, 0.0f32, 0.0f32);
        let mut before = start.clone();
        let frames = (4.0 * FPS) as u32;
        for _ in 0..frames {
            sim.step();
            let now = sim.read();
            for (i, (a, b)) in before.iter().zip(&now).take(300).enumerate() {
                assert!(b.firefly(), "a star died or was born again");
                low[i] = low[i].min(b.alpha);
                top[i] = top[i].max(b.alpha);
                sum += b.alpha;
                jump = jump.max((b.alpha - a.alpha).abs());
                fastest = fastest.max(b.vel.length());
            }
            before = now;
        }
        for (a, b) in start.iter().zip(&before).take(300) {
            // The upper part of the slab, where it was born.
            assert!(b.pos.y >= STAR_LOW_M - 0.01 && b.pos.y <= SLAB_HIGH_M + 0.01);
            assert!(a.pos.distance(b.pos) < 0.1, "{:?} to {:?}", a.pos, b.pos);
            assert!((b.stamp - look.stamp as f32).abs() < 0.1 && (b.motion - 1.0).abs() < 0.1);
            // Warm white, a little toward white.
            assert!(b.rgb.x > 0.99 && b.rgb.y > 0.85 && b.rgb.z > 0.67 && b.rgb.z < b.rgb.y);
        }
        sim.finish();
        let depth: Vec<f32> = low
            .iter()
            .zip(&top)
            .filter(|(_, t)| **t > 0.05)
            .map(|(l, t)| l / t)
            .collect();
        assert!(depth.len() > 250, "{} stars lit", depth.len());
        let shallowest = depth.iter().copied().fold(0.0f32, f32::max);
        let mean_depth = depth.iter().sum::<f32>() / depth.len() as f32;
        (
            mean_depth,
            shallowest,
            sum / (300.0 * frames as f32),
            jump,
            fastest,
        )
    };
    let (quiet_depth, _, quiet_mean, quiet_jump, fastest) = run(0.0);
    let (loud_depth, loud_shallowest, loud_mean, loud_jump, _) = run(1.0);
    eprintln!(
        "stars: dimmest/brightest {quiet_depth:.2} quiet, {loud_depth:.2} loud · mean opacity {quiet_mean:.3} quiet, {loud_mean:.3} loud · largest step {quiet_jump:.4} quiet, {loud_jump:.4} loud · fastest {fastest:.4} m/s"
    );
    // Nearly still: under a centimeter a second.
    assert!(fastest < 0.01, "{fastest}");
    // Quiet, a star dims to a little over half; loud, to a seventh or so
    // (a 4 s window holds a whole period of every star).
    assert!(quiet_depth > 0.5 && quiet_depth < 0.65, "{quiet_depth}");
    assert!(loud_depth < 0.25, "{loud_depth}");
    assert!(loud_shallowest < 0.4, "{loud_shallowest}");
    // Together they hold about level: the highs make them scintillate,
    // they do not pump the room's light.
    assert!(
        (loud_mean / quiet_mean - 1.0).abs() < 0.25,
        "{quiet_mean} to {loud_mean}"
    );
    // And no star moves its light by much in one frame.
    assert!(
        quiet_jump < 0.03 && loud_jump < 0.07,
        "{quiet_jump} {loud_jump}"
    );
}

/// Flint's air: ash and sparks. Let go with the bass: a pool fills fast
/// with it full and slowly with it silent. Each is born on the floor, goes
/// up at a steady speed with a small sway, never faster on a frame than
/// the one before, and has faded out before it dies of age.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn ash_rises_from_the_floor_with_the_bass_fades_and_dies() {
    let knobs = three_hundred();
    let look = Look::of(Some(Theme::Flint));
    assert_eq!(look.motion, Motion::Rise);
    let aux = air_aux(Some(Theme::Flint), &knobs, 0.0);
    let alive = |sim: &Sim| sim.read().iter().filter(|s| s.firefly()).count();
    // Silent: a trickle. Full: the pool's 300 within a few seconds.
    let mut quiet = Sim::new(2_048, &[], &aux);
    quiet.run(3.0);
    let trickle = alive(&quiet);
    quiet.finish();
    let mut sim = Sim::new(2_048, &[], &aux);
    sim.bands[0] = 1.0;
    sim.run(3.0);
    let burst = alive(&sim);
    eprintln!("rise: {trickle} alive after 3 s silent, {burst} with the bass full");
    assert!(trickle < 100, "{trickle}");
    assert!(burst > 280, "{burst}");
    // 14 s more, frame by frame: longer than the longest life.
    let up = knobs.drift * 5.0;
    let (mut deaths, mut births, mut highest) = (0, 0, f32::MIN);
    let mut before = sim.read();
    for _ in 0..(14.0 * FPS) as u32 {
        sim.step();
        let now = sim.read();
        for (a, b) in before.iter().zip(&now).take(300) {
            match (a.firefly(), b.firefly()) {
                (true, false) => {
                    deaths += 1;
                    // Dead of age, and unseen by then.
                    assert!(a.clock > 4.9, "died at {} s", a.clock);
                    assert!(a.alpha < 0.01, "died lit at {}", a.alpha);
                }
                (false, true) => {
                    births += 1;
                    // On the floor, unseen until it fades in.
                    assert!(
                        b.pos.y >= SLAB_LOW_M - 1e-3
                            && b.pos.y <= SLAB_LOW_M + 0.05 * HALF_M + 1e-3,
                        "born at {:?}",
                        b.pos
                    );
                    assert!(b.alpha <= 0.0, "born lit at {}", b.alpha);
                    assert!((b.motion - 2.0).abs() < 0.1);
                    // Amber, a little toward white.
                    assert!(b.rgb.x > 0.99 && b.rgb.y > 0.6 && b.rgb.y < 0.72 && b.rgb.z < 0.36);
                }
                (true, true) => {
                    // Up at its own steady speed, 0.6 to 1.4 of the
                    // mean, the sway a fifth of it at most.
                    assert!(b.clock <= 10.0 + DT, "{} s old", b.clock);
                    assert!(
                        b.vel.y >= up * 0.6 / 1.1 - 1e-4 && b.vel.y <= up * 1.4 / 1.1 + 1e-4,
                        "{}",
                        b.vel.y
                    );
                    if a.vel.length() > 0.0 {
                        assert!((b.vel.y - a.vel.y).abs() < 1e-5);
                        assert!(glam::Vec2::new(b.vel.x, b.vel.z).length() <= b.vel.y * 0.2 + 1e-5);
                        assert!((b.pos - a.pos).distance(b.vel * DT) < 1e-5);
                        assert!(
                            (b.alpha - a.alpha).abs() < 0.05,
                            "{} to {}",
                            a.alpha,
                            b.alpha
                        );
                    }
                    highest = highest.max(b.pos.y);
                }
                (false, false) => {}
            }
        }
        before = now;
    }
    eprintln!(
        "rise: {deaths} died of age and {births} were let go in 14 s, the highest at {highest:.2} m"
    );
    // Every one of the first 300 lived out its life, and the bass kept
    // letting more go.
    assert!(deaths >= 280, "{deaths}");
    assert!(births >= 250, "{births}");
    // They climb past a seated eye, 1.2 m over the floor, and none leaves
    // the volume.
    assert!(highest > SLAB_LOW_M + 2.0, "{highest}");
    assert!(highest < SLAB_HIGH_M + 0.4 * HALF_M, "{highest}");
    sim.finish();
}

/// Drift's air: plankton. Every particle is carried the one way, each
/// with a small wobble of its own, and one that leaves the volume comes
/// back in on the far side unseen: none is born again and none jumps
/// while it can be seen.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn plankton_is_carried_one_way_and_wraps_unseen() {
    // A fast current, so 30 s carries every particle through the volume,
    // and not so fast that its rise and fall takes one out of the slab.
    let knobs = Knobs {
        drift: 0.15,
        ..three_hundred()
    };
    assert_eq!(Look::of(Some(Theme::Drift)).motion, Motion::Drift);
    let mut sim = Sim::new(2_048, &[], &air_aux(Some(Theme::Drift), &knobs, 0.0));
    sim.run(6.0);
    let way = Vec3::new(0.94, 0.0, 0.3412);
    let mut before = sim.read();
    assert_eq!(before.iter().filter(|s| s.firefly()).count(), 300);
    let (mut wraps, mut widest, mut mean) = (0, 0.0f32, Vec3::ZERO);
    let frames = (30.0 * FPS) as u32;
    for _ in 0..frames {
        sim.step();
        let now = sim.read();
        for (a, b) in before.iter().zip(&now).take(300) {
            assert!(
                b.firefly() && b.age >= a.age,
                "plankton died or was born again"
            );
            widest = widest.max(b.vel.angle_between(way).to_degrees());
            mean += b.vel;
            let step = b.pos - a.pos;
            if step.distance(b.vel * DT) > 1e-4 {
                // The wrap: a whole volume across, and unseen on both
                // sides of it.
                wraps += 1;
                assert!(
                    (step.x.abs() - 2.0 * 1.02 * HALF_M).abs() < 0.02
                        || (step.z.abs() - 2.0 * 1.02 * HALF_M).abs() < 0.02,
                    "{step:?}"
                );
                assert!(a.alpha == 0.0 && b.alpha == 0.0, "wrapped lit");
            }
            let across = b.pos.x.abs().max(b.pos.z.abs());
            assert!(across <= HALF_M * 1.03, "{:?}", b.pos);
            assert!(across < HALF_M || b.alpha == 0.0);
        }
        before = now;
    }
    let mean = mean / (300.0 * frames as f32);
    eprintln!(
        "drift: {wraps} wraps in 30 s, the widest a velocity turns off the way {widest:.1} deg, the mean velocity {mean:?}"
    );
    assert!(wraps > 200, "{wraps}");
    assert!(widest < 30.0, "{widest}");
    // Together: the one way, at 1.5 of the drift speed on average.
    assert!(mean.angle_between(way).to_degrees() < 3.0);
    assert!(
        (mean.length() / (knobs.drift * 1.5) - 1.0).abs() < 0.15,
        "{}",
        mean.length()
    );
    sim.finish();
}

/// Mirror's air: a mirror ball's specks. All turn round the anchor's
/// upright axis by the app's phase, each at a radius and a height of its
/// own that it keeps.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn the_specks_orbit_together_by_the_apps_phase() {
    let knobs = three_hundred();
    let look = Look::of(Some(Theme::Mirror));
    assert_eq!(look.motion, Motion::Orbit);
    let mut phase = 0.0f32;
    let mut sim = Sim::new(2_048, &[], &air_aux(Some(Theme::Mirror), &knobs, phase));
    let mut advance = |sim: &mut Sim, seconds: f32| {
        for _ in 0..(seconds * FPS) as u32 {
            phase = orbit_step(phase, DT, knobs.drift);
            sim.write_aux(AUX_FIREFLIES + 3, &[look.rows(phase)[1]]);
            sim.step();
        }
        phase
    };
    let at = advance(&mut sim, 6.0);
    let start = sim.read();
    assert_eq!(start.iter().filter(|s| s.firefly()).count(), 300);
    let later = advance(&mut sim, 10.0);
    let end = sim.read();
    let turned = (later - at).rem_euclid(std::f32::consts::TAU);
    // 10 s at the default drift: 1.5 rad.
    assert!((turned - 1.5).abs() < 0.01, "{turned}");
    let level = |p: Vec3| glam::Vec2::new(p.x, p.z);
    let mut farthest = 0.0f32;
    for (a, b) in start.iter().zip(&end).take(300) {
        assert!(b.firefly() && b.age >= a.age);
        let (ra, rb) = (level(a.pos).length(), level(b.pos).length());
        assert!((ra - rb).abs() < 1e-3, "radius {ra} to {rb}");
        assert!((a.pos.y - b.pos.y).abs() < 1e-4);
        assert!(ra <= 0.9 * HALF_M + 1e-3);
        assert!(a.pos.y >= SLAB_LOW_M - 1e-3 && a.pos.y <= SLAB_HIGH_M + 1e-3);
        farthest = farthest.max(ra);
        if ra > 0.2 {
            // Every one by the same angle, the way x turns toward z.
            let swept = level(a.pos).angle_to(level(b.pos));
            assert!((swept - turned).abs() < 2e-3, "{swept} against {turned}");
        }
    }
    // They fill the disk out to nine tenths of the half extent.
    assert!(farthest > 0.8 * HALF_M, "{farthest}");
    sim.finish();
}

/// Classic's air, by band: a particle follows the bass, the mids or the
/// highs by its slot, wears that band's color, and its light answers that
/// band and no other. Contour's is one color on one band, the fireflies'
/// wander otherwise.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn by_band_each_particle_wears_and_follows_its_own() {
    let knobs = Knobs {
        brightness: 0.5,
        music: 1.0,
        ..three_hundred()
    };
    let lit = |theme: Theme, bands: [f32; 3], rms: f32| {
        let mut sim = Sim::new(2_048, &[], &air_aux(Some(theme), &knobs, 0.0));
        sim.bands = bands;
        sim.rms = rms;
        sim.run(8.0);
        let all = sim.read();
        sim.finish();
        all
    };
    let quiet = lit(Theme::Classic, [0.0; 3], 0.0);
    let mut seen = [0; 3];
    for (band, color) in BAND_COLORS.iter().enumerate() {
        let mut bands = [0.0; 3];
        bands[band] = 1.0;
        // The loudness full as well: an air by band does not read it.
        let loud = lit(Theme::Classic, bands, 1.0);
        for (i, (q, l)) in quiet.iter().zip(&loud).take(300).enumerate() {
            assert!(q.firefly() && (q.motion - 0.0).abs() < 0.1);
            assert!(
                q.rgb.distance(Vec3::from(BAND_COLORS[i % 3])) < 1e-5,
                "slot {i}"
            );
            if q.alpha < 1e-3 {
                continue;
            }
            assert!(q.pos.distance(l.pos) < 1e-4);
            let light = l.alpha / q.alpha;
            if i % 3 == band {
                seen[band] += 1;
                assert!((light - 1.6).abs() < 0.01, "slot {i} on its band x{light}");
                assert!(l.rgb.distance(Vec3::from(*color)) < 1e-5);
            } else {
                assert!((light - 1.0).abs() < 1e-3, "slot {i} off its band x{light}");
            }
        }
    }
    assert!(seen.iter().all(|n| *n > 80), "{seen:?}");
    // Contour: green, a little toward white, on the highs alone, at the
    // fireflies' own places and pace.
    let (calm, high, bass) = (
        lit(Theme::Contour, [0.0; 3], 0.0),
        lit(Theme::Contour, [0.0, 0.0, 1.0], 0.0),
        lit(Theme::Contour, [1.0, 1.0, 0.0], 1.0),
    );
    let mut lit_n = 0;
    for ((c, h), b) in calm.iter().zip(&high).zip(&bass).take(300) {
        assert!(c.firefly() && (c.motion - 0.0).abs() < 0.1);
        assert!(
            c.rgb.y > 0.99 && c.rgb.x >= 0.3 && c.rgb.x < 0.45 && c.rgb.z >= 0.5 && c.rgb.z < 0.61
        );
        if c.alpha < 1e-3 {
            continue;
        }
        lit_n += 1;
        assert!((h.alpha / c.alpha - 1.6).abs() < 0.01);
        assert!((b.alpha / c.alpha - 1.0).abs() < 1e-3);
    }
    assert!(lit_n > 250, "{lit_n}");
}

/// A change of theme mid-song: the old air fades out where it is, in its
/// own motion and color, within a second; the new one is born into its
/// own and fades in. No particle changes its motion or its color while it
/// is seen, none dies lit, and the light of all of them never steps.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn a_change_of_theme_fades_the_old_air_out_and_the_new_in() {
    let knobs = three_hundred();
    let mut sim = Sim::new(2_048, &[], &air_aux(None, &knobs, 0.0));
    sim.run(8.0);
    let mut before = sim.read();
    assert_eq!(before.iter().filter(|s| s.firefly()).count(), 300);
    assert!(
        before
            .iter()
            .take(300)
            .all(|s| s.stamp == 0.0 && s.motion == 0.0)
    );
    let total = |all: &[Sample]| all.iter().take(300).map(|s| s.alpha).sum::<f32>();
    let lit = total(&before);
    // Aurora comes: stars.
    let aurora = Look::of(Some(Theme::Aurora));
    sim.write_aux(AUX_FIREFLIES + 2, &aurora.rows(0.0));
    let (mut step_most, mut gone_at, mut frames) = (0.0f32, None, 0u32);
    let mut dimmest = lit;
    for _ in 0..(8.0 * FPS) as u32 {
        sim.step();
        frames += 1;
        let now = sim.read();
        let (mut old, mut old_was, mut old_is) = (0, 0.0f32, 0.0f32);
        for (a, b) in before.iter().zip(&now).take(300) {
            if a.firefly() && b.firefly() && (a.stamp - b.stamp).abs() < 0.1 {
                // The same particle a frame later: its motion and its
                // color its own, and it moved by its velocity.
                assert!((a.motion - b.motion).abs() < 0.1 && a.rgb.distance(b.rgb) < 1e-6);
                assert!((b.pos - a.pos).distance(b.vel * DT) < 1e-5);
                if b.stamp == 0.0 {
                    old += 1;
                    old_was += a.alpha;
                    old_is += b.alpha;
                }
            }
            if a.firefly() && !b.firefly() {
                assert!(a.alpha < 0.02, "died lit at {}", a.alpha);
            }
            if !a.firefly() && b.firefly() {
                assert!((b.stamp - aurora.stamp as f32).abs() < 0.1);
                assert!((b.motion - 1.0).abs() < 0.1 && b.alpha == 0.0);
                assert!(b.pos.y >= STAR_LOW_M - 1e-3);
            }
        }
        // The old air's light only falls: each firefly keeps its own
        // rhythm while it fades, so one may still rise a little, but not
        // all of them together.
        assert!(
            old_is <= old_was + 1e-4,
            "the old air brightened: {old_was} to {old_is}"
        );
        if old == 0 && gone_at.is_none() {
            gone_at = Some(frames as f32 * DT);
        }
        step_most = step_most.max((total(&now) - total(&before)).abs());
        dimmest = dimmest.min(total(&now));
        before = now;
    }
    let gone_at = gone_at.expect("the old air left");
    let stars = before
        .iter()
        .take(300)
        .filter(|s| s.firefly() && (s.motion - 1.0).abs() < 0.1)
        .count();
    eprintln!(
        "swap: the fireflies gone in {gone_at:.2} s, {stars} stars 8 s on, the light of all from {lit:.1} down to {dimmest:.1} and to {:.1}, its largest step in a frame {:.2}% of the start",
        total(&before),
        100.0 * step_most / lit
    );
    // The old air is out in the swap's 0.75 s and a frame or two.
    assert!(gone_at > 0.5 && gone_at < 0.85, "{gone_at}");
    assert_eq!(stars, 300);
    // The room's air goes down and comes back, never on one frame.
    assert!(step_most < 0.03 * lit, "{step_most} of {lit}");
    sim.finish();
}

/// A thrown burst arcs and falls: in the air a hand particle's velocity
/// changes by gravity and the air's drag alone, frame after frame, so
/// nothing jitters it. It lands without a bounce, slows, and fades where
/// it lies.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn a_hand_particle_arcs_falls_and_does_not_bounce() {
    let knobs = hands_only();
    let seeds: Vec<Seed> = (0..64)
        .map(|k| {
            let a = k as f32 * 0.4;
            seed(
                Vec3::new(0.0, 1.2, 0.0),
                Vec3::new(a.cos(), 0.6, a.sin()) * (0.3 + 0.01 * k as f32),
            )
        })
        .collect();
    let mut sim = Sim::new(1_024, &seeds, &aux_for(&[floor()], &knobs));
    let first = 1_024 - seeds.len();
    let mut before: Vec<Sample> = sim.read()[first..].to_vec();
    // Frames each has lain on the floor.
    let mut landed = vec![0u32; seeds.len()];
    let mut flying_frames = 0;
    for _ in 0..(4.0 * FPS) as u32 {
        sim.step();
        let now: Vec<Sample> = sim.read()[first..].to_vec();
        for (k, (a, b)) in before.iter().zip(&now).enumerate() {
            if !b.hand() {
                continue;
            }
            let on_floor = b.pos.y < MARGIN + 1e-3;
            // Well clear of the floor: the frame before it lands it is
            // already taken as lying on it.
            if b.pos.y > MARGIN + 0.01 && landed[k] == 0 {
                // Gravity, then the drag, and nothing else.
                let mut want = a.vel;
                want.y -= knobs.gravity * DT;
                want *= 1.0 - (1.0 - DRAG) * DT * 60.0;
                assert!(b.vel.distance(want) < 1e-5, "{:?} for {want:?}", b.vel);
                flying_frames += 1;
            }
            if on_floor {
                landed[k] += 1;
                // No bounce: it never leaves the floor again.
                assert!(b.vel.y <= 1e-4, "up at {} m/s off the floor", b.vel.y);
            }
            if landed[k] > 0 {
                assert!(on_floor, "back in the air at y {}", b.pos.y);
            }
            // Lying on the floor it only slows.
            if landed[k] > 2 {
                assert!(b.vel.y.abs() < 1e-4, "{:?}", b.vel);
                assert!(b.vel.length() <= a.vel.length() + 1e-5);
            }
        }
        before = now;
    }
    assert!(flying_frames > 1_000, "{flying_frames} frames in the air");
    assert!(landed.iter().all(|l| *l > 0), "every particle landed");
    // At rest it ages three times as fast.
    let age = before.iter().map(|s| s.age).fold(0.0f32, f32::max);
    assert!(age > 1.0 + 4.0 + 2.0, "aged {age} s in 4 s");
    assert!(
        before.iter().all(|s| s.vel.length() < 0.05),
        "still sliding"
    );
    sim.finish();
}

/// Where the floor meets a wall a hand particle runs along the wall's
/// foot: it keeps the way it was going, at the edge's speed, and stays
/// there. With the edge flow off it stops at the wall and lies still.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn a_hand_particle_runs_along_the_foot_of_a_wall() {
    // Sliding on the floor toward the wall at z = -2, half of them drifting
    // left and half right.
    let seeds: Vec<Seed> = (0..32)
        .map(|k| {
            let sideways = if k % 2 == 0 { 0.2 } else { -0.2 };
            seed(
                Vec3::new(-0.5 + 0.03 * k as f32, MARGIN, -1.6),
                Vec3::new(sideways, 0.0, -1.2),
            )
        })
        .collect();
    let boxes = [floor(), wall_at_z(-2.0)];
    let run = |knobs: &Knobs| {
        let mut sim = Sim::new(1_024, &seeds, &aux_for(&boxes, knobs));
        sim.run(1.5);
        let mid: Vec<Sample> = sim.read()[1_024 - seeds.len()..].to_vec();
        sim.run(1.0);
        let end: Vec<Sample> = sim.read()[1_024 - seeds.len()..].to_vec();
        sim.finish();
        (mid, end)
    };
    let (mid, end) = run(&hands_only());
    for (k, (a, b)) in mid.iter().zip(&end).enumerate() {
        let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
        for s in [a, b] {
            assert!(s.hand(), "particle {k} died");
            // On the floor, within the band of the wall's face.
            assert!(s.pos.y < MARGIN + 1e-3, "{:?}", s.pos);
            let off = s.pos.z + 2.0;
            assert!(
                (0.0..EDGE_BAND_M + MARGIN).contains(&off),
                "{off} m off the wall"
            );
            // Along the wall, the way it came, and not into or off it.
            assert!(s.vel.x * sign > 0.15, "particle {k}: {:?}", s.vel);
            // The edge's speed is 0.18 to 0.42 m/s by the particle, and
            // the air's drag holds it a tenth under.
            assert!((0.15..0.43).contains(&s.vel.x.abs()), "{:?}", s.vel);
            assert!(s.vel.z.abs() < 0.02 && s.vel.y.abs() < 0.02, "{:?}", s.vel);
            // Its way is settled in the life lane.
            assert!(s.life > 2.5, "life lane {}", s.life);
        }
        // It ran on along the wall for the second, and aged as the clock.
        assert!(
            (b.pos.x - a.pos.x) * sign > 0.15,
            "particle {k} moved {}",
            b.pos.x - a.pos.x
        );
        assert!((b.age - a.age - 1.0).abs() < 0.05, "aged {}", b.age - a.age);
    }
    // Off: it stops against the wall and rests.
    let off = Knobs {
        edge_flow: 0.0,
        ..hands_only()
    };
    let (_, end) = run(&off);
    for s in &end {
        assert!(s.hand());
        assert!(s.vel.length() < 0.05, "{:?}", s.vel);
        assert!(s.life < 2.5, "life lane {}", s.life);
    }
}

/// A table's rim: a hand particle sliding across the top stops at the rim
/// instead of falling off, runs along it, turns the corner and runs along
/// the next. A table against a wall has its back rim and the wall's face
/// in the same place, and the particle goes the same way along both. With
/// the edge flow off it slides off the table and falls to the floor.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn a_hand_particle_runs_round_a_tables_rim() {
    let table = table_at(0.0, -1.0);
    let top = 0.7 + MARGIN;
    // On the top, sliding toward the +x rim, some drifting each way.
    let seeds: Vec<Seed> = (0..24)
        .map(|k| {
            let sideways = if k % 2 == 0 { 0.25 } else { -0.25 };
            seed(
                Vec3::new(0.1, top, -1.2 + 0.016 * k as f32),
                Vec3::new(0.9, 0.0, sideways),
            )
        })
        .collect();
    // The table's back rim is at z = -1.3: the wall's face is there too.
    for boxes in [
        vec![floor(), table],
        vec![floor(), table, wall_at_z(-1.3 - MARGIN)],
    ] {
        let mut sim = Sim::new(1_024, &seeds, &aux_for(&boxes, &hands_only()));
        let first = 1_024 - seeds.len();
        let mut before: Vec<Sample> = sim.read()[first..].to_vec();
        let mut rims: Vec<Vec<u8>> = vec![Vec::new(); seeds.len()];
        let mut sharpest = 0.0f32;
        for frame in 0..(5.0 * FPS) as u32 {
            sim.step();
            let now: Vec<Sample> = sim.read()[first..].to_vec();
            for (k, (a, b)) in before.iter().zip(&now).enumerate() {
                assert!(b.hand(), "particle {k} died");
                // Never off the table: on its top, inside its rim.
                assert!(
                    (b.pos.y - top).abs() < 2e-3,
                    "particle {k} at y {}",
                    b.pos.y
                );
                assert!(b.pos.x.abs() <= 0.4 + MARGIN + 1e-4, "{:?}", b.pos);
                assert!((b.pos.z + 1.0).abs() <= 0.3 + MARGIN + 1e-4, "{:?}", b.pos);
                // After the first second it is on the rim for good, and
                // its direction only ever turns smoothly.
                if frame > FPS as u32 {
                    sharpest = sharpest.max(turn_deg(a.vel, b.vel));
                    let inside =
                        (0.4 + MARGIN - b.pos.x.abs()).min(0.3 + MARGIN - (b.pos.z + 1.0).abs());
                    assert!(
                        inside < EDGE_BAND_M,
                        "particle {k} is {inside} m in from the rim"
                    );
                    assert!(b.vel.length() > 0.08, "particle {k} stalled: {:?}", b.vel);
                }
                // Which rim it is on: +x, +z, -x or -z.
                let dx = 0.4 + MARGIN - b.pos.x.abs();
                let dz = 0.3 + MARGIN - (b.pos.z + 1.0).abs();
                let rim = match (dx < dz, b.pos.x > 0.0, b.pos.z + 1.0 > 0.0) {
                    (true, true, _) => 0,
                    (false, _, true) => 1,
                    (true, false, _) => 2,
                    (false, _, false) => 3,
                };
                if dx.min(dz) < 0.01 && rims[k].last() != Some(&rim) {
                    rims[k].push(rim);
                }
            }
            before = now;
        }
        sim.finish();
        // A steer of 6 a second turns a corner over a few frames.
        assert!(sharpest < 12.0, "{sharpest} degrees in one frame");
        for (k, seen) in rims.iter().enumerate() {
            // Round at least one corner, each rim after the last in one
            // sense: 0 1 2 3 0 or 0 3 2 1 0.
            assert!(seen.len() >= 2, "particle {k} stayed on {seen:?}");
            let step = (seen[1] + 4 - seen[0]) % 4;
            assert!(step == 1 || step == 3, "particle {k}: {seen:?}");
            for pair in seen.windows(2) {
                assert_eq!((pair[1] + 4 - pair[0]) % 4, step, "particle {k}: {seen:?}");
            }
        }
    }
    // Off: over the rim and down to the floor.
    let off = Knobs {
        edge_flow: 0.0,
        ..hands_only()
    };
    let mut sim = Sim::new(1_024, &seeds, &aux_for(&[floor(), table], &off));
    sim.run(3.0);
    let end: Vec<Sample> = sim.read()[1_024 - seeds.len()..].to_vec();
    sim.finish();
    for s in &end {
        assert!(s.hand());
        assert!(s.pos.y < MARGIN + 1e-3, "still up at y {}", s.pos.y);
    }
}

/// Two walls make an upright edge: a hand particle let go in the corner,
/// high above the floor, slides down it at the edge's speed instead of
/// dropping. One let go beside a single wall just falls.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn a_hand_particle_slides_down_a_corner() {
    let corner = seed(Vec3::new(-1.98, 2.2, -1.98), Vec3::ZERO);
    let beside = seed(Vec3::new(0.0, 2.2, -1.98), Vec3::ZERO);
    let boxes = [floor(), wall_at_z(-2.0), wall_at_x(-2.0)];
    let mut sim = Sim::new(1_024, &[corner, beside], &aux_for(&boxes, &hands_only()));
    sim.run(1.0);
    let now: Vec<Sample> = sim.read()[1_022..].to_vec();
    sim.finish();
    let (c, b) = (now[0], now[1]);
    // In the corner: down at the edge's speed, give or take what gravity
    // adds against the steer, still within the band of both walls.
    assert!(c.vel.y < -0.15 && c.vel.y > -0.9, "{:?}", c.vel);
    assert!(c.vel.x.abs() < 0.02 && c.vel.z.abs() < 0.02, "{:?}", c.vel);
    assert!(c.pos.y > 1.3, "fell to {}", c.pos.y);
    // Beside one wall: a free fall, most of the way down in the second.
    assert!(b.vel.y < -1.8, "{:?}", b.vel);
    assert!(b.pos.y < c.pos.y - 0.5);
}

/// A burst is born where the instrument rows put it, at the size and
/// opacity an impact has, in the hands' slots alone, and a pool with every
/// hand slot alive takes it from the old hand particles, never from a
/// firefly.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn a_burst_takes_the_hands_slots_and_never_a_firefly() {
    let knobs = Knobs {
        count: 400,
        ..Knobs::default()
    };
    let mut sim = Sim::new(2_048, &[], &aux_for(&[floor()], &knobs));
    sim.run(6.0);
    let burst = |count: u32, alive: u32| {
        let b = Burst {
            center: Vec3::new(0.5, 1.2, -1.0),
            radius: 0.12,
            count,
            hand: 1,
            impact: true,
            brightness: 1.0,
            back: Vec3::ZERO,
        };
        let mut rows = instrument_rows(Some(b), None, Vec3::ZERO);
        rows[0][3] = hand_steal(count, alive, 2_048, 400);
        rows
    };
    // One frame of 300: exactly 300 born, within the ball.
    sim.write_aux(AUX_INSTRUMENTS, &burst(300, 400));
    sim.step();
    sim.write_aux(AUX_INSTRUMENTS, &instrument_rows(None, None, Vec3::ZERO));
    let all = sim.read();
    let born: Vec<&Sample> = all.iter().filter(|s| s.hand()).collect();
    assert_eq!(born.len(), 300);
    assert_eq!(all.iter().filter(|s| s.firefly()).count(), 400);
    assert!(
        all.iter().take(400).all(|s| !s.hand()),
        "a burst in a firefly's slot"
    );
    for s in &born {
        assert!(s.pos.distance(Vec3::new(0.5, 1.2, -1.0)) <= 0.12 + 1e-4);
        assert!((0.3..=1.2 + 1e-4).contains(&s.vel.length()), "{:?}", s.vel);
        assert!((s.alpha - 0.35).abs() < 1e-5);
        assert!((0.007..=0.013).contains(&s.size), "{}", s.size);
    }
    // Fill the hands' share, then burst again each frame for a second:
    // the fireflies are all still there and all still fireflies.
    for _ in 0..8 {
        sim.write_aux(AUX_INSTRUMENTS, &burst(300, 400));
        sim.step();
    }
    let full = sim.read();
    assert_eq!(full.iter().filter(|s| s.hand()).count(), 1_648);
    for _ in 0..FPS as u32 {
        sim.write_aux(AUX_INSTRUMENTS, &burst(300, 2_048));
        sim.step();
    }
    let after = sim.read();
    assert_eq!(after.iter().filter(|s| s.firefly()).count(), 400);
    assert_eq!(after.iter().filter(|s| s.hand()).count(), 1_648);
    // The steal took living hand particles: some are younger than the fill.
    let young = after.iter().filter(|s| s.hand() && s.age < 0.9).count();
    assert!(young > 300, "{young} hand particles born from the living");
    sim.finish();
}

/// A throw's streak is born along the stretch it flew since the last
/// frame, not in a clump at the frame's point: 300 a frame, 8 cm of flight
/// a frame, a 3 cm radius. Over a few frames the births leave no gap along
/// the flight, and none lies off it by more than the radius.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn a_throws_streak_is_born_along_the_stretch_it_flew() {
    let knobs = hands_only();
    let mut sim = Sim::new(4_096, &[], &aux_for(&[floor()], &knobs));
    let from = Vec3::new(0.0, 1.4, -0.5);
    let step = Vec3::new(0.0, 0.0, -0.083);
    let radius = 0.03;
    let frames = 8;
    for i in 1..=frames {
        let center = from + step * i as f32;
        let b = Burst {
            center,
            radius,
            count: 300,
            hand: 1,
            impact: false,
            brightness: 1.0,
            back: -step,
        };
        sim.write_aux(AUX_INSTRUMENTS, &instrument_rows(Some(b), None, Vec3::ZERO));
        sim.write_aux(AUX_POUR, &[aim_row(Some(b), None)]);
        sim.step();
    }
    sim.write_aux(AUX_INSTRUMENTS, &instrument_rows(None, None, Vec3::ZERO));
    sim.write_aux(AUX_POUR, &[aim_row(None, None)]);
    let born: Vec<Sample> = sim.read().into_iter().filter(Sample::hand).collect();
    assert_eq!(born.len(), 300 * frames);
    // Along the flight (z): every 2 cm of it holds births, where clumps a
    // frame apart would leave 2 cm empty between each pair. The particles
    // have flown a few frames at up to 0.3 m/s, hence the margins.
    let reach = step.length() * frames as f32;
    let mut bins = vec![0u32; (reach / 0.02) as usize];
    for s in &born {
        let along = from.z - s.pos.z;
        assert!(
            (-radius - 0.05..=reach + radius + 0.05).contains(&along),
            "{along} m along a flight of {reach} m"
        );
        let off = Vec3::new(s.pos.x - from.x, 0.0, 0.0).length();
        assert!(off <= radius + 0.05, "{off} m off the flight");
        if along >= 0.0 && along < reach {
            let bin = ((along / 0.02) as usize).min(bins.len() - 1);
            bins[bin] += 1;
        }
    }
    let mean = born.len() as f32 / bins.len() as f32;
    for (i, &n) in bins.iter().enumerate() {
        assert!(
            n as f32 > mean * 0.4,
            "bin {i} holds {n} of a mean {mean}: {bins:?}"
        );
    }
    sim.finish();
}

/// The preset as the app runs it: loaded by the core's effect loader into a
/// one-cue scene, its layer disabled, its sim dispatched outside the scene
/// renderer's frame and drawn by `render_world`, as `scene.rs` does on the
/// headset. The pool is the cap, the fireflies fill to the count, a burst
/// adds its particles, the alive count the app logs never passes the pool,
/// and the eye target gets light.
#[test]
#[ignore = "requires a GPU/software adapter"]
fn the_preset_runs_through_the_cores_particle_system() {
    use fosfora_app::audio::AudioFrame;
    use fosfora_app::audio::analyzer::{SPECTROGRAM_MELS, SPECTRUM_BINS};
    use fosfora_app::audio::hop::HopOutput;
    use fosfora_app::gpu::particle::types::ParticleAux;
    use fosfora_app::gpu::particle::{ParticleSystem, WorldCamera, WorldTarget};
    use fosfora_app::headless::scene_renderer::SceneRenderer;

    const PRESET: &str = include_str!("../../../assets/xr/effects/fireflies_xr_world.pfx");
    const DIM: u32 = 256;
    let (device, queue, _guard) = gpu();
    // The core finds its assets from the working directory.
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::env::set_current_dir(&repo).unwrap();
    // The one-cue scene the XR app writes for `mode world`.
    let dir = std::env::temp_dir().join(format!("fosfora-fireflies-world-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Cue.json"),
        r#"{"layers":[{"effect_name":"Fireflies"}]}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("_scene.json"),
        r#"{"version":1,"name":"XR Fireflies","loop_mode":false,"advance_mode":"Manual",
           "cues":[{"preset_name":"Cue","transition":"Cut","label":"Fireflies"}]}"#,
    )
    .unwrap();

    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut sr = SceneRenderer::new(
        device.clone(),
        queue.clone(),
        64,
        64,
        fosfora_app::settings::ParticleQuality::High,
        dir.clone(),
    )
    .expect("renderer");
    // The desktop library does not scan assets/xr/; the APK stages the
    // preset among the effects.
    let pfx: fosfora_app::effect::format::PfxEffect =
        serde_json::from_str(PRESET).expect("fireflies_xr_world.pfx parses");
    assert_eq!(pfx.particles.as_ref().unwrap().max_count, POOL_CAP);
    sr.effect_loader.effects.push(pfx);
    sr.install_scene(fosfora_app::headless::load::load_scene_dir(&dir).expect("scene loads"));
    sr.start();
    assert!(sr.warnings.is_empty(), "warnings: {:?}", sr.warnings);
    assert_eq!(sr.layer_stack.layers.len(), 1);
    sr.layer_stack.layers[0].enabled = false;
    fn system(sr: &mut SceneRenderer) -> &mut ParticleSystem {
        sr.layer_stack.layers[0]
            .as_effect_mut()
            .and_then(|e| e.pass_executor.particle_system.as_mut())
            .expect("the preset builds a particle system")
    }

    // The head at the anchor with the app's near fade, the floor a meter
    // under it, and the default knobs.
    let knobs = Knobs::default();
    let mut rows = aux_for(
        &[TestBox {
            center: Vec3::new(0.0, -1.05, 0.0),
            half: Vec3::new(10.0, 0.05, 10.0),
        }],
        &knobs,
    );
    rows[0] = [0.0, 0.0, 0.0, 0.15];
    assert_eq!(rows.len(), WORLD_AUX_ROWS);
    let aux = |rows: &[[f32; 4]]| -> Vec<ParticleAux> {
        rows.iter().map(|&home| ParticleAux { home }).collect()
    };
    let anchor = Vec3::new(0.0, 1.0, 0.0);
    let camera = WorldCamera {
        view: glam::Mat4::from_translation(-anchor),
        proj: glam::Mat4::perspective_rh(90f32.to_radians(), 1.0, 0.05, 100.0),
        anchor,
    };
    let texture = |format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fireflies-eye"),
            size: wgpu::Extent3d {
                width: DIM,
                height: DIM,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | usage,
            view_formats: &[],
        })
    };
    let color = texture(
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureUsages::COPY_SRC,
    );
    let depth = texture(
        wgpu::TextureFormat::Depth32Float,
        wgpu::TextureUsages::empty(),
    );
    let (color_view, depth_view) = (
        color.create_view(&wgpu::TextureViewDescriptor::default()),
        depth.create_view(&wgpu::TextureViewDescriptor::default()),
    );
    let fps = FPS as u32;
    let alive = |sr: &mut SceneRenderer| {
        let ps = system(sr);
        ps.request_counter_readback();
        wait(device);
        ps.poll_counter_readback();
        ps.alive_count
    };
    let mut frame = 0u32;
    let mut run = |sr: &mut SceneRenderer, rows: &[[f32; 4]], frames: u32, draw: bool| {
        for k in 0..frames {
            let ts = f64::from(frame) / f64::from(fps);
            let mut f = fosfora_app::headless::loop_driver::synth_features(frame, fps, 120.0);
            f.rms = 0.5;
            let hop = HopOutput {
                frame: AudioFrame {
                    features: f,
                    spectrum: [0.3; SPECTRUM_BINS],
                    mel: [0.3; SPECTROGRAM_MELS],
                    dmfcc: [0.0; 13],
                    timestamp: ts,
                    phase_frozen: false,
                    bar_duration: 2.0,
                    beat_time: None,
                    downbeat_time: None,
                    section_boundary: None,
                },
                beat_fired: false,
                downbeat_fired: false,
                drop_fired: false,
                beat_activation: 0.0,
                pre_norm: f,
            };
            let wave = vec![0.0; fosfora_app::gpu::audio_textures::WAVEFORM_PEEK];
            system(sr).update_aux_in_place(queue, &aux(rows));
            sr.step(ts, DT, &hop, &wave, false);
            let ps = system(sr);
            let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            ps.poll_counter_readback();
            ps.dispatch(&mut enc, queue);
            if draw && k + 1 == frames {
                // The eye pass: clear color to alpha 0 (passthrough) and
                // depth to far, then the world pass loads both.
                drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("fireflies-eye"),
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
                }));
                ps.render_world(
                    device,
                    &mut enc,
                    queue,
                    WorldTarget {
                        color: &color_view,
                        color_format: wgpu::TextureFormat::Rgba8UnormSrgb,
                        depth: Some(&depth_view),
                        depth_format: wgpu::TextureFormat::Depth32Float,
                    },
                    &camera,
                    wgpu::LoadOp::Load,
                );
            }
            queue.submit([enc.finish()]);
            frame += 1;
        }
    };

    // Eight seconds: the fireflies are all there, and nothing else.
    run(&mut sr, &rows, 8 * fps, true);
    assert_eq!(alive(&mut sr), knobs.count);
    // Light in the eye target: sprites over the cleared, transparent view.
    let bpr = DIM * 4;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fireflies-readback"),
        size: u64::from(bpr * DIM),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    enc.copy_texture_to_buffer(
        color.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bpr),
                rows_per_image: Some(DIM),
            },
        },
        wgpu::Extent3d {
            width: DIM,
            height: DIM,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([enc.finish()]);
    buf.slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    wait(device);
    let pixels = buf.slice(..).get_mapped_range().to_vec();
    buf.unmap();
    let lit = pixels
        .chunks_exact(4)
        .filter(|p| p[0] > 8 || p[1] > 8)
        .count();
    let brightest = pixels
        .chunks_exact(4)
        .map(|p| p[0].max(p[1]))
        .max()
        .unwrap();
    eprintln!(
        "{lit} of {} pixels lit, the brightest at {brightest}",
        DIM * DIM
    );
    // Sparse: points of light, not a wash over the view.
    assert!(lit > 50, "{lit} pixels lit");
    assert!(lit < (DIM * DIM / 4) as usize, "{lit} pixels lit");
    // Warm: more red and green than blue where it is lit.
    let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
    for p in pixels.chunks_exact(4) {
        r += u64::from(p[0]);
        g += u64::from(p[1]);
        b += u64::from(p[2]);
    }
    assert!(b < r && b < g, "rgb sums {r} {g} {b}");

    // A throw's impact: its 6,000 over four frames join the fireflies.
    let impact = |count: u32| Burst {
        center: Vec3::new(0.0, 0.2, -1.5),
        radius: 0.12,
        count,
        hand: 1,
        impact: true,
        brightness: 1.0,
        back: Vec3::ZERO,
    };
    let mut bursting = rows.clone();
    bursting[AUX_INSTRUMENTS..AUX_INSTRUMENTS + 3].copy_from_slice(&instrument_rows(
        Some(impact(1_500)),
        None,
        Vec3::ZERO,
    ));
    run(&mut sr, &bursting, 4, false);
    run(&mut sr, &rows, 2, false);
    assert_eq!(alive(&mut sr), knobs.count + 6_000);
    // Hammered for five seconds at the steal's most: the pool is full and
    // the alive count the app logs is the pool, never more.
    let mut hammer = instrument_rows(Some(impact(50_000)), None, Vec3::ZERO);
    hammer[0][3] = MAX_STEAL;
    bursting[AUX_INSTRUMENTS..AUX_INSTRUMENTS + 3].copy_from_slice(&hammer);
    for _ in 0..5 {
        run(&mut sr, &bursting, fps, false);
        assert!(alive(&mut sr) <= POOL_CAP);
    }
    assert_eq!(alive(&mut sr), POOL_CAP);
    // Left alone, the hand particles land, fade and die, and the fireflies
    // are what is left.
    run(&mut sr, &rows, 8 * fps, false);
    assert_eq!(alive(&mut sr), knobs.count);

    wait(device);
    let err = pollster::block_on(device.pop_error_scope());
    assert!(err.is_none(), "validation error: {err:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
