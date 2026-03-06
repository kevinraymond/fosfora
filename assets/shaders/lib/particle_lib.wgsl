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

    // Obstacle collision
    obstacle_enabled: f32,    // 0.0 or 1.0
    obstacle_threshold: f32,  // alpha cutoff
    obstacle_mode: u32,       // 0=bounce, 1=stick, 2=flow
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

    // Zero-crossing rate
    zcr: f32,
    _pad_zcr_a: f32,
    _pad_zcr_b: f32,
    _pad_zcr_c: f32,
    _pad_zcr_d: f32,
    _pad_zcr_e: f32,
    _pad_zcr_f: f32,
    _pad_zcr_g: f32,
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
// counters: [0]=alive_count, [1]=dead_count, [2]=emit_used, [3]=reserved
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

// --- Obstacle texture bindings (group 1, bindings 2+3) ---

@group(1) @binding(2) var obstacle_tex: texture_2d<f32>;
@group(1) @binding(3) var obstacle_sampler: sampler;

// Map clip-space position [-1,1] to obstacle UV [0,1].
// Clip Y is up (+1=top), texture V is down (0=top), so flip Y.
fn obstacle_uv(pos: vec2f) -> vec2f {
    return vec2f(pos.x * 0.5 + 0.5, -pos.y * 0.5 + 0.5);
}

// Sample obstacle alpha at clip-space position. Returns 0 if disabled.
fn obstacle_alpha(pos: vec2f) -> f32 {
    if u.obstacle_enabled < 0.5 { return 0.0; }
    let uv = obstacle_uv(pos);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 { return 0.0; }
    return textureSampleLevel(obstacle_tex, obstacle_sampler, uv, 0.0).a;
}

// Compute outward surface normal from alpha gradient (central differences).
fn obstacle_normal(pos: vec2f) -> vec2f {
    let dims = vec2f(textureDimensions(obstacle_tex));
    let texel = 1.0 / dims;
    // Central differences in UV space, converted from clip space
    let eps = texel * 2.0; // 2 texels for smoother gradient
    let uv = obstacle_uv(pos);

    let ax = textureSampleLevel(obstacle_tex, obstacle_sampler, uv + vec2f(eps.x, 0.0), 0.0).a;
    let bx = textureSampleLevel(obstacle_tex, obstacle_sampler, uv - vec2f(eps.x, 0.0), 0.0).a;
    let ay = textureSampleLevel(obstacle_tex, obstacle_sampler, uv + vec2f(0.0, eps.y), 0.0).a;
    let by = textureSampleLevel(obstacle_tex, obstacle_sampler, uv - vec2f(0.0, eps.y), 0.0).a;

    // Gradient of alpha field (points toward higher alpha = into obstacle)
    let grad = vec2f(ax - bx, ay - by);
    let len = length(grad);
    if len < 0.001 { return vec2f(0.0, -1.0); } // fallback normal
    // Outward normal = negative gradient direction
    return -grad / len;
}

// Apply obstacle collision. Returns vec4f(new_pos.xy, new_vel.xy).
// Call after position integration: prev_pos is before integration, pos is after.
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
    // Place particle just before the surface along its trajectory
    let safe_pos = mix(prev_pos, pos, lo) + normal * 0.002;

    switch u.obstacle_mode {
        // Bounce: reflect velocity along normal, scale by elasticity
        case 0u: {
            let v_dot_n = dot(vel, normal);
            // Only reflect if moving into the obstacle
            if v_dot_n >= 0.0 { return vec4f(safe_pos, vel); }
            let reflected = vel - normal * 2.0 * v_dot_n;
            return vec4f(safe_pos, reflected * u.obstacle_elasticity);
        }
        // Stick: zero velocity, hold at surface
        case 1u: {
            return vec4f(safe_pos, vec2f(0.0));
        }
        // Flow: redirect into tangential direction, preserving energy
        case 2u: {
            let v_dot_n = dot(vel, normal);
            if v_dot_n >= 0.0 { return vec4f(safe_pos, vel); }
            // Tangent: 90-degree rotation of normal
            let tangent = vec2f(-normal.y, normal.x);
            // Pick tangent direction matching existing motion
            let tangent_dir = select(-tangent, tangent, dot(vel, tangent) >= 0.0);
            // Existing tangential speed + redirected normal speed
            let tangent_vel = tangent_dir * (abs(dot(vel, tangent_dir)) + abs(v_dot_n) * u.obstacle_elasticity);
            return vec4f(safe_pos, tangent_vel);
        }
        // Contain: bounce off outside of obstacle shape (particles trapped inside)
        case 3u: {
            let v_dot_n = dot(vel, normal);
            if v_dot_n >= 0.0 { return vec4f(safe_pos, vel); }
            let reflected = vel - normal * 2.0 * v_dot_n;
            return vec4f(safe_pos, reflected * u.obstacle_elasticity);
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
    // Ring buffer: write to slot = frame_index % trail_length
    // frame_index is passed via reserved field in flags
    // We use a simple approach: write to (global frame counter % trail_length)
    // The frame counter is embedded in the seed's integer part
    let frame = u32(u.time * 60.0); // ~60fps frame counter
    let slot = frame % u.trail_length;
    let base = idx * u.trail_length + slot;
    trail_buffer[base] = trail_point;
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

fn hash(n: f32) -> f32 {
    return fract(sin(n) * 43758.5453123);
}

fn hash2(p: vec2f) -> f32 {
    return fract(sin(dot(p, vec2f(127.1, 311.7))) * 43758.5453);
}

fn rand_vec2(seed: f32) -> vec2f {
    return vec2f(hash(seed), hash(seed + 1.0)) * 2.0 - 1.0;
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
// Returns divergence-free velocity from phosphor_noise2 (auto-prepended).
fn curl_noise_2d(p: vec2f) -> vec2f {
    let eps = 0.01;
    let dx = phosphor_noise2(p + vec2f(eps, 0.0)) - phosphor_noise2(p - vec2f(eps, 0.0));
    let dy = phosphor_noise2(p + vec2f(0.0, eps)) - phosphor_noise2(p - vec2f(0.0, eps));
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
        let n1 = abs(phosphor_noise2(p * freq)) * 2.0 - 1.0;
        let n2 = abs(phosphor_noise2(p * freq + vec2f(31.7, 47.3))) * 2.0 - 1.0;
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
