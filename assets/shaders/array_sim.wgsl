// Array particle simulation — toroidal speaker emitters per audio band.
//
// 5 bands, particles divided by idx % 5. Each band has a ring emitter
// at a position determined by the arrangement param (stack vs concentric).
// Particles fire outward from the ring like speaker cones pushing air.
//
// --- Param mapping ---
// param(0) = trail_decay    (bg shader)
// param(1) = ring_radius    (base ring size)
// param(2) = spread         (emission cone half-angle)
// param(3) = arrangement         (0=stack, 1=concentric)
// param(4) = color_mode     (0=per-band, 0.5=mono, 1.0=velocity)
// param(5) = speed_mult     (outward velocity)
// param(6) = beat_pulse     (ring breathing + burst)
// param(7) = emitter_glow   (bg shader)

const TAU: f32 = 6.2831853;
const NUM_BANDS: u32 = 5u;
// Golden angle: TAU * (1 - 1/phi) — maximally uniform angular distribution
const GOLDEN_ANGLE: f32 = 2.3999632;

fn emitter_center(band: u32, arrangement: f32) -> vec2f {
    // Stack: 5 emitters arranged vertically
    let stack_y = f32(band) / 4.0 * 1.4 - 0.7; // -0.7 to +0.7
    let stack_pos = vec2f(0.0, stack_y);
    // Concentric: all centered at origin
    let concentric_pos = vec2f(0.0, 0.0);
    return mix(stack_pos, concentric_pos, arrangement);
}

fn emitter_ring_radius(band: u32, arrangement: f32, radius_param: f32) -> f32 {
    // Stack: rings sized by band (bass=largest, air=smallest)
    let scale0 = 1.0;
    let scale1 = 0.85;
    let scale2 = 0.7;
    let scale3 = 0.55;
    let scale4 = 0.4;
    var band_scale: f32;
    switch band {
        case 0u: { band_scale = scale0; }
        case 1u: { band_scale = scale1; }
        case 2u: { band_scale = scale2; }
        case 3u: { band_scale = scale3; }
        default: { band_scale = scale4; }
    }
    let stack_r = radius_param * 0.15 * band_scale;
    // Concentric: radius increases per band (bass=inner, air=outer)
    let concentric_r = radius_param * (0.1 + f32(band) * 0.15);
    return mix(stack_r, concentric_r, arrangement);
}

fn band_energy(band: u32) -> f32 {
    switch band {
        case 0u: { return max(u.sub_bass * 0.6 + u.bass * 0.4, 0.02); }
        case 1u: { return max(u.bass * 0.3 + u.low_mid * 0.7, 0.02); }
        case 2u: { return max(u.low_mid * 0.3 + u.mid * 0.7, 0.02); }
        case 3u: { return max(u.mid * 0.3 + u.upper_mid * 0.7, 0.02); }
        default: { return max(u.presence * 0.5 + u.brilliance * 0.5, 0.02); }
    }
}

fn band_color(band: u32) -> vec3f {
    switch band {
        case 0u: { return vec3f(1.0, 0.2, 0.1); }   // sub-bass: deep red-orange
        case 1u: { return vec3f(1.0, 0.5, 0.1); }   // bass: orange
        case 2u: { return vec3f(0.2, 0.9, 0.4); }   // low-mid: green
        case 3u: { return vec3f(0.2, 0.5, 1.0); }   // high-mid: blue
        default: { return vec3f(0.7, 0.8, 1.0); }   // air: ice-white
    }
}

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 13.71;
    let band = idx % NUM_BANDS;
    let arrangement = param(3u);
    let radius_param = param(1u);
    let spread_param = param(2u);
    let speed_mult = param(5u) * 0.8 + 0.2;
    let beat_pulse = param(6u);

    let energy = band_energy(band);

    // Emitter position on ring circumference
    let center = emitter_center(band, arrangement);
    let base_r = emitter_ring_radius(band, arrangement, radius_param);
    // Beat-phase breathing: modulate ring radius
    let breath = sin(u.beat_phase * TAU) * beat_pulse * 0.3;
    let ring_r = base_r * (1.0 + breath);

    // Golden-angle spacing: each particle within a band gets a unique,
    // evenly-distributed angle. Per-band offset (band * TAU/5) interleaves
    // bands in concentric mode so the combined rings have full coverage.
    let particle_in_band = idx / NUM_BANDS;
    let theta = f32(particle_in_band) * GOLDEN_ANGLE
              + f32(band) * TAU / f32(NUM_BANDS)
              + hash(seed_base) * 0.15;  // small jitter to break mechanical look
    let ring_offset = vec2f(cos(theta), sin(theta)) * ring_r;
    let pos = center + ring_offset;

    // Outward direction from ring center
    let outward = select(normalize(ring_offset), vec2f(cos(theta), sin(theta)), ring_r < 0.001);

    // Apply spread: rotate outward direction by random angle within cone
    let spread_angle = (hash(seed_base + 1.0) - 0.5) * spread_param * 2.0;
    let ca = cos(spread_angle);
    let sa = sin(spread_angle);
    let dir = vec2f(outward.x * ca - outward.y * sa, outward.x * sa + outward.y * ca);

    // Speed scaled by audio energy
    let speed = u.initial_speed * speed_mult * (0.3 + energy * 2.5)
              * (0.85 + hash(seed_base + 2.0) * 0.3);
    let vel = dir * speed;

    // Size: bass particles larger, air smaller
    var size_scale: f32;
    switch band {
        case 0u: { size_scale = 1.3; }
        case 1u: { size_scale = 1.1; }
        case 2u: { size_scale = 1.0; }
        case 3u: { size_scale = 0.85; }
        default: { size_scale = 0.7; }
    }
    let init_size = u.initial_size * size_scale * (0.8 + hash(seed_base + 3.0) * 0.4);

    // Color
    var col = band_color(band);
    let color_mode = param(4u);
    if color_mode >= 0.33 && color_mode < 0.66 {
        // Monochrome
        col = vec3f(0.6, 0.8, 1.0);
    } else if color_mode >= 0.66 {
        // Velocity-based
        col = vec3f(0.8, 0.8, 0.8);
    }

    let life_var = 1.0 + (hash(seed_base + 5.0) - 0.5) * 0.3;

    p.pos_life = vec4f(pos, init_size, 1.0);
    p.vel_size = vec4f(vel, f32(band), init_size);
    p.color = vec4f(col * 0.04, 0.15);
    p.flags = vec4f(hash(seed_base + 6.0) * 0.1, u.lifetime * life_var, 0.0, 0.0);
    return p;
}

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles { return; }

    var p = read_particle(idx);
    let life = p.pos_life.w;
    let age = p.flags.x;
    let max_life = p.flags.y;

    if life <= 0.0 {
        let slot = emit_claim();
        if slot < u.emit_count {
            p = emit_particle(idx);
            write_particle(idx, p);
            mark_alive(idx);
        } else {
            write_particle(idx, p);
        }
        return;
    }

    let new_age = age + u.delta_time;
    if new_age >= max_life {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    let life_frac = new_age / max_life;
    let dt = u.delta_time;
    var pos = p.pos_life.xy;
    var vel = p.vel_size.xy;
    let band = u32(p.vel_size.z);
    let init_size = p.pos_life.z;
    let energy = band_energy(band);

    // Continuous outward push from audio energy
    let center = emitter_center(band, param(3u));
    let from_center = pos - center;
    let dist = length(from_center);
    if dist > 0.001 {
        let push_dir = normalize(from_center);
        vel += push_dir * energy * 0.5 * dt;
    }

    // Drag
    vel *= pow(u.drag, dt * 60.0);

    // Beat pulse: velocity burst
    let beat_pulse = param(6u);
    if beat_pulse > 0.05 && u.beat > 0.5 {
        if dist > 0.001 {
            vel += normalize(from_center) * beat_pulse * 0.25;
        }
    }

    // Onset jitter
    if u.onset > 0.3 {
        let a = hash(f32(idx) * 7.13 + u.time) * TAU;
        vel += vec2f(cos(a), sin(a)) * u.onset * 0.03;
    }

    let prev_pos = pos;
    pos += vel * dt;

    // Obstacle collision
    let coll = apply_obstacle_collision(pos, vel, prev_pos);
    pos = coll.xy;
    vel = coll.zw;

    // Kill out of bounds
    if abs(pos.x) > 1.5 || abs(pos.y) > 1.5 {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    // Size + alpha
    let size = init_size * eval_size_curve(life_frac) * (0.8 + energy * 0.4);
    let fade_in = smoothstep(0.0, 0.05, life_frac);
    let fade_out = 1.0 - smoothstep(0.8, 1.0, life_frac);
    let alpha = 0.15 * fade_in * fade_out * eval_opacity_curve(life_frac);

    // Color
    var col = band_color(band);
    let color_mode = param(4u);
    if color_mode >= 0.33 && color_mode < 0.66 {
        col = vec3f(0.6, 0.8, 1.0);
    } else if color_mode >= 0.66 {
        let st = clamp(length(vel) * 3.0, 0.0, 1.0);
        col = mix(vec3f(0.3, 0.4, 0.8), vec3f(1.0, 0.7, 0.3), st);
    }
    col *= 0.03 + energy * 0.1;

    p.pos_life = vec4f(pos, init_size, 1.0);
    p.vel_size = vec4f(vel, f32(band), size);
    p.color = vec4f(col, alpha);
    p.flags.x = new_age;
    write_particle(idx, p);
    mark_alive(idx);
}
