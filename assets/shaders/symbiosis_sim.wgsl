// Symbiosis — multi-species particle life simulation.
//
// Species interact via an asymmetric 8x8 force matrix stored in ParticleUniforms.
// CPU manages matrix state (presets, beat-shuffle, flux perturbation).
//
// --- Param mapping ---
// param(0) = num_species  (0-1 maps to 2-8)
// param(1) = force_scale  (interaction strength multiplier)
// param(2) = friction     (velocity damping)
// param(3) = max_radius   (outer interaction radius)
// param(4) = min_radius   (inner hard-repulsion radius)
// param(5) = color_mode   (0=species hue, 1=velocity-tinted)
// param(6) = preset       (force matrix preset index — managed by CPU)
// param(7) = audio_drive  (master audio reactivity scaling)
//
// --- Particle field usage ---
// pos_life:  xy = position [-1,1], z = unused, w = life (1/0)
// vel_size:  xy = velocity, z = unused, w = display size
// color:     rgba
// flags:     x = age, y = max_lifetime, z = species_id, w = unused

const PI: f32 = 3.1415927;
const TWO_PI: f32 = 6.2831853;
const MAX_PER_CELL: u32 = 48u;

// ============================================================
// Mapped params
// ============================================================

fn num_species() -> u32 { return u32(round(param(0u) * 6.0 + 2.0)); }
fn force_scale() -> f32 { return mix(0.1, 3.0, param(1u)); }
fn friction() -> f32 { return mix(0.02, 0.5, param(2u)); }
fn max_radius() -> f32 { return mix(0.02, 0.25, param(3u)); }
fn min_radius_frac() -> f32 { return mix(0.1, 0.5, param(4u)); }
fn color_mode() -> f32 { return param(5u); }
fn audio_drive() -> f32 { return param(7u); }

// ============================================================
// Toroidal wrapping helpers
// ============================================================

fn wrap_pos(p: vec2f) -> vec2f {
    var r = p;
    if r.x > 1.0 { r.x -= 2.0; }
    if r.x < -1.0 { r.x += 2.0; }
    if r.y > 1.0 { r.y -= 2.0; }
    if r.y < -1.0 { r.y += 2.0; }
    return r;
}

fn wrapped_diff(a: vec2f, b: vec2f) -> vec2f {
    var d = b - a;
    if d.x > 1.0 { d.x -= 2.0; }
    if d.x < -1.0 { d.x += 2.0; }
    if d.y > 1.0 { d.y -= 2.0; }
    if d.y < -1.0 { d.y += 2.0; }
    return d;
}

// ============================================================
// HSV to RGB
// ============================================================

fn hsv2rgb(h: f32, s: f32, v: f32) -> vec3f {
    let c = v * s;
    let hp = h * 6.0;
    let x = c * (1.0 - abs(hp % 2.0 - 1.0));
    var rgb: vec3f;
    if hp < 1.0 { rgb = vec3f(c, x, 0.0); }
    else if hp < 2.0 { rgb = vec3f(x, c, 0.0); }
    else if hp < 3.0 { rgb = vec3f(0.0, c, x); }
    else if hp < 4.0 { rgb = vec3f(0.0, x, c); }
    else if hp < 5.0 { rgb = vec3f(x, 0.0, c); }
    else { rgb = vec3f(c, 0.0, x); }
    return rgb + vec3f(v - c);
}

// ============================================================
// Emission
// ============================================================

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 13.37;

    // Disc emitter — uniform area sampling
    let r = sqrt(hash(seed_base)) * u.emitter_radius;
    let theta = hash(seed_base + 1.0) * TWO_PI;
    let pos = u.emitter_pos + vec2f(cos(theta), sin(theta)) * r;

    // Assign species
    let ns = num_species();
    let species = u32(hash(seed_base + 2.0) * f32(ns)) % ns;

    // Species color (hue rotated by dominant_chroma for pitch-class mapping)
    let hue = fract(f32(species) / f32(ns) + u.dominant_chroma);
    let col = hsv2rgb(hue, 0.85, 0.9);

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(0.0, 0.0, 0.0, u.initial_size);
    p.color = vec4f(col, 0.8);
    p.flags = vec4f(0.0, u.lifetime, f32(species), 0.0);
    return p;
}

// ============================================================
// Main simulation
// ============================================================

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles { return; }

    var p = read_particle(idx);
    let life = p.pos_life.w;
    let age = p.flags.x;
    let max_life = p.flags.y;

    // --- Dead/emit ---
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
    // Very long lifetime — don't kill particles in steady-state
    if new_age >= max_life {
        // Re-emit instead of dying
        p = emit_particle(idx);
        write_particle(idx, p);
        mark_alive(idx);
        return;
    }

    let dt = u.delta_time;
    let pos = p.pos_life.xy;
    var vel = p.vel_size.xy;
    let my_species = u32(p.flags.z);
    let ns = num_species();
    let drive = audio_drive();

    // Audio-modulated params
    let fscale = force_scale() * (1.0 + u.bass * drive * 1.5);
    let fric = friction() * (1.0 - u.mid * drive * 0.3);
    let r_max = max_radius() * (1.0 + (u.presence + u.brilliance) * 0.5 * drive * 0.3
                                  + u.flux * drive * 0.4);
    let r_min = r_max * min_radius_frac();

    // --- Neighbor iteration: dynamic-radius spatial hash scan ---
    // Cell size = 2.0 / grid_w; search enough cells to cover r_max
    let cell_size = 2.0 / f32(SH_GRID_W);
    let search_r = max(1, i32(ceil(r_max / cell_size)));
    let my_cell = sh_pos_to_cell(pos);
    var total_force = vec2f(0.0);

    for (var dy = -search_r; dy <= search_r; dy++) {
        for (var dx = -search_r; dx <= search_r; dx++) {
            // Toroidal cell wrapping
            var cx = my_cell.x + dx;
            var cy = my_cell.y + dy;
            if cx < 0 { cx += i32(SH_GRID_W); }
            if cx >= i32(SH_GRID_W) { cx -= i32(SH_GRID_W); }
            if cy < 0 { cy += i32(SH_GRID_H); }
            if cy >= i32(SH_GRID_H) { cy -= i32(SH_GRID_H); }

            let range = sh_cell_range(cx, cy);
            let start = range.x;
            let count = min(range.y, MAX_PER_CELL);

            for (var i = 0u; i < count; i++) {
                let ni = sh_sorted_indices[start + i];
                if ni == idx { continue; }

                let n_pl = pos_life_in[ni];
                if n_pl.w <= 0.0 { continue; }

                // Wrapped distance
                let diff = wrapped_diff(pos, n_pl.xy);
                let dist = length(diff);
                if dist < 0.0001 || dist > r_max { continue; }

                let dir = diff / dist;
                let other_species = u32(flags_in[ni].z);

                if dist < r_min {
                    // Universal hard repulsion (species-independent)
                    let repel = (1.0 - dist / r_min);
                    total_force -= dir * repel * fscale * 2.0;
                } else {
                    // Triangle force profile between min_radius and max_radius
                    let mid = (r_min + r_max) * 0.5;
                    var strength: f32;
                    if dist < mid {
                        strength = (dist - r_min) / (mid - r_min);
                    } else {
                        strength = (r_max - dist) / (r_max - mid);
                    }
                    var f = get_force(my_species % ns, other_species % ns);
                    // MFCC-driven force modulation: mfcc(4..6) modulate species 0..2's outgoing forces
                    // Raw MFCCs can be ~±50, so clamp to ±1 before applying as force multiplier
                    if my_species < 3u {
                        let mfcc_mod = clamp(mfcc(4u + my_species) * 0.05, -1.0, 1.0);
                        f *= (1.0 + mfcc_mod * drive);
                    }
                    total_force += dir * f * strength * fscale;
                }
            }
        }
    }

    // Integration
    vel += total_force * dt;

    // ZCR-driven Brownian noise: noisy signals = jittery particles
    let jitter_strength = u.zcr * drive * 0.04;
    let jitter = vec2f(
        hash(u.seed + f32(idx) * 3.7) * 2.0 - 1.0,
        hash(u.seed + f32(idx) * 7.3 + 1.0) * 2.0 - 1.0
    ) * jitter_strength;
    vel += jitter;

    // Friction damping
    vel *= pow(1.0 - fric, dt * 60.0);

    // Speed clamp to prevent runaway
    let spd = length(vel);
    let max_speed = 1.5;
    if spd > max_speed {
        vel = vel / spd * max_speed;
    }

    // Obstacle collision before wrapping
    let prev_pos = pos;
    var unwrapped_pos = pos + vel * dt;
    let coll = apply_obstacle_collision(unwrapped_pos, vel, prev_pos);
    unwrapped_pos = coll.xy;
    vel = coll.zw;
    let new_pos = wrap_pos(unwrapped_pos);

    // --- Color ---
    let hue = fract(f32(my_species) / f32(ns) + u.dominant_chroma);
    let cm = color_mode();
    let speed_brightness = clamp(spd * 3.0, 0.3, 1.0);
    let sat = mix(0.85, max(0.4, 0.85 - spd * 0.5), cm);
    let val = mix(0.9, speed_brightness, cm * 0.5);
    let col = hsv2rgb(hue, sat, val);
    let alpha = 0.7 + spd * 0.5;

    p.pos_life = vec4f(new_pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, u.initial_size);
    p.color = vec4f(col, clamp(alpha, 0.3, 1.0));
    p.flags = vec4f(new_age, max_life, f32(my_species), 0.0);

    write_particle(idx, p);
    mark_alive(idx);
}
