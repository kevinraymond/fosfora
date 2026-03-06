// Cascade particle simulation — audio-reactive walls from screen edges
//
// Pixel-perfect wall emission: one emitter per pixel along each edge.
// Particle index maps deterministically to an edge pixel position.
// Audio energy controls how fast/far each wall extends inward.
//
// Bottom = bass, Left = mid, Right = mid, Top = centroid/highs

// --- Param mapping ---
// param(0) = trail_decay    (bg shader)
// param(1) = inward_speed   (base velocity)
// param(2) = spread         (angular spread)
// param(3) = curl_strength  (turbulence)
// param(4) = color_mode     (palette)
// param(5) = edge_glow      (bg shader)
// param(6) = convergence    (audio push multiplier)
// param(7) = beat_sync      (beat pulse)

fn band_color(band: u32) -> vec3f {
    switch band {
        case 0u: { return vec3f(1.0, 0.4, 0.1); }   // bottom: warm orange
        case 1u: { return vec3f(0.1, 0.9, 0.6); }   // left: teal
        case 2u: { return vec3f(0.4, 0.3, 1.0); }   // right: violet
        default: { return vec3f(0.7, 0.9, 1.0); }   // top: cyan-white
    }
}

fn band_energy(band: u32) -> f32 {
    switch band {
        case 0u: { return max(u.bass + u.sub_bass * 0.5, 0.05); }
        case 1u: { return max(u.mid, 0.04); }
        case 2u: { return max(u.mid, 0.04); }
        default: { return max(u.centroid, 0.03); }
    }
}

fn band_inward(band: u32) -> vec2f {
    switch band {
        case 0u: { return vec2f(0.0, 1.0); }
        case 1u: { return vec2f(1.0, 0.0); }
        case 2u: { return vec2f(-1.0, 0.0); }
        default: { return vec2f(0.0, -1.0); }
    }
}

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 17.31;

    // --- Deterministic edge pixel assignment ---
    // Map particle index to a specific pixel along the screen perimeter.
    // This guarantees uniform, gap-free coverage: one emitter per pixel.
    let res_x = u32(u.resolution.x);
    let res_y = u32(u.resolution.y);
    let perimeter = res_x * 2u + res_y * 2u;
    let edge_pixel = idx % perimeter;

    var band: u32;
    var pos: vec2f;

    if edge_pixel < res_x {
        // Bottom edge: pixel 0..res_x-1
        band = 0u;
        let t = (f32(edge_pixel) + 0.5) / f32(res_x) * 2.0 - 1.0;
        pos = vec2f(t, -1.0);
    } else if edge_pixel < res_x + res_y {
        // Right edge: pixel 0..res_y-1
        band = 2u;
        let pixel = edge_pixel - res_x;
        let t = 1.0 - (f32(pixel) + 0.5) / f32(res_y) * 2.0;
        pos = vec2f(1.0, t);
    } else if edge_pixel < res_x * 2u + res_y {
        // Top edge: pixel 0..res_x-1
        band = 3u;
        let pixel = edge_pixel - res_x - res_y;
        let t = (f32(pixel) + 0.5) / f32(res_x) * 2.0 - 1.0;
        pos = vec2f(t, 1.0);
    } else {
        // Left edge: pixel 0..res_y-1
        band = 1u;
        let pixel = edge_pixel - res_x * 2u - res_y;
        let t = 1.0 - (f32(pixel) + 0.5) / f32(res_y) * 2.0;
        pos = vec2f(-1.0, t);
    }

    let energy = band_energy(band);

    // Velocity: perpendicular to edge, speed driven by audio
    let inward = band_inward(band);
    let spread_param = param(2u);
    let spread_angle = (hash(seed_base + 2.0) - 0.5) * spread_param * 1.6;
    let ca = cos(spread_angle);
    let sa = sin(spread_angle);
    let dir = vec2f(inward.x * ca - inward.y * sa, inward.x * sa + inward.y * ca);

    // Audio energy drives speed → controls wall depth
    let speed_base = param(1u) * 0.8 + 0.2;
    let speed = u.initial_speed * speed_base * (0.15 + energy * 2.0)
              * (0.85 + hash(seed_base + 3.0) * 0.3);
    let vel = dir * speed;

    // Size
    let init_size = u.initial_size * (0.8 + hash(seed_base + 4.0) * 0.4);

    // Color
    var col = band_color(band);
    let color_mode = param(4u);
    if color_mode >= 0.33 && color_mode < 0.66 {
        col = vec3f(0.6, 0.8, 1.0);
    } else if color_mode >= 0.66 {
        col = vec3f(0.8, 0.8, 0.8);
    }

    let life_var = 1.0 + (hash(seed_base + 7.0) - 0.5) * 0.3;

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

    // Continuous audio push
    let energy = band_energy(band);
    let push_dir = band_inward(band);
    let convergence = param(6u);
    vel += push_dir * energy * convergence * 1.0 * dt;

    // Gentle curl noise
    let curl_str = param(3u);
    if curl_str > 0.01 {
        let np = pos * 2.5 + vec2f(u.time * 0.2);
        vel += curl_noise_2d(np) * curl_str * 0.3 * dt;
    }

    // Drag
    vel *= pow(u.drag, dt * 60.0);

    // Beat pulse
    let beat_sync = param(7u);
    if beat_sync > 0.05 && u.beat > 0.5 {
        vel += push_dir * beat_sync * 0.3;
    }

    // Onset jitter
    if u.onset > 0.3 {
        let a = hash(f32(idx) * 7.13 + u.time) * 6.2831853;
        vel += vec2f(cos(a), sin(a)) * u.onset * 0.03;
    }

    let prev_pos = pos;
    pos += vel * dt;

    // Obstacle collision
    let coll = apply_obstacle_collision(pos, vel, prev_pos);
    pos = coll.xy;
    vel = coll.zw;

    // Kill out of bounds
    if abs(pos.x) > 1.4 || abs(pos.y) > 1.4 {
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
    col *= 0.04 + energy * 0.08;

    p.pos_life = vec4f(pos, init_size, 1.0);
    p.vel_size = vec4f(vel, f32(band), size);
    p.color = vec4f(col, alpha);
    p.flags.x = new_age;
    write_particle(idx, p);
    mark_alive(idx);
}
