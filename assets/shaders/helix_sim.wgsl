// Helix particle simulation — Lorentz force electromagnetic field.
// F = q(E + v × B) produces helical spiraling trajectories.
// In 2D+: F_x = q * (E_x + v_y * B_z), F_y = q * (E_y - v_x * B_z)
// Mixed positive/negative charges create interweaving spirals.
// Structs, bindings, and helpers are in particle_lib.wgsl (auto-prepended).

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 13.37;

    // Line emitter at bottom
    let t = hash(seed_base) * 2.0 - 1.0;
    let pos = u.emitter_pos + vec2f(t * u.emitter_radius, 0.0);

    // Initial velocity: mostly upward with some spread
    let spread = 0.1 + hash(seed_base + 1.0) * 0.2;
    let angle = 1.5708 + (hash(seed_base + 2.0) - 0.5) * spread; // ~90 degrees (up) ± spread
    let speed = u.initial_speed * (0.7 + 0.6 * hash(seed_base + 3.0));
    let vel = vec2f(cos(angle), sin(angle)) * speed;

    // Charge: 50/50 positive/negative, stored in flags.z
    let charge = select(-1.0, 1.0, hash(seed_base + 4.0) > 0.5);
    // Mass variation: stored in flags.w (affects spiral radius)
    let mass = 0.5 + hash(seed_base + 5.0) * 1.0;

    // Color: positive = blue/cyan, negative = red/orange
    var col: vec3f;
    let color_mode = hash(seed_base + 8.0);
    if charge > 0.0 {
        let hue = 0.55 + color_mode * 0.15; // blue-cyan range
        let r_c = abs(hue * 6.0 - 3.0) - 1.0;
        let g_c = 2.0 - abs(hue * 6.0 - 2.0);
        let b_c = 2.0 - abs(hue * 6.0 - 4.0);
        col = clamp(vec3f(r_c, g_c, b_c), vec3f(0.0), vec3f(1.0));
    } else {
        let hue = 0.0 + color_mode * 0.1; // red-orange range
        let r_c = abs(hue * 6.0 - 3.0) - 1.0;
        let g_c = 2.0 - abs(hue * 6.0 - 2.0);
        let b_c = 2.0 - abs(hue * 6.0 - 4.0);
        col = clamp(vec3f(r_c, g_c, b_c), vec3f(0.0), vec3f(1.0));
    }
    let brightness = 0.05 + u.rms * 0.04;
    col *= brightness;

    let initial_age = hash(seed_base + 9.0) * u.lifetime * 0.05;

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, u.initial_size * (0.6 + hash(seed_base + 6.0) * 0.8));
    p.color = vec4f(col, 0.2 + hash(seed_base + 7.0) * 0.1);
    p.flags = vec4f(initial_age, u.lifetime, charge, mass);
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
        let slot = emit_claim();
        if slot < u.emit_count {
            p = emit_particle(idx);
            particles_out[idx] = p;
            mark_alive(idx);
        } else {
            particles_out[idx] = p;
        }
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
    let charge = p.flags.z;
    let mass = p.flags.w;
    var vel = p.vel_size.xy;
    var pos = p.pos_life.xy;

    // Magnetic field B_z — perpendicular to screen, audio-reactive
    // Higher bass = tighter spirals
    let b_z = (3.0 + u.bass * 8.0) * (1.0 + u.mid * 0.3);

    // Electric field E — primarily upward with beat-driven radial component
    var e_field = vec2f(0.0, 0.05); // constant upward drift
    if u.beat > 0.5 {
        // Beat pulse: radial E field (outward)
        let dir = normalize(pos + vec2f(0.001, 0.001));
        e_field += dir * 0.3;
    }
    // Onset: lateral perturbation
    if u.onset > 0.3 {
        e_field += vec2f(
            (hash(f32(idx) * 0.1 + u.time) - 0.5) * u.onset * 0.4,
            0.0
        );
    }

    // Lorentz force: F = q(E + v × B)
    // In 2D with B along z: v × B = (v_y * B_z, -v_x * B_z)
    let lorentz_x = charge * (e_field.x + vel.y * b_z) / mass;
    let lorentz_y = charge * (e_field.y - vel.x * b_z) / mass;

    vel += vec2f(lorentz_x, lorentz_y) * dt;

    // Light drag
    vel *= 1.0 - (1.0 - u.drag) * dt * 60.0;

    // Integrate position
    let new_pos = pos + vel * dt;

    // Kill if off screen (top, sides)
    if new_pos.y > 1.3 || abs(new_pos.x) > 1.3 {
        p.pos_life.w = 0.0;
        particles_out[idx] = p;
        return;
    }

    // Size: depth/height modulated
    let height_frac = (new_pos.y + 1.0) * 0.5; // 0 at bottom, 1 at top
    let base_size = mix(p.vel_size.w, u.size_end, life_frac * 0.5);
    let size = base_size * (1.0 + u.rms * 0.2) * (0.7 + 0.3 * height_frac);

    // Alpha: fade at death, height-based brightening
    let fade_in = smoothstep(0.0, 0.05, life_frac);
    let fade_out = 1.0 - smoothstep(0.7, 1.0, life_frac);
    let alpha = p.color.a * fade_in * fade_out;

    // Color: subtle charge-dependent evolution
    var col = p.color.rgb;
    if charge > 0.0 {
        col += vec3f(-0.02, 0.01, 0.03) * life_frac * 0.5;
    } else {
        col += vec3f(0.03, -0.01, -0.02) * life_frac * 0.5;
    }
    col = clamp(col, vec3f(0.0), vec3f(1.0));

    p.pos_life = vec4f(new_pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = vec4f(col, alpha);
    p.flags.x = new_age;

    particles_out[idx] = p;
    mark_alive(idx);
}
