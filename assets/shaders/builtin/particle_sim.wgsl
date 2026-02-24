// Default particle simulation compute shader.
// Custom .pfx effects can override this via compute_shader field.

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

    bass: f32,
    mid: f32,
    treble: f32,
    rms: f32,
    onset: f32,
    centroid: f32,
    flux: f32,
    flatness: f32,

    resolution: vec2f,
    _pad: vec2f,
}

struct Particle {
    pos_life: vec4f,  // xy=position, z=reserved, w=life (1=alive, 0=dead)
    vel_size: vec4f,  // xy=velocity, z=reserved, w=size
    color: vec4f,     // rgba
    flags: vec4f,     // x=age, y=lifetime, z=emitter_id, w=reserved
}

@group(0) @binding(0) var<uniform> u: ParticleUniforms;
@group(0) @binding(1) var<storage, read> particles_in: array<Particle>;
@group(0) @binding(2) var<storage, read_write> particles_out: array<Particle>;
@group(0) @binding(3) var<storage, read_write> emit_counter: atomic<u32>;

// Hash for pseudo-random numbers
fn hash(n: f32) -> f32 {
    return fract(sin(n) * 43758.5453123);
}

fn hash2(p: vec2f) -> f32 {
    return fract(sin(dot(p, vec2f(127.1, 311.7))) * 43758.5453);
}

fn rand_vec2(seed: f32) -> vec2f {
    return vec2f(hash(seed), hash(seed + 1.0)) * 2.0 - 1.0;
}

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 7.31;

    // Position based on emitter shape
    var pos = u.emitter_pos;
    switch u.emitter_shape {
        case 1u: { // ring
            let angle = hash(seed_base) * 6.2831853;
            pos += vec2f(cos(angle), sin(angle)) * u.emitter_radius;
        }
        case 2u: { // line
            let t = hash(seed_base) * 2.0 - 1.0;
            pos += vec2f(t * u.emitter_radius, 0.0);
        }
        case 3u: { // screen
            pos = rand_vec2(seed_base);
        }
        default: { // point
            pos += rand_vec2(seed_base) * 0.001;
        }
    }

    // Random velocity
    let angle = hash(seed_base + 3.0) * 6.2831853;
    let speed = u.initial_speed * (0.5 + 0.5 * hash(seed_base + 5.0));
    let vel = vec2f(cos(angle), sin(angle)) * speed;

    // Color with audio reactivity
    let hue = hash(seed_base + 7.0) * 0.3 + u.centroid * 0.7;
    let r = abs(hue * 6.0 - 3.0) - 1.0;
    let g = 2.0 - abs(hue * 6.0 - 2.0);
    let b = 2.0 - abs(hue * 6.0 - 4.0);
    let brightness = 0.8 + u.rms * 0.5;

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, u.initial_size);
    p.color = vec4f(clamp(vec3f(r, g, b), vec3f(0.0), vec3f(1.0)) * brightness, 1.0);
    p.flags = vec4f(0.0, u.lifetime, 0.0, 0.0);
    return p;
}

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles {
        return;
    }

    var p = particles_in[idx];
    let life = p.pos_life.w;
    let age = p.flags.x;
    let max_life = p.flags.y;

    if life <= 0.0 {
        // Dead particle — try to claim an emission slot
        let slot = atomicAdd(&emit_counter, 1u);
        if slot < u.emit_count {
            p = emit_particle(idx);
        }
        particles_out[idx] = p;
        return;
    }

    // Age the particle
    let new_age = age + u.delta_time;
    if new_age >= max_life {
        // Kill it
        p.pos_life.w = 0.0;
        particles_out[idx] = p;
        return;
    }

    let life_frac = new_age / max_life; // 0..1

    // Apply forces
    var vel = p.vel_size.xy;

    // Gravity
    vel += u.gravity * u.delta_time;

    // Drag
    vel *= pow(u.drag, u.delta_time * 60.0);

    // Turbulence (simple noise-based)
    if u.turbulence > 0.0 {
        let turb_seed = p.pos_life.xy * 3.0 + vec2f(u.time * 0.5);
        let turb = vec2f(
            hash2(turb_seed) - 0.5,
            hash2(turb_seed + vec2f(17.0)) - 0.5
        ) * u.turbulence * u.delta_time;
        vel += turb;
    }

    // Attraction to point
    if u.attraction_strength != 0.0 {
        let to_target = u.attraction_point - p.pos_life.xy;
        let dist = length(to_target);
        if dist > 0.001 {
            vel += normalize(to_target) * u.attraction_strength * u.delta_time;
        }
    }

    // Integrate position
    let pos = p.pos_life.xy + vel * u.delta_time;

    // Size interpolation
    let size = mix(p.vel_size.w, u.size_end, life_frac);

    // Fade alpha near death
    let alpha = 1.0 - smoothstep(0.7, 1.0, life_frac);

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color.a = alpha;
    p.flags.x = new_age;

    particles_out[idx] = p;
}
