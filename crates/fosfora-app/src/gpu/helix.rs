//! Helix (V22) — the recent audio history swept into a twisting ribbon you fly
//! along, rendered as volumetric density.
//!
//! The third density producer for the shared R3 marcher, after particle scatter
//! ([`crate::gpu::volumetric`]) and the Lattice CA ([`crate::gpu::lattice`]).
//! Like Lattice it owns its own `r32float` volume, fills it with the `pub(crate)`
//! helpers from `volumetric`, and hands density + aux to [`create_raymarch`].
//!
//! **Axis convention: +Z is now, −Z is the oldest retained slice.** The camera
//! looks down −Z, so history recedes ahead of the viewer.
//!
//! The history itself is a small CPU-written ring of [`HelixSlice`] records —
//! one per *slice tick*, not per frame. Ticks run at a fixed [`HelixParams::slice_rate`]
//! so the window spans `slice_count / slice_rate` seconds no matter what the frame
//! rate is doing; at the defaults that is 256 / 32 = 8 seconds. `ParticleSystem`
//! only ever sees CPU-side [`AudioFeatures`], so Helix deliberately does not reach
//! for the A17 GPU audio textures — they are not plumbed to this side of the app,
//! and a 16 KB ring is cheaper than making them so.
//!
//! Each frame the sweep shader rebuilds the entire volume from that ring. See
//! `helix_sweep.wgsl` for why a whole-volume rewrite beats appending one Z-slice.

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor, CommandEncoder,
    ComputePipeline, Device, Queue, RenderPipeline, ShaderStages, TextureFormat, TextureView,
};

use crate::audio::features::AudioFeatures;
use crate::gpu::volumetric::{
    VolumetricParams, VolumetricUniforms, create_compute_pipeline, create_raymarch,
    storage_ro_entry, storage_texture_3d_entry, uniform_entry,
};

/// `@workgroup_size(4,4,4)` — a 4^3 block of voxels per workgroup.
const HELIX_WORKGROUP: u32 = 4;

/// Default voxel resolution per axis. Shares the Lattice ladder (32/64/128/256).
pub const DEFAULT_GRID_RES: u32 = 128;

/// Retained history slices. 256 at the default 32 Hz tick = an 8-second ribbon.
pub const DEFAULT_SLICE_COUNT: u32 = 256;

/// Bounds on the ring length. The floor keeps `slice_lerp` well-defined; the
/// ceiling keeps the buffer trivially small (1024 * 64 B = 64 KB).
const MIN_SLICE_COUNT: u32 = 16;
const MAX_SLICE_COUNT: u32 = 1024;

pub fn clamp_slice_count(n: u32) -> u32 {
    n.clamp(MIN_SLICE_COUNT, MAX_SLICE_COUNT)
}

/// Allowed grid resolutions, mirroring [`crate::gpu::lattice::GRID_RES_CHOICES`].
pub const GRID_RES_CHOICES: [u32; 4] = [32, 64, 128, 256];

pub fn clamp_grid_res(g: u32) -> u32 {
    *GRID_RES_CHOICES
        .iter()
        .min_by_key(|c| c.abs_diff(g))
        .unwrap_or(&DEFAULT_GRID_RES)
}

// --- GPU blocks ---------------------------------------------------------------

/// GPU-side uniform block for `cs_sweep`. Declared inline in `helix_sweep.wgsl`
/// (single consumer, so there is no duplication to hoist into a preamble the way
/// `VolUniforms` / `LatticeUniforms` are). 12 scalars = 48 bytes, a multiple of 16
/// for the uniform address space; `helix_uniforms_is_48_bytes` asserts it.
#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
pub struct HelixUniforms {
    pub grid_res: u32,
    pub slice_count: u32,
    pub head: u32, // ring index of the newest slice
    pub time: f32,
    pub radius: f32,
    pub thickness: f32,
    pub twist_gain: f32,
    pub spectrum_gain: f32,
    pub wander: f32,
    pub ripple_gain: f32,
    pub hue_spread: f32,
    pub _pad0: f32,
}

/// One retained moment of audio. 16 floats = 64 bytes; byte-matches `HelixSlice`
/// in `helix_sweep.wgsl`, which reads it as four `vec4f`.
#[repr(C)]
#[derive(Debug, Copy, Clone, Default, Pod, Zeroable)]
pub struct HelixSlice {
    /// sub_bass, bass, low_mid, mid
    pub bands_lo: [f32; 4],
    /// upper_mid, presence, brilliance, rms
    pub bands_hi: [f32; 4],
    /// centre_x, centre_y, twist (accumulated, wrapped), centroid 0..1
    pub path: [f32; 4],
    /// kick, wave_lo, wave_hi, unused
    pub extra: [f32; 4],
}

// --- Host-side params ----------------------------------------------------------

/// Tunable Helix parameters. Held on the effect's `ParticleSystem`, built from the
/// `.pfx` definition at load and edited live from the Helix panel. Embeds a
/// [`VolumetricParams`] for the reused camera / palette / marcher controls, the
/// same way [`crate::gpu::lattice::LatticeParams`] does.
#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HelixParams {
    /// Camera / palette / look — reuses the R3 marcher's tunables.
    pub render: VolumetricParams,
    /// Voxel resolution per axis.
    pub grid_res: u32,
    /// Retained history slices.
    pub slice_count: u32,
    /// History ticks per second. Window length = `slice_count / slice_rate`.
    pub slice_rate: f32,
    /// Base cross-section radius, in unit-cube units.
    pub radius: f32,
    /// Shell half-thickness. The ribbon is hollow; the camera flies inside it.
    pub thickness: f32,
    /// Radians of profile rotation per unit Z — the corkscrew.
    pub twist_gain: f32,
    /// Extra twist accumulated per second, scaled by `beat_phase` so the ribbon
    /// turns in time with the track.
    pub twist_rate: f32,
    /// How far the 7 band energies deform the radius profile.
    pub spectrum_gain: f32,
    /// Centreline excursion from the Z axis.
    pub wander: f32,
    /// Speed of the centreline's drift.
    pub wander_rate: f32,
    /// Waveform ripple depth on the shell.
    pub ripple_gain: f32,
    /// Spread of the centroid → hue mapping written into the aux volume.
    pub hue_spread: f32,
}

impl Default for HelixParams {
    fn default() -> Self {
        Self {
            render: VolumetricParams {
                // The camera lives INSIDE the volume looking down -Z; the orbit
                // defaults would put it outside looking in. cam_distance is the
                // camera's Z coordinate in this mode — just inside the "now" face.
                cam_mode: 1,
                cam_distance: 0.80,
                cam_yaw: 0.0,
                cam_pitch: 0.0,
                // Tube envelope: fade at the far end only, so the walls the camera
                // flies between keep their brightness.
                env_shape: 2,
                // Inside a hollow shell, rays that graze along the wall accumulate
                // a very long optical path. High absorption with LOW emission is
                // what keeps that from washing the frame to flat fog: near material
                // then occludes far material instead of adding to it.
                absorption: 3.4,
                emission_gain: 0.42,
                // Narrower than the orbit default — a wide field puts the near wall
                // across the whole frame edge and buries the tunnel.
                fov: 2.1,
                density_threshold: 0.02,
                march_steps: 96,
                jitter_amp: 1.0,
                // Fine, strong FBM: from inside, the wall is close and the marcher's
                // detail noise is what gives it surface texture. The orbit default
                // (3.0) is tuned for viewing a whole volume from outside.
                detail_scale: 9.0,
                detail_strength: 0.55,
                // aux carries centroid-as-hue, so the marcher's age tint is the
                // colour channel here rather than an off-by-default extra.
                age_influence: 0.6,
                ..VolumetricParams::default()
            },
            grid_res: DEFAULT_GRID_RES,
            slice_count: DEFAULT_SLICE_COUNT,
            slice_rate: 32.0,
            radius: 0.58,
            // A tight shell is what makes the ribbon read as a ribbon from inside.
            // Widening this to 0.06 washed the flythrough into a soft funnel; grid
            // resolution turned out to matter far less (128^3 and 256^3 look nearly
            // identical at this thickness), so the default stays at 128.
            thickness: 0.032,
            twist_gain: 2.4,
            twist_rate: 0.9,
            spectrum_gain: 0.55,
            wander: 0.30,
            wander_rate: 0.17,
            ripple_gain: 0.05,
            hue_spread: 0.7,
        }
    }
}

impl HelixParams {
    pub fn build_uniforms(&self, head: u32, slice_count: u32, time: f32) -> HelixUniforms {
        HelixUniforms {
            grid_res: clamp_grid_res(self.grid_res),
            slice_count,
            head,
            time,
            radius: self.radius.max(0.01),
            thickness: self.thickness.max(1e-3),
            twist_gain: self.twist_gain,
            spectrum_gain: self.spectrum_gain.max(0.0),
            wander: self.wander,
            ripple_gain: self.ripple_gain,
            hue_spread: self.hue_spread,
            _pad0: 0.0,
        }
    }
}

// --- .pfx definition -----------------------------------------------------------

/// The `particles.helix` block of a `.pfx`. Present ⇒ the effect renders the swept
/// ribbon volume through the R3 marcher instead of particles, exactly as
/// `particles.lattice` does for the CA.
///
/// This block carries only what is NOT a performance knob. Everything a VJ would
/// reach for mid-set lives in the `.pfx` `inputs` instead, so it appears in the
/// Parameters panel and can be driven by MIDI / OSC / audio — see
/// [`HELIX_PARAM_NAMES`]. Declaring a value in both places would be two sources of
/// truth, and the param would silently win every frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HelixDef {
    /// Reallocates the density + aux volumes, so it is deliberately not bindable.
    pub grid_res: u32,
    /// Reallocates the history ring, likewise.
    pub slice_count: u32,
    pub slice_rate: f32,
    pub twist_rate: f32,
    pub wander_rate: f32,
    pub look: HelixLookDef,
}

impl Default for HelixDef {
    fn default() -> Self {
        let p = HelixParams::default();
        Self {
            grid_res: p.grid_res,
            slice_count: p.slice_count,
            slice_rate: p.slice_rate,
            twist_rate: p.twist_rate,
            wander_rate: p.wander_rate,
            look: HelixLookDef::default(),
        }
    }
}

/// The shared-marcher look block of a [`HelixDef`] — the subset of
/// [`VolumetricParams`] a `.pfx` may tune that is NOT already a bindable param.
/// Defaults come from [`HelixParams::default`], NOT [`VolumetricParams::default`]:
/// the flythrough needs its own camera, envelope and absorption, and inheriting
/// the orbit defaults would put the camera outside the volume.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HelixLookDef {
    pub detail_strength: f32,
    pub detail_scale: f32,
    pub density_threshold: f32,
    /// Look direction offset from straight-ahead.
    pub cam_yaw: f32,
    pub cam_pitch: f32,
    pub march_steps: u32,
    /// Strength of the per-slice hue tint in the marcher.
    pub age_influence: f32,
}

impl Default for HelixLookDef {
    fn default() -> Self {
        let v = HelixParams::default().render;
        Self {
            detail_strength: v.detail_strength,
            detail_scale: v.detail_scale,
            density_threshold: v.density_threshold,
            cam_yaw: v.cam_yaw,
            cam_pitch: v.cam_pitch,
            march_steps: v.march_steps,
            age_influence: v.age_influence,
        }
    }
}

impl From<&HelixDef> for HelixParams {
    fn from(def: &HelixDef) -> Self {
        let base = HelixParams::default();
        Self {
            render: VolumetricParams {
                detail_strength: def.look.detail_strength,
                detail_scale: def.look.detail_scale,
                density_threshold: def.look.density_threshold,
                cam_yaw: def.look.cam_yaw,
                cam_pitch: def.look.cam_pitch,
                march_steps: def.look.march_steps,
                age_influence: def.look.age_influence,
                // Not tunable from the .pfx: these define what Helix IS. A preset
                // that switched cam_mode back to orbit would just be a worse
                // Lattice.
                cam_mode: 1,
                env_shape: 2,
                ..base.render
            },
            grid_res: clamp_grid_res(def.grid_res),
            slice_count: clamp_slice_count(def.slice_count),
            slice_rate: def.slice_rate.clamp(1.0, 240.0),
            twist_rate: def.twist_rate,
            wander_rate: def.wander_rate,
            // The remaining fields are param-driven (see `apply_ui_params`); these
            // defaults only stand until the first frame forwards the slots, and in
            // headless probes that never do.
            ..base
        }
    }
}

// --- Bindable parameters -------------------------------------------------------

/// The `.pfx` `inputs` Helix declares, in slot order. Living in `inputs` rather
/// than the `helix` def block is what puts them in the Parameters panel and on the
/// binding bus — a contextual panel's state is not reachable from
/// `apply_binding_target`, so anything parked there can never be driven by MIDI,
/// OSC or audio.
///
/// Helix has no compute shader, so unlike a normal particle effect none of these
/// reach a sim through `effect_params`; they are read CPU-side into
/// [`HelixParams::apply_ui_params`], the same route Splat uses for its camera
/// slots. `helix_pfx_inputs_match_defaults` pins the `.pfx` defaults to
/// [`HelixParams::default`] so the two cannot drift.
pub const HELIX_PARAM_NAMES: [&str; 12] = [
    "radius",
    "thickness",
    "twist",
    "spectrum",
    "wander",
    "ripple",
    "depth",
    "zoom",
    "hue",
    "timbre_hue",
    "absorption",
    "emission",
];

impl HelixParams {
    /// Overwrite the performance knobs from this frame's param slots.
    ///
    /// Params are the source of truth for these fields, so this runs every frame
    /// while Helix is active; the values held in `HelixParams` between calls are
    /// only what the `.pfx`/preset seeded and what headless probes use.
    pub fn apply_ui_params(&mut self, p: &[f32; HELIX_PARAM_NAMES.len()]) {
        self.radius = p[0];
        self.thickness = p[1];
        self.twist_gain = p[2];
        self.spectrum_gain = p[3];
        self.wander = p[4];
        self.ripple_gain = p[5];
        self.render.cam_distance = p[6];
        self.render.fov = p[7];
        self.render.palette_hue = p[8];
        self.hue_spread = p[9];
        self.render.absorption = p[10];
        self.render.emission_gain = p[11];
    }
}

// --- History accumulation ------------------------------------------------------

/// CPU-side ring of retained slices plus the phases that must persist between
/// ticks. Split out from [`HelixSim`] so it can be unit-tested without a GPU.
#[derive(Debug, Clone)]
pub struct HelixHistory {
    slices: Vec<HelixSlice>,
    head: usize,
    /// Time owed to the next slice tick (seconds).
    accum: f32,
    /// Accumulated ribbon twist, WRAPPED to [-PI, PI] every tick.
    ///
    /// Wrapping is not cosmetic: an unbounded angle that is later eased or blended
    /// unwinds through every turn it ever accumulated. `slice_lerp` in the shader
    /// blends the short arc for the same reason.
    twist: f32,
    /// Phase of the centreline drift.
    wander_phase: f32,
    /// Number of ticks pushed since reset — the ring is only fully valid once this
    /// reaches `slices.len()`.
    pushed: u64,
}

impl HelixHistory {
    pub fn new(slice_count: u32) -> Self {
        let n = clamp_slice_count(slice_count) as usize;
        Self {
            slices: vec![HelixSlice::default(); n],
            head: 0,
            accum: 0.0,
            twist: 0.0,
            wander_phase: 0.0,
            pushed: 0,
        }
    }

    pub fn head(&self) -> u32 {
        self.head as u32
    }

    pub fn slices(&self) -> &[HelixSlice] {
        &self.slices
    }

    /// Ticks pushed since reset. Only the tick-rate tests read this; it exists to
    /// make "the window is N seconds, not N frames" directly assertable.
    #[cfg(test)]
    pub fn pushed(&self) -> u64 {
        self.pushed
    }

    /// Advance the tick clock and append however many slices are owed. Returns the
    /// number appended (0 on most frames, >1 only after a stall).
    ///
    /// Ticking on a wall-clock rate rather than per frame is what makes the ribbon
    /// a fixed number of SECONDS long regardless of frame rate.
    pub fn advance(&mut self, audio: &AudioFeatures, params: &HelixParams, dt: f32) -> u32 {
        let rate = params.slice_rate.clamp(1.0, 240.0);
        let period = 1.0 / rate;
        self.accum += dt.clamp(0.0, 0.25);
        let mut pushed = 0u32;
        // Cap the catch-up burst so a long stall cannot rewrite the whole ring in
        // one frame (it would erase the visible history in a single step).
        while self.accum >= period && pushed < 8 {
            self.accum -= period;
            self.push(audio, params, period);
            pushed += 1;
        }
        if self.accum >= period {
            self.accum = 0.0;
        }
        pushed
    }

    fn push(&mut self, audio: &AudioFeatures, params: &HelixParams, dt: f32) {
        // Twist advances faster on-beat, so the corkscrew is tempo-locked rather
        // than a constant spin. Wrapped every tick — see the field docs.
        let beat_drive = 0.5 + audio.beat_phase;
        self.twist += dt * params.twist_rate * beat_drive;
        self.twist = wrap_pi(self.twist);

        self.wander_phase += dt * params.wander_rate;
        // Two incommensurate rates so the centreline never repeats a loop; the
        // brightness of the moment nudges it, so the path is a record of the track.
        let tilt = (audio.centroid - 0.5) * 0.6;
        let cx = (self.wander_phase * std::f32::consts::TAU).sin()
            + 0.4 * (self.wander_phase * std::f32::consts::TAU * 1.7).cos()
            + tilt;
        let cy = (self.wander_phase * std::f32::consts::TAU * 1.31).cos()
            + 0.4 * (self.wander_phase * std::f32::consts::TAU * 0.63).sin();

        let slice = HelixSlice {
            bands_lo: [audio.sub_bass, audio.bass, audio.low_mid, audio.mid],
            bands_hi: [audio.upper_mid, audio.presence, audio.brilliance, audio.rms],
            path: [
                (cx * 0.5).clamp(-1.0, 1.0),
                (cy * 0.5).clamp(-1.0, 1.0),
                self.twist,
                audio.centroid.clamp(0.0, 1.0),
            ],
            // Waveform detail the band energies have averaged away. `onset` and
            // `flux` stand in for the min/max envelope, which is GPU-side only.
            extra: [audio.kick, audio.onset, audio.flux, 0.0],
        };

        self.head = (self.head + 1) % self.slices.len();
        self.slices[self.head] = slice;
        self.pushed += 1;
    }

    /// Where the camera should sit, and how far to bank, to be flying *inside* the
    /// ribbon at depth `z` (unit-cube coordinates, +1 = now).
    ///
    /// Without this the camera would sit on the Z axis while the ribbon wandered
    /// off it, and the flythrough would spend most of the track outside the tube
    /// looking at its flank. Returns `(x, y, roll)`.
    pub fn camera_pose(&self, params: &HelixParams, z: f32) -> (f32, f32, f32) {
        let n = self.slices.len();
        if n == 0 {
            return (0.0, 0.0, 0.0);
        }
        let t = ((z + 1.0) * 0.5).clamp(0.0, 1.0);
        let age = ((1.0 - t) * (n - 1) as f32).round() as usize;
        let idx = (self.head + n - age.min(n - 1)) % n;
        let s = self.slices[idx];
        (
            s.path[0] * params.wander,
            s.path[1] * params.wander,
            // Bank with the ribbon's twist so the horizon rolls with the corkscrew.
            s.path[2],
        )
    }
}

/// Wrap an angle to [-PI, PI].
///
/// Every accumulated angle in the codebase needs this: unwrapped, a later ease or
/// blend travels the long way round through every turn already accumulated.
fn wrap_pi(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (a + PI).rem_euclid(TAU) - PI
}

// --- The simulation ------------------------------------------------------------

/// Owns the history ring, the density + aux volumes it sweeps into, and the sweep
/// / raymarch pipelines. Self-contained density producer: build it at any allowed
/// resolution and it reuses the R3 marcher.
pub struct HelixSim {
    grid_res: u32,
    slice_count: u32,

    // Density volume written by the sweep, sampled by the marcher. Kept alive for
    // the bind groups (owned here, never re-read directly).
    #[allow(dead_code)]
    density_view: TextureView,
    #[allow(dead_code)]
    density_texture: wgpu::Texture,

    // Per-voxel hue (from the slice's spectral centroid); the marcher reads it as
    // its `aux` channel scaled by `age_influence`.
    #[allow(dead_code)]
    aux_view: TextureView,
    #[allow(dead_code)]
    aux_texture: wgpu::Texture,

    sweep_uniform_buffer: wgpu::Buffer,  // HelixUniforms
    render_uniform_buffer: wgpu::Buffer, // VolumetricUniforms (marcher)
    slice_buffer: wgpu::Buffer,          // ring of HelixSlice

    sweep_pipeline: ComputePipeline,
    sweep_bind_group: BindGroup,

    raymarch_pipeline: RenderPipeline,
    raymarch_bind_group: BindGroup,
}

impl HelixSim {
    pub fn new(
        device: &Device,
        hdr_format: TextureFormat,
        grid_res: u32,
        slice_count: u32,
    ) -> Self {
        let grid_res = clamp_grid_res(grid_res);
        let slice_count = clamp_slice_count(slice_count);

        let make_volume = |label: &str| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: grid_res,
                    height: grid_res,
                    depth_or_array_layers: grid_res,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: TextureFormat::R32Float,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            (tex, view)
        };
        let (density_texture, density_view) = make_volume("helix-density");
        let (aux_texture, aux_view) = make_volume("helix-aux");

        let sweep_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("helix-sweep-uniforms"),
            size: std::mem::size_of::<HelixUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let render_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("helix-render-uniforms"),
            size: std::mem::size_of::<VolumetricUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let slice_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("helix-slices"),
            size: (slice_count as u64) * std::mem::size_of::<HelixSlice>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // --- Sweep pipeline: uniform + slice ring (ro) + density + aux (write) ---
        let sweep_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("helix-sweep-bgl"),
            entries: &[
                uniform_entry(0, ShaderStages::COMPUTE),
                storage_ro_entry(1),         // slices
                storage_texture_3d_entry(2), // density_out
                storage_texture_3d_entry(3), // aux_out
            ],
        });
        let sweep_pipeline = create_compute_pipeline(
            device,
            "helix-sweep",
            include_str!("../../../../assets/shaders/builtin/helix_sweep.wgsl"),
            "cs_sweep",
            &sweep_bgl,
        );
        let sweep_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("helix-sweep-bg"),
            layout: &sweep_bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: sweep_uniform_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: slice_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&density_view),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&aux_view),
                },
            ],
        });

        // --- Ray march (shared R3 pipeline over the Helix density + hue aux) ---
        let (raymarch_pipeline, raymarch_bind_group) = create_raymarch(
            device,
            hdr_format,
            &render_uniform_buffer,
            &density_view,
            &aux_view,
        );

        log::info!("Helix sim initialized ({grid_res}^3, {slice_count} slices)");

        Self {
            grid_res,
            slice_count,
            density_view,
            density_texture,
            aux_view,
            aux_texture,
            sweep_uniform_buffer,
            render_uniform_buffer,
            slice_buffer,
            sweep_pipeline,
            sweep_bind_group,
            raymarch_pipeline,
            raymarch_bind_group,
        }
    }

    pub fn grid_res(&self) -> u32 {
        self.grid_res
    }

    pub fn slice_count(&self) -> u32 {
        self.slice_count
    }

    /// Upload the history ring. Cheap enough to do every frame (16 KB at the
    /// defaults) and far simpler than tracking which slices changed.
    pub fn upload_history(&self, queue: &Queue, history: &HelixHistory) {
        let n = self.slice_count as usize;
        let src = history.slices();
        // A resized ring is handled by uploading the overlap; the sim is rebuilt on
        // a real slice-count change, so this only guards the transient frame.
        let take = src.len().min(n);
        queue.write_buffer(&self.slice_buffer, 0, bytemuck::cast_slice(&src[..take]));
    }

    /// Upload the sweep uniforms (grid_res / slice_count re-stamped to this sim).
    pub fn upload_sweep_uniforms(&self, queue: &Queue, uniforms: &HelixUniforms) {
        let mut u = *uniforms;
        u.grid_res = self.grid_res;
        u.slice_count = self.slice_count;
        queue.write_buffer(&self.sweep_uniform_buffer, 0, bytemuck::bytes_of(&u));
    }

    /// Upload the ray-march camera/palette uniforms (grid_res re-stamped so the
    /// marcher samples the density texture at this sim's resolution).
    pub fn upload_render_uniforms(&self, queue: &Queue, uniforms: &VolumetricUniforms) {
        let mut u = *uniforms;
        u.grid_res = self.grid_res;
        queue.write_buffer(&self.render_uniform_buffer, 0, bytemuck::bytes_of(&u));
    }

    /// Rebuild the whole density + aux volume from the current history ring.
    pub fn sweep(&self, encoder: &mut CommandEncoder) {
        let g = self.grid_res.div_ceil(HELIX_WORKGROUP);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("helix-sweep"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.sweep_pipeline);
        pass.set_bind_group(0, &self.sweep_bind_group, &[]);
        pass.dispatch_workgroups(g, g, g);
    }

    pub fn render_raymarch(&self, encoder: &mut CommandEncoder, target: &TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("helix-raymarch"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.raymarch_pipeline);
        pass.set_bind_group(0, &self.raymarch_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::test_gpu::{gpu_guard, test_gpu};

    #[test]
    fn helix_uniforms_is_48_bytes() {
        // 12 x 4-byte scalars, 16-byte-aligned for the uniform address space.
        // Must byte-match `HelixUniforms` in helix_sweep.wgsl.
        assert_eq!(std::mem::size_of::<HelixUniforms>(), 48);
    }

    #[test]
    fn helix_slice_is_64_bytes() {
        // 4 x vec4f as the shader reads it.
        assert_eq!(std::mem::size_of::<HelixSlice>(), 64);
    }

    /// The shipped preset must deserialise into a `HelixDef` and turn the effect
    /// on. Embedded with `include_str!` so the test is CWD-independent.
    #[test]
    fn shipped_preset_parses() {
        let src = include_str!("../../../../assets/effects/helix.pfx");
        let v: serde_json::Value = serde_json::from_str(src).expect("helix.pfx is valid JSON");
        let def: HelixDef = serde_json::from_value(v["particles"]["helix"].clone())
            .expect("helix block maps to HelixDef");
        let p = HelixParams::from(&def);
        // Helix IS the flythrough — a preset must not be able to land on the orbit
        // camera or a lateral envelope.
        assert_eq!(p.render.cam_mode, 1);
        assert_eq!(p.render.env_shape, 2);
        assert_eq!(p.grid_res, 128);
        assert!((p.slice_count as f32 / p.slice_rate - 8.0).abs() < 0.01);
    }

    /// The performance knobs are declared in the `.pfx` `inputs` and applied
    /// CPU-side, so nothing in the type system ties their defaults to
    /// [`HelixParams::default`] — which is what headless probes and the first
    /// frame use. Pin them, or the shipped effect drifts away from what every
    /// test renders.
    #[test]
    fn helix_pfx_inputs_match_defaults() {
        let src = include_str!("../../../../assets/effects/helix.pfx");
        let v: serde_json::Value = serde_json::from_str(src).unwrap();
        let inputs = v["inputs"].as_array().expect("helix.pfx declares inputs");
        assert_eq!(
            inputs.len(),
            HELIX_PARAM_NAMES.len(),
            "every bindable knob needs a slot, in order"
        );

        let mut slots = [0.0f32; HELIX_PARAM_NAMES.len()];
        for (i, (input, expected_name)) in inputs.iter().zip(HELIX_PARAM_NAMES).enumerate() {
            assert_eq!(
                input["name"].as_str().unwrap(),
                expected_name,
                "slot {i} is read positionally in app.rs — order is load-bearing"
            );
            slots[i] = input["default"].as_f64().unwrap() as f32;
        }

        // Applying the shipped defaults must be a no-op against the Rust defaults.
        let mut from_params = HelixParams::default();
        from_params.apply_ui_params(&slots);
        let base = HelixParams::default();
        let close = |a: f32, b: f32, what: &str| {
            assert!((a - b).abs() < 1e-6, "{what}: pfx {a} != default {b}");
        };
        close(from_params.radius, base.radius, "radius");
        close(from_params.thickness, base.thickness, "thickness");
        close(from_params.twist_gain, base.twist_gain, "twist");
        close(from_params.spectrum_gain, base.spectrum_gain, "spectrum");
        close(from_params.wander, base.wander, "wander");
        close(from_params.ripple_gain, base.ripple_gain, "ripple");
        close(
            from_params.render.cam_distance,
            base.render.cam_distance,
            "depth",
        );
        close(from_params.render.fov, base.render.fov, "zoom");
        close(
            from_params.render.palette_hue,
            base.render.palette_hue,
            "hue",
        );
        close(from_params.hue_spread, base.hue_spread, "timbre_hue");
        close(
            from_params.render.absorption,
            base.render.absorption,
            "absorption",
        );
        close(
            from_params.render.emission_gain,
            base.render.emission_gain,
            "emission",
        );
    }

    /// A `.pfx` with an empty helix block must still be a valid Helix, not a
    /// half-configured one — every field has to have a working default.
    #[test]
    fn empty_helix_block_is_valid() {
        let def: HelixDef = serde_json::from_str("{}").unwrap();
        let p = HelixParams::from(&def);
        assert_eq!(p.render.cam_mode, 1);
        assert!(p.radius > 0.0);
        assert!(p.thickness > 0.0);
        assert!(p.slice_rate >= 1.0);
    }

    #[test]
    fn wrap_pi_keeps_angles_bounded() {
        use std::f32::consts::{PI, TAU};
        assert!((wrap_pi(0.0) - 0.0).abs() < 1e-6);
        assert!((wrap_pi(TAU) - 0.0).abs() < 1e-5);
        assert!((wrap_pi(PI + 0.1) - (-PI + 0.1)).abs() < 1e-4);
        // The point of wrapping: a long accumulation stays in range instead of
        // growing without bound and unwinding later.
        assert!(wrap_pi(1000.0 * TAU + 0.25).abs() <= PI + 1e-4);
    }

    /// The tick clock must make the window a fixed number of SECONDS, not frames.
    #[test]
    fn history_ticks_on_wall_clock_not_frames() {
        let params = HelixParams::default(); // slice_rate 32 Hz
        let audio = AudioFeatures::default();

        // 1 second of 60 fps frames -> ~32 slices, not 60.
        let mut fast = HelixHistory::new(256);
        for _ in 0..60 {
            fast.advance(&audio, &params, 1.0 / 60.0);
        }
        // 1 second of 24 fps frames -> the same ~32 slices.
        let mut slow = HelixHistory::new(256);
        for _ in 0..24 {
            slow.advance(&audio, &params, 1.0 / 24.0);
        }

        assert_eq!(fast.pushed(), 32, "60 fps for 1s should tick 32 times");
        assert_eq!(slow.pushed(), 32, "24 fps for 1s should tick 32 times");
    }

    /// A stall must not rewrite the entire ring in one frame — that would erase all
    /// visible history in a single step.
    #[test]
    fn history_catchup_is_capped() {
        let params = HelixParams::default();
        let audio = AudioFeatures::default();
        let mut h = HelixHistory::new(256);
        let pushed = h.advance(&audio, &params, 10.0);
        assert!(pushed <= 8, "catch-up burst should be capped, got {pushed}");
    }

    /// Regression for the accumulated-angle bug: twist must stay bounded no matter
    /// how long the effect runs.
    #[test]
    fn twist_stays_bounded_over_a_long_run() {
        use std::f32::consts::PI;
        let params = HelixParams::default();
        let audio = AudioFeatures {
            beat_phase: 0.9,
            ..AudioFeatures::default()
        };
        let mut h = HelixHistory::new(64);
        // ~10 minutes of ticks.
        for _ in 0..(32 * 600) {
            h.advance(&audio, &params, 1.0 / 32.0);
        }
        for s in h.slices() {
            assert!(
                s.path[2].abs() <= PI + 1e-3,
                "twist escaped [-PI,PI]: {}",
                s.path[2]
            );
        }
    }

    /// Newest slice lands at `head`, and older material walks backwards from it —
    /// the indexing the shader's `slice_at(age)` assumes.
    #[test]
    fn ring_head_points_at_the_newest_slice() {
        let params = HelixParams::default();
        let mut h = HelixHistory::new(16);
        for i in 0..5 {
            let audio = AudioFeatures {
                rms: (i + 1) as f32 * 0.1,
                ..AudioFeatures::default()
            };
            h.advance(&audio, &params, 1.0 / params.slice_rate);
        }
        let n = h.slices().len();
        let head = h.head() as usize;
        // rms is stored in bands_hi[3]; newest push had rms 0.5.
        assert!((h.slices()[head].bands_hi[3] - 0.5).abs() < 1e-6);
        let prev = (head + n - 1) % n;
        assert!((h.slices()[prev].bands_hi[3] - 0.4).abs() < 1e-6);
    }

    /// Synthetic "track" for the probe: a build from sparse bass to dense bright
    /// material with beats, so the ribbon has something to vary along its length.
    fn synth_history(params: &HelixParams, seconds: f32) -> HelixHistory {
        let mut h = HelixHistory::new(params.slice_count);
        let dt = 1.0 / params.slice_rate;
        let steps = (seconds / dt) as u32;
        for i in 0..steps {
            let t = i as f32 * dt;
            let build = (t / seconds).clamp(0.0, 1.0);
            // 2 Hz beat (120 BPM).
            let phase = (t * 2.0).fract();
            let on_beat = if phase < 0.08 { 1.0 } else { 0.0 };
            let audio = AudioFeatures {
                sub_bass: 0.5 + 0.4 * (t * 1.7).sin() * (1.0 - build * 0.4),
                bass: 0.45 + 0.35 * (t * 2.3).sin(),
                low_mid: 0.2 + 0.5 * build,
                mid: 0.15 + 0.55 * build * (0.6 + 0.4 * (t * 3.1).sin()),
                upper_mid: 0.1 + 0.6 * build,
                presence: 0.05 + 0.7 * build * (0.5 + 0.5 * (t * 5.0).cos()),
                brilliance: 0.05 + 0.6 * build,
                rms: 0.35 + 0.45 * build,
                kick: on_beat,
                onset: on_beat * 0.8,
                flux: 0.2 + 0.3 * build,
                centroid: 0.25 + 0.6 * build,
                beat_phase: phase,
                ..AudioFeatures::default()
            };
            h.advance(&audio, params, dt);
        }
        h
    }

    /// Headless offscreen render of the ribbon to PNG. This is the Phase-1
    /// distinctness gate: it answers "is the geometry right", NOT "is the effect
    /// good" — a still frame cannot judge an effect whose whole subject is motion.
    /// Run with `HELIX_PNG_DIR=/path cargo test -- --ignored helix_render_previews`.
    #[test]
    #[ignore = "requires a GPU/software adapter; writes PNGs"]
    fn helix_render_previews() {
        let out_dir = std::env::var("HELIX_PNG_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();

        let (w, h) = (512u32, 512u32);
        let fmt = TextureFormat::Rgba8UnormSrgb;

        let mut params = HelixParams::default();
        if let Ok(g) = std::env::var("HELIX_PREVIEW_GRID") {
            params.grid_res = clamp_grid_res(g.parse().unwrap_or(params.grid_res));
        }
        if let Ok(v) = std::env::var("HELIX_RADIUS") {
            params.radius = v.parse().unwrap_or(params.radius);
        }
        if let Ok(v) = std::env::var("HELIX_THICKNESS") {
            params.thickness = v.parse().unwrap_or(params.thickness);
        }
        if let Ok(v) = std::env::var("HELIX_TWIST") {
            params.twist_gain = v.parse().unwrap_or(params.twist_gain);
        }
        if let Ok(v) = std::env::var("HELIX_SPECTRUM") {
            params.spectrum_gain = v.parse().unwrap_or(params.spectrum_gain);
        }
        if let Ok(v) = std::env::var("HELIX_WANDER") {
            params.wander = v.parse().unwrap_or(params.wander);
        }
        if let Ok(v) = std::env::var("HELIX_EMISSION") {
            params.render.emission_gain = v.parse().unwrap_or(params.render.emission_gain);
        }
        if let Ok(v) = std::env::var("HELIX_ABSORPTION") {
            params.render.absorption = v.parse().unwrap_or(params.render.absorption);
        }
        if let Ok(v) = std::env::var("HELIX_FOV") {
            params.render.fov = v.parse().unwrap_or(params.render.fov);
        }
        if let Ok(v) = std::env::var("HELIX_DETAIL_SCALE") {
            params.render.detail_scale = v.parse().unwrap_or(params.render.detail_scale);
        }
        if let Ok(v) = std::env::var("HELIX_DETAIL_STRENGTH") {
            params.render.detail_strength = v.parse().unwrap_or(params.render.detail_strength);
        }
        // The marcher scrolls its detail FBM by `u.time`, so this knob is how you
        // see what a frame-time jump does to the wall texture.
        let render_time: f32 = std::env::var("HELIX_TIME")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);

        let history = synth_history(&params, 8.0);
        let sim = HelixSim::new(&device, fmt, params.grid_res, params.slice_count);

        // `flythrough` is the shipping camera; the two orbit views are diagnostics
        // that show the ribbon's whole shape from outside, which the flythrough by
        // definition cannot.
        // (name, cam_mode, yaw, pitch, distance/z)
        let views: [(&str, u32, f32, f32, f32); 4] = [
            ("flythrough", 1, 0.0, 0.0, 0.88),
            ("flythrough_deep", 1, 0.0, 0.0, 0.1),
            ("orbit", 0, 0.0, 0.25, 2.2),
            ("side", 0, std::f32::consts::FRAC_PI_2, 0.0, 2.4),
        ];

        for (name, cam_mode, yaw, pitch, dist) in views {
            let mut fc =
                crate::gpu::frame_capture::FrameCapture::new(&device, w, h, fmt, "preview");

            sim.upload_history(&queue, &history);
            let hu = params.build_uniforms(history.head(), sim.slice_count(), 0.0);
            sim.upload_sweep_uniforms(&queue, &hu);

            let mut render = params.render;
            render.cam_mode = cam_mode;
            render.cam_yaw = yaw;
            render.cam_pitch = pitch;
            render.cam_distance = dist;
            if cam_mode == 1 {
                let (cx, cy, roll) = history.camera_pose(&params, dist);
                render.cam_x = cx;
                render.cam_y = cy;
                render.cam_roll = roll;
            } else {
                // The orbit diagnostics look at the volume from outside, where the
                // tube envelope's Z-only fade would leave hard lateral cube faces.
                render.env_shape = 0;
            }
            let mut ru = render.build_uniforms(
                [w as f32, h as f32],
                render_time,
                0.0,
                0.0,
                0.5,
                0.0,
                0.0,
                0.0,
            );
            ru.grid_res = params.grid_res;
            sim.upload_render_uniforms(&queue, &ru);

            let mut enc = device.create_command_encoder(&Default::default());
            sim.sweep(&mut enc);
            {
                let _clear = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("preview-clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &fc.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
            }
            sim.render_raymarch(&mut enc, &fc.view);
            fc.copy_to_staging(&mut enc);
            queue.submit([enc.finish()]);
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();

            fc.request_map();
            let data = loop {
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                if let Some(d) = fc.take_mapped_data(&device) {
                    break d;
                }
            };

            // Guard against the failure this probe exists to catch: an empty or
            // uniformly-filled volume both "render fine" and tell you nothing.
            let lit = data
                .chunks_exact(4)
                .filter(|px| px[0] as u32 + px[1] as u32 + px[2] as u32 > 24)
                .count();
            let frac = lit as f32 / (w * h) as f32;

            // Write the PNG BEFORE asserting — when the guard trips, the image is
            // the only thing that says why.
            let path = format!("{out_dir}/helix_{name}.png");
            image::RgbaImage::from_raw(w, h, data)
                .expect("raw->image")
                .save(&path)
                .expect("save png");
            eprintln!("wrote {path} ({:.1}% lit)", frac * 100.0);

            assert!(
                frac > 0.01,
                "{name}: volume rendered essentially empty ({:.3}% lit)",
                frac * 100.0
            );
            assert!(
                frac < 0.98,
                "{name}: volume rendered as a solid fill ({:.3}% lit) — the shell \
                 collapsed or the profile blew past the cube",
                frac * 100.0
            );
        }
    }
}
