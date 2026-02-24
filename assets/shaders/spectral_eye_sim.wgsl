// Spectral Eye custom particle compute shader.
// Orbital mechanics: particles orbit center with tangential velocity,
// beat-burst radial push, attraction back to orbit radius.

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
    pos_life: vec4f,
    vel_size: vec4f,
    color: vec4f,
    flags: vec4f,
}

@group(0) @binding(0) var<uniform> u: ParticleUniforms;
@group(0) @binding(1) var<storage, read> particles_in: array<Particle>;
@group(0) @binding(2) var<storage, read_write> particles_out: array<Particle>;
@group(0) @binding(3) var<storage, read_write> emit_counter: atomic<u32>;

fn hash(n: f32) -> f32 {
    return fract(sin(n) * 43758.5453123);
}

// Aspect ratio: width / height
fn aspect() -> f32 {
    return u.resolution.x / u.resolution.y;
}

// Convert clip-space position to aspect-corrected space (circular orbits look circular)
fn to_screen(p: vec2f) -> vec2f {
    return vec2f(p.x * aspect(), p.y);
}

// Convert force/velocity from screen space back to clip space
fn to_clip(v: vec2f) -> vec2f {
    return vec2f(v.x / aspect(), v.y);
}

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 13.37;

    // Spawn on ring — in aspect-corrected space, then convert back
    let angle = hash(seed_base) * 6.2831853;
    let r = u.emitter_radius * (0.9 + 0.2 * hash(seed_base + 1.0));
    let screen_offset = vec2f(cos(angle), sin(angle)) * r;
    let pos = u.emitter_pos + to_clip(screen_offset);

    // Tangential velocity in screen space, converted to clip space
    let screen_tangent = vec2f(-sin(angle), cos(angle));
    let speed = u.initial_speed;
    let dir = select(-1.0, 1.0, hash(seed_base + 5.0) > 0.5);
    let vel = to_clip(screen_tangent * speed * dir);

    // Color: rainbow around the ring
    let hue = (angle / 6.2831853) * 0.5 + u.centroid * 0.5;
    let r_c = abs(hue * 6.0 - 3.0) - 1.0;
    let g_c = 2.0 - abs(hue * 6.0 - 2.0);
    let b_c = 2.0 - abs(hue * 6.0 - 4.0);
    let brightness = 0.12 + u.rms * 0.08;

    // Stagger initial age to prevent synchronized death
    let initial_age = hash(seed_base + 9.0) * u.lifetime * 0.5;

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, u.initial_size * (0.8 + u.bass * 0.2));
    p.color = vec4f(clamp(vec3f(r_c, g_c, b_c), vec3f(0.0), vec3f(1.0)) * brightness, 0.5);
    p.flags = vec4f(initial_age, u.lifetime, 0.0, 0.0);
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
        let slot = atomicAdd(&emit_counter, 1u);
        if slot < u.emit_count {
            p = emit_particle(idx);
        }
        particles_out[idx] = p;
        return;
    }

    let new_age = age + u.delta_time;
    if new_age >= max_life {
        p.pos_life.w = 0.0;
        particles_out[idx] = p;
        return;
    }

    let life_frac = new_age / max_life;
    let dt = u.delta_time;

    // Work in aspect-corrected screen space for all orbital calculations
    let screen_pos = to_screen(p.pos_life.xy);
    let screen_center = to_screen(u.emitter_pos);
    var screen_vel = to_screen(p.vel_size.xy);

    let to_center = screen_center - screen_pos;
    let dist = length(to_center);
    let dir_to_center = select(vec2f(0.0, 1.0), to_center / dist, dist > 0.001);
    let tangent = vec2f(-dir_to_center.y, dir_to_center.x);

    // Decompose velocity into radial and tangential
    let radial_vel = dot(screen_vel, dir_to_center);
    let tangential_vel = dot(screen_vel, tangent);

    // 1. Strong spring toward orbit radius
    let orbit_radius = u.emitter_radius;
    let displacement = dist - orbit_radius;
    screen_vel += dir_to_center * displacement * 12.0 * dt;

    // 2. Heavy radial damping — kill oscillation immediately
    //    Remove most of the radial velocity each frame
    screen_vel -= dir_to_center * radial_vel * 8.0 * dt;

    // 3. Drive tangential speed toward orbital speed (preserve direction)
    let orbital_speed = u.initial_speed * (1.0 + u.mid * 0.3);
    if abs(tangential_vel) > 0.001 {
        let desired_tang = orbital_speed * sign(tangential_vel);
        screen_vel += tangent * (desired_tang - tangential_vel) * 3.0 * dt;
    } else {
        screen_vel += tangent * orbital_speed * dt;
    }

    // Beat burst: radial push outward on onset
    if u.onset > 0.5 {
        screen_vel -= dir_to_center * u.onset * 0.08;
    }

    // Subtle turbulence
    let turb_angle = hash(f32(idx) * 0.1 + u.time) * 6.2831853;
    screen_vel += vec2f(cos(turb_angle), sin(turb_angle)) * u.bass * 0.003;

    // Very gentle drag
    screen_vel *= 1.0 - 0.005 * dt * 60.0;

    // Convert back to clip space and integrate
    let vel = to_clip(screen_vel);
    let new_pos = p.pos_life.xy + vel * dt;

    // Size and alpha
    let base_size = mix(p.vel_size.w, u.size_end, life_frac);
    let size = base_size * (1.0 + u.rms * 0.15);
    let alpha = (1.0 - smoothstep(0.5, 1.0, life_frac)) * 0.4;

    // Gentle color shift
    var col = p.color.rgb;
    let shift = u.centroid * 0.1 * life_frac;
    col = clamp(vec3f(col.r + shift, col.g, col.b - shift * 0.3), vec3f(0.0), vec3f(1.0));

    p.pos_life = vec4f(new_pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = vec4f(col, alpha);
    p.flags.x = new_age;

    particles_out[idx] = p;
}
