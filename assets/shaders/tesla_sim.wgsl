// Tesla particle simulation v7 — magnetic field line tracer
//
// GOAL: Recreate the classic iron-filings-around-a-bar-magnet pattern.
// Particles trace magnetic field lines from source poles (+) to sink poles (-).
// Charge sign determines direction of travel along field lines.
// Near sink poles, particles are recycled (killed + re-emitted at source poles)
// rather than forced into orbital motion.
//
// ============================================================================
// DIAGNOSIS OF V6 BUGS (what was going wrong):
//
// 1. ORBITAL CORRECTION WAS THE #1 PROBLEM (old lines 209-217)
//    smoothstep(0.18, 0.03, dist_to_pole) blended particles into tangential
//    orbit within 0.18 units of ANY pole. This huge radius turned every pole
//    into a swirling vortex — the "counter-clockwise rotating clusters" you saw.
//    Iron filings don't orbit poles; they converge at them. FIX: Remove orbital
//    correction entirely. Instead, kill particles that reach a sink pole and
//    respawn them at a source pole — this traces complete field lines.
//
// 2. NO DRAG WAS APPLIED (your UI showed drag=0.998 but it was never used)
//    tesla_sim never called apply_builtin_forces() and never applied u.drag.
//    Without drag, particles accumulated kinetic energy from the helix
//    oscillation and onset jitter, drifting off field lines over time.
//    FIX: Apply drag each frame.
//
// 3. VELOCITY WAS SET, NOT INTEGRATED
//    Each frame, vel was overwritten to B_dir * charge * speed, ignoring the
//    particle's previous velocity entirely. This means the helix_vel added on
//    line 242 was the ONLY frame-to-frame continuity, creating jerky motion.
//    FIX: Use a soft steering approach — blend current velocity toward the
//    field direction rather than snapping to it. This gives smooth curves.
//
// 4. HELIX AMPLITUDE WAS TOO HIGH FOR IRON-FILING LOOK
//    The perpendicular oscillation scattered particles off field lines.
//    Iron filings align tightly along field lines with minimal lateral spread.
//    FIX: Reduce helix amplitude dramatically, make it cosmetic rather than
//    structural, and tie it to audio reactivity.
//
// 5. EMISSION ONLY FROM POLE VICINITY
//    Emitting only in rings around poles meant particles started in the
//    strongest field region and immediately got caught by the orbital
//    correction. FIX: Emit along a broader region between poles and also
//    at poles, so field lines are populated across their full length.
//
// 6. COLOR BLOBS FROM ADDITIVE BLENDING + HIGH ALPHA
//    Base alpha 0.35 + additive blending + trail decay 0.88 = bright blobs
//    that wash out individual particle trails. FIX: Lower base alpha,
//    let trails provide the visual density.
//
// ============================================================================
// Structs, bindings, and helpers are in particle_lib.wgsl (auto-prepended).

// --- Param mapping ---
// param(0) = trail_decay  (0.6–0.98, used by background shader)
// param(1) = field_strength (0–1, maps to force multiplier)
// param(2) = charge_ratio  (0–1, fraction of positive particles)
// param(3) = dipole_mode   (0–1, selects pole arrangement)
// param(4) = field_rotation (0–1, rate of pole rotation)
// param(5) = color_mode    (0–1, selects coloring scheme)
// param(6) = helix_tightness (0–1, perpendicular oscillation amount)
// param(7) = flip_sensitivity (0–1, beat-triggered charge flipping)

// ============================================================================
// DIPOLE POSITION HELPERS
// ============================================================================

fn get_dipole_positions(t: f32, mode: f32, rotation: f32, out_count: ptr<function, u32>) -> array<vec2f, 5> {
    var positions: array<vec2f, 5>;
    let angle = t * rotation * 0.5;
    let ca = cos(angle);
    let sa = sin(angle);

    if mode < 0.5 {
        // Two-pole (bar magnet)
        *out_count = 2u;
        positions[0] = vec2f(ca * 0.4, sa * 0.4);
        positions[1] = vec2f(-ca * 0.4, -sa * 0.4);
    } else if mode < 0.75 {
        // Quadrupole
        *out_count = 4u;
        for (var i = 0u; i < 4u; i++) {
            let a = angle + f32(i) * 1.5707963;
            positions[i] = vec2f(cos(a), sin(a)) * 0.35;
        }
    } else {
        // Quad + center
        *out_count = 5u;
        for (var i = 0u; i < 4u; i++) {
            let a = angle + f32(i) * 1.5707963;
            positions[i] = vec2f(cos(a), sin(a)) * 0.35;
        }
        positions[4] = vec2f(0.0);
    }

    return positions;
}

// Get the polarity sign for each pole.
// For antiparallel mode: pole 0 = +1 (North), pole 1 = -1 (South)
// This creates the classic bar magnet field pattern.
fn get_pole_sign(i: u32, mode: f32) -> f32 {
    if mode < 0.25 {
        // Parallel: all same sign (radial starburst)
        return 1.0;
    } else if mode < 0.5 {
        // Antiparallel: classic N-S bar magnet
        if i == 1u { return -1.0; }
        return 1.0;
    } else {
        // Ring/quad: alternating signs
        if i % 2u == 1u { return -1.0; }
        return 1.0;
    }
}

// ============================================================================
// MAGNETIC FIELD COMPUTATION
// ============================================================================

// Magnetic field at position p: superposition of monopole contributions.
// Each pole produces a radial field: B = sign * r_hat / |r|^2
// The superposition of a +pole and -pole gives classic dipole field lines
// that curve from North to South — exactly the iron filing pattern.
fn magnetic_field(p: vec2f, t: f32, mode: f32, rotation: f32) -> vec2f {
    var dcount: u32;
    let dipoles = get_dipole_positions(t, mode, rotation, &dcount);
    var B = vec2f(0.0);

    for (var i = 0u; i < dcount; i++) {
        let r = p - dipoles[i];
        let dist2 = dot(r, r) + 0.005; // softening to prevent singularity
        let dist = sqrt(dist2);
        let pole_sign = get_pole_sign(i, mode);

        // Monopole field: B = sign * r / |r|^3
        B += pole_sign * r / (dist2 * dist);
    }

    return B;
}

// ============================================================================
// POLE PROXIMITY ANALYSIS
// ============================================================================

// For a charged particle, find:
//  - nearest "source" pole (same sign as charge → field pushes away from it)
//  - nearest "sink" pole (opposite sign → field pulls toward it)
// Returns: vec4f(sink_dist, source_dist, sink_pole_idx, source_pole_idx)
fn analyze_poles(pos: vec2f, charge: f32, t: f32, mode: f32, rotation: f32) -> vec4f {
    var dcount: u32;
    let dipoles = get_dipole_positions(t, mode, rotation, &dcount);

    var sink_dist = 999.0;
    var source_dist = 999.0;
    var sink_idx = 0u;
    var source_idx = 0u;

    for (var i = 0u; i < dcount; i++) {
        let d = length(pos - dipoles[i]);
        let pole_sign = get_pole_sign(i, mode);

        // A positive charge follows +B direction (away from + pole, toward - pole)
        // So for a positive charge: + pole = source, - pole = sink
        // For a negative charge: - pole = source, + pole = sink
        let is_source = (pole_sign * charge) > 0.0;

        if is_source {
            if d < source_dist {
                source_dist = d;
                source_idx = i;
            }
        } else {
            if d < sink_dist {
                sink_dist = d;
                sink_idx = i;
            }
        }
    }

    return vec4f(sink_dist, source_dist, f32(sink_idx), f32(source_idx));
}

// ============================================================================
// EMISSION
// ============================================================================

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 17.31;
    let mode = param(3u);
    let rotation = param(4u);

    // Determine charge
    let charge_ratio = param(2u) + (u.centroid - 0.5) * 0.15;
    let is_positive = hash(seed_base + 3.0) < clamp(charge_ratio, 0.1, 0.9);
    let charge = select(-1.0, 1.0, is_positive);

    // Emission strategy: mix of pole-vicinity and mid-field emission
    // This populates field lines across their full length
    var dcount: u32;
    let dipoles = get_dipole_positions(u.time, mode, rotation, &dcount);

    let emit_mode = hash(seed_base + 20.0);
    var pos: vec2f;

    if emit_mode < 0.6 {
        // 60%: Emit near SOURCE poles (where field lines originate for this charge)
        // Positive charges: source = positive poles; Negative: source = negative poles
        var source_poles: array<u32, 5>;
        var n_sources = 0u;
        for (var i = 0u; i < dcount; i++) {
            let pole_sign = get_pole_sign(i, mode);
            if (pole_sign * charge) > 0.0 {
                source_poles[n_sources] = i;
                n_sources += 1u;
            }
        }
        // Fallback: if no source poles (parallel mode), use any pole
        if n_sources == 0u {
            n_sources = dcount;
            for (var i = 0u; i < dcount; i++) {
                source_poles[i] = i;
            }
        }
        let which = u32(hash(seed_base + 10.0) * f32(n_sources)) % n_sources;
        let dipole_pos = dipoles[source_poles[which]];

        // Emit in a ring around the source pole
        let emit_angle = hash(seed_base) * 6.2831853;
        let emit_r = 0.03 + hash(seed_base + 1.0) * 0.12;
        pos = dipole_pos + vec2f(cos(emit_angle), sin(emit_angle)) * emit_r;
    } else if emit_mode < 0.85 {
        // 25%: Emit along the axis between poles (mid-field)
        // This seeds the curved field lines in the middle region
        if mode < 0.5 && dcount >= 2u {
            let t_lerp = hash(seed_base + 11.0);
            let midpoint = mix(dipoles[0], dipoles[1], t_lerp);
            let perp_offset = (hash(seed_base + 12.0) - 0.5) * 0.4;
            let axis = normalize(dipoles[1] - dipoles[0]);
            let perp = vec2f(-axis.y, axis.x);
            pos = midpoint + perp * perp_offset;
        } else {
            // For quad modes, emit in a ring around origin
            let emit_angle = hash(seed_base) * 6.2831853;
            let emit_r = 0.1 + hash(seed_base + 1.0) * 0.3;
            pos = vec2f(cos(emit_angle), sin(emit_angle)) * emit_r;
        }
    } else {
        // 15%: Scattered emission (fills in sparse areas)
        pos = vec2f(
            (hash(seed_base + 13.0) - 0.5) * 1.6,
            (hash(seed_base + 14.0) - 0.5) * 1.6
        );
    }

    // Initial velocity: follow the field direction at spawn point
    let B = magnetic_field(pos, u.time, mode, rotation);
    let B_mag = length(B);
    let B_dir = B / (B_mag + 0.0001);
    let speed = u.initial_speed * (0.6 + 0.8 * hash(seed_base + 2.0));
    let vel = B_dir * charge * speed;

    // Color: cyan for positive, magenta for negative
    let color_mode = param(5u);
    var col: vec3f;
    if color_mode < 0.33 {
        col = select(vec3f(1.0, 0.3, 0.8), vec3f(0.3, 0.8, 1.0), is_positive);
    } else if color_mode < 0.66 {
        // Monochrome blue
        col = vec3f(0.4, 0.7, 1.0);
    } else {
        // Field-strength based (will be updated each frame)
        col = vec3f(0.3, 0.6, 1.0);
    }
    col *= 0.4 + u.rms * 0.15;

    let helix_phase = hash(seed_base + 8.0) * 6.2831853;
    let init_size = u.initial_size * (0.7 + hash(seed_base + 5.0) * 0.6);
    let life_var = 1.0 + (hash(seed_base + 6.0) - 0.5) * 0.4;

    p.pos_life = vec4f(pos, init_size, 1.0);
    p.vel_size = vec4f(vel, 0.0, init_size);
    p.color = vec4f(col, 0.2 + hash(seed_base + 7.0) * 0.12);
    // flags: x=age, y=max_life, z=charge (+1/-1), w=helix_phase
    p.flags = vec4f(0.0, u.lifetime * life_var, charge, helix_phase);
    return p;
}

// ============================================================================
// MAIN SIMULATION
// ============================================================================

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

    // --- Dead particle: try to emit ---
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

    // --- Age check ---
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
    var charge = p.flags.z;
    let helix_phase = p.flags.w;

    let field_str = param(1u);
    let mode = param(3u);
    let rotation = param(4u);
    let tightness = param(6u);

    // === BEAT POLARITY FLIP (do before field calculation) ===
    let flip_sens = param(7u);
    if u.beat > 0.5 && flip_sens > 0.05 {
        let flip_hash = hash(f32(idx) * 3.77 + floor(u.time * 4.0));
        if flip_hash < flip_sens * 0.5 {
            charge = -charge;
            p.flags.z = charge;
        }
    }

    // === MAGNETIC FIELD AT CURRENT POSITION ===
    let B = magnetic_field(pos, u.time, mode, rotation);
    let B_mag = length(B);
    let B_dir = B / (B_mag + 0.0001);

    // === FIELD-LINE STEERING ===
    // Instead of snapping velocity to field direction each frame (jerky),
    // we steer the current velocity toward the field direction.
    // This gives smooth, continuous curves that trace field lines.
    let target_speed = u.initial_speed * (0.3 + field_str * 1.2) * (1.0 + u.bass * 0.4);

    // Speed varies with field strength: faster near poles (prevents pileup),
    // moderate in mid-field (visible trails)
    let field_speed_mod = 0.5 + clamp(B_mag * 0.3, 0.0, 1.5);
    let target_vel = B_dir * charge * target_speed * field_speed_mod;

    // Steering blend: high value = tightly follow field lines (iron filing look)
    // Lower value = more inertial, swoopier motion
    let steer_rate = 8.0; // How quickly velocity aligns to field (units: 1/second)
    let blend = 1.0 - exp(-steer_rate * dt);
    vel = mix(vel, target_vel, blend);

    // === DRAG (critical — prevents energy buildup) ===
    vel *= pow(u.drag, dt * 60.0);

    // === HELIX: subtle perpendicular oscillation ===
    // Much smaller than v6 — cosmetic shimmer, not structural.
    // Audio-reactive: tighter helix on bass hits.
    let perp = vec2f(-B_dir.y, B_dir.x);
    let helix_freq = 8.0 + tightness * 20.0;
    let helix_base_amp = 0.002 + tightness * 0.015;
    let helix_amp = helix_base_amp * (1.0 + u.mid * 0.5);
    let helix_offset = perp * helix_amp * sin(new_age * helix_freq + helix_phase);

    // === ONSET JITTER (subtle field perturbation on transients) ===
    if u.onset > 0.3 {
        let jitter_angle = hash(f32(idx) * 7.13 + u.time) * 6.2831853;
        vel += vec2f(cos(jitter_angle), sin(jitter_angle)) * u.onset * 0.02;
    }

    // === INTEGRATE POSITION ===
    let prev_pos = pos;
    pos += vel * dt + helix_offset * dt * 60.0;

    // Obstacle collision
    let coll = apply_obstacle_collision(pos, vel, prev_pos);
    pos = coll.xy;
    vel = coll.zw;

    // === SINK POLE RECYCLING ===
    // When a particle reaches its sink pole, kill it so it gets re-emitted
    // at a source pole next frame. This traces complete field lines.
    let poles = analyze_poles(pos, charge, u.time, mode, rotation);
    let sink_dist = poles.x;
    let source_dist = poles.y;

    let kill_radius = 0.05; // How close to sink before recycling
    if sink_dist < kill_radius {
        // Particle reached its destination pole — recycle it
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    // === BOUNDARY: soft kill instead of wrap ===
    // Particles that leave the visible area die (iron filings don't wrap)
    let bounds = 1.4;
    if abs(pos.x) > bounds || abs(pos.y) > bounds {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    // === SIZE ===
    let init_size = p.pos_life.z;
    let size = init_size * eval_size_curve(life_frac) * (1.0 + u.rms * 0.2);

    // === ALPHA ===
    // Fade in quickly, fade out near death.
    // Dim near poles to soften the convergence points.
    let fade_in = smoothstep(0.0, 0.05, life_frac);
    let fade_out = 1.0 - smoothstep(0.85, 1.0, life_frac);
    let pole_dim = smoothstep(0.0, 0.12, min(sink_dist, source_dist));
    let alpha = p.color.a * fade_in * fade_out * eval_opacity_curve(life_frac) * (0.3 + 0.7 * pole_dim);

    // === COLOR ===
    var col: vec3f;
    let color_mode = param(5u);
    let spd = length(vel);

    if color_mode < 0.33 {
        // Charge-based: cyan (+) / magenta (-)
        if charge > 0.0 {
            col = vec3f(0.3, 0.8, 1.0);
        } else {
            col = vec3f(1.0, 0.3, 0.8);
        }
        col *= 0.4 + u.rms * 0.15;
    } else if color_mode < 0.66 {
        // Speed-based (shows field strength spatially)
        let speed_t = clamp(spd / (target_speed * 1.5), 0.0, 1.0);
        col = mix(vec3f(0.15, 0.3, 0.8), vec3f(0.8, 0.5, 0.15), speed_t) * 0.4;
    } else {
        // Field-strength based (bright near poles, dim between)
        let field_t = clamp(B_mag * 0.2, 0.0, 1.0);
        col = mix(vec3f(0.1, 0.2, 0.5), vec3f(0.5, 0.8, 1.0), field_t) * 0.4;
    }

    col *= 1.0 + spd * 0.05;
    col = clamp(col, vec3f(0.0), vec3f(1.0));

    // === WRITE OUTPUT ===
    p.pos_life = vec4f(pos, init_size, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = vec4f(col, alpha);
    p.flags.x = new_age;
    p.flags.z = charge; // preserve potential beat flip

    write_particle(idx, p);
    mark_alive(idx);
}
