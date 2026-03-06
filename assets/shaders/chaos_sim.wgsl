// Chaos — strange attractor particle system with RK4 integration.
//
// Particles follow 5 strange attractor dynamics (Lorenz, Rössler, Halvorsen,
// Thomas, Chen) in normalized 3D space, projected to 2D with slow rotation.
// Audio-reactive bifurcation parameter crosses chaos boundaries for dramatic
// order→chaos transitions. Feedback trails accumulate the attractor shape.
//
// --- Param mapping ---
// param(0) = trail_decay    (bg shader)
// param(1) = attractor      (0→1 maps to type 0–4, morphed by centroid)
// param(2) = bifurcation    (0→1 maps per-attractor range)
// param(3) = dt_scale        (0→1 maps to 0.001–0.02)
// param(4) = spread          (noise perturbation strength)
// param(5) = color_mode      (4 modes: velocity/depth/wing/age)
// param(6) = projection      (perspective strength)
// param(7) = audio_drive     (how much audio affects bifurcation + morph)
//
// --- Particle field usage ---
// pos_life:  xy = screen position (projected), z = depth, w = life (1/0)
// vel_size:  xyz = 3D attractor state, w = display size
// color:     rgba
// flags:     x = age, y = max_lifetime, z = random seed, w = wing indicator

const TAU: f32 = 6.2831853;
const PI: f32 = 3.1415927;

// ============================================================
// Attractor derivatives — all work in normalized [-1,1] space
// ============================================================

// Lorenz attractor: σ=10, β=8/3, ρ=bifurcation (20→32)
// Native range: x,y ±20, z 5–45. Scale: 1/30, 1/30, 1/20 offset z-25
fn lorenz_deriv(p: vec3f, bifurc: f32, audio_mid: f32) -> vec3f {
    // Denormalize from [-1,1] to native space
    let x = p.x * 30.0;
    let y = p.y * 30.0;
    let z = p.z * 20.0 + 25.0;

    let sigma = 10.0 + audio_mid * 3.0;
    let beta = 8.0 / 3.0;
    let rho = mix(20.0, 32.0, bifurc);

    let dx = sigma * (y - x);
    let dy = x * (rho - z) - y;
    let dz = x * y - beta * z;

    // Renormalize derivatives
    return vec3f(dx / 30.0, dy / 30.0, dz / 20.0);
}

// Rössler attractor: a=0.2, b=0.2, c=bifurcation (3→8)
// Native range: x,y ±12, z 0–25. Scale: 1/12, 1/12, 1/12.5 offset z-12.5
fn rossler_deriv(p: vec3f, bifurc: f32, audio_mid: f32) -> vec3f {
    let x = p.x * 12.0;
    let y = p.y * 12.0;
    let z = p.z * 12.5 + 12.5;

    let a = 0.2 + audio_mid * 0.05;
    let b = 0.2;
    let c = mix(3.0, 8.0, bifurc);

    let dx = -y - z;
    let dy = x + a * y;
    let dz = b + z * (x - c);

    return vec3f(dx / 12.0, dy / 12.0, dz / 12.5);
}

// Halvorsen attractor: a=bifurcation (1.0→2.5)
// Native range: ±15 symmetric. Scale: 1/15 all axes
fn halvorsen_deriv(p: vec3f, bifurc: f32, audio_mid: f32) -> vec3f {
    let x = p.x * 15.0;
    let y = p.y * 15.0;
    let z = p.z * 15.0;

    let a = mix(1.0, 2.5, bifurc) + audio_mid * 0.2;

    let dx = -a * x - 4.0 * y - 4.0 * z - y * y;
    let dy = -a * y - 4.0 * z - 4.0 * x - z * z;
    let dz = -a * z - 4.0 * x - 4.0 * y - x * x;

    return vec3f(dx / 15.0, dy / 15.0, dz / 15.0);
}

// Thomas attractor: b=bifurcation (0.1→0.3)
// Native range: ±4 (small, slow). Scale: 1/4 all axes
fn thomas_deriv(p: vec3f, bifurc: f32, audio_mid: f32) -> vec3f {
    let x = p.x * 4.0;
    let y = p.y * 4.0;
    let z = p.z * 4.0;

    let b = mix(0.1, 0.3, bifurc) + audio_mid * 0.02;

    let dx = sin(y) - b * x;
    let dy = sin(z) - b * y;
    let dz = sin(x) - b * z;

    return vec3f(dx / 4.0, dy / 4.0, dz / 4.0);
}

// Chen attractor: a=35, b=3, c=bifurcation (20→35)
// Native range: x,y ±30, z 5–45. Scale: 1/30, 1/30, 1/20 offset z-25
fn chen_deriv(p: vec3f, bifurc: f32, audio_mid: f32) -> vec3f {
    let x = p.x * 30.0;
    let y = p.y * 30.0;
    let z = p.z * 20.0 + 25.0;

    let a = 35.0 + audio_mid * 3.0;
    let b = 3.0;
    let c = mix(20.0, 35.0, bifurc);

    let dx = a * (y - x);
    let dy = (c - a) * x - x * z + c * y;
    let dz = x * y - b * z;

    return vec3f(dx / 30.0, dy / 30.0, dz / 20.0);
}

// Dispatch to attractor by index (0–4), wrapping around
fn attractor_deriv(p: vec3f, kind: u32, bifurc: f32, audio_mid: f32) -> vec3f {
    switch (kind % 5u) {
        case 0u: { return lorenz_deriv(p, bifurc, audio_mid); }
        case 1u: { return rossler_deriv(p, bifurc, audio_mid); }
        case 2u: { return halvorsen_deriv(p, bifurc, audio_mid); }
        case 3u: { return thomas_deriv(p, bifurc, audio_mid); }
        case 4u: { return chen_deriv(p, bifurc, audio_mid); }
        default: { return lorenz_deriv(p, bifurc, audio_mid); }
    }
}

// ============================================================
// Morphing derivative — blend between adjacent attractor types
// ============================================================

fn morphed_deriv(p: vec3f, raw_type: f32, bifurc: f32, audio_mid: f32) -> vec3f {
    let clamped = clamp(raw_type, 0.0, 4.999);
    let type_a = u32(floor(clamped));
    let type_b = type_a + 1u;
    let morph_t = fract(clamped);

    let da = attractor_deriv(p, type_a, bifurc, audio_mid);
    let db = attractor_deriv(p, type_b, bifurc, audio_mid);
    return mix(da, db, morph_t);
}

// ============================================================
// RK4 integrator
// ============================================================

fn rk4_step(pos: vec3f, dt: f32, raw_type: f32, bifurc: f32, audio_mid: f32) -> vec3f {
    let k1 = morphed_deriv(pos, raw_type, bifurc, audio_mid);
    let k2 = morphed_deriv(pos + k1 * dt * 0.5, raw_type, bifurc, audio_mid);
    let k3 = morphed_deriv(pos + k2 * dt * 0.5, raw_type, bifurc, audio_mid);
    let k4 = morphed_deriv(pos + k3 * dt, raw_type, bifurc, audio_mid);
    return pos + (k1 + k2 * 2.0 + k3 * 2.0 + k4) * (dt / 6.0);
}

// ============================================================
// 3D → 2D projection with slow Y-axis rotation
// ============================================================

fn project(pos3d: vec3f, proj_strength: f32) -> vec3f {
    // Slow constant-speed Y-axis rotation (never audio-driven speed)
    let angle = u.time * 0.15;
    let ca = cos(angle);
    let sa = sin(angle);
    let rotated = vec3f(
        pos3d.x * ca + pos3d.z * sa,
        pos3d.y,
        -pos3d.x * sa + pos3d.z * ca
    );

    // Perspective: depth modulates scale
    let persp = mix(1.0, 2.0, proj_strength);
    let depth_scale = 1.0 / (1.0 + (rotated.z * 0.5 + 0.5) * (persp - 1.0));

    let screen_x = rotated.x * depth_scale;
    let screen_y = rotated.y * depth_scale;
    let screen_z = rotated.z;  // store depth for coloring

    return vec3f(screen_x, screen_y, screen_z);
}

// ============================================================
// Color modes
// ============================================================

fn velocity_color(speed: f32) -> vec3f {
    let t = clamp(speed * 8.0, 0.0, 1.0);
    let c0 = vec3f(0.05, 0.1, 0.5);   // slow: deep blue
    let c1 = vec3f(0.1, 0.5, 0.9);    // medium: cyan-blue
    let c2 = vec3f(0.9, 0.5, 0.1);    // fast: orange
    let c3 = vec3f(1.1, 0.8, 0.3);    // very fast: hot gold
    if t < 0.33 {
        return mix(c0, c1, t / 0.33);
    } else if t < 0.66 {
        return mix(c1, c2, (t - 0.33) / 0.33);
    }
    return mix(c2, c3, (t - 0.66) / 0.34);
}

fn depth_color(z: f32) -> vec3f {
    let t = clamp(z * 0.5 + 0.5, 0.0, 1.0);
    let cool = vec3f(0.1, 0.2, 0.7);   // near: cool blue
    let warm = vec3f(0.9, 0.4, 0.15);  // far: warm orange
    return mix(cool, warm, t);
}

fn wing_color(wing: f32) -> vec3f {
    // Lorenz wings: cyan vs magenta
    let cyan = vec3f(0.1, 0.7, 0.9);
    let magenta = vec3f(0.9, 0.2, 0.7);
    return select(cyan, magenta, wing > 0.0);
}

fn age_color(life_frac: f32) -> vec3f {
    let t = clamp(life_frac, 0.0, 1.0);
    let young = vec3f(0.9, 0.7, 0.3);  // bright warm
    let old = vec3f(0.1, 0.15, 0.5);   // dim cool
    return mix(young, old, t);
}

// ============================================================
// Emission
// ============================================================

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_val = u.seed + f32(idx) * 13.71;

    let h0 = hash(seed_val);
    let h1 = hash(seed_val + 1.0);
    let h2 = hash(seed_val + 2.0);
    let h3 = hash(seed_val + 3.0);
    let h4 = hash(seed_val + 4.0);

    // Initialize 3D position near attractor center with small offset
    let spread = param(4u) * 0.3 + 0.05;
    let init_x = (h0 - 0.5) * spread;
    let init_y = (h1 - 0.5) * spread;
    let init_z = (h2 - 0.5) * spread;

    // Project initial position for screen coordinates
    let proj = project(vec3f(init_x, init_y, init_z), param(6u));

    // Lifetime: 60s ± 40% variance for staggered renewal
    let life_var = 1.0 + (h3 - 0.5) * 0.8;

    let init_size = u.initial_size * (0.8 + h4 * 0.4);
    let col = vec3f(0.1, 0.15, 0.4) * 0.03;

    p.pos_life = vec4f(proj.xy, proj.z, 1.0);
    p.vel_size = vec4f(init_x, init_y, init_z, init_size);
    p.color = vec4f(col, 0.15);
    p.flags = vec4f(0.0, u.lifetime * life_var, h0, select(-1.0, 1.0, init_x > 0.0));
    return p;
}

// ============================================================
// Main compute shader
// ============================================================

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    let max_p = u.max_particles;
    if idx >= max_p {
        return;
    }

    var p = read_particle(idx);
    let is_alive = p.pos_life.w > 0.0;

    // --- Dead particle: attempt emission ---
    if !is_alive {
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

    // --- Alive particle: integrate attractor dynamics ---
    let dt_frame = u.delta_time;
    let age = p.flags.x;
    let max_life = p.flags.y;
    let seed_rand = p.flags.z;  // per-particle random, constant
    let audio_drive = param(7u);

    let new_age = age + dt_frame;
    if new_age >= max_life {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }
    let life_frac = new_age / max_life;

    // 3D attractor state lives in vel_size.xyz
    var att_pos = p.vel_size.xyz;

    // --- Attractor type (with morphing via centroid) ---
    let raw_type = param(1u) * 4.0 + u.centroid * audio_drive * 2.0;

    // --- Bifurcation (audio bass crosses chaos boundary) ---
    let bifurc_base = param(2u);
    let onset_push = u.onset * audio_drive * 0.3;
    let bass_shift = u.bass * audio_drive * 0.25;
    let bifurc = clamp(bifurc_base + bass_shift + onset_push, 0.0, 1.0);

    // --- Time step (brilliance speeds up trajectory) ---
    let dt_base = mix(0.001, 0.02, param(3u));
    let dt_mult = 1.0 + u.brilliance * audio_drive * 1.5;
    let dt_sim = dt_base * dt_mult;

    // --- RK4 integration ---
    att_pos = rk4_step(att_pos, dt_sim, raw_type, bifurc, u.mid * audio_drive);

    // --- Spread: per-particle noise perturbation ---
    let spread_str = param(4u) * 0.001;
    if spread_str > 0.0 {
        let noise_seed = vec2f(seed_rand * 100.0 + u.time * 0.3, f32(idx) * 0.017);
        let nx = phosphor_noise2(noise_seed) - 0.5;
        let ny = phosphor_noise2(noise_seed + vec2f(31.7, 0.0)) - 0.5;
        let nz = phosphor_noise2(noise_seed + vec2f(0.0, 47.3)) - 0.5;
        att_pos += vec3f(nx, ny, nz) * spread_str;
    }

    // --- Divergence guard ---
    let att_len = length(att_pos);
    if att_len != att_len || att_len > 1.5 {
        // NaN or diverged: reset near origin with random offset
        let rh0 = hash(f32(idx) * 7.13 + u.time * 10.0);
        let rh1 = hash(f32(idx) * 11.37 + u.time * 10.0);
        let rh2 = hash(f32(idx) * 3.91 + u.time * 10.0);
        att_pos = vec3f(
            (rh0 - 0.5) * 0.2,
            (rh1 - 0.5) * 0.2,
            (rh2 - 0.5) * 0.2
        );
    }
    // Soft clamp to normalized space
    att_pos = clamp(att_pos, vec3f(-1.2), vec3f(1.2));

    // --- Update wing indicator ---
    // Denormalize x for the current attractor to determine wing
    let wing = select(-1.0, 1.0, att_pos.x > 0.0);

    // --- 3D → 2D projection ---
    let proj = project(att_pos, param(6u));
    let screen_pos = proj.xy;
    let depth = proj.z;

    // --- Obstacle collision (in screen space) ---
    let prev_screen = p.pos_life.xy;
    let coll = apply_obstacle_collision(screen_pos, screen_pos - prev_screen, prev_screen);
    let final_pos = coll.xy;

    // --- Derivative for velocity coloring ---
    let deriv = morphed_deriv(att_pos, raw_type, bifurc, u.mid * audio_drive);
    let speed = length(deriv);

    // --- Size ---
    let init_size = u.initial_size * (0.8 + hash(seed_rand * 3.0) * 0.4);
    var size = init_size * eval_size_curve(life_frac);
    // Depth-based size (closer = bigger)
    let depth_scale = 1.0 / (1.0 + (depth * 0.5 + 0.5) * mix(0.0, 1.0, param(6u)));
    size *= 0.8 + depth_scale * 0.4;
    // RMS size pulsing
    size *= 1.0 + u.rms * 0.3;

    // --- Fade ---
    let fade_in = smoothstep(0.0, 0.03, life_frac);
    let fade_out = 1.0 - smoothstep(0.9, 1.0, life_frac);
    let alpha_base = fade_in * fade_out * eval_opacity_curve(life_frac);

    // --- Color ---
    let color_mode = param(5u);
    var col = vec3f(0.0);

    if color_mode < 0.25 {
        col = velocity_color(speed);
    } else if color_mode < 0.5 {
        col = depth_color(depth);
    } else if color_mode < 0.75 {
        col = wing_color(wing);
    } else {
        col = age_color(life_frac);
    }

    // Base brightness (additive particles, many overlap → keep dim per particle)
    col *= 0.04;

    // Audio brightness: RMS glow + warm-tinted onset flash
    col *= 1.0 + u.rms * 0.8;
    col += vec3f(0.8, 0.4, 0.1) * u.onset * 0.03;

    // Depth-based brightness (closer = brighter)
    col *= 0.7 + depth_scale * 0.5;

    let alpha = alpha_base * 0.20;

    // --- Write out ---
    p.pos_life = vec4f(final_pos, depth, 1.0);
    p.vel_size = vec4f(att_pos, size);
    p.color = vec4f(col, alpha);
    p.flags = vec4f(new_age, max_life, seed_rand, wing);
    write_particle(idx, p);
    mark_alive(idx);
}
