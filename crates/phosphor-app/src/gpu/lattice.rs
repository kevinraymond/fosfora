//! Lattice — 3D cellular automata in the volumetric density field (flagship).
//!
//! A voxel grid of cells (dead / alive / dying) evolves by 3D totalistic rules
//! that count neighbours in a 3x3x3 Moore (26) or Von Neumann (6) neighbourhood.
//! Produces self-organising volumetric structures: growing crystals, pulsing
//! organisms, expanding shells, collapsing caverns. Unlike the 2D reaction-
//! diffusion / physarum fields, the state IS the 3D volume.
//!
//! Lattice is a self-contained **density producer**: it owns its own resizable
//! `r32float` 3D density texture and reuses the R3 ray marcher unchanged (via the
//! shared [`crate::gpu::volumetric::create_raymarch`] builder plus the
//! `VolumetricUniforms` / `VolumetricParams` types for camera / palette). Each CA
//! generation is one compute pass over ping-pong `array<u32>` state buffers; a
//! separate once-per-frame display pass EMA-blends the freshest state into the
//! density texture the marcher samples, so fast rules fade instead of strobing.
//!
//! All rule / audio -> mask logic lives here on the CPU; the shader just applies
//! whatever birth / survival bitmasks the uniform block carries.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor, CommandEncoder,
    ComputePipeline, Device, Queue, RenderPipeline, ShaderStages, TextureFormat, TextureView,
};

use crate::gpu::volumetric::{
    VolumetricParams, VolumetricUniforms, create_compute_pipeline, create_raymarch,
    storage_ro_entry, storage_rw_entry, storage_texture_3d_rw_entry, uniform_entry,
};

/// `@workgroup_size(4,4,4)` — a 4^3 block of voxels per workgroup.
const LATTICE_WORKGROUP: u32 = 4;

/// Default grid resolution (per axis). Selectable 32 / 64 / 128 / 256.
pub const DEFAULT_GRID_RES: u32 = 128;

/// Allowed grid resolutions (UI selector + `clamp_grid_res`).
pub const GRID_RES_CHOICES: [u32; 4] = [32, 64, 128, 256];

/// Named 3D-CA rule presets in `S<survival>/B<birth>/<states>/<neighbourhood>`
/// notation (Softology / Rampe catalogue). Parsed to bitmasks by [`parse_rule`].
pub const PRESET_RULES: [(&str, &str); 8] = [
    ("Clouds", "S13-26/B13-14,17-19/2/M"),
    ("Shells", "S4-8/B6-8/14/M"),
    ("Pyroclastic", "S4-7/B6-8/10/M"),
    ("Chunky", "S6-9/B6-8/14/M"),
    ("Brain", "S6-10/B5-7/12/M"),
    ("Builder", "S2,6,9/B4,6,8-9/10/M"),
    ("445", "S4/B4/5/M"),
    ("Pulse", "S2-3/B3/2/VN"),
];

/// Default preset index (Pyroclastic — expanding, fading shells).
pub const DEFAULT_PRESET: u32 = 2;

/// Snap an arbitrary value to the nearest allowed grid resolution.
pub fn clamp_grid_res(r: u32) -> u32 {
    *GRID_RES_CHOICES
        .iter()
        .min_by_key(|&&c| c.abs_diff(r))
        .unwrap_or(&DEFAULT_GRID_RES)
}

// --- Rule notation -> 27-bit masks -------------------------------------------

/// Set bit `c` for each neighbour count in the inclusive ranges (clamped 0..=26).
fn mask_ranges(ranges: &[(u32, u32)]) -> u32 {
    let mut m = 0u32;
    for &(lo, hi) in ranges {
        let mut c = lo;
        while c <= hi.min(26) {
            m |= 1 << c;
            c += 1;
        }
    }
    m
}

/// Parse a comma-separated count list like `"13-14,17-19"` or `"1,3"` into a mask.
fn parse_counts(s: &str) -> u32 {
    let mut m = 0u32;
    for tok in s.split(',') {
        let t = tok.trim();
        if t.is_empty() {
            continue;
        }
        if let Some((a, b)) = t.split_once('-') {
            if let (Ok(lo), Ok(hi)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>()) {
                m |= mask_ranges(&[(lo, hi)]);
            }
        } else if let Ok(c) = t.parse::<u32>() {
            if c <= 26 {
                m |= 1 << c;
            }
        }
    }
    m
}

/// Parse `"S4-7/B6-8/10/M"` -> `(birth_mask, survival_mask, num_states, neighbourhood)`.
/// Neighbourhood: `M` = Moore (0), `N`/`V` = Von Neumann (1). Field order is
/// survival, birth, states, neighbourhood but parsing is prefix-driven so order is
/// tolerant.
pub fn parse_rule(rule: &str) -> (u32, u32, u32, u32) {
    let (mut birth, mut survival, mut states, mut nbhd) = (0u32, 0u32, 2u32, 0u32);
    for part in rule.split('/') {
        let p = part.trim();
        if let Some(rest) = p.strip_prefix(['S', 's']) {
            survival = parse_counts(rest);
        } else if let Some(rest) = p.strip_prefix(['B', 'b']) {
            birth = parse_counts(rest);
        } else if p.eq_ignore_ascii_case("M") {
            nbhd = 0;
        } else if p.eq_ignore_ascii_case("VN")
            || p.eq_ignore_ascii_case("N")
            || p.eq_ignore_ascii_case("V")
        {
            nbhd = 1;
        } else if let Ok(n) = p.parse::<u32>() {
            states = n.clamp(2, 20);
        }
    }
    (birth, survival, states, nbhd)
}

/// Dilate a neighbour-count mask by `steps` (spread each set bit to its 0..=26
/// neighbours). Used to bias birth (mids) / survival (highs) more permissive.
fn dilate_mask(mask: u32, steps: u32) -> u32 {
    let mut m = mask;
    for _ in 0..steps {
        m |= ((m << 1) | (m >> 1)) & 0x07FF_FFFF; // keep bits 0..26
    }
    m & 0x07FF_FFFF
}

/// Audio energy -> dilation steps (0 / 1 / 2).
fn dilate_steps(energy: f32) -> u32 {
    if energy > 0.66 {
        2
    } else {
        u32::from(energy > 0.33)
    }
}

/// Max CA generations run in a single frame (backstop against a frame hitch
/// burst-simulating a huge jump).
pub const LATTICE_MAX_STEPS_PER_FRAME: u32 = 8;

/// Live-cell fraction below which the grid counts as died-out. Set below a fresh
/// central seed's fill (a small noise sphere is only a few hundred cells) so a
/// just-seeded grid is never mistaken for a dead one — combined with the post-seed
/// grace period, this stops slow rules (Shells/Brain/Chunky) from
/// reseed-thrashing before their first generation can even run.
pub const LATTICE_DEATH_FRACTION: f32 = 0.0002;
/// Seconds a grid must stay dead before auto-reseeding. Short so a rule that
/// genuinely dies out in silence revives quickly, but non-zero so a momentary
/// dip through the death fraction doesn't trigger a spurious reseed.
pub const LATTICE_RESEED_DWELL_SECS: f32 = 0.5;
/// Seconds after a (re)seed during which auto-reseed is suppressed, giving the
/// fresh seed time to grow into structure. Without it, a slow rule whose seed is
/// still below the death fraction reseeds every dwell forever, never advancing a
/// generation (stuck as a flickering dot in silence).
pub const LATTICE_RESEED_GRACE_SECS: f32 = 2.5;

/// Advance the auto-reseed timer for one frame, returning
/// `(should_reseed, new_stagnant_secs)`. Reseed is a pure *death* safety net: a
/// grid that has died out (< [`LATTICE_DEATH_FRACTION`]) for longer than
/// [`LATTICE_RESEED_DWELL_SECS`] is revived (and the timer resets); any live
/// population resets the timer immediately. Saturation is no longer a trigger —
/// the per-cell lifetime cap (`max_age`) keeps growth rules churning so they never
/// pack into a solid ball, which is what a saturation reseed used to (jarringly)
/// correct. Pure so the policy is testable without a GPU.
pub fn lattice_stagnation_tick(frac: f32, stagnant_secs: f32, dt: f32) -> (bool, f32) {
    if frac >= LATTICE_DEATH_FRACTION {
        return (false, 0.0);
    }
    let secs = stagnant_secs + dt.max(0.0);
    if secs >= LATTICE_RESEED_DWELL_SECS {
        (true, 0.0)
    } else {
        (false, secs)
    }
}

/// Drain a fractional generations-per-second accumulator into an integer step
/// count for this frame, returning `(steps, new_accum)`. `bass` in `[0,1]` scales
/// the rate between `bass_floor * gen_per_sec` (silence) and `gen_per_sec` (full
/// bass). Steps are capped at [`LATTICE_MAX_STEPS_PER_FRAME`]; any backlog beyond
/// one generation is dropped so a long frame doesn't burst-simulate.
pub fn lattice_step_budget(
    accum: f32,
    gen_per_sec: f32,
    bass_floor: f32,
    bass: f32,
    dt: f32,
) -> (u32, f32) {
    let floor = bass_floor.clamp(0.0, 1.0);
    let drive = floor + (1.0 - floor) * bass.clamp(0.0, 1.0);
    let acc = accum.max(0.0) + gen_per_sec.max(0.0) * drive * dt.max(0.0);
    let steps = (acc.floor() as u32).min(LATTICE_MAX_STEPS_PER_FRAME);
    let residual = (acc - steps as f32).min(1.0);
    (steps, residual)
}

// --- GPU uniform block --------------------------------------------------------

/// GPU-side uniform block for `cs_seed`, `cs_step`, and `cs_display`. Mirrored
/// byte-for-byte by `LatticeUniforms` in `lattice_seed.wgsl` / `lattice_step.wgsl`
/// / `lattice_display.wgsl`. 20 scalars = 80 bytes (multiple of 16 for the uniform
/// address space).
#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
pub struct LatticeUniforms {
    pub grid_res: u32,
    pub birth_mask: u32,
    pub survival_mask: u32,
    pub num_states: u32,
    pub neighborhood: u32,  // 0 = Moore(26), 1 = Von Neumann(6)
    pub boundary: u32,      // 0 = toroidal wrap, 1 = clamp (dead border)
    pub frame: u32,         // temporal hash salt
    pub init_mode: u32,     // 0 random, 1 center, 2 multi-seed, 3 clear (seed pass)
    pub init_density: f32,  // random-fill probability
    pub seed_size: u32,     // seed / injection cluster radius (cells)
    pub seed_hash: u32,     // PRNG salt (randomise / reseed variation)
    pub inject_active: u32, // onset seed-cluster injection this frame (0/1)
    pub perturb_prob: f32,  // flux-scaled per-cell perturbation probability
    pub smooth_rate: f32,   // density EMA rate (1/s) in the display pass
    pub color_mode: u32,    // reserved (density-based color modes)
    pub time: f32,
    pub dt: f32,            // frame delta-time (s) for the display-pass EMA
    pub domain_mode: u32,   // 0 = full cube, 1 = spherical domain
    pub domain_radius: f32, // sphere radius as a fraction of the half-extent
    pub max_age: u32,       // lifetime cap in generations (0 = off / infinite)
}

// --- Host-side params ---------------------------------------------------------

/// Tunable Lattice parameters (host-side). Held on the effect's `ParticleSystem`,
/// built from the `.pfx` [`LatticeDef`] at load and edited live via the contextual
/// Lattice panel. Embeds a [`VolumetricParams`] for the reused camera / palette /
/// marcher controls.
#[derive(Debug, Copy, Clone)]
pub struct LatticeParams {
    /// Camera / palette / look — reuses the R3 marcher's tunables.
    pub render: VolumetricParams,
    /// Voxel resolution per axis (32 / 64 / 128 / 256).
    pub grid_res: u32,
    /// Selected preset index into [`PRESET_RULES`].
    pub rule_preset: u32,
    /// When true, `birth_mask`/`survival_mask` are used directly (preset changes
    /// don't overwrite them).
    pub manual_masks: bool,
    pub birth_mask: u32,
    pub survival_mask: u32,
    pub num_states: u32,
    /// Lifetime cap in CA generations: a live cell dies once its age reaches this,
    /// forcing continuous turnover so growth rules hollow out (the oldest cells,
    /// the core, die first) instead of packing into a solid ball. This is the
    /// primary "breathing" control; `0` disables the cap (infinite lifetime).
    pub max_age: u32,
    pub neighborhood: u32,
    pub boundary: u32,
    /// CA generations per second at full bass (scaled toward `bass_floor` in
    /// silence). Drained by a fractional accumulator so it is frame-rate stable.
    pub gen_per_sec: f32,
    /// Fraction of `gen_per_sec` that still runs at silence (0 = fully freezes,
    /// 1 = bass-independent). Keeps slow structures breathing between beats.
    pub bass_floor: f32,
    /// Density EMA rate (1/s) applied in the display pass — decouples the shown
    /// volume from raw CA generations so fast rules fade instead of strobing.
    pub smooth_rate: f32,
    /// Live domain: 0 = full cube, 1 = sphere (cells outside are forced dead,
    /// removing the cube silhouette for growth rules).
    pub domain_mode: u32,
    /// Sphere radius as a fraction of the grid half-extent (domain_mode == 1).
    pub domain_radius: f32,
    /// Cap on audio mask dilation (0 disables it; growth-heavy rules want 0).
    pub dilation_max: u32,
    pub init_mode: u32,
    pub init_density: f32,
    pub seed_size: u32,
    pub color_mode: u32,
    /// Scales spectral-flux into the per-cell perturbation probability.
    pub perturb_scale: f32,
    /// PRNG salt, bumped by the randomise / reseed controls.
    pub seed_hash: u32,
}

impl Default for LatticeParams {
    fn default() -> Self {
        let (birth, survival, states, nbhd) = parse_rule(PRESET_RULES[DEFAULT_PRESET as usize].1);
        Self {
            // Lattice's density is a raw CA field (no scatter/resolve softening),
            // so it drives the marcher with a spherical envelope + ray jitter that
            // R3 leaves off by default.
            render: VolumetricParams {
                env_shape: 1,
                jitter_amp: 1.0,
                ..VolumetricParams::default()
            },
            grid_res: DEFAULT_GRID_RES,
            rule_preset: DEFAULT_PRESET,
            manual_masks: false,
            birth_mask: birth,
            survival_mask: survival,
            num_states: states,
            // Off by default; each preset sets its own lifetime via the .pfx.
            max_age: 0,
            neighborhood: nbhd,
            boundary: 0,
            gen_per_sec: 8.0,
            bass_floor: 0.35,
            smooth_rate: 8.0,
            domain_mode: 1,
            domain_radius: 0.9,
            // Off by default: dilating the rule on loud audio makes it more
            // permissive, which packs the domain into a featureless solid ball.
            dilation_max: 0,
            init_mode: 0,
            init_density: 0.12,
            seed_size: 6,
            color_mode: 0,
            perturb_scale: 0.02,
            seed_hash: 0x9E37_79B9,
        }
    }
}

impl LatticeParams {
    /// Apply preset `idx`: resolve its masks/states/neighbourhood and clear the
    /// manual-mask flag.
    pub fn apply_preset(&mut self, idx: u32) {
        let idx = idx.min(PRESET_RULES.len() as u32 - 1);
        let (birth, survival, states, nbhd) = parse_rule(PRESET_RULES[idx as usize].1);
        self.rule_preset = idx;
        self.birth_mask = birth;
        self.survival_mask = survival;
        self.num_states = states;
        self.neighborhood = nbhd;
        self.manual_masks = false;
    }

    /// Randomise the birth/survival masks within ranges likely to produce
    /// interesting behaviour (birth 2..8, survival 3..12 neighbours). Deterministic
    /// per call via the advancing `seed_hash` LCG; sets manual-mask mode.
    pub fn randomize_rule(&mut self) {
        let mut s = self.seed_hash;
        let mut next = || {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            s
        };
        let mut birth = 0u32;
        for c in 2..=8u32 {
            if next() & 1 == 1 {
                birth |= 1 << c;
            }
        }
        let mut survival = 0u32;
        for c in 3..=12u32 {
            if next() & 1 == 1 {
                survival |= 1 << c;
            }
        }
        // Avoid a degenerate empty rule.
        self.birth_mask = if birth == 0 { 1 << 5 } else { birth };
        self.survival_mask = if survival == 0 { 1 << 5 } else { survival };
        self.manual_masks = true;
        self.seed_hash = next();
    }

    /// Pack params + per-frame audio into the GPU uniform block. `grid_res` is
    /// re-stamped by [`LatticeSim::upload_ca_uniforms`] to the sim's actual size.
    #[allow(clippy::too_many_arguments)]
    pub fn build_uniforms(
        &self,
        frame: u32,
        onset: f32,
        flux: f32,
        mid: f32,
        high: f32,
        time: f32,
        dt: f32,
    ) -> LatticeUniforms {
        // Base rule (preset-resolved or manual), biased by audio: mids make birth
        // more permissive, highs make survival more permissive (dilate the masks),
        // capped per-preset so growth-heavy rules don't saturate the whole domain.
        let cap = self.dilation_max;
        let birth = dilate_mask(self.birth_mask, dilate_steps(mid).min(cap));
        let survival = dilate_mask(self.survival_mask, dilate_steps(high).min(cap));
        LatticeUniforms {
            grid_res: self.grid_res,
            birth_mask: birth,
            survival_mask: survival,
            num_states: self.num_states.clamp(2, 20),
            neighborhood: self.neighborhood,
            boundary: self.boundary,
            frame,
            init_mode: self.init_mode,
            init_density: self.init_density,
            seed_size: self.seed_size.max(1),
            seed_hash: self.seed_hash,
            inject_active: u32::from(onset > 0.5),
            perturb_prob: (self.perturb_scale * flux).clamp(0.0, 0.2),
            smooth_rate: self.smooth_rate.max(0.1),
            color_mode: self.color_mode,
            time,
            dt: dt.clamp(0.0, 0.1),
            domain_mode: self.domain_mode,
            domain_radius: self.domain_radius.clamp(0.1, 1.0),
            // Cap below the age field's 255 saturation so a set lifetime can fire.
            max_age: self.max_age.min(254),
        }
    }
}

// --- Effect def (.pfx `particles.lattice` block) ------------------------------

/// Serde config for a Lattice effect, carried in a `.pfx` `particles.lattice`
/// block (parallels [`crate::gpu::particle::types::ReactionDiffusionDef`]).
/// Converted to a runtime [`LatticeParams`] at effect load via `From`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LatticeDef {
    #[serde(default = "default_lattice_grid")]
    pub grid_res: u32,
    /// Named preset from [`PRESET_RULES`] (e.g. "Pyroclastic"). Ignored if `rule` is set.
    #[serde(default)]
    pub preset: String,
    /// Optional raw rule-notation override, e.g. "S4-7/B6-8/10/M".
    #[serde(default)]
    pub rule: String,
    /// CA generations per second at full bass (frame-rate independent).
    #[serde(default = "default_lattice_gen_per_sec")]
    pub gen_per_sec: f32,
    /// Fraction of `gen_per_sec` that still runs at silence (0..1).
    #[serde(default = "default_lattice_bass_floor")]
    pub bass_floor: f32,
    /// Density EMA rate (1/s) applied in the display pass.
    #[serde(default = "default_lattice_smooth_rate")]
    pub smooth_rate: f32,
    /// Lifetime cap in CA generations: a live cell dies once it reaches this age,
    /// forcing continuous turnover so growth rules hollow out instead of packing
    /// into a solid ball. The primary "breathing" control. 0 disables the cap.
    #[serde(default)]
    pub max_age: u32,
    /// Boundary policy: 0 = toroidal wrap, 1 = dead border (clamp).
    #[serde(default)]
    pub boundary: u32,
    /// Live domain: 0 = full cube, 1 = sphere.
    #[serde(default = "default_lattice_domain_mode")]
    pub domain_mode: u32,
    /// Sphere radius as a fraction of the grid half-extent.
    #[serde(default = "default_lattice_domain_radius")]
    pub domain_radius: f32,
    /// Cap on audio mask dilation (0 disables).
    #[serde(default = "default_lattice_dilation_max")]
    pub dilation_max: u32,
    #[serde(default)]
    pub init_mode: u32,
    #[serde(default = "default_lattice_init_density")]
    pub init_density: f32,
    #[serde(default = "default_lattice_seed_size")]
    pub seed_size: u32,
    #[serde(default)]
    pub perturb_scale: f32,
    /// Marcher look (camera / palette / detail). Defaults to the R3 marcher
    /// defaults; each preset overrides hue / absorption / camera for its own read.
    #[serde(default)]
    pub look: LatticeLookDef,
}

/// The shared-marcher look block of a [`LatticeDef`] — the subset of
/// [`VolumetricParams`] a `.pfx` may tune per preset. All fields default to the
/// R3 [`VolumetricParams::default`] values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LatticeLookDef {
    pub palette_hue: f32,
    pub absorption: f32,
    pub emission_gain: f32,
    pub detail_strength: f32,
    pub detail_scale: f32,
    pub cam_distance: f32,
    pub cam_pitch: f32,
    pub cam_orbit_speed: f32,
    pub march_steps: u32,
    /// Age → hue modulation (Tier B). 0 disables the age tint.
    pub age_influence: f32,
}

impl Default for LatticeLookDef {
    fn default() -> Self {
        let v = VolumetricParams::default();
        Self {
            palette_hue: v.palette_hue,
            absorption: v.absorption,
            emission_gain: v.emission_gain,
            detail_strength: v.detail_strength,
            detail_scale: v.detail_scale,
            cam_distance: v.cam_distance,
            cam_pitch: v.cam_pitch,
            cam_orbit_speed: v.cam_orbit_speed,
            march_steps: v.march_steps,
            age_influence: v.age_influence,
        }
    }
}

fn default_lattice_grid() -> u32 {
    DEFAULT_GRID_RES
}
fn default_lattice_gen_per_sec() -> f32 {
    8.0
}
fn default_lattice_bass_floor() -> f32 {
    0.35
}
fn default_lattice_smooth_rate() -> f32 {
    8.0
}
fn default_lattice_domain_mode() -> u32 {
    1
}
fn default_lattice_domain_radius() -> f32 {
    0.9
}
fn default_lattice_dilation_max() -> u32 {
    0
}
fn default_lattice_init_density() -> f32 {
    0.12
}
fn default_lattice_seed_size() -> u32 {
    6
}

impl From<&LatticeDef> for LatticeParams {
    fn from(def: &LatticeDef) -> Self {
        // Resolve the rule: explicit notation wins; else a named preset; else default.
        let (birth_mask, survival_mask, num_states, neighborhood, rule_preset, manual_masks) =
            if !def.rule.trim().is_empty() {
                let (b, s, st, nb) = parse_rule(&def.rule);
                (b, s, st, nb, DEFAULT_PRESET, true)
            } else if let Some(idx) = PRESET_RULES
                .iter()
                .position(|(name, _)| name.eq_ignore_ascii_case(def.preset.trim()))
            {
                let (b, s, st, nb) = parse_rule(PRESET_RULES[idx].1);
                (b, s, st, nb, idx as u32, false)
            } else {
                let (b, s, st, nb) = parse_rule(PRESET_RULES[DEFAULT_PRESET as usize].1);
                (b, s, st, nb, DEFAULT_PRESET, false)
            };
        // Start from the Lattice defaults (spherical envelope + jitter on), then
        // overlay the preset's look block.
        let base = LatticeParams::default();
        let look = &def.look;
        let render = VolumetricParams {
            palette_hue: look.palette_hue,
            absorption: look.absorption,
            emission_gain: look.emission_gain,
            detail_strength: look.detail_strength,
            detail_scale: look.detail_scale,
            cam_distance: look.cam_distance,
            cam_pitch: look.cam_pitch,
            cam_orbit_speed: look.cam_orbit_speed,
            march_steps: look.march_steps.max(1),
            age_influence: look.age_influence,
            ..base.render
        };
        LatticeParams {
            render,
            grid_res: clamp_grid_res(def.grid_res),
            rule_preset,
            manual_masks,
            birth_mask,
            survival_mask,
            num_states,
            neighborhood,
            boundary: def.boundary.min(1),
            gen_per_sec: def.gen_per_sec.clamp(0.0, 30.0),
            bass_floor: def.bass_floor.clamp(0.0, 1.0),
            smooth_rate: def.smooth_rate.clamp(0.5, 40.0),
            max_age: def.max_age.min(254),
            domain_mode: def.domain_mode.min(1),
            domain_radius: def.domain_radius.clamp(0.1, 1.0),
            dilation_max: def.dilation_max.min(2),
            init_mode: def.init_mode.min(4),
            // High-survival rules (Clouds S13-26) need a dense random field to
            // sustain, so the ceiling is well above the old 0.3.
            init_density: def.init_density.clamp(0.01, 0.6),
            seed_size: def.seed_size.clamp(1, 32),
            perturb_scale: def.perturb_scale.clamp(0.0, 0.2),
            ..base
        }
    }
}

// --- The simulation -----------------------------------------------------------

/// Owns the CA state (ping-pong storage buffers), the density volume it writes,
/// and the seed / step / raymarch pipelines. Self-contained density producer:
/// build it at any allowed resolution and it reuses the R3 marcher.
pub struct LatticeSim {
    grid_res: u32,

    // Ping-pong CA state: array<u32> cell values (0 dead, 1 alive, 2..N-1 dying).
    // Kept alive here; the bind groups hold their own refs (never re-read directly).
    #[allow(dead_code)]
    state_buffers: [wgpu::Buffer; 2],
    current: Cell<usize>,

    // Density volume written by the CA, sampled by the ray marcher. Kept alive for
    // the bind groups (the view + texture are owned here, never re-read directly).
    #[allow(dead_code)]
    density_view: TextureView,
    #[allow(dead_code)]
    density_texture: wgpu::Texture,

    // Normalised cell age (0..1), written by the display pass, sampled by the
    // marcher for the age→hue tint. Same format/resolution as the density volume.
    #[allow(dead_code)]
    age_view: TextureView,
    #[allow(dead_code)]
    age_texture: wgpu::Texture,

    ca_uniform_buffer: wgpu::Buffer, // LatticeUniforms (seed + step + display)
    render_uniform_buffer: wgpu::Buffer, // VolumetricUniforms (marcher)

    seed_pipeline: ComputePipeline,
    seed_bind_groups: [BindGroup; 2], // writes state_buffers[i]

    step_pipeline: ComputePipeline,
    step_bind_groups: [BindGroup; 2], // [in=A,out=B] / [in=B,out=A]

    // Once-per-frame EMA of the freshest state buffer into the density texture.
    display_pipeline: ComputePipeline,
    display_bind_groups: [BindGroup; 2], // reads state_buffers[i], writes density

    // Live-cell population readback (auto-reseed on stagnation). The display pass
    // sums alive cells into `population_buffer`; it's copied to a mappable staging
    // buffer and read back on the CPU one frame later (mirrors the particle counter).
    population_buffer: wgpu::Buffer,
    population_readback: wgpu::Buffer,
    pop_map_pending: Arc<AtomicBool>,
    pop_map_ready: Arc<AtomicBool>,

    raymarch_pipeline: RenderPipeline,
    raymarch_bind_group: BindGroup,
}

impl LatticeSim {
    pub fn new(device: &Device, hdr_format: TextureFormat, grid_res: u32) -> Self {
        let grid_res = clamp_grid_res(grid_res);
        let cell_count = (grid_res as u64) * (grid_res as u64) * (grid_res as u64);

        // Density 3D texture (r32float, storage-written + sampled).
        let density_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lattice-density"),
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
        let density_view = density_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Age volume (normalised generations-since-birth), written alongside density.
        let age_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lattice-age"),
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
        let age_view = age_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Ping-pong CA state buffers.
        let state_buffers = std::array::from_fn(|i| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(if i == 0 {
                    "lattice-state-a"
                } else {
                    "lattice-state-b"
                }),
                size: cell_count * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });

        let ca_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lattice-ca-uniforms"),
            size: std::mem::size_of::<LatticeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let render_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lattice-render-uniforms"),
            size: std::mem::size_of::<VolumetricUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Live-cell population (single atomic u32) + its mappable readback buffer.
        let population_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lattice-population"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let population_readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lattice-population-readback"),
            size: 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // --- Seed pipeline: uniform + state_out (rw) ---
        let seed_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("lattice-seed-bgl"),
            entries: &[
                uniform_entry(0, ShaderStages::COMPUTE),
                storage_rw_entry(1), // state_out
            ],
        });
        let seed_pipeline = create_compute_pipeline(
            device,
            "lattice-seed",
            include_str!("../../../../assets/shaders/builtin/lattice_seed.wgsl"),
            "cs_seed",
            &seed_bgl,
        );
        let seed_bind_groups = std::array::from_fn(|i| {
            device.create_bind_group(&BindGroupDescriptor {
                label: Some("lattice-seed-bg"),
                layout: &seed_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: ca_uniform_buffer.as_entire_binding(),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: state_buffers[i].as_entire_binding(),
                    },
                ],
            })
        });

        // --- Step pipeline: uniform + state_in (ro) + state_out (rw) ---
        let step_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("lattice-step-bgl"),
            entries: &[
                uniform_entry(0, ShaderStages::COMPUTE),
                storage_ro_entry(1), // state_in
                storage_rw_entry(2), // state_out
            ],
        });
        let step_pipeline = create_compute_pipeline(
            device,
            "lattice-step",
            include_str!("../../../../assets/shaders/builtin/lattice_step.wgsl"),
            "cs_step",
            &step_bgl,
        );
        let step_bind_groups = std::array::from_fn(|i| {
            let (in_idx, out_idx) = (i, 1 - i);
            device.create_bind_group(&BindGroupDescriptor {
                label: Some("lattice-step-bg"),
                layout: &step_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: ca_uniform_buffer.as_entire_binding(),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: state_buffers[in_idx].as_entire_binding(),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: state_buffers[out_idx].as_entire_binding(),
                    },
                ],
            })
        });

        // --- Display pipeline: uniform + state_in (ro) + density + age (read_write EMA) ---
        let display_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("lattice-display-bgl"),
            entries: &[
                uniform_entry(0, ShaderStages::COMPUTE),
                storage_ro_entry(1),            // state_in (freshest buffer)
                storage_texture_3d_rw_entry(2), // density (EMA in place)
                storage_texture_3d_rw_entry(3), // age (EMA in place)
                storage_rw_entry(4),            // population (atomic sum)
            ],
        });
        let display_pipeline = create_compute_pipeline(
            device,
            "lattice-display",
            include_str!("../../../../assets/shaders/builtin/lattice_display.wgsl"),
            "cs_display",
            &display_bgl,
        );
        let display_bind_groups = std::array::from_fn(|i| {
            device.create_bind_group(&BindGroupDescriptor {
                label: Some("lattice-display-bg"),
                layout: &display_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: ca_uniform_buffer.as_entire_binding(),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: state_buffers[i].as_entire_binding(),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&density_view),
                    },
                    BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&age_view),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: population_buffer.as_entire_binding(),
                    },
                ],
            })
        });

        // --- Ray march (shared R3 pipeline over the Lattice density + age aux) ---
        let (raymarch_pipeline, raymarch_bind_group) = create_raymarch(
            device,
            hdr_format,
            &render_uniform_buffer,
            &density_view,
            &age_view,
        );

        log::info!("Lattice sim initialized ({grid_res}^3)");

        Self {
            grid_res,
            state_buffers,
            current: Cell::new(0),
            density_view,
            density_texture,
            age_view,
            age_texture,
            ca_uniform_buffer,
            render_uniform_buffer,
            seed_pipeline,
            seed_bind_groups,
            step_pipeline,
            step_bind_groups,
            display_pipeline,
            display_bind_groups,
            population_buffer,
            population_readback,
            pop_map_pending: Arc::new(AtomicBool::new(false)),
            pop_map_ready: Arc::new(AtomicBool::new(false)),
            raymarch_pipeline,
            raymarch_bind_group,
        }
    }

    pub fn grid_res(&self) -> u32 {
        self.grid_res
    }

    /// Upload the CA uniform block (grid_res re-stamped to this sim's size).
    pub fn upload_ca_uniforms(&self, queue: &Queue, uniforms: &LatticeUniforms) {
        let mut u = *uniforms;
        u.grid_res = self.grid_res;
        queue.write_buffer(&self.ca_uniform_buffer, 0, bytemuck::bytes_of(&u));
    }

    /// Upload the ray-march camera/palette uniforms (grid_res re-stamped so the
    /// marcher samples the density texture at this sim's resolution).
    pub fn upload_render_uniforms(&self, queue: &Queue, uniforms: &VolumetricUniforms) {
        let mut u = *uniforms;
        u.grid_res = self.grid_res;
        queue.write_buffer(&self.render_uniform_buffer, 0, bytemuck::bytes_of(&u));
    }

    /// Seed / re-initialise the CURRENT state buffer + density (init or reseed).
    pub fn seed(&self, encoder: &mut CommandEncoder) {
        let g = self.grid_res.div_ceil(LATTICE_WORKGROUP);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("lattice-seed"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.seed_pipeline);
        pass.set_bind_group(0, &self.seed_bind_groups[self.current.get()], &[]);
        pass.dispatch_workgroups(g, g, g);
    }

    /// One CA generation: read current state, write the other + density, flip.
    pub fn step(&self, encoder: &mut CommandEncoder) {
        let g = self.grid_res.div_ceil(LATTICE_WORKGROUP);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("lattice-step"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.step_pipeline);
            pass.set_bind_group(0, &self.step_bind_groups[self.current.get()], &[]);
            pass.dispatch_workgroups(g, g, g);
        }
        self.current.set(1 - self.current.get());
    }

    /// Once-per-frame EMA of the freshest CA state buffer into the density texture.
    /// Runs after the step loop (also on step-free frames), so what the marcher
    /// samples always fades toward the current generation instead of snapping. Also
    /// sums the live-cell population and copies it to staging for CPU readback.
    pub fn display(&self, encoder: &mut CommandEncoder) {
        // Reset the population accumulator before the pass writes into it.
        encoder.clear_buffer(&self.population_buffer, 0, None);
        {
            let g = self.grid_res.div_ceil(LATTICE_WORKGROUP);
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("lattice-display"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.display_pipeline);
            pass.set_bind_group(0, &self.display_bind_groups[self.current.get()], &[]);
            pass.dispatch_workgroups(g, g, g);
        }
        // Stage the population for readback (skip while a previous map is pending —
        // copying into a mapped buffer would be a submit error).
        if !self.pop_map_pending.load(Ordering::Relaxed) {
            encoder.copy_buffer_to_buffer(
                &self.population_buffer,
                0,
                &self.population_readback,
                0,
                4,
            );
        }
    }

    /// Request an async map of the population readback buffer. Call once per frame
    /// after `queue.submit()` (mirrors the particle counter readback).
    pub fn request_population_readback(&self) {
        if self.pop_map_pending.load(Ordering::Relaxed) {
            return;
        }
        self.pop_map_pending.store(true, Ordering::Release);
        let pending = self.pop_map_pending.clone();
        let ready = self.pop_map_ready.clone();
        self.population_readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                if result.is_ok() {
                    ready.store(true, Ordering::Release);
                } else {
                    pending.store(false, Ordering::Release);
                }
            });
    }

    /// Poll the population readback. Returns the live-cell count if the map
    /// completed this frame, else `None`. Call once per frame before dispatch.
    pub fn poll_population_readback(&self) -> Option<u32> {
        if !self.pop_map_ready.load(Ordering::Acquire) {
            return None;
        }
        let count = {
            let view = self.population_readback.slice(..).get_mapped_range();
            let data: &[u32] = bytemuck::cast_slice(&view);
            data[0]
        };
        self.population_readback.unmap();
        self.pop_map_ready.store(false, Ordering::Release);
        self.pop_map_pending.store(false, Ordering::Release);
        Some(count)
    }

    /// Total cell count (grid_res^3) — the denominator for the population fraction.
    pub fn cell_count(&self) -> u32 {
        self.grid_res * self.grid_res * self.grid_res
    }

    /// Ray march the Lattice density, compositing over `target` (LoadOp::Load).
    /// The render uniforms are uploaded by [`upload_render_uniforms`] the same frame.
    pub fn render_raymarch(&self, encoder: &mut CommandEncoder, target: &TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("lattice-raymarch"),
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

    #[test]
    fn lattice_uniforms_size() {
        // 20 x 4-byte scalars, 16-byte-aligned for the uniform address space.
        assert_eq!(std::mem::size_of::<LatticeUniforms>(), 80);
    }

    #[test]
    fn parse_rule_ranges_and_lists() {
        // Pyroclastic S4-7/B6-8/10/M
        let (birth, survival, states, nbhd) = parse_rule("S4-7/B6-8/10/M");
        assert_eq!(survival, 0b1111 << 4); // bits 4,5,6,7
        assert_eq!(birth, 0b111 << 6); // bits 6,7,8
        assert_eq!(states, 10);
        assert_eq!(nbhd, 0);

        // Builder S2,6,9/B4,6,8-9/10/M — comma list
        let (birth, survival, states, nbhd) = parse_rule("S2,6,9/B4,6,8-9/10/M");
        assert_eq!(survival, (1 << 2) | (1 << 6) | (1 << 9));
        assert_eq!(birth, (1 << 4) | (1 << 6) | (1 << 8) | (1 << 9));
        assert_eq!(states, 10);
        assert_eq!(nbhd, 0);

        // Pulse — Von Neumann
        let (_, _, _, nbhd) = parse_rule("S2-3/B3/2/VN");
        assert_eq!(nbhd, 1);
    }

    #[test]
    fn all_presets_parse() {
        for (name, rule) in PRESET_RULES {
            let (birth, survival, states, _) = parse_rule(rule);
            assert!(birth != 0, "{name} has empty birth mask");
            assert!(survival != 0, "{name} has empty survival mask");
            assert!((2..=20).contains(&states), "{name} states out of range");
        }
    }

    /// The 8 shipped `.pfx` presets must deserialize into a `LatticeDef` (new
    /// cadence / domain / look fields included) and map to a usable `LatticeParams`.
    /// Embedded with `include_str!` so the test is CWD-independent.
    #[test]
    fn shipped_presets_deserialize() {
        const PRESETS: [&str; 8] = [
            include_str!("../../../../assets/effects/lattice_clouds.pfx"),
            include_str!("../../../../assets/effects/lattice_shells.pfx"),
            include_str!("../../../../assets/effects/lattice_pyroclastic.pfx"),
            include_str!("../../../../assets/effects/lattice_chunky.pfx"),
            include_str!("../../../../assets/effects/lattice_brain.pfx"),
            include_str!("../../../../assets/effects/lattice_builder.pfx"),
            include_str!("../../../../assets/effects/lattice_445.pfx"),
            include_str!("../../../../assets/effects/lattice_pulse.pfx"),
        ];
        for src in PRESETS {
            let v: serde_json::Value = serde_json::from_str(src).expect("preset is valid JSON");
            let lat = &v["particles"]["lattice"];
            assert!(!lat.is_null(), "preset missing particles.lattice");
            let def: LatticeDef =
                serde_json::from_value(lat.clone()).expect("lattice block maps to LatticeDef");
            let p = LatticeParams::from(&def);
            assert!(p.gen_per_sec > 0.0);
            assert!((0.0..=1.0).contains(&p.bass_floor));
            assert!(p.smooth_rate >= 0.5);
            assert!(p.dilation_max <= 2);
            assert!((0.1..=1.0).contains(&p.domain_radius));
            // Lattice always drives the marcher with the spherical envelope + jitter.
            assert_eq!(p.render.env_shape, 1);
            assert!(p.render.jitter_amp > 0.0);
        }
    }

    #[test]
    fn grid_res_snaps_to_choices() {
        assert_eq!(clamp_grid_res(100), 128);
        assert_eq!(clamp_grid_res(40), 32);
        assert_eq!(clamp_grid_res(200), 256);
        assert_eq!(clamp_grid_res(1), 32);
    }

    #[test]
    fn step_budget_rate_and_cap() {
        // Full bass at 10 gen/s, 0.1 s frame → ~1 step per frame, no backlog.
        let (steps, acc) = lattice_step_budget(0.0, 10.0, 0.35, 1.0, 0.1);
        assert_eq!(steps, 1);
        assert!(acc.abs() < 1e-6);

        // Silence still breathes at the floor rate: 10 gen/s * 0.35 = 3.5/s.
        // Over 1 s of accumulation it should produce ~3 steps (capped at 8).
        let mut a = 0.0;
        let mut total = 0u32;
        for _ in 0..60 {
            let (s, na) = lattice_step_budget(a, 10.0, 0.35, 0.0, 1.0 / 60.0);
            total += s;
            a = na;
        }
        assert!((3..=4).contains(&total), "silence-floor steps: {total}");

        // A long hitch (huge dt) is capped and never bursts: residual ≤ 1.
        let (steps, acc) = lattice_step_budget(0.0, 30.0, 0.35, 1.0, 5.0);
        assert_eq!(steps, LATTICE_MAX_STEPS_PER_FRAME);
        assert!(acc <= 1.0 + 1e-6);

        // Zero rate → never steps.
        let (steps, _) = lattice_step_budget(0.0, 0.0, 0.35, 1.0, 0.1);
        assert_eq!(steps, 0);
    }

    #[test]
    fn stagnation_reseed_policy() {
        let dt = 1.0 / 60.0;
        // Healthy population never reseeds and keeps the timer at 0.
        let (reseed, secs) = lattice_stagnation_tick(0.1, 3.9, dt);
        assert!(!reseed);
        assert_eq!(secs, 0.0);

        // Died-out accumulates and reseeds within the dwell.
        let mut secs = 0.0;
        let mut fired = 0;
        for _ in 0..(60 * 3) {
            let (reseed, s) = lattice_stagnation_tick(0.0, secs, dt);
            secs = s;
            if reseed {
                fired += 1;
            }
        }
        assert!(fired >= 1, "died-out should auto-reseed within 3 s");

        // Saturation is NOT a trigger anymore — a packed grid stays put (the
        // per-cell lifetime handles turnover, not a global reseed).
        let (reseed, s1) = lattice_stagnation_tick(0.8, 0.0, dt);
        assert!(!reseed);
        assert_eq!(s1, 0.0);

        // A live frame resets the timer.
        let (_, s3) = lattice_stagnation_tick(0.2, 2.0, dt);
        assert_eq!(s3, 0.0);
    }

    #[test]
    fn dilate_stays_in_27_bits() {
        assert_eq!(dilate_mask(0x07FF_FFFF, 3) & !0x07FF_FFFF, 0);
        // Dilating a single mid bit adds its neighbours.
        assert_eq!(dilate_mask(1 << 5, 1), (1 << 4) | (1 << 5) | (1 << 6));
    }

    #[test]
    fn lattice_def_named_preset_and_clamps() {
        let def = LatticeDef {
            grid_res: 200, // snaps to 256
            preset: "pyroclastic".to_string(),
            rule: String::new(),
            gen_per_sec: 40.0,  // clamps to 30
            bass_floor: 2.0,    // clamps to 1
            smooth_rate: 100.0, // clamps to 40
            max_age: 999,       // clamps to 254
            boundary: 5,        // clamps to 1
            domain_mode: 9,     // clamps to 1
            domain_radius: 2.0, // clamps to 1
            dilation_max: 9,    // clamps to 2
            init_mode: 9,       // clamps to 4
            init_density: 0.8,  // clamps to 0.6
            seed_size: 99,      // clamps to 32
            perturb_scale: 1.0, // clamps to 0.2
            look: LatticeLookDef::default(),
        };
        let p = LatticeParams::from(&def);
        assert_eq!(p.grid_res, 256);
        assert!(!p.manual_masks);
        assert_eq!(p.rule_preset, DEFAULT_PRESET); // Pyroclastic is the default preset
        let (b, s, st, _) = parse_rule("S4-7/B6-8/10/M");
        assert_eq!((p.birth_mask, p.survival_mask, p.num_states), (b, s, st));
        assert_eq!(p.gen_per_sec, 30.0);
        assert_eq!(p.bass_floor, 1.0);
        assert_eq!(p.smooth_rate, 40.0);
        assert_eq!(p.boundary, 1);
        assert_eq!(p.domain_mode, 1);
        assert!((p.domain_radius - 1.0).abs() < 1e-6);
        assert_eq!(p.dilation_max, 2);
        assert_eq!(p.init_mode, 4);
        assert!((p.init_density - 0.6).abs() < 1e-6);
        assert_eq!(p.seed_size, 32);
        assert!((p.perturb_scale - 0.2).abs() < 1e-6);
        assert_eq!(p.max_age, 254);
    }

    /// Headless GPU smoke: build a `LatticeSim` (all four pipelines, incl. the
    /// r32float read-write display BGL + the 80/112-byte uniforms against every
    /// WGSL mirror) and run seed → step → display on a real device. Ignored by
    /// default (needs an adapter); run explicitly with
    /// `cargo test -- --ignored lattice_gpu_smoke`. Validates the one capability
    /// that can't be checked statically: r32float read-write storage textures.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn lattice_gpu_smoke() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("no wgpu adapter");
        eprintln!("smoke adapter: {:?}", adapter.get_info());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("lattice-smoke"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("no wgpu device");

        device.push_error_scope(wgpu::ErrorFilter::Validation);

        // Also build the R3 renderer so scatter/resolve/raymarch parse against the
        // shared 112-byte VolumetricUniforms (this path isn't otherwise exercised
        // here, and it must stay valid after the uniform grew).
        let dummy = |sz: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: sz,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let pos_life = [dummy(64), dummy(64)];
        let alive_idx = [dummy(64), dummy(64)];
        let counter = dummy(64);
        let _r3 = crate::gpu::volumetric::VolumetricRenderer::new(
            &device,
            TextureFormat::Rgba16Float,
            &pos_life,
            &alive_idx,
            &counter,
        );

        let grid = 32u32;
        let sim = LatticeSim::new(&device, TextureFormat::Rgba16Float, grid);

        // Seed a known random fill (init_mode 0 @ 0.25) and display with NO steps,
        // so the live-cell population is deterministic (~0.25 of the grid). This
        // checks the workgroup-reduction count is numerically right, not just wired.
        let params = LatticeParams {
            init_mode: 0,
            init_density: 0.25,
            domain_mode: 0, // full cube so every seeded cell counts
            ..LatticeParams::default()
        };
        sim.upload_ca_uniforms(
            &queue,
            &params.build_uniforms(0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0 / 60.0),
        );
        sim.upload_render_uniforms(
            &queue,
            &VolumetricParams::default().build_uniforms(
                [256.0, 256.0],
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
            ),
        );
        let mut enc =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        sim.seed(&mut enc);
        sim.display(&mut enc);
        queue.submit([enc.finish()]);
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        sim.request_population_readback();
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        let pop = sim.poll_population_readback().expect("population readback");
        let cells = (grid * grid * grid) as f32;
        let frac = pop as f32 / cells;
        assert!(
            (0.20..0.30).contains(&frac),
            "population fraction {frac} off expected ~0.25 (pop={pop})"
        );

        // Also exercise the step path (writes packed age state) once.
        let mut enc =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        sim.step(&mut enc);
        sim.display(&mut enc);
        queue.submit([enc.finish()]);
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();

        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "wgpu validation error: {err:?}");
    }

    /// End-to-end auto-reseed proof: drive a deliberately-dying config (445 from a
    /// solid seed) in silence and confirm the population dies out, then an auto-
    /// reseed revives it. Replicates `ParticleSystem`'s per-frame loop headlessly.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn lattice_reseed_revives() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("no wgpu adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("lattice-reseed"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("no wgpu device");

        let grid = 32u32;
        // 445 collapses from a solid seed; cube domain + always-on stepping so it
        // dies fast in "silence", exercising the reseed path repeatedly.
        let mut params = LatticeParams {
            grid_res: grid,
            init_mode: 1, // solid center sphere
            init_density: 0.5,
            seed_size: 7,
            gen_per_sec: 25.0,
            bass_floor: 1.0, // runs at full rate even with bass = 0
            domain_mode: 0,
            smooth_rate: 20.0,
            ..LatticeParams::default()
        };
        let (b, s, st, nb) = parse_rule("S4/B4/5/M");
        params.birth_mask = b;
        params.survival_mask = s;
        params.num_states = st;
        params.neighborhood = nb;

        let sim = LatticeSim::new(&device, TextureFormat::Rgba16Float, grid);
        let cells = sim.cell_count() as f32;
        let dt = 1.0 / 60.0;
        let mut accum = 0.0f32;
        let mut stagnant = 0.0f32;
        let mut needs_seed = true;
        let mut reseeds = 0u32;
        let mut saw_dead = false;
        let mut revived_after_death = false;
        let mut last_frac = 1.0f32;

        for frame in 0..900u32 {
            // silence: bass = 0, no onset/flux.
            sim.upload_ca_uniforms(
                &queue,
                &params.build_uniforms(frame, 0.0, 0.0, 0.0, 0.0, frame as f32 * dt, dt),
            );
            let mut enc = device.create_command_encoder(&Default::default());
            if needs_seed {
                sim.seed(&mut enc);
                needs_seed = false;
                accum = 0.0;
            }
            let (steps, residual) =
                lattice_step_budget(accum, params.gen_per_sec, params.bass_floor, 0.0, dt);
            accum = residual;
            for _ in 0..steps {
                sim.step(&mut enc);
            }
            sim.display(&mut enc);
            queue.submit([enc.finish()]);
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            sim.request_population_readback();
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            let pop = sim.poll_population_readback().expect("population readback");
            let frac = pop as f32 / cells;

            if frac < LATTICE_DEATH_FRACTION {
                saw_dead = true;
            }
            // A jump from near-dead back to clearly-alive is the reseed reviving it.
            if saw_dead && last_frac < LATTICE_DEATH_FRACTION && frac > 0.01 {
                revived_after_death = true;
            }
            last_frac = frac;

            let (reseed, next) = lattice_stagnation_tick(frac, stagnant, dt);
            stagnant = next;
            if reseed {
                params.seed_hash = params
                    .seed_hash
                    .wrapping_mul(1_664_525)
                    .wrapping_add(1_013_904_223);
                needs_seed = true;
                reseeds += 1;
            }
        }

        assert!(saw_dead, "445 from a solid seed should die out");
        assert!(reseeds >= 1, "auto-reseed should have fired at least once");
        assert!(
            revived_after_death,
            "population should revive after an auto-reseed"
        );
    }

    /// Headless offscreen render of several presets to PNG (for eyeballing the
    /// silhouette / structure / per-preset hue). Ignored by default; run with
    /// `LATTICE_PNG_DIR=/path cargo test -- --ignored lattice_render_previews`.
    #[test]
    #[ignore = "requires a GPU/software adapter; writes PNGs"]
    fn lattice_render_previews() {
        let out_dir = std::env::var("LATTICE_PNG_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("no wgpu adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("lattice-preview"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("no wgpu device");

        let all: [(&str, &str, u32); 8] = [
            (
                "shells",
                include_str!("../../../../assets/effects/lattice_shells.pfx"),
                26,
            ),
            (
                "pyroclastic",
                include_str!("../../../../assets/effects/lattice_pyroclastic.pfx"),
                14,
            ),
            (
                "clouds",
                include_str!("../../../../assets/effects/lattice_clouds.pfx"),
                22,
            ),
            (
                "chunky",
                include_str!("../../../../assets/effects/lattice_chunky.pfx"),
                22,
            ),
            (
                "brain",
                include_str!("../../../../assets/effects/lattice_brain.pfx"),
                26,
            ),
            (
                "builder",
                include_str!("../../../../assets/effects/lattice_builder.pfx"),
                20,
            ),
            (
                "445",
                include_str!("../../../../assets/effects/lattice_445.pfx"),
                18,
            ),
            (
                "pulse",
                include_str!("../../../../assets/effects/lattice_pulse.pfx"),
                16,
            ),
        ];
        // LATTICE_SWEEP=<preset> renders that preset across a range of generation
        // counts (files named <preset>_gNN.png) to expose its evolution; otherwise
        // render every preset once at its tuned generation count.
        let sweep = std::env::var("LATTICE_SWEEP").ok();
        let presets: Vec<(String, &str, u32)> = if let Some(name) = &sweep {
            let src = all
                .iter()
                .find(|p| p.0 == name)
                .expect("unknown sweep preset")
                .1;
            // LATTICE_SWEEP_GENS="40,60,80" overrides the default generation ladder.
            let gens: Vec<u32> = std::env::var("LATTICE_SWEEP_GENS")
                .ok()
                .map(|s| s.split(',').filter_map(|t| t.trim().parse().ok()).collect())
                .filter(|v: &Vec<u32>| !v.is_empty())
                .unwrap_or_else(|| vec![3, 6, 9, 12, 16, 22, 30, 40]);
            gens.iter()
                .map(|&g| (format!("{name}_g{g:03}"), src, g))
                .collect()
        } else {
            all.iter().map(|p| (p.0.to_string(), p.1, p.2)).collect()
        };
        let (w, h) = (512u32, 512u32);
        let fmt = TextureFormat::Rgba8UnormSrgb;

        for (name, src, gens) in presets {
            let v: serde_json::Value = serde_json::from_str(src).unwrap();
            let def: LatticeDef =
                serde_json::from_value(v["particles"]["lattice"].clone()).unwrap();
            let mut params = LatticeParams::from(&def);
            // Faithful to the shipped grid by default; override for a quick llvmpipe run.
            if let Ok(g) = std::env::var("LATTICE_PREVIEW_GRID") {
                params.grid_res = clamp_grid_res(g.parse().unwrap_or(params.grid_res));
            }
            if let Ok(d) = std::env::var("LATTICE_DOMAIN") {
                params.domain_mode = d.parse().unwrap_or(params.domain_mode).min(1);
            }
            if let Ok(m) = std::env::var("LATTICE_INIT_MODE") {
                params.init_mode = m.parse().unwrap_or(params.init_mode).min(4);
            }
            if let Ok(d) = std::env::var("LATTICE_INIT_DENSITY") {
                params.init_density = d.parse().unwrap_or(params.init_density).clamp(0.01, 0.6);
            }
            if let Ok(s) = std::env::var("LATTICE_SEED_SIZE") {
                params.seed_size = s.parse().unwrap_or(params.seed_size).clamp(1, 64);
            }
            if let Ok(rule) = std::env::var("LATTICE_RULE") {
                let (b, s, st, nb) = parse_rule(&rule);
                params.birth_mask = b;
                params.survival_mask = s;
                params.num_states = st;
                params.neighborhood = nb;
            }
            if let Ok(a) = std::env::var("LATTICE_MAX_AGE") {
                params.max_age = a.parse().unwrap_or(params.max_age).min(254);
            }
            if let Ok(s) = std::env::var("LATTICE_SMOOTH") {
                params.smooth_rate = s.parse().unwrap_or(params.smooth_rate).clamp(0.5, 40.0);
            }
            if let Ok(a) = std::env::var("LATTICE_ABSORPTION") {
                params.render.absorption = a.parse().unwrap_or(params.render.absorption);
            }
            if let Ok(e) = std::env::var("LATTICE_EMISSION") {
                params.render.emission_gain = e.parse().unwrap_or(params.render.emission_gain);
            }
            if let Ok(h) = std::env::var("LATTICE_HUE") {
                params.render.palette_hue = h.parse().unwrap_or(params.render.palette_hue);
            }

            let sim = LatticeSim::new(&device, fmt, params.grid_res);
            let mut fc =
                crate::gpu::frame_capture::FrameCapture::new(&device, w, h, fmt, "preview");

            // CA uniforms (full bass so steps run); grid_res is re-stamped by the sim.
            let cu = params.build_uniforms(1, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0 / 60.0);
            sim.upload_ca_uniforms(&queue, &cu);

            // Seed, then evolve `gens` generations.
            let mut enc = device.create_command_encoder(&Default::default());
            sim.seed(&mut enc);
            for _ in 0..gens {
                sim.step(&mut enc);
            }
            // Converge the display EMA onto the final state.
            let cu_snap = params.build_uniforms(1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1);
            sim.upload_ca_uniforms(&queue, &cu_snap);
            for _ in 0..40 {
                sim.display(&mut enc);
            }
            queue.submit([enc.finish()]);
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();

            // Render: clear to black, then ray march the density into the capture.
            let mut ru =
                params
                    .render
                    .build_uniforms([w as f32, h as f32], 2.2, 0.0, 0.0, 0.0, 0.0, 0.0);
            ru.grid_res = params.grid_res;
            ru.env_shape = params.domain_mode.min(1); // mirror the app's domain→envelope tie
            sim.upload_render_uniforms(&queue, &ru);
            let mut enc = device.create_command_encoder(&Default::default());
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
            let path = format!("{out_dir}/lattice_{name}.png");
            image::RgbaImage::from_raw(w, h, data)
                .expect("raw->image")
                .save(&path)
                .expect("save png");
            eprintln!("wrote {path}");
        }
    }

    /// Render each preset as an evolving frame sequence (CA generations advancing +
    /// a camera orbit) so the motion — what actually distinguishes the rules — can
    /// be assembled into GIFs. Files: `lattice_anim_<preset>_<fff>.png`. Ignored;
    /// run with `LATTICE_PNG_DIR=/path cargo test -- --ignored lattice_render_animation`.
    #[test]
    #[ignore = "requires a GPU/software adapter; writes many PNGs"]
    fn lattice_render_animation() {
        let out_dir = std::env::var("LATTICE_PNG_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let frames: u32 = std::env::var("LATTICE_ANIM_FRAMES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(60);
        let grid: u32 = std::env::var("LATTICE_PREVIEW_GRID")
            .ok()
            .and_then(|s| s.parse().ok())
            .map(clamp_grid_res)
            .unwrap_or(96);

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("no wgpu adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("lattice-anim"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("no wgpu device");

        let presets: [(&str, &str); 8] = [
            (
                "shells",
                include_str!("../../../../assets/effects/lattice_shells.pfx"),
            ),
            (
                "pyroclastic",
                include_str!("../../../../assets/effects/lattice_pyroclastic.pfx"),
            ),
            (
                "clouds",
                include_str!("../../../../assets/effects/lattice_clouds.pfx"),
            ),
            (
                "chunky",
                include_str!("../../../../assets/effects/lattice_chunky.pfx"),
            ),
            (
                "brain",
                include_str!("../../../../assets/effects/lattice_brain.pfx"),
            ),
            (
                "builder",
                include_str!("../../../../assets/effects/lattice_builder.pfx"),
            ),
            (
                "445",
                include_str!("../../../../assets/effects/lattice_445.pfx"),
            ),
            (
                "pulse",
                include_str!("../../../../assets/effects/lattice_pulse.pfx"),
            ),
        ];
        let (w, h) = (384u32, 384u32);
        let fmt = TextureFormat::Rgba8UnormSrgb;
        let fps = 20.0f32;
        let dt = 1.0 / fps;
        let wait = || wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        };

        // LATTICE_ANIM_ONLY=<preset> renders just one; LATTICE_SIM_AUDIO=1 feeds a
        // loud beat (onset pulses + mid/high energy) to reproduce live saturation.
        let only = std::env::var("LATTICE_ANIM_ONLY").ok();
        let sim_audio = std::env::var("LATTICE_SIM_AUDIO").is_ok();
        let no_reseed = std::env::var("LATTICE_NO_RESEED").is_ok();
        for (name, src) in presets {
            if only.as_deref().is_some_and(|o| o != name) {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(src).unwrap();
            let def: LatticeDef =
                serde_json::from_value(v["particles"]["lattice"].clone()).unwrap();
            let mut params = LatticeParams::from(&def);
            params.grid_res = grid;
            params.render.cam_orbit_speed = 0.7; // clear rotation across the clip
            if let Ok(m) = std::env::var("LATTICE_INIT_MODE") {
                params.init_mode = m.parse().unwrap_or(params.init_mode).min(4);
            }
            if let Ok(d) = std::env::var("LATTICE_INIT_DENSITY") {
                params.init_density = d.parse().unwrap_or(params.init_density).clamp(0.01, 0.6);
            }
            if let Ok(rule) = std::env::var("LATTICE_RULE") {
                let (b, s, st, nb) = parse_rule(&rule);
                params.birth_mask = b;
                params.survival_mask = s;
                params.num_states = st;
                params.neighborhood = nb;
            }

            let sim = LatticeSim::new(&device, fmt, params.grid_res);
            let mut fc = crate::gpu::frame_capture::FrameCapture::new(&device, w, h, fmt, "anim");
            let mut accum = 0.0f32;
            let mut needs_seed = true; // seeded on the first iteration
            let mut stagnant = 0.0f32;
            let cells = sim.cell_count() as f32;

            for f in 0..frames {
                let t = f as f32 * dt;
                // Simulated loud beat: onset every 12 frames (~1.7/s at 20fps),
                // sustained mid/high energy — the condition that saturates live.
                let (onset, mid, high) = if sim_audio {
                    (if f % 12 < 2 { 1.0 } else { 0.0 }, 0.7, 0.7)
                } else {
                    (0.0, 0.0, 0.0)
                };
                // Advance the CA at full energy so the evolution reads in a short clip.
                sim.upload_ca_uniforms(
                    &queue,
                    &params.build_uniforms(f, onset, 0.0, mid, high, t, dt),
                );
                let (steps, residual) =
                    lattice_step_budget(accum, params.gen_per_sec, params.bass_floor, 1.0, dt);
                accum = residual;
                let mut ru =
                    params
                        .render
                        .build_uniforms([w as f32, h as f32], t, 0.0, 0.0, 0.0, 0.0, 0.0);
                ru.grid_res = params.grid_res;
                ru.env_shape = params.domain_mode.min(1);
                sim.upload_render_uniforms(&queue, &ru);

                let mut enc = device.create_command_encoder(&Default::default());
                if needs_seed {
                    sim.seed(&mut enc);
                    needs_seed = false;
                    accum = 0.0;
                }
                for _ in 0..steps {
                    sim.step(&mut enc);
                }
                sim.display(&mut enc);
                {
                    let _clear = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("anim-clear"),
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
                device.poll(wait()).unwrap();

                fc.request_map();
                let data = loop {
                    device.poll(wait()).unwrap();
                    if let Some(d) = fc.take_mapped_data(&device) {
                        break d;
                    }
                };
                let path = format!("{out_dir}/lattice_anim_{name}_{f:03}.png");
                image::RgbaImage::from_raw(w, h, data)
                    .expect("raw->image")
                    .save(&path)
                    .expect("save png");

                // Mirror the app's auto-reseed so the preview reflects live behavior:
                // a filled/dead grid cycles back to a fresh seed.
                sim.request_population_readback();
                device.poll(wait()).unwrap();
                if let Some(pop) = sim.poll_population_readback().filter(|_| !no_reseed) {
                    let frac = pop as f32 / cells;
                    let (reseed, next) = lattice_stagnation_tick(frac, stagnant, dt);
                    stagnant = next;
                    if reseed {
                        params.seed_hash = params
                            .seed_hash
                            .wrapping_mul(1_664_525)
                            .wrapping_add(1_013_904_223);
                        needs_seed = true;
                    }
                }
            }
            eprintln!("wrote {frames} frames for {name}");
        }
    }

    #[test]
    fn lattice_def_raw_rule_overrides_preset() {
        // An explicit `rule` wins over `preset` and sets manual-mask mode.
        let def = LatticeDef {
            grid_res: 64,
            preset: "Clouds".to_string(),
            rule: "S5/B6/3/N".to_string(),
            gen_per_sec: 6.0,
            bass_floor: 0.3,
            smooth_rate: 8.0,
            max_age: 0,
            boundary: 0,
            domain_mode: 1,
            domain_radius: 0.9,
            dilation_max: 1,
            init_mode: 0,
            init_density: 0.1,
            seed_size: 5,
            perturb_scale: 0.0,
            look: LatticeLookDef::default(),
        };
        let p = LatticeParams::from(&def);
        assert!(p.manual_masks);
        assert_eq!(p.birth_mask, 1 << 6);
        assert_eq!(p.survival_mask, 1 << 5);
        assert_eq!(p.num_states, 3);
        assert_eq!(p.neighborhood, 1); // Von Neumann
    }

    /// TEMPORARY diagnostic: trace a preset's population fraction under SILENCE,
    /// replicating the app's per-frame loop (step budget at bass=0, display,
    /// population readback, stagnation-tick auto-reseed). Prints frac + generation
    /// count each second and every reseed, so the steady-state fill can be compared
    /// against the saturate threshold. Ignored; run with:
    ///   LATTICE_TRACE_RULE=builder LATTICE_PREVIEW_GRID=128 LATTICE_TRACE_FRAMES=2400 \
    ///     cargo test -p phosphor-app --release -- --ignored --nocapture lattice_silence_trace
    #[test]
    #[ignore = "requires a GPU/software adapter; diagnostic trace"]
    fn lattice_silence_trace() {
        let rule = std::env::var("LATTICE_TRACE_RULE").unwrap_or_else(|_| "builder".to_string());
        let frames: u32 = std::env::var("LATTICE_TRACE_FRAMES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2400);
        let grid: u32 = std::env::var("LATTICE_PREVIEW_GRID")
            .ok()
            .and_then(|s| s.parse().ok())
            .map(clamp_grid_res)
            .unwrap_or(DEFAULT_GRID_RES);
        let no_reseed = std::env::var("LATTICE_NO_RESEED").is_ok();
        let bass: f32 = std::env::var("LATTICE_TRACE_BASS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);

        let src: &str = match rule.as_str() {
            "builder" => include_str!("../../../../assets/effects/lattice_builder.pfx"),
            "clouds" => include_str!("../../../../assets/effects/lattice_clouds.pfx"),
            "brain" => include_str!("../../../../assets/effects/lattice_brain.pfx"),
            "pyroclastic" => include_str!("../../../../assets/effects/lattice_pyroclastic.pfx"),
            "shells" => include_str!("../../../../assets/effects/lattice_shells.pfx"),
            "chunky" => include_str!("../../../../assets/effects/lattice_chunky.pfx"),
            "445" => include_str!("../../../../assets/effects/lattice_445.pfx"),
            "pulse" => include_str!("../../../../assets/effects/lattice_pulse.pfx"),
            _ => include_str!("../../../../assets/effects/lattice_builder.pfx"),
        };
        let v: serde_json::Value = serde_json::from_str(src).unwrap();
        let def: LatticeDef = serde_json::from_value(v["particles"]["lattice"].clone()).unwrap();
        let mut params = LatticeParams::from(&def);
        params.grid_res = grid;
        if let Ok(m) = std::env::var("LATTICE_INIT_MODE") {
            params.init_mode = m.parse().unwrap_or(params.init_mode).min(4);
        }
        if let Ok(d) = std::env::var("LATTICE_INIT_DENSITY") {
            params.init_density = d.parse().unwrap_or(params.init_density).clamp(0.01, 0.6);
        }
        if let Ok(a) = std::env::var("LATTICE_MAX_AGE") {
            params.max_age = a.parse().unwrap_or(params.max_age).min(254);
        }
        if let Ok(rule) = std::env::var("LATTICE_RULE") {
            let (b, s, st, nb) = parse_rule(&rule);
            params.birth_mask = b;
            params.survival_mask = s;
            params.num_states = st;
            params.neighborhood = nb;
        }

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("no wgpu adapter");
        eprintln!("trace adapter: {:?}", adapter.get_info());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("lattice-trace"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("no wgpu device");

        let sim = LatticeSim::new(&device, TextureFormat::Rgba16Float, params.grid_res);
        let cells = sim.cell_count() as f32;
        let dt = 1.0 / 60.0;
        eprintln!(
            "rule={rule} grid={} domain_mode={} radius={} max_age={} bass={bass} reseed={}",
            params.grid_res, params.domain_mode, params.domain_radius, params.max_age, !no_reseed
        );

        let wait = || wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        };
        let mut accum = 0.0f32;
        let mut needs_seed = true;
        let mut stagnant = 0.0f32;
        let mut grace = 0.0f32;
        let mut gens = 0u64;
        let mut reseeds = 0u32;
        let mut max_frac = 0.0f32;

        for f in 0..frames {
            sim.upload_ca_uniforms(
                &queue,
                &params.build_uniforms(f, 0.0, 0.0, 0.0, 0.0, f as f32 * dt, dt),
            );
            let mut enc = device.create_command_encoder(&Default::default());
            if needs_seed {
                sim.seed(&mut enc);
                needs_seed = false;
                accum = 0.0;
                grace = LATTICE_RESEED_GRACE_SECS; // mirror the app's post-seed grace
                stagnant = 0.0;
            }
            let (steps, residual) =
                lattice_step_budget(accum, params.gen_per_sec, params.bass_floor, bass, dt);
            accum = residual;
            gens += steps as u64;
            for _ in 0..steps {
                sim.step(&mut enc);
            }
            sim.display(&mut enc);
            queue.submit([enc.finish()]);
            device.poll(wait()).unwrap();
            sim.request_population_readback();
            device.poll(wait()).unwrap();
            let pop = sim.poll_population_readback().expect("population readback");
            let frac = pop as f32 / cells;
            max_frac = max_frac.max(frac);

            let (reseed, next) = if no_reseed || grace > 0.0 {
                grace = (grace - dt).max(0.0);
                (false, 0.0)
            } else {
                lattice_stagnation_tick(frac, stagnant, dt)
            };
            stagnant = next;
            if reseed {
                params.seed_hash = params
                    .seed_hash
                    .wrapping_mul(1_664_525)
                    .wrapping_add(1_013_904_223);
                needs_seed = true;
                reseeds += 1;
                eprintln!("  f={f:4} gen={gens:4} frac={frac:.4} -> RESEED #{reseeds}");
            } else if f % 60 == 0 {
                eprintln!("  f={f:4} gen={gens:4} frac={frac:.4} stagnant={stagnant:.2}");
            }
        }
        eprintln!("DONE rule={rule}: gens={gens} reseeds={reseeds} max_frac={max_frac:.4}");
    }
}
