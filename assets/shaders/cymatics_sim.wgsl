// Cymatics particle simulation — Chladni pattern attraction.
// Particles are attracted to nodal lines of standing wave patterns:
// cos(nπx/L)cos(mπy/L) - cos(mπx/L)cos(nπy/L) = 0
// Audio frequency bands select mode numbers, creating evolving geometric patterns.
// Structs, bindings, and helpers are in particle_lib.wgsl (auto-prepended).

const PI: f32 = 3.14159265;

// Chladni function: returns signed distance-like value
// Zero crossings are the nodal lines
fn chladni(p: vec2f, n: f32, m: f32) -> f32 {
    return cos(n * PI * p.x) * cos(m * PI * p.y) - cos(m * PI * p.x) * cos(n * PI * p.y);
}

// Gradient of Chladni function via central differences
fn chladni_gradient(p: vec2f, n: f32, m: f32) -> vec2f {
    let eps = 0.005;
    let dx = chladni(p + vec2f(eps, 0.0), n, m) - chladni(p - vec2f(eps, 0.0), n, m);
    let dy = chladni(p + vec2f(0.0, eps), n, m) - chladni(p - vec2f(0.0, eps), n, m);
    return vec2f(dx, dy) / (2.0 * eps);
}

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 13.37;

    // Screen emitter
    let pos = rand_vec2(seed_base) * 0.9;

    // Small random velocity
    let angle = hash(seed_base + 2.0) * 6.2831853;
    let speed = u.initial_speed * (0.3 + 0.7 * hash(seed_base + 3.0));
    let vel = vec2f(cos(angle), sin(angle)) * speed;

    // Color: cool tones
    let hue = fract(0.55 + hash(seed_base + 5.0) * 0.2 + u.centroid * 0.2);
    let r_c = abs(hue * 6.0 - 3.0) - 1.0;
    let g_c = 2.0 - abs(hue * 6.0 - 2.0);
    let b_c = 2.0 - abs(hue * 6.0 - 4.0);
    let brightness = 0.03 + u.rms * 0.02;
    let col = clamp(vec3f(r_c, g_c, b_c), vec3f(0.0), vec3f(1.0)) * brightness;

    let initial_age = hash(seed_base + 9.0) * u.lifetime * 0.3;

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, u.initial_size * (0.7 + hash(seed_base + 6.0) * 0.6));
    p.color = vec4f(col, 0.12 + hash(seed_base + 7.0) * 0.08);
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
    let pos = p.pos_life.xy;

    // Mode numbers from audio bands
    // Bass selects lower modes (1-4), mid selects higher (3-7)
    let n = floor(1.0 + u.bass * 3.0 + 0.5);   // 1-4
    let m = floor(2.0 + u.mid * 5.0 + 0.5);     // 2-7

    // Gradient descent toward nodal lines
    let val = chladni(pos, n, m);
    let grad = chladni_gradient(pos, n, m);
    let grad_len = length(grad);

    // Attraction force: proportional to value (pushes toward zero crossing)
    // Negative gradient × value = toward nodal lines
    let attract_k = u.attraction_strength;
    if grad_len > 0.001 {
        vel -= normalize(grad) * val * attract_k * dt;
    }

    // Random walk component — prevents settling
    let turb = (hash(f32(idx) * 0.37 + u.time * 2.0) - 0.5) * 6.28318;
    vel += vec2f(cos(turb), sin(turb)) * 0.02 * dt;

    // Onset: scatter particles away from pattern
    if u.onset > 0.3 {
        let scatter_dir = normalize(pos + vec2f(0.001, 0.001));
        vel += scatter_dir * u.onset * 0.1 * dt;
    }

    // Beat: snap to nearest nodal line (reduce velocity, increase attraction)
    if u.beat > 0.5 {
        vel *= 0.5;
    }

    // Drag
    vel *= 1.0 - (1.0 - u.drag) * dt * 60.0;

    // Boundary: soft wrap
    var new_pos = pos + vel * dt;
    new_pos = clamp(new_pos, vec2f(-1.0), vec2f(1.0));

    // Size: particles on nodal lines are larger/brighter
    let on_line = exp(-abs(val) * 10.0);
    let base_size = mix(p.vel_size.w, u.size_end, life_frac * 0.3);
    let size = base_size * (0.8 + 0.5 * on_line) * (1.0 + u.rms * 0.2);

    // Alpha: brighter on nodal lines
    let fade_in = smoothstep(0.0, 0.05, life_frac);
    let fade_out = 1.0 - smoothstep(0.75, 1.0, life_frac);
    let alpha = p.color.a * fade_in * fade_out * (0.6 + 0.4 * on_line);

    // Color: shift based on distance from nodal line
    var col = p.color.rgb;
    col += vec3f(0.1, 0.05, -0.05) * on_line;
    col = clamp(col, vec3f(0.0), vec3f(1.0));

    p.pos_life = vec4f(new_pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = vec4f(col, alpha);
    p.flags.x = new_age;

    particles_out[idx] = p;
    mark_alive(idx);
}
