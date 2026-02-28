// Flux particle simulation — particles follow 3D curl noise flow field.
// Audio-reactive: bass drives flow strength, beat syncs speed changes,
// onset triggers radial bursts. Screen emitter fills space with organic smoke.
// Structs, bindings, and helpers are in particle_lib.wgsl (auto-prepended).

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 13.37;

    // Screen emitter: random position across full screen
    let pos = rand_vec2(seed_base);

    // Small random initial velocity (flow field will take over)
    let angle = hash(seed_base + 2.0) * 6.2831853;
    let speed = u.initial_speed * (0.3 + 0.7 * hash(seed_base + 3.0));
    let vel = vec2f(cos(angle), sin(angle)) * speed;

    // Color: cool smoke tones, audio-shifted
    let hue = fract(hash(seed_base + 5.0) * 0.3 + 0.55 + u.centroid * 0.25);
    let r_c = abs(hue * 6.0 - 3.0) - 1.0;
    let g_c = 2.0 - abs(hue * 6.0 - 2.0);
    let b_c = 2.0 - abs(hue * 6.0 - 4.0);
    let brightness = 0.04 + u.rms * 0.03;
    let col = clamp(vec3f(r_c, g_c, b_c), vec3f(0.0), vec3f(1.0)) * brightness;

    // Stagger initial age
    let initial_age = hash(seed_base + 9.0) * u.lifetime * 0.3;

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, u.initial_size * (0.7 + hash(seed_base + 6.0) * 0.6));
    p.color = vec4f(col, 0.15 + hash(seed_base + 7.0) * 0.1);
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
    var vel = p.vel_size.xy;

    // --- Flow field: primary force ---
    // Sample curl noise and apply as velocity
    let flow_vel = sample_flow_field(p.pos_life.xy);
    // Audio modulation: bass increases flow strength
    let audio_flow_mult = 1.0 + u.bass * 0.8 + u.mid * 0.3;
    vel += flow_vel * audio_flow_mult * dt;

    // Beat: brief speed boost in flow direction
    if u.beat > 0.5 {
        vel += flow_vel * 0.5;
    }

    // Onset: radial push outward from center
    if u.onset > 0.3 {
        let dir = normalize(p.pos_life.xy + vec2f(0.001, 0.001));
        vel += dir * u.onset * 0.05 * dt;
    }

    // Gentle turbulence on top of flow
    let turb = phosphor_noise2(p.pos_life.xy * 5.0 + vec2f(u.time * 0.3, u.time * 0.25));
    let turb_angle = turb * 6.28318;
    vel += vec2f(cos(turb_angle), sin(turb_angle)) * 0.003 * dt;

    // Drag
    vel *= 1.0 - (1.0 - u.drag) * dt * 60.0;

    // Wrap particles that go off-screen (keeps density uniform)
    var new_pos = p.pos_life.xy + vel * dt;
    if new_pos.x > 1.1 { new_pos.x -= 2.2; }
    if new_pos.x < -1.1 { new_pos.x += 2.2; }
    if new_pos.y > 1.1 { new_pos.y -= 2.2; }
    if new_pos.y < -1.1 { new_pos.y += 2.2; }

    // Size: gentle shrink over life, audio reactive
    let base_size = mix(p.vel_size.w, u.size_end, life_frac * life_frac);
    let size = base_size * (1.0 + u.rms * 0.3);

    // Alpha: fade in, fade out, audio-reactive brightness
    let fade_in = smoothstep(0.0, 0.05, life_frac);
    let fade_out = 1.0 - smoothstep(0.7, 1.0, life_frac);
    let alpha = p.color.a * fade_in * fade_out;

    // Color: warm shift with age and audio
    var col = p.color.rgb;
    let warm_shift = life_frac * 0.3 + u.mid * 0.1;
    col = vec3f(col.r + warm_shift * 0.2, col.g, col.b - warm_shift * 0.1);
    col = clamp(col, vec3f(0.0), vec3f(1.0));

    p.pos_life = vec4f(new_pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = vec4f(col, alpha);
    p.flags.x = new_age;

    particles_out[idx] = p;
    mark_alive(idx);
}
