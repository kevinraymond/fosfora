// Phosphor particle library — shared structs, bindings, and helpers.
// Auto-prepended to all particle compute shaders (same pattern as noise/palette/sdf libs).

struct ParticleUniforms {
    delta_time: f32,
    time: f32,
    max_particles: u32,
    emit_count: u32,

    emitter_pos: vec2f,
    emitter_radius: f32,
    emitter_shape: u32,

    lifetime: f32,
    initial_speed: f32,
    initial_size: f32,
    size_end: f32,

    gravity: vec2f,
    drag: f32,
    turbulence: f32,

    attraction_point: vec2f,
    attraction_strength: f32,
    seed: f32,

    sub_bass: f32,
    bass: f32,
    mid: f32,
    rms: f32,
    kick: f32,
    onset: f32,
    centroid: f32,
    flux: f32,
    beat: f32,
    beat_phase: f32,
    low_mid: f32,
    upper_mid: f32,
    presence: f32,
    brilliance: f32,

    resolution: vec2f,

    // Flow field params
    flow_strength: f32,
    flow_scale: f32,
    flow_speed: f32,
    flow_enabled: f32,

    // Trail params
    trail_length: u32,
    trail_width: f32,
    prev_emitter_pos: vec2f,

    // Wind + vortex + ground
    wind: vec2f,
    vortex_center: vec2f,
    vortex_strength: f32,
    vortex_radius: f32,
    ground_y: f32,
    ground_bounce: f32,

    // Noise params
    noise_octaves: u32,
    noise_lacunarity: f32,
    noise_persistence: f32,
    noise_mode: u32,

    // Emitter enhancements
    emitter_angle: f32,
    emitter_spread: f32,
    speed_variance: f32,
    life_variance: f32,
    size_variance: f32,
    velocity_inherit: f32,
    noise_speed: f32,
    dominant_chroma: f32,

    // Lifetime curves (8-point LUTs)
    size_curve: array<vec4f, 2>,
    opacity_curve: array<vec4f, 2>,

    // Color gradient (packed RGBA u32)
    color_gradient: array<vec4u, 2>,

    // Spin + curve config
    spin_speed: f32,
    gradient_count: u32,
    curve_flags: u32,
    depth_sort: u32,

    // Effect params forwarded from ParamStore (8 floats = params 0..7)
    effect_params_0: vec4f,
    effect_params_1: vec4f,

    // Obstacle collision (obstacle_fit lives in the trailing zcr block)
    obstacle_enabled: f32,    // 0.0 or 1.0
    obstacle_threshold: f32,  // alpha cutoff
    obstacle_mode: u32,       // 0=bounce, 1=stick, 2=flow, 3=contain
    obstacle_elasticity: f32, // restitution/friction

    // MFCC + Chroma audio features
    mfcc: array<vec4f, 4>,     // 13 MFCCs (indices 0-12 used, 13-15 padding)
    chroma: array<vec4f, 3>,   // 12 pitch class energies (C=0, C#=1, ..., B=11)

    // Force matrix for particle-life (symbiosis): 8x8 = 64 floats
    force_matrix: array<vec4f, 16>,

    // Morph (shape target morphing)
    morph_progress: f32,
    morph_source: u32,
    morph_dest: u32,
    morph_flags: u32,   // bit 0 = transitioning, bits 1-3 = transition_style

    // Zero-crossing rate + spectral shape + tempo
    zcr: f32,
    flatness: f32,      // Noise vs tone (Wiener entropy)
    rolloff: f32,       // 85% energy frequency (normalized)
    bandwidth: f32,     // Spectral spread
    bpm: f32,           // BPM / 300 (normalized 0-1)
    beat_strength: f32, // Strength of the detected beat
    bar_phase: f32,     // A12 0-1 sawtooth over the current bar (#1505; 0.0 until DSP)
    obstacle_fit: u32,  // 0=stretch (legacy), 1=contain ("Fit"), 2=cover ("Fill") (#1790)

    // A14 HPSS split (#1796 ABI bump)
    percussive_energy: f32, // transient (percussive-masked) energy, dB-mapped 0-1
    harmonic_energy: f32,   // sustained (harmonic-masked) energy, dB-mapped 0-1
    harmonic_ratio: f32,    // harmonic vs percussive balance, 0-1
    frame_index: u32,       // trail ring head counter (see trail_write)

    // A18 structure (#1797 ABI bump)
    buildup: f32,       // riser/tension logistic, EMA-smoothed 0-1
    drop: f32,          // drop trigger — 1.0 for exactly one frame
    splat_roundness: f32,   // Splat shard→sphere morph, 0–1 (.pfx slot 12)
    obstacle_water_scale: f32,  // water raises the collision surface (#1851)

    // Splat orbit camera + audio envelopes (#1800 ABI bump).
    // Zero for non-splat effects — sims must treat cam_focal == 0 as "no camera".
    cam_yaw: f32,           // orbit azimuth, radians (CPU-accumulated)
    cam_pitch: f32,         // orbit elevation, radians
    cam_distance: f32,      // orbit radius in scene units (scene normalized to r≈1)
    cam_focal: f32,         // focal-length multiplier = cot(fov/2), volumetric convention
    splat_focal_depth: f32, // DoF focal plane in view-depth units
    splat_explode: f32,     // drop envelope: max(env·exp(−dt/0.45), drop)
    splat_sorted: f32,      // 1.0 = sorted-composite path (raw intrinsic alpha); 0.0 = OIT
    splat_sh_degree: f32,   // 0 = DC only; 1–3 = view-dependent SH bands in splat_sh

    // A13 stereo + A13b per-band pan (#1801 ABI bump, 896 -> 944 B).
    pan: f32,           // broadband balance: 0.5 = centred, 0 = hard left, 1 = hard right
    stereo_width: f32,  // mid/side ratio: 0 = mono, ->1 = fully decorrelated
    stereo_corr: f32,   // L/R correlation: 0.5 = decorrelated, 1 = mono, 0 = anti-phase
    _pad_stereo: f32,
    // Per-band pan, same order and convention as `pan`, for the 7 bands sub_bass..brilliance.
    // A band carrying no energy holds 0.5. Read it with band_pan(i).
    band_pan: array<vec4f, 2>,

    // Eulerian fluid flow field (#1939, 944 -> 960 B). Gate + coupling for
    // fluid_velocity() below; the solver's own params live in fluid_sim.wgsl.
    fluid_enabled: f32,   // 0 or 1
    fluid_coupling: f32,  // how strongly particles relax toward the field (0..1)
    trail_head: u32,   // ribbon ring head on a fixed 60 Hz clock (#2351)
    trail_steps: u32,  // slots the head advanced this frame; writer fills them all
}

// Access effect param by index (mirrors fragment shader's param() function).
// Only params 0..7 are available in compute shaders.
fn param(i: u32) -> f32 {
    if i < 4u {
        return u.effect_params_0[i];
    }
    return u.effect_params_1[i - 4u];
}

fn mfcc(i: u32) -> f32 {
    return u.mfcc[i / 4u][i % 4u];
}

fn chroma_val(i: u32) -> f32 {
    return u.chroma[i / 4u][i % 4u];
}

// A13b per-band pan, i in 0..6 (sub_bass, bass, low_mid, mid, upper_mid, presence, brilliance).
// 0.5 = centred. Returns 0.5 for a band with no energy, so it is safe to use unguarded.
fn band_pan(i: u32) -> f32 {
    return u.band_pan[i / 4u][i % 4u];
}

// Read force matrix entry: from_sp→to_sp interaction strength (8-wide stride).
fn get_force(from_sp: u32, to_sp: u32) -> f32 {
    let idx = from_sp * 8u + to_sp;
    return u.force_matrix[idx / 4u][idx % 4u];
}

struct Particle {
    pos_life: vec4f,  // xy=position, z=reserved, w=life (1=alive, 0=dead)
    vel_size: vec4f,  // xy=velocity, z=reserved, w=size
    color: vec4f,     // rgba
    flags: vec4f,     // x=age, y=lifetime, z=effect-specific, w=effect-specific
}

// Species convention (multi-species sims: Symbiosis, Polycephalum): the species/organism id is
// carried in `flags.z` as f32 (read back with `u32(p.flags.z)`), NOT `flags.x` (which is age).
// Polycephalum uses one species per pitch class 0..11 and stores its heading angle in `flags.w`.

struct ParticleAux {
    home: vec4f,  // xy=home position, z=packed RGBA (bitcast u32->f32), w=sprite_index
}

// --- Bindings (group 0) — SoA layout ---

@group(0) @binding(0) var<uniform> u: ParticleUniforms;
@group(0) @binding(1) var<storage, read> pos_life_in: array<vec4f>;
@group(0) @binding(2) var<storage, read> vel_size_in: array<vec4f>;
@group(0) @binding(3) var<storage, read> color_in: array<vec4f>;
@group(0) @binding(4) var<storage, read> flags_in: array<vec4f>;
@group(0) @binding(5) var<storage, read_write> pos_life_out: array<vec4f>;
@group(0) @binding(6) var<storage, read_write> vel_size_out: array<vec4f>;
@group(0) @binding(7) var<storage, read_write> color_out: array<vec4f>;
@group(0) @binding(8) var<storage, read_write> flags_out: array<vec4f>;
// counters: [0]=alive_count, [1]=dead_count, [2]=emit_used,
// [3]=aux emit — a second emit budget for multi-voice sims (Cleave #1798)
@group(0) @binding(9) var<storage, read_write> counters: array<atomic<u32>, 4>;
@group(0) @binding(10) var<storage, read> aux: array<ParticleAux>;
@group(0) @binding(11) var<storage, read> dead_indices: array<u32>;
@group(0) @binding(12) var<storage, read_write> alive_indices_out: array<u32>;

// Read a particle from the SoA input arrays into a Particle struct.
fn read_particle(idx: u32) -> Particle {
    return Particle(pos_life_in[idx], vel_size_in[idx], color_in[idx], flags_in[idx]);
}

// Write a Particle struct to the SoA output arrays.
fn write_particle(idx: u32, p: Particle) {
    pos_life_out[idx] = p.pos_life;
    vel_size_out[idx] = p.vel_size;
    color_out[idx] = p.color;
    flags_out[idx] = p.flags;
}

// --- Flow field bindings (group 1) ---

@group(1) @binding(0) var flow_field_tex: texture_3d<f32>;
@group(1) @binding(1) var flow_field_sampler: sampler;

// Sample the 3D curl noise flow field at a position.
// pos: particle position in clip space [-1,1]
// Returns velocity offset in clip space.
fn sample_flow_field(pos: vec2f) -> vec2f {
    if u.flow_enabled < 0.5 {
        return vec2f(0.0);
    }
    // Map clip space [-1,1] to UV [0,1] for texture sampling
    let uv = (pos * 0.5 + 0.5) * u.flow_scale;
    // Scroll z-axis over time for animation
    let w = fract(u.time * u.flow_speed * 0.1);
    let sample = textureSampleLevel(flow_field_tex, flow_field_sampler, vec3f(uv, w), 0.0);
    // xyz = curl velocity, scale by strength
    return sample.xy * u.flow_strength;
}

// --- Obstacle texture bindings (group 1, bindings 2+3, water 4) ---

@group(1) @binding(2) var obstacle_tex: texture_2d<f32>;
@group(1) @binding(3) var obstacle_sampler: sampler;
// Accumulated water height from the virtual-pipes sim (#1851), `.r` = depth.
// A 1×1 zero texture when water is disabled, so it contributes nothing.
@group(1) @binding(4) var water_tex: texture_2d<f32>;
// Eulerian fluid velocity field (#1939), `.rg` = clip-space velocity (y-up), so
// it can be added straight to a particle's velocity. A 1×1 zero texture when the
// fluid sim is disabled. Read it with fluid_velocity(pos).
@group(1) @binding(5) var fluid_vel_tex: texture_2d<f32>;

// Sample the incompressible flow field the FluidSim solves around the obstacle
// (fluid_sim.wgsl). Returns clip-space velocity that already respects the solid
// boundary (flows around, wakes, eddies). Zero when the field is disabled.
fn fluid_velocity(pos: vec2f) -> vec2f {
    if u.fluid_enabled < 0.5 { return vec2f(0.0); }
    // The velocity texture is stored row 0 = top of screen; flip V to match.
    let uv = pos * 0.5 + 0.5;
    return textureSampleLevel(fluid_vel_tex, obstacle_sampler, vec2f(uv.x, 1.0 - uv.y), 0.0).rg;
}

// Per-axis clip→screen direction scale (larger axis normalized to 1).
// Only the x:y ratio matters for the collision reflection math (#1790).
fn obstacle_aspect() -> vec2f {
    let res = max(u.resolution, vec2f(1.0));
    return res / max(res.x, res.y);
}

// Size of the fitted obstacle rect in screen-normalized [0,1] units, centered.
// Stretch=(1,1) legacy; Contain fits inside (letterbox); Cover fills (crop).
fn obstacle_fit_size() -> vec2f {
    if u.obstacle_fit == 0u { return vec2f(1.0); }
    let res = max(u.resolution, vec2f(1.0));
    let dims = max(vec2f(textureDimensions(obstacle_tex)), vec2f(1.0));
    let fit = res / dims;
    let s = select(min(fit.x, fit.y), max(fit.x, fit.y), u.obstacle_fit == 2u);
    return dims * s / res;
}

// Map clip-space position [-1,1] to obstacle UV [0,1] honoring the fit mode.
// Clip Y is up (+1=top), texture V is down (0=top), so flip Y.
fn obstacle_uv(pos: vec2f) -> vec2f {
    let s = vec2f(pos.x * 0.5 + 0.5, -pos.y * 0.5 + 0.5);
    return (s - 0.5) / obstacle_fit_size() + 0.5;
}

// Effective collision height at a UV: terrain (obstacle alpha) plus accumulated
// water raised by `obstacle_water_scale`. When water is disabled the water
// texture is a 1×1 zero and the scale is 0, so this is exactly the terrain.
fn obstacle_height_uv(uv: vec2f) -> f32 {
    let terr = textureSampleLevel(obstacle_tex, obstacle_sampler, uv, 0.0).a;
    let water = textureSampleLevel(water_tex, obstacle_sampler, uv, 0.0).r;
    return terr + u.obstacle_water_scale * water;
}

// Sample the effective obstacle height at clip-space position. Returns 0 if
// disabled or outside the fitted rect. (Named `obstacle_alpha` for continuity —
// it is the value the threshold test compares against.)
fn obstacle_alpha(pos: vec2f) -> f32 {
    if u.obstacle_enabled < 0.5 { return 0.0; }
    let uv = obstacle_uv(pos);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 { return 0.0; }
    return obstacle_height_uv(uv);
}

// Raw SCREEN-space gradient of the effective height (points UPHILL, toward
// higher terrain+water). Central differences over ≥2-texel steps so it is
// proportional to the true on-screen slope regardless of aspect/fit (#1790).
fn obstacle_gradient(pos: vec2f) -> vec2f {
    let res = max(u.resolution, vec2f(1.0));
    let dims = max(vec2f(textureDimensions(obstacle_tex)), vec2f(1.0));
    let size = obstacle_fit_size();
    let tex_px = size * res / dims;           // screen pixels per texel, per axis
    let h_px = 2.0 * max(tex_px.x, tex_px.y); // >= 2-texel step on both axes
    let eps = vec2f(h_px / (res.x * size.x), h_px / (res.y * size.y));
    let uv = obstacle_uv(pos);

    let ax = obstacle_height_uv(uv + vec2f(eps.x, 0.0));
    let bx = obstacle_height_uv(uv - vec2f(eps.x, 0.0));
    let ay = obstacle_height_uv(uv + vec2f(0.0, eps.y));
    let by = obstacle_height_uv(uv - vec2f(0.0, eps.y));

    // Screen-space, y-up: +eps.y in V is DOWN-screen, hence (by - ay).
    return vec2f(ax - bx, by - ay);
}

// Outward surface normal (unit, away from higher height) in SCREEN space.
fn obstacle_normal(pos: vec2f) -> vec2f {
    let grad = obstacle_gradient(pos);
    let len = length(grad);
    if len < 0.001 {
        // Degenerate gradient: push toward screen center (sensible for the
        // alpha-0 letterbox bars in Contain mode); straight up if at center.
        let to_center = -pos * obstacle_aspect();
        if length(to_center) < 0.001 { return vec2f(0.0, 1.0); }
        return normalize(to_center);
    }
    // Outward normal = away from higher alpha.
    return -grad / len;
}

// Apply obstacle collision. Returns vec4f(new_pos.xy, new_vel.xy).
// Call after position integration: prev_pos is before integration, pos is after.
// Drape (mode 4) is NOT handled here — it is a surface-flow sim implemented in
// the host effect (tide_sim.wgsl), using obstacle_gradient/obstacle_height_uv.
fn apply_obstacle_collision(pos: vec2f, vel: vec2f, prev_pos: vec2f) -> vec4f {
    if u.obstacle_enabled < 0.5 { return vec4f(pos, vel); }

    let alpha = obstacle_alpha(pos);
    let is_contain = u.obstacle_mode == 3u;

    // Normal modes: collide when inside obstacle (alpha >= threshold)
    // Contain mode: collide when outside obstacle (alpha < threshold)
    if !is_contain && alpha < u.obstacle_threshold { return vec4f(pos, vel); }
    if is_contain && alpha >= u.obstacle_threshold { return vec4f(pos, vel); }

    let raw_normal = obstacle_normal(pos);
    // Contain mode: invert normal to point inward (toward high-alpha region)
    let normal = select(raw_normal, -raw_normal, is_contain);

    // Binary search for surface contact point along the integration step.
    // This prevents particles from tunneling deep into the obstacle and
    // bouncing back and forth (strobe effect).
    var lo = 0.0;
    var hi = 1.0;
    for (var i = 0; i < 4; i++) {
        let mid = (lo + hi) * 0.5;
        let test_pos = mix(prev_pos, pos, mid);
        let test_alpha = obstacle_alpha(test_pos);
        // Normal: safe side is low alpha; Contain: safe side is high alpha
        let in_collision = select(test_alpha >= u.obstacle_threshold, test_alpha < u.obstacle_threshold, is_contain);
        if in_collision {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    // Place particle just before the surface along its trajectory, nudged out
    // along the clip direction whose on-screen image is `normal`.
    let asp = obstacle_aspect();
    let safe_pos = mix(prev_pos, pos, lo) + normalize(normal / asp) * 0.002;

    // Response math runs in SCREEN space (y-up) so reflections look right on
    // a non-square viewport — reflection is not affine-invariant, so doing it
    // in clip space would skew every deflection angle (#1790). At elasticity
    // 1.0 the preserved quantity is screen-space speed.
    let v_s = vel * asp;
    switch u.obstacle_mode {
        // Bounce (0): reflect off the obstacle surface.
        // Contain (3): identical reflection, normal already inverted above.
        case 0u, 3u: {
            let v_dot_n = dot(v_s, normal);
            // Only reflect if moving into the obstacle
            if v_dot_n >= 0.0 { return vec4f(safe_pos, vel); }
            let reflected = v_s - normal * 2.0 * v_dot_n;
            return vec4f(safe_pos, (reflected / asp) * u.obstacle_elasticity);
        }
        // Stick: zero velocity, hold at surface
        case 1u: {
            return vec4f(safe_pos, vec2f(0.0));
        }
        // Flow: redirect into tangential direction, preserving energy
        case 2u: {
            let v_dot_n = dot(v_s, normal);
            if v_dot_n >= 0.0 { return vec4f(safe_pos, vel); }
            // Tangent: 90-degree rotation of normal
            let tangent = vec2f(-normal.y, normal.x);
            // Pick tangent direction matching existing motion
            let tangent_dir = select(-tangent, tangent, dot(v_s, tangent) >= 0.0);
            // Existing tangential speed + redirected normal speed
            let tangent_vel = tangent_dir * (abs(dot(v_s, tangent_dir)) + abs(v_dot_n) * u.obstacle_elasticity);
            return vec4f(safe_pos, tangent_vel / asp);
        }
        default: {
            return vec4f(pos, vel);
        }
    }
}

// --- Trail buffer (group 2, optional) ---

@group(2) @binding(0) var<storage, read_write> trail_buffer: array<vec4f>;

// Write current position to the trail ring buffer.
// Call this after position integration for alive particles.
// trail_point: vec4f(pos.x, pos.y, size, alpha)
fn trail_write(idx: u32, trail_point: vec4f) {
    if u.trail_length < 2u {
        return;
    }
    // The ring advances on a fixed 60 Hz clock, not once per rendered frame
    // (#2351): one point per frame made the trail's DURATION a function of frame
    // rate, so a 16-point trail was 267 ms at 60 fps but 533 ms at 30 and 133 ms
    // at 120. `trail_head` is now time-quantized upstream, and the renderer reads
    // the same counter via RenderUniforms.trail_head.
    //
    // #1796's hazard is real and this is how it is answered. That note forbade
    // deriving the slot from wall-clock time because a compositor hiccup leaps
    // the head past slots still holding quarter-second-stale points, and every
    // ribbon flashes a long segment to them. So the head is never merely
    // *sampled* from the clock: `trail_steps` says how many slots it moved, and
    // every one of them is filled here. Above 60 fps steps is 0 on some frames
    // and the newest point simply refreshes the current slot; below 60 fps steps
    // is 2 or more and the skipped slots are backfilled rather than left stale.
    // A long stall is clamped upstream to a full ring, which rewrites everything.
    let steps = max(u.trail_steps, 1u);
    let head = u.trail_head % u.trail_length;
    let base = idx * u.trail_length;
    for (var s = 0u; s < steps; s++) {
        // Modular step-back, never `head - s`: these are u32, and u32 wrap is
        // only congruent mod trail_length when trail_length divides 2^32.
        let slot = (head + u.trail_length - (s % u.trail_length)) % u.trail_length;
        trail_buffer[base + slot] = trail_point;
    }
}

// --- Spatial hash neighbor query (group 3, optional) ---

@group(3) @binding(0) var<storage, read> sh_cell_offsets: array<u32>;
@group(3) @binding(1) var<storage, read> sh_cell_counts: array<u32>;
@group(3) @binding(2) var<storage, read> sh_sorted_indices: array<u32>;

const SH_GRID_W: u32 = 40u;
const SH_GRID_H: u32 = 40u;

fn sh_pos_to_cell(pos: vec2f) -> vec2i {
    let gx = i32(clamp((pos.x * 0.5 + 0.5) * f32(SH_GRID_W), 0.0, f32(SH_GRID_W - 1u)));
    let gy = i32(clamp((pos.y * 0.5 + 0.5) * f32(SH_GRID_H), 0.0, f32(SH_GRID_H - 1u)));
    return vec2i(gx, gy);
}

fn sh_cell_index(gx: i32, gy: i32) -> u32 {
    return u32(gy) * SH_GRID_W + u32(gx);
}

// Get the start index and count for a grid cell.
// Returns vec2u(offset, count). If cell is out of bounds, returns (0, 0).
fn sh_cell_range(gx: i32, gy: i32) -> vec2u {
    if gx < 0 || gx >= i32(SH_GRID_W) || gy < 0 || gy >= i32(SH_GRID_H) {
        return vec2u(0u, 0u);
    }
    let cell = sh_cell_index(gx, gy);
    return vec2u(sh_cell_offsets[cell], sh_cell_counts[cell]);
}

// --- Hash / random utilities ---

// Integer hash (lowbias32). PREFER THIS for anything index-scaled. The fract-sin
// hash() below degrades on GPU for arguments beyond ~1e4 — with idx-scaled args
// (u.seed + f32(idx)*K reaches 1e5..1e7) a band of indices rolls near-constant
// tiny values, passing ANY probability threshold every re-roll: an immortal
// audio-independent emitter, plus a complementary band of slots that never fire.
//
// Two separate failure modes, both fixed by mixing u32 exactly:
//   1. sin() range reduction collapses at large arguments (the band above).
//   2. f32 spacing — at idx 2e6 the seed lands near 3.5e7 where the ULP is 4.0,
//      so hash(s), hash(s+1.0), hash(s+2.0) are BIT-IDENTICAL. Never draw
//      decorrelated values with float offsets; XOR-salt an integer seed instead.
fn uhash(x: u32) -> u32 {
    var h = x;
    h = h ^ (h >> 16u);
    h = h * 0x7feb352du;
    h = h ^ (h >> 15u);
    h = h * 0x846ca68bu;
    h = h ^ (h >> 16u);
    return h;
}

fn uhash_f(x: u32) -> f32 {
    return f32(uhash(x)) / 4294967296.0;
}

fn hash(n: f32) -> f32 {
    return fract(sin(n) * 43758.5453123);
}

fn hash2(p: vec2f) -> f32 {
    return fract(sin(dot(p, vec2f(127.1, 311.7))) * 43758.5453);
}

// Two components in [-1, 1] from one seed.
//
// KNOWN DEFECT, deliberately not fixed here: at large seeds the `+ 1.0` offset
// rounds away entirely (at 2M particles `u.seed + f32(idx) * 17.31` reaches
// ~3.5e7 where the f32 ULP is 4.0), so this returns x == y and spawns collapse
// onto the diagonal; and the fract-sin hash bands indices besides. The integer
// version is `rand_vec2_u` below.
//
// It is NOT swapped wholesale because measured A/B says the artefact is
// load-bearing: with the integer version Flux's punchy embers flatten into an
// even wash and Turing's particles stop tracing the reaction-diffusion
// filaments, because the banding was stacking particles into fewer, brighter
// points. Cymatics is the opposite — see cymatics_sim.wgsl, which opts in.
// Migrating a caller is a LOOK change; A/B it with particle_effect_previews.
fn rand_vec2(seed: f32) -> vec2f {
    return vec2f(hash(seed), hash(seed + 1.0)) * 2.0 - 1.0;
}

// Correct integer-hashed version: distinct floats stay distinct (bitcast, not
// convert) and the two components are XOR-salted rather than offset — an offset
// is exactly what fails above. Opt in per call site, after an A/B.
fn rand_vec2_u(seed: f32) -> vec2f {
    // The extra salt keeps seed 0.0 off uhash's fixed point (uhash(0) == 0).
    let s = uhash(bitcast<u32>(seed) ^ 0x9e3779b9u);
    return vec2f(uhash_f(s), uhash_f(s ^ 0x85ebca6bu)) * 2.0 - 1.0;
}

// --- Aspect ratio helpers ---

fn aspect() -> f32 {
    return u.resolution.x / u.resolution.y;
}

fn to_screen(p: vec2f) -> vec2f {
    return vec2f(p.x * aspect(), p.y);
}

fn to_clip(v: vec2f) -> vec2f {
    return vec2f(v.x / aspect(), v.y);
}

// --- Alive/dead list management ---

// Claim an emission slot. Returns the slot index.
// Compare against u.emit_count to check if emission is allowed.
fn emit_claim() -> u32 {
    return atomicAdd(&counters[2], 1u);
}

// Mark a particle index as alive (append to alive output list).
fn mark_alive(idx: u32) {
    let pos = atomicAdd(&counters[0], 1u);
    alive_indices_out[pos] = idx;
}

// --- FBM noise + curl noise ---

// 2D curl noise: rotated gradient of scalar noise field.
// Returns divergence-free velocity from fosfora_noise2 (auto-prepended).
fn curl_noise_2d(p: vec2f) -> vec2f {
    let eps = 0.01;
    let dx = fosfora_noise2(p + vec2f(eps, 0.0)) - fosfora_noise2(p - vec2f(eps, 0.0));
    let dy = fosfora_noise2(p + vec2f(0.0, eps)) - fosfora_noise2(p - vec2f(0.0, eps));
    return vec2f(dy, -dx) / (2.0 * eps);
}

// FBM curl noise with configurable octaves.
fn fbm_curl_2d(p: vec2f, octaves: u32, lacunarity: f32, persistence: f32) -> vec2f {
    var result = vec2f(0.0);
    var freq = 1.0;
    var amp = 1.0;
    var total_amp = 0.0;
    for (var i = 0u; i < octaves; i++) {
        result += curl_noise_2d(p * freq) * amp;
        total_amp += amp;
        freq *= lacunarity;
        amp *= persistence;
    }
    return result / max(total_amp, 0.001);
}

// FBM turbulence (absolute value noise) with configurable octaves.
fn fbm_turbulence_2d(p: vec2f, octaves: u32, lacunarity: f32, persistence: f32) -> vec2f {
    var result = vec2f(0.0);
    var freq = 1.0;
    var amp = 1.0;
    var total_amp = 0.0;
    for (var i = 0u; i < octaves; i++) {
        let n1 = abs(fosfora_noise2(p * freq)) * 2.0 - 1.0;
        let n2 = abs(fosfora_noise2(p * freq + vec2f(31.7, 47.3))) * 2.0 - 1.0;
        result += vec2f(n1, n2) * amp;
        total_amp += amp;
        freq *= lacunarity;
        amp *= persistence;
    }
    return result / max(total_amp, 0.001);
}

// Apply all builtin forces to a velocity. Call from simulation shaders.
// Applies: gravity → wind → drag → noise (FBM or legacy hash) → attraction → vortex → flow field.
fn apply_builtin_forces(pos: vec2f, vel: vec2f, dt: f32) -> vec2f {
    var v = vel;

    // Gravity
    v += u.gravity * dt;

    // Wind
    v += u.wind * dt;

    // Drag
    v *= pow(u.drag, dt * 60.0);

    // Noise-based turbulence
    if u.noise_octaves > 0u {
        let noise_pos = pos * 3.0 + vec2f(u.time * u.noise_speed);
        if u.noise_mode == 1u {
            // Curl noise (divergence-free)
            v += fbm_curl_2d(noise_pos, u.noise_octaves, u.noise_lacunarity, u.noise_persistence) * u.turbulence * dt;
        } else {
            // Turbulence (abs noise)
            v += fbm_turbulence_2d(noise_pos, u.noise_octaves, u.noise_lacunarity, u.noise_persistence) * u.turbulence * dt;
        }
    } else if u.turbulence > 0.0 {
        // Legacy hash turbulence (backward compat)
        let turb_seed = pos * 3.0 + vec2f(u.time * 0.5);
        let turb = vec2f(
            hash2(turb_seed) - 0.5,
            hash2(turb_seed + vec2f(17.0)) - 0.5
        ) * u.turbulence * dt;
        v += turb;
    }

    // Attraction to point
    if u.attraction_strength != 0.0 {
        let to_target = u.attraction_point - pos;
        let dist = length(to_target);
        if dist > 0.001 {
            v += normalize(to_target) * u.attraction_strength * dt;
        }
    }

    // Vortex field
    if u.vortex_strength != 0.0 {
        let to_center = pos - u.vortex_center;
        let dist = length(to_center);
        if dist > 0.001 {
            let falloff = smoothstep(u.vortex_radius, 0.0, dist);
            let tangent = vec2f(-to_center.y, to_center.x) / dist;
            v += tangent * u.vortex_strength * falloff * dt;
        }
    }

    // Flow field (3D texture)
    v += sample_flow_field(pos);

    return v;
}

// Apply ground bounce. Returns vec4f(pos.xy, vel.xy).
fn apply_ground_bounce(pos: vec2f, vel: vec2f) -> vec4f {
    var p = pos;
    var v = vel;
    if p.y < u.ground_y && v.y < 0.0 {
        p.y = u.ground_y + (u.ground_y - p.y) * u.ground_bounce;
        v.y = -v.y * u.ground_bounce;
        v.x *= 0.95; // friction on bounce
    }
    return vec4f(p, v);
}

// --- Lifetime curve helpers ---

// Sample an 8-point LUT stored across two vec4f values.
fn sample_curve_lut(t: f32, lut_a: vec4f, lut_b: vec4f) -> f32 {
    let tc = clamp(t, 0.0, 0.999);
    let idx_f = tc * 7.0;
    let idx = u32(idx_f);
    let frac = idx_f - f32(idx);

    // Read values from the two vec4f (indices 0-3 in lut_a, 4-7 in lut_b)
    var v0: f32;
    var v1: f32;
    switch idx {
        case 0u: { v0 = lut_a.x; v1 = lut_a.y; }
        case 1u: { v0 = lut_a.y; v1 = lut_a.z; }
        case 2u: { v0 = lut_a.z; v1 = lut_a.w; }
        case 3u: { v0 = lut_a.w; v1 = lut_b.x; }
        case 4u: { v0 = lut_b.x; v1 = lut_b.y; }
        case 5u: { v0 = lut_b.y; v1 = lut_b.z; }
        case 6u: { v0 = lut_b.z; v1 = lut_b.w; }
        default: { v0 = lut_b.w; v1 = lut_b.w; }
    }
    return mix(v0, v1, frac);
}

// Evaluate size curve. Returns 1.0 if disabled (neutral multiplier).
fn eval_size_curve(life_frac: f32) -> f32 {
    if (u.curve_flags & 1u) == 0u { return 1.0; }
    return sample_curve_lut(life_frac, u.size_curve[0], u.size_curve[1]);
}

// Evaluate opacity curve. Returns 1.0 if disabled (neutral multiplier).
fn eval_opacity_curve(life_frac: f32) -> f32 {
    if (u.curve_flags & 2u) == 0u { return 1.0; }
    return sample_curve_lut(life_frac, u.opacity_curve[0], u.opacity_curve[1]);
}

// Unpack a packed RGBA u32 to vec4f (0-1 range).
fn unpack_color(packed: u32) -> vec4f {
    let r = f32((packed >> 24u) & 0xFFu) / 255.0;
    let g = f32((packed >> 16u) & 0xFFu) / 255.0;
    let b = f32((packed >> 8u) & 0xFFu) / 255.0;
    let a = f32(packed & 0xFFu) / 255.0;
    return vec4f(r, g, b, a);
}

// Sample color gradient over lifetime. Returns original color if no gradient defined.
fn eval_color_gradient(life_frac: f32) -> vec4f {
    if u.gradient_count <= 0u { return vec4f(1.0); }
    if u.gradient_count == 1u { return unpack_color(u.color_gradient[0].x); }

    let tc = clamp(life_frac, 0.0, 0.999);
    let max_idx = f32(u.gradient_count - 1u);
    let idx_f = tc * max_idx;
    let idx = u32(idx_f);
    let frac = idx_f - f32(idx);

    // Read packed colors from array<vec4u, 2> (indices 0-3 in [0], 4-7 in [1])
    let c0 = unpack_color(read_gradient(idx));
    let c1 = unpack_color(read_gradient(min(idx + 1u, u.gradient_count - 1u)));
    return mix(c0, c1, frac);
}

// Read gradient color at index from packed array<vec4u, 2>.
fn read_gradient(idx: u32) -> u32 {
    let vec_idx = idx / 4u;
    let comp_idx = idx % 4u;
    let v = u.color_gradient[vec_idx];
    switch comp_idx {
        case 0u: { return v.x; }
        case 1u: { return v.y; }
        case 2u: { return v.z; }
        default: { return v.w; }
    }
}

// ---- Deprecated aliases (pre-rename API, kept so user custom effects keep
// compiling). Do not use in new code; may be removed in a future major release. ----

