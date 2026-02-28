// Chaos particle simulation — Lorenz and Rossler strange attractors.
// Particles trace attractor trajectories using RK4 integration.
// pos_life.z stores the actual Z coordinate for 3D depth.
// vel_size.z stores z-velocity. Projected to 2D for rendering.
// Audio-reactive attractor parameters create dramatic bifurcation transitions.
// Structs, bindings, and helpers are in particle_lib.wgsl (auto-prepended).

// Lorenz attractor: dx/dt = sigma*(y-x), dy/dt = x*(rho-z)-y, dz/dt = x*y - beta*z
fn lorenz(p: vec3f, sigma: f32, rho: f32, beta: f32) -> vec3f {
    return vec3f(
        sigma * (p.y - p.x),
        p.x * (rho - p.z) - p.y,
        p.x * p.y - beta * p.z
    );
}

// Rossler attractor: dx/dt = -y-z, dy/dt = x + a*y, dz/dt = b + z*(x-c)
fn rossler(p: vec3f, a: f32, b: f32, c: f32) -> vec3f {
    return vec3f(
        -p.y - p.z,
        p.x + a * p.y,
        b + p.z * (p.x - c)
    );
}

// RK4 integration step
fn rk4_lorenz(p: vec3f, dt: f32, sigma: f32, rho: f32, beta: f32) -> vec3f {
    let k1 = lorenz(p, sigma, rho, beta);
    let k2 = lorenz(p + k1 * dt * 0.5, sigma, rho, beta);
    let k3 = lorenz(p + k2 * dt * 0.5, sigma, rho, beta);
    let k4 = lorenz(p + k3 * dt, sigma, rho, beta);
    return p + (k1 + k2 * 2.0 + k3 * 2.0 + k4) * dt / 6.0;
}

fn rk4_rossler(p: vec3f, dt: f32, a: f32, b: f32, c: f32) -> vec3f {
    let k1 = rossler(p, a, b, c);
    let k2 = rossler(p + k1 * dt * 0.5, a, b, c);
    let k3 = rossler(p + k2 * dt * 0.5, a, b, c);
    let k4 = rossler(p + k3 * dt, a, b, c);
    return p + (k1 + k2 * 2.0 + k3 * 2.0 + k4) * dt / 6.0;
}

// Simple perspective projection: 3D → 2D screen space
fn project(p: vec3f, zoom: f32) -> vec2f {
    let depth_factor = 1.0 / (1.0 + p.z * 0.02);
    return vec2f(p.x, p.y) * zoom * depth_factor;
}

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 13.37;

    // Start near the attractor's stable region with small random offset
    // Lorenz: typical values around (1, 1, 25)
    let px = (hash(seed_base) - 0.5) * 30.0 + 1.0;
    let py = (hash(seed_base + 1.0) - 0.5) * 30.0 + 1.0;
    let pz = hash(seed_base + 2.0) * 40.0 + 5.0;

    // Color: position-based with audio shift
    let hue = fract(hash(seed_base + 5.0) * 0.4 + 0.1 + u.centroid * 0.3);
    let r_c = abs(hue * 6.0 - 3.0) - 1.0;
    let g_c = 2.0 - abs(hue * 6.0 - 2.0);
    let b_c = 2.0 - abs(hue * 6.0 - 4.0);
    let brightness = 0.03 + u.rms * 0.02;
    let col = clamp(vec3f(r_c, g_c, b_c), vec3f(0.0), vec3f(1.0)) * brightness;

    // Store 3D position: xy in pos_life, z in pos_life.z (was reserved)
    // Initial velocity is zero (attractor dynamics drive motion)
    let zoom_param = 0.3 + u.emitter_radius * 0.7; // controlled by zoom param
    let projected = project(vec3f(px, py, pz), zoom_param * 0.03);

    let initial_age = hash(seed_base + 9.0) * u.lifetime * 0.1;

    p.pos_life = vec4f(projected, 0.0, 1.0);
    p.vel_size = vec4f(0.0, 0.0, 0.0, u.initial_size * (0.6 + hash(seed_base + 6.0) * 0.8));
    p.color = vec4f(col, 0.12 + hash(seed_base + 7.0) * 0.08);
    // flags: x=age, y=lifetime, z=3D_x (packed), w=3D_y (packed)
    // We store 3D coords in flags.z/w and use vel_size.z for 3D_z
    p.flags = vec4f(initial_age, u.lifetime, px, py);
    // Store pz in vel_size.z (was reserved)
    p.vel_size.z = pz;
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

    // Recover 3D position from flags.z/w and vel_size.z
    var pos3d = vec3f(p.flags.z, p.flags.w, p.vel_size.z);

    // Attractor parameters — audio reactive
    // Lorenz defaults: sigma=10, rho=28, beta=8/3
    // bass drives rho (bifurcation: chaotic at rho>24.74, periodic at lower values)
    let sigma = 10.0 + u.centroid * 5.0;
    let rho = 20.0 + u.bass * 15.0 + u.mid * 5.0; // 20-40 range, chaotic transition
    let beta = 2.667 + u.mid * 1.0;

    // Rossler defaults: a=0.2, b=0.2, c=5.7
    let rossler_a = 0.2 + u.centroid * 0.1;
    let rossler_b = 0.2;
    let rossler_c = 5.7 + u.bass * 3.0;

    // Attractor mix: 0=Lorenz, 1=Rossler
    let mix_param = u.attraction_strength; // repurpose attraction_strength as attractor_mix

    // Integration timestep (scaled by speed param and dt)
    let speed = 0.5 + u.initial_speed * 2.0;
    let sim_dt = u.delta_time * speed * 0.5;

    // RK4 step for each attractor
    let lorenz_pos = rk4_lorenz(pos3d, sim_dt, sigma, rho, beta);
    let rossler_pos = rk4_rossler(pos3d * 0.15, sim_dt, rossler_a, rossler_b, rossler_c) / 0.15;

    // Blend between attractors
    pos3d = mix(lorenz_pos, rossler_pos, mix_param);

    // Onset perturbation
    if u.onset > 0.3 {
        let kick = vec3f(
            (hash(f32(idx) * 0.1 + u.time) - 0.5) * 5.0,
            (hash(f32(idx) * 0.2 + u.time) - 0.5) * 5.0,
            (hash(f32(idx) * 0.3 + u.time) - 0.5) * 5.0
        );
        pos3d += kick * u.onset * 0.3;
    }

    // Clamp to prevent divergence
    pos3d = clamp(pos3d, vec3f(-80.0), vec3f(80.0));

    // Project to 2D with slow rotation
    let zoom = 0.02 + u.emitter_radius * 0.02;
    let rot_speed = 0.15 + u.mid * 0.1;
    let rot_angle = u.time * rot_speed;
    let ca = cos(rot_angle);
    let sa = sin(rot_angle);
    // Rotate around Y axis for dynamic viewing angle
    let rotated = vec3f(
        pos3d.x * ca + pos3d.z * sa,
        pos3d.y,
        -pos3d.x * sa + pos3d.z * ca
    );
    let projected = project(rotated, zoom);

    // Depth-based size and brightness
    let depth_norm = clamp((rotated.z + 40.0) / 80.0, 0.0, 1.0);
    let depth_mod = 0.5 + 0.5 * depth_norm; // front is brighter

    let base_size = mix(p.vel_size.w, u.size_end, life_frac * 0.5);
    let size = base_size * depth_mod * (1.0 + u.rms * 0.3);

    let fade_in = smoothstep(0.0, 0.05, life_frac);
    let fade_out = 1.0 - smoothstep(0.8, 1.0, life_frac);
    let alpha = p.color.a * fade_in * fade_out * depth_mod;

    // Color: warm shift over lifetime, audio hue shift
    var col = p.color.rgb;
    let warm = life_frac * 0.15 + u.centroid * 0.1;
    col = clamp(vec3f(col.r + warm * 0.2, col.g - warm * 0.05, col.b - warm * 0.1), vec3f(0.0), vec3f(1.0));

    // Store 3D position back to flags.z/w and vel_size.z
    p.flags = vec4f(new_age, max_life, pos3d.x, pos3d.y);
    p.vel_size = vec4f(p.vel_size.xy, pos3d.z, size);
    p.pos_life = vec4f(projected, 0.0, 1.0);
    p.color = vec4f(col, alpha);

    particles_out[idx] = p;
    mark_alive(idx);
}
