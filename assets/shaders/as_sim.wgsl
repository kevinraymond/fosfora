// Phosphor particle simulation — two particle "worms" tracing a P shape.
// Each strand is a thick tube of particles that twist around the P spine.
// Cross-section is a filled disc; depth modulates brightness for 3D illusion.
// Structs, bindings, and helpers are in particle_lib.wgsl (auto-prepended).

// ---- P-shaped spine path ----

fn eval_p(t: f32) -> vec2f {
    let stem_t = clamp(t / 0.38, 0.0, 1.0);
    let stem_bow = sin(stem_t * 3.14159) * 0.015;
    let stem_tail = (1.0 - stem_t) * (1.0 - stem_t) * (1.0 - stem_t) * 0.05;
    let stem_pos = vec2f(-0.14 + stem_bow - stem_tail, -0.37 + stem_t * 0.74);

    let bowl_s = clamp((t - 0.32) / 0.68, 0.0, 1.0);
    let angle = bowl_s * 3.14159;
    let sag = sin(angle) * sin(angle) * -0.08;
    let bowl_pos = vec2f(-0.14 + 0.38 * sin(angle), 0.15 + 0.22 * cos(angle) + sag);

    let blend = smoothstep(0.30, 0.40, t);
    return mix(stem_pos, bowl_pos, blend);
}

fn eval_p_tangent(t: f32) -> vec2f {
    let eps = 0.003;
    let a = eval_p(max(t - eps, 0.0));
    let b = eval_p(min(t + eps, 0.999));
    let d = b - a;
    let len = length(d);
    if len < 0.0001 {
        return vec2f(0.0, 1.0);
    }
    return d / len;
}

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 17.31;

    // Which worm? ~50/50 split (teal=0, amber=1)
    let curve_id = step(0.5, hash(seed_base + 99.0));
    let strand_phase = curve_id * 3.14159; // 0 or pi

    // Random position along P path
    let t = hash(seed_base);

    // Base spine position and tangent
    let base_pos = eval_p(t);
    let tangent = eval_p_tangent(t);
    let perp = vec2f(-tangent.y, tangent.x);

    // Scale
    let scale = u.emitter_radius / 0.30 * (1.0 + u.bass * 0.04);

    // Organic breathing
    let wobble = vec2f(
        sin(u.time * 0.3 + t * 8.0) * 0.010 + sin(u.time * 0.7 + t * 13.0) * 0.005,
        cos(u.time * 0.25 + t * 6.0) * 0.010 + cos(u.time * 0.6 + t * 11.0) * 0.005
    );

    // Helix: strand center orbits the spine
    let helix_angle = t * 3.0 * 6.28318 + u.time * 0.5 + strand_phase;
    let helix_offset = perp * cos(helix_angle) * 0.05;

    // Strand center in screen space
    let strand_center = base_pos * scale + wobble + helix_offset;

    // ---- Tube cross-section: filled disc around the strand center ----
    let tube_r = 0.012 + u.rms * 0.004; // tight tube
    let disc_angle = hash(seed_base + 2.0) * 6.28318;
    let disc_r = sqrt(hash(seed_base + 3.0)) * tube_r;

    // Screen-space offset (perpendicular to path)
    let tube_screen = perp * disc_r * cos(disc_angle);
    // Depth component (into screen) — modulates brightness
    let tube_depth = sin(disc_angle); // -1 to 1, normalized
    let depth_mod = 0.5 + 0.5 * tube_depth; // 0 (back) to 1 (front)

    // Sparkle: ~6% chance, scattered further out
    let is_sparkle = hash(seed_base + 5.0) > 0.94;
    var screen_pos = strand_center + tube_screen;
    if is_sparkle {
        let halo_r = 0.015 + hash(seed_base + 10.0) * 0.035;
        let halo_a = hash(seed_base + 11.0) * 6.28318;
        screen_pos += vec2f(cos(halo_a), sin(halo_a)) * halo_r;
    }

    let pos = to_clip(screen_pos);

    // Velocity: tangential flow only — particles travel inside the tube
    let flow_dir = select(1.0, -1.0, hash(seed_base + 7.0) > 0.5);
    let vel = to_clip(tangent * 0.025 * flow_dir * scale);

    // Size: depth-modulated (front particles larger)
    var size: f32;
    if is_sparkle {
        size = 0.008 + hash(seed_base + 6.0) * 0.010;
    } else {
        size = u.initial_size * (0.7 + hash(seed_base + 6.0) * 0.6) * (0.6 + 0.4 * depth_mod);
    }

    // Color: vivid, depth-modulated
    let brightness = (0.45 + u.rms * 0.25) * (0.45 + 0.55 * depth_mod);
    var col: vec3f;
    if curve_id < 0.5 {
        col = vec3f(0.12, 1.0, 0.65) * brightness;
    } else {
        col = vec3f(1.0, 0.75, 0.25) * brightness;
    }
    if is_sparkle {
        col = mix(col, vec3f(1.1, 1.05, 0.95), 0.3) * 1.2;
    }

    let base_alpha = select(0.55, 0.80, is_sparkle) * (0.45 + 0.55 * depth_mod);
    let initial_age = hash(seed_base + 9.0) * u.lifetime * 0.08;

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = vec4f(col, base_alpha);
    p.flags = vec4f(initial_age, u.lifetime, curve_id, base_alpha);

    return p;
}

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles {
        return;
    }

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
    var vel = p.vel_size.xy;

    // Gentle downward gravity
    vel += vec2f(0.0, -0.004) * dt;

    // Turbulence
    let turb = phosphor_noise2(p.pos_life.xy * 4.0 + vec2f(u.time * 0.4, u.time * 0.3));
    let turb_angle = turb * 6.28318;
    vel += vec2f(cos(turb_angle), sin(turb_angle)) * u.turbulence * 0.001 * dt;

    // Audio: onset gives a gentle push
    if u.onset > 0.3 {
        let dir = normalize(p.pos_life.xy + vec2f(0.001, 0.001));
        vel += dir * u.onset * 0.006 * dt;
    }

    // Beat: inward contraction pulse
    if u.beat > 0.5 {
        vel -= p.pos_life.xy * 0.06 * dt;
    }

    // Drag
    vel *= 1.0 - (1.0 - u.drag) * dt * 60.0;

    let prev_pos = p.pos_life.xy;
    var new_pos = p.pos_life.xy + vel * dt;

    // Obstacle collision
    let coll = apply_obstacle_collision(new_pos, vel, prev_pos);
    new_pos = coll.xy;
    vel = coll.zw;

    let size = mix(p.vel_size.w, u.size_end, life_frac * life_frac);
    let fade = 1.0 - smoothstep(0.6, 1.0, life_frac);
    let alpha = p.flags.w * fade;

    p.pos_life = vec4f(new_pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color.a = alpha;
    p.flags.x = new_age;

    write_particle(idx, p);
    mark_alive(idx);
}
