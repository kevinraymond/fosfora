// Default particle simulation compute shader.
// Custom .pfx effects can override this via compute_shader field.
// Structs, bindings, and helpers are in particle_lib.wgsl (auto-prepended).

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
        case 5u: { // disc — uniform area distribution
            let angle = hash(seed_base) * 6.2831853;
            let r = sqrt(hash(seed_base + 1.0)) * u.emitter_radius;
            pos += vec2f(cos(angle), sin(angle)) * r;
        }
        case 6u: { // cone — position at emitter center (direction handled in velocity)
            pos += rand_vec2(seed_base) * 0.001;
        }
        default: { // point (0)
            pos += rand_vec2(seed_base) * 0.001;
        }
    }

    // Velocity direction
    var vel_dir: vec2f;
    if u.emitter_spread > 0.0 {
        // Cone emission: direction within angular spread
        let angle = u.emitter_angle + (hash(seed_base + 3.0) - 0.5) * 2.0 * u.emitter_spread;
        vel_dir = vec2f(cos(angle), sin(angle));
    } else {
        // Omnidirectional
        let angle = hash(seed_base + 3.0) * 6.2831853;
        vel_dir = vec2f(cos(angle), sin(angle));
    }

    // Speed with variance
    let speed_base = u.initial_speed;
    let speed_rand = 0.5 + 0.5 * hash(seed_base + 5.0);
    let speed = speed_base * mix(1.0, speed_rand, u.speed_variance);
    var vel = vel_dir * speed;

    // Emitter velocity inheritance
    if u.velocity_inherit > 0.0 {
        let emitter_vel = (u.emitter_pos - u.prev_emitter_pos) / max(u.delta_time, 0.001);
        vel += emitter_vel * u.velocity_inherit;
    }

    // Lifetime with variance
    let life = u.lifetime * mix(1.0, 0.5 + hash(seed_base + 9.0), u.life_variance);

    // Size with variance
    let size = u.initial_size * mix(1.0, 0.5 + hash(seed_base + 11.0), u.size_variance);

    // Color: audio-reactive hue or gradient start
    var col: vec3f;
    if u.gradient_count > 0u {
        col = eval_color_gradient(0.0).rgb;
    } else {
        let hue = hash(seed_base + 7.0) * 0.3 + u.centroid * 0.7;
        let r = abs(hue * 6.0 - 3.0) - 1.0;
        let g = 2.0 - abs(hue * 6.0 - 2.0);
        let b = 2.0 - abs(hue * 6.0 - 4.0);
        let brightness = 0.8 + u.rms * 0.5;
        col = clamp(vec3f(r, g, b), vec3f(0.0), vec3f(1.0)) * brightness;
    }

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = vec4f(col, 1.0);
    p.flags = vec4f(0.0, life, 0.0, 0.0);
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
        // Dead particle — try to claim an emission slot
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

    // Age the particle
    let new_age = age + u.delta_time;
    if new_age >= max_life {
        // Kill it
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    let life_frac = new_age / max_life; // 0..1
    let dt = u.delta_time;

    // Apply all builtin forces (gravity, wind, drag, noise, attraction, vortex, flow field)
    var vel = apply_builtin_forces(p.pos_life.xy, p.vel_size.xy, dt);

    // Integrate position
    var pos = p.pos_life.xy + vel * dt;

    // Ground bounce
    if u.ground_bounce > 0.0 {
        let r = apply_ground_bounce(pos, vel);
        pos = r.xy;
        vel = r.zw;
    }

    // Size: interpolation with optional curve multiplier
    let base_size = mix(p.vel_size.w, u.size_end, life_frac);
    let size = base_size * eval_size_curve(life_frac);

    // Opacity: fade near death with optional curve multiplier
    let base_alpha = 1.0 - smoothstep(0.7, 1.0, life_frac);
    let alpha = base_alpha * eval_opacity_curve(life_frac);

    // Color gradient over lifetime
    var col = p.color.rgb;
    if u.gradient_count > 0u {
        col = eval_color_gradient(life_frac).rgb;
    }

    // Spin: accumulate angle in pos_life.z
    var spin_angle = p.pos_life.z;
    if u.spin_speed != 0.0 {
        spin_angle += u.spin_speed * dt;
    }

    p.pos_life = vec4f(pos, spin_angle, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = vec4f(col, alpha);
    p.flags.x = new_age;

    write_particle(idx, p);
    mark_alive(idx);
}
