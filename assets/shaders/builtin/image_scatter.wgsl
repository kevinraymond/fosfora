// Image-to-particle decomposition compute shader.
// Particles scatter on beat and reform to image positions via spring force.
// Auxiliary buffer provides home positions and packed RGBA colors.
// Structs, bindings, and helpers are in particle_lib.wgsl (auto-prepended).

fn unpack_rgba(packed: f32) -> vec4f {
    let bits = bitcast<u32>(packed);
    return vec4f(
        f32(bits & 0xFFu) / 255.0,
        f32((bits >> 8u) & 0xFFu) / 255.0,
        f32((bits >> 16u) & 0xFFu) / 255.0,
        f32((bits >> 24u) & 0xFFu) / 255.0,
    );
}

// Hardcoded spring-damper constants — independent of effect's ParticleDef.
const SPRING_K: f32 = 12.0;       // Spring stiffness — strong pull home
const DAMPING: f32 = 0.85;        // Per-frame velocity retention at 60fps
const SCATTER_SCALE: f32 = 0.12;  // Beat scatter impulse
const MAX_VEL: f32 = 1.0;         // Velocity cap

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles {
        return;
    }

    let home = aux[idx].home;
    let home_pos = home.xy;
    let home_color = unpack_rgba(home.z);

    // Skip transparent particles (padding beyond sampled image pixels)
    if home_color.a < 0.01 {
        // Park invisible particles offscreen, dead
        var p = read_particle(idx);
        p.pos_life = vec4f(99.0, 99.0, 0.0, 0.0);
        p.color = vec4f(0.0);
        write_particle(idx, p);
        return;
    }

    var p = read_particle(idx);

    // Initial emit: particles start at home position
    if p.pos_life.w <= 0.0 {
        let slot = emit_claim();
        if slot < u.emit_count {
            let seed_base = u.seed + f32(idx) * 7.31;
            p.pos_life = vec4f(home_pos + vec2f(hash(seed_base), hash(seed_base + 1.0)) * 0.01, 0.0, 1.0);
            p.vel_size = vec4f(0.0, 0.0, 0.0, u.initial_size);
            p.color = home_color;
            p.flags = vec4f(hash(seed_base + 2.0) * u.lifetime * 0.5, u.lifetime, 0.0, 0.0);
            write_particle(idx, p);
            mark_alive(idx);
        } else {
            write_particle(idx, p);
        }
        return;
    }

    var pos = p.pos_life.xy;
    var vel = p.vel_size.xy;
    let dt = u.delta_time;

    // Spring force toward home position
    let to_home = home_pos - pos;
    vel += to_home * SPRING_K * dt;

    // Damping
    vel *= pow(DAMPING, dt * 60.0);

    // Beat scatter: gentle impulse away from home
    if u.beat > 0.5 {
        let seed_base = f32(idx) * 3.17 + u.time;
        let random_dir = vec2f(hash(seed_base) - 0.5, hash(seed_base + 7.0) - 0.5);
        let scatter_dir = normalize(pos - home_pos + random_dir * 0.5 + vec2f(0.001));
        vel += scatter_dir * SCATTER_SCALE * (1.0 + u.kick * 0.5);
    }

    // Velocity cap
    let speed = length(vel);
    if speed > MAX_VEL {
        vel = vel * (MAX_VEL / speed);
    }

    // Integrate
    pos += vel * dt;

    // Preserve original image color (no audio color shift)
    let color = home_color;

    // Gradient-based size modulation: smooth areas larger (fill gaps), edges neutral
    let gradient = home.w;
    let grad_norm = clamp(gradient / 80.0, 0.0, 1.0);
    let grad_size = mix(1.3, 1.0, grad_norm);

    // Size: slight bass pulse + gradient
    let lum = dot(home_color.rgb, vec3f(0.299, 0.587, 0.114));
    var size = u.initial_size * (1.0 + u.bass * 0.2) * grad_size;

    // Sparkle boost: bright pixels at high-gradient locations (isolated stars, glints)
    // get an audio-reactive size pulse that creates active twinkling
    let sparkle = lum * grad_norm;
    if sparkle > 0.3 {
        let sparkle_strength = smoothstep(0.3, 0.8, sparkle);
        let phase = hash(f32(idx) * 1.618);
        let twinkle = sin(u.time * 6.0 + phase * 6.2831853) * 0.5 + 0.5;
        let audio_mod = 0.5 + u.onset * 0.8 + u.mid * 0.3;
        size *= 1.0 + sparkle_strength * twinkle * audio_mod * 0.8;
    }

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = color;
    p.flags.x += dt;

    // Wrap-around instead of death (image particles are persistent)
    if p.flags.x >= p.flags.y {
        p.flags.x = 0.0;
    }

    write_particle(idx, p);
    mark_alive(idx);
}
