// Murmur — research-accurate starling murmuration particle system.
//
// Upgrades from basic Reynolds Boids to three features that distinguish
// real murmurations: topological K=7 nearest neighbors (scale-free
// correlations), Vicsek noise-driven order-chaos phase transitions,
// and predator avoidance (flock splitting on audio onset/kick).
//
// --- Param mapping ---
// param(0) = noise_eta     (Vicsek noise baseline: 0=order, 1=chaos)
// param(1) = cohesion      (cohesion weight)
// param(2) = color_mode    (0=silhouette, 1=rim lighting — shared with murmur_bg.wgsl)
// param(3) = predator      (predator avoidance strength)
// param(4) = separation    (separation weight)
// param(5) = speed         (base flock cruise speed)
// param(6) = smoothing     (heading low-pass filter)
// param(7) = audio_drive   (master audio reactivity scaling)
//
// --- Particle field usage ---
// pos_life:  xy = position [-1,1], z = initial size, w = life (1/0)
// vel_size:  xy = velocity, z = unused, w = display size
// color:     rgba
// flags:     x = age, y = max_lifetime, z = heading angle, w = unused

const PI: f32 = 3.1415927;
const K: u32 = 7u;
const MAX_PER_CELL: u32 = 24u;  // ~16 particles/cell with scalable grid; 24 covers full cell + margin
const PREDATOR_RADIUS: f32 = 0.28;

// ============================================================
// Mapped params — all audio-modulated in cs_main
// ============================================================

fn noise_eta_base() -> f32 { return param(0u); }
// Alignment ~1.0 (normalized), cohesion ~0.3, separation ~1.0
// Adaptive topological separation prevents collapse at higher cohesion
fn cohesion_weight() -> f32 { return mix(0.1, 0.6, param(1u)); }
fn predator_strength() -> f32 { return mix(0.0, 5.0, param(3u)); }
fn separation_weight() -> f32 { return mix(0.3, 1.8, param(4u)); }
fn cruise_speed() -> f32 { return mix(0.04, 0.25, param(5u)); }
fn heading_smoothing() -> f32 { return mix(0.05, 0.35, param(6u)); }
fn audio_drive() -> f32 { return param(7u); }

// ============================================================
// Predator position — deterministic from uniforms, no Rust changes
// ============================================================

fn predator_pos() -> vec2f {
    // Slow Lissajous drift
    let liss = vec2f(
        sin(u.time * 0.17) * 0.5,
        cos(u.time * 0.23) * 0.35
    );
    // Onset-triggered jump to new strike zone
    let strike_seed = floor(u.time * 0.5 + u.onset * 3.0);
    let strike = vec2f(
        hash(strike_seed * 7.13) * 1.4 - 0.7,
        hash(strike_seed * 11.31) * 1.0 - 0.5
    );
    // Blend: onset pushes toward strike zone, decays back to Lissajous
    let onset_hold = smoothstep(0.0, 0.4, u.onset) * 0.7 + smoothstep(0.0, 0.3, u.kick) * 0.3;
    return mix(liss, strike, onset_hold);
}

fn predator_intensity() -> f32 {
    // mfcc(3u) = formant structure: vocal-like sounds make predator more aggressive
    let formant_boost = 1.0 + max(mfcc(3u), 0.0) * 1.5;
    return max(u.onset * 2.0, u.kick * 1.5) * formant_boost;
}

// ============================================================
// Emission
// ============================================================

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 13.37;

    // Disk emitter fallback (sqrt for uniform area coverage)
    let r = sqrt(hash(seed_base)) * u.emitter_radius;
    let theta = hash(seed_base + 1.0) * 6.2831853;

    // Try to emit near an existing alive particle (distributes into flock)
    // Probe random cells across the whole screen, not just near emitter
    let probe_pos = rand_vec2(seed_base + 10.0) * 0.9; // [-0.9, 0.9]
    let probe_cell = sh_pos_to_cell(probe_pos);
    let probe_range = sh_cell_range(probe_cell.x, probe_cell.y);
    var spawn_pos = u.emitter_pos + vec2f(cos(theta), sin(theta)) * r; // fallback
    var donor_heading = hash(seed_base + 2.0) * 6.2831853; // random fallback heading

    if probe_range.y > 0u {
        let pick = u32(hash(seed_base + 12.0) * f32(probe_range.y)) % probe_range.y;
        let donor = sh_sorted_indices[probe_range.x + pick];
        let donor_pl = pos_life_in[donor];
        if donor_pl.w > 0.0 {
            // Spawn near the donor with small random offset
            let jitter = rand_vec2(seed_base + 13.0) * 0.02;
            spawn_pos = donor_pl.xy + jitter;
            // Inherit donor's heading so new bird flies with the flock immediately
            donor_heading = flags_in[donor].z;
        }
    }
    let pos = spawn_pos;

    // Heading: inherited from donor (or random if no donor found)
    let angle = donor_heading + (hash(seed_base + 14.0) - 0.5) * 0.3; // small jitter
    let speed = cruise_speed() * (0.8 + 0.4 * hash(seed_base + 3.0));
    let vel = vec2f(cos(angle), sin(angle)) * speed;

    // Dark bird silhouette colors via gradient or default
    var col: vec3f;
    if u.gradient_count > 0u {
        let t = hash(seed_base + 5.0);
        let grad = eval_color_gradient(t);
        col = grad.rgb;
    } else {
        let tone = 0.04 + hash(seed_base + 5.0) * 0.03;
        col = vec3f(tone);
    }

    let initial_age = hash(seed_base + 9.0) * u.lifetime * 0.5;
    let init_size = u.initial_size * (0.7 + hash(seed_base + 6.0) * 0.6);

    p.pos_life = vec4f(pos, init_size, 1.0);
    p.vel_size = vec4f(vel, 0.0, init_size);
    p.color = vec4f(col, 0.0);  // invisible on emit frame; spawn_fade ramps in next frame
    p.flags = vec4f(initial_age, u.lifetime, angle, u.time);  // w = birth time
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

    // --- Dead/emit preamble ---
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
    let pos = p.pos_life.xy;
    let heading = p.flags.z;

    // --- Read all params + audio modulation ---
    let drive = audio_drive();
    let sep_w = separation_weight();
    // mfcc(1u) = spectral tilt: bright timbre tightens flock, dark loosens
    let timbre_coh = 1.0 + (mfcc(1u) * 0.5 + u.mid * 0.5) * drive * 1.5;
    let coh_w = cohesion_weight() * timbre_coh;
    let heading_smooth = heading_smoothing();

    // Vicsek noise: eta sweeps order→chaos with bass
    let eta = clamp(noise_eta_base() * PI + u.bass * drive * PI, 0.0, PI);

    // Predator
    let pred_pos = predator_pos();
    let pred_intensity = predator_intensity();
    let pred_str = predator_strength();

    // --- K=7 topological neighbor scan ---
    // Scan center cell first, then ring of 8 neighbors (avoids directional bias)
    let my_cell = sh_pos_to_cell(pos);

    // Cell scan offsets: center first, then 8 surrounding
    var cell_dx: array<i32, 9> = array<i32, 9>(0, -1, 1, 0, 0, -1, 1, -1, 1);
    var cell_dy: array<i32, 9> = array<i32, 9>(0, 0, 0, -1, 1, -1, -1, 1, 1);

    // K-nearest arrays (insertion sort)
    var knn_dist: array<f32, 7> = array<f32, 7>(999.0, 999.0, 999.0, 999.0, 999.0, 999.0, 999.0);
    var knn_idx: array<u32, 7> = array<u32, 7>(0u, 0u, 0u, 0u, 0u, 0u, 0u);
    var knn_count: u32 = 0u;

    for (var ci = 0u; ci < 9u; ci++) {
        let cx = my_cell.x + cell_dx[ci];
        let cy = my_cell.y + cell_dy[ci];
        let range = sh_cell_range(cx, cy);
        let start = range.x;
        let total_in_cell = range.y;
        let count = min(total_in_cell, MAX_PER_CELL);

        // Randomize start offset within cell — breaks systematic bias that causes banding
        let scan_offset = u32(hash(f32(idx) * 0.31 + f32(ci) * 7.7) * f32(total_in_cell));

        for (var i = 0u; i < count; i++) {

            let ni = sh_sorted_indices[start + ((i + scan_offset) % total_in_cell)];
            if ni == idx { continue; }

            let n_pl = pos_life_in[ni];
            if n_pl.w <= 0.0 { continue; }

            let diff = pos - n_pl.xy;
            let dist_sq = dot(diff, diff);
            if dist_sq < 0.0000001 { continue; }

            let dist = sqrt(dist_sq);

            // K-nearest insertion sort
            let worst_k = select(K - 1u, knn_count, knn_count < K);
            if knn_count < K || dist < knn_dist[worst_k] {
                var ins = knn_count;
                if ins >= K { ins = K - 1u; }

                // Shift down: find where this distance slots in
                loop {
                    if ins < 1u { break; }
                    if knn_dist[ins - 1u] <= dist { break; }
                    if ins < K {
                        knn_dist[ins] = knn_dist[ins - 1u];
                        knn_idx[ins] = knn_idx[ins - 1u];
                    }
                    ins--;
                }

                if ins < K {
                    knn_dist[ins] = dist;
                    knn_idx[ins] = ni;
                }
                if knn_count < K {
                    knn_count++;
                }
            }
        }
    }

    // --- All three forces from K=7 neighbors (fully topological) ---

    // Alignment: normalized mean heading (Vicsek-style, magnitude 1.0)
    var sum_cos = cos(heading);
    var sum_sin = sin(heading);
    for (var k = 0u; k < knn_count; k++) {
        let n_heading = flags_in[knn_idx[k]].z;
        sum_cos += cos(n_heading);
        sum_sin += sin(n_heading);
    }
    let align_vec = vec2f(sum_cos, sum_sin);
    let align_mag = length(align_vec);
    var target_vel = select(vec2f(cos(heading), sin(heading)), align_vec / align_mag, align_mag > 0.001);

    var edge_factor = 0.0;
    if knn_count > 0u {
        // Cohesion: steer toward K=7 center of mass
        var com = vec2f(0.0);
        for (var k = 0u; k < knn_count; k++) {
            com += pos_life_in[knn_idx[k]].xy;
        }
        com /= f32(knn_count);
        let to_com = com - pos;
        let com_dist = length(to_com);
        if com_dist > 0.001 {
            target_vel += normalize(to_com) * coh_w * min(com_dist * 20.0, 1.0);
        }

        // Separation: adaptive from K=7 (density-invariant)
        // Use K-th neighbor distance as interaction scale — works from 40K to 1M
        let k_radius = knn_dist[knn_count - 1u];
        // Separation threshold: closest ~40% of KNN range
        let sep_threshold = k_radius * (0.4 + (u.presence + u.brilliance) * 0.15 * drive);
        var sep_force = vec2f(0.0);
        for (var k = 0u; k < knn_count; k++) {
            if knn_dist[k] < sep_threshold {
                let n_pos = pos_life_in[knn_idx[k]].xy;
                let away = pos - n_pos;
                // Inverse-distance: stronger when closer (1/d scaling, normalized by threshold)
                sep_force += normalize(away) * (sep_threshold / knn_dist[k] - 1.0);
            }
        }
        let sep_len = length(sep_force);
        if sep_len > 0.001 {
            target_vel += normalize(sep_force) * sep_w;
        }

        // Edge detection: anisotropy of KNN neighborhood
        // High com_dist/k_radius = neighbors only on one side = flock edge
        edge_factor = smoothstep(0.35, 0.75, com_dist / k_radius);
    }

    // --- Predator avoidance ---
    if pred_str > 0.01 && pred_intensity > 0.05 {
        let to_bird = pos - pred_pos;
        let pred_dist = length(to_bird);
        if pred_dist < PREDATOR_RADIUS && pred_dist > 0.001 {
            let falloff = exp(-pred_dist * pred_dist / (PREDATOR_RADIUS * PREDATOR_RADIUS * 0.15));
            target_vel += normalize(to_bird) * pred_str * pred_intensity * falloff;
        }
    }

    // --- Roost centering: steers heading toward emitter to prevent drift ---
    // Must be in target_vel (not boundary) so it feeds into heading computation.
    // Otherwise alignment consensus overpowers position-only corrections.
    // Quadratic ramp: gentle within ~0.4 radius, strong beyond 0.6
    let to_roost = u.emitter_pos - pos;
    let roost_dist = length(to_roost);
    if roost_dist > 0.3 {
        let excess = roost_dist - 0.3;
        let roost_str = excess * excess * 3.0;
        target_vel += normalize(to_roost) * roost_str;
    }

    // --- Vicsek noise: angular perturbation ---
    let noise_val = (hash(f32(idx) * 0.37 + u.time * 3.0) - 0.5) * 2.0 * eta;
    let raw_heading = atan2(target_vel.y, target_vel.x) + noise_val;

    // --- Heading smoothing: angular low-pass filter ---
    let angle_delta = atan2(sin(raw_heading - heading), cos(raw_heading - heading));
    let new_heading = heading + angle_delta * heading_smooth;

    // --- Speed: base * centroid_mod * flux agitation * per-bird * beat pulse ---
    // Min/max speed clamping prevents stalling (blob collapse) and runaway
    let base_spd = cruise_speed();
    let centroid_mod = 1.0 + (u.centroid - 0.5) * drive * 0.8;
    // Spectral flux = timbral change rate → flock agitation
    let flux_mod = 1.0 + u.flux * drive * 0.6;
    let per_bird = 0.85 + hash(f32(idx) * 7.1) * 0.3;
    let beat_pulse = 1.0 + sin(u.beat_phase * PI * 2.0) * 0.08;
    let speed = clamp(base_spd * centroid_mod * flux_mod * per_bird * beat_pulse, base_spd * 0.5, base_spd * 2.5);
    let new_vel = vec2f(cos(new_heading), sin(new_heading)) * speed;

    // --- Boundary: soft repulsion from screen edges ---
    var boundary = vec2f(0.0);
    let edge = 0.15;
    let bstr = 4.0;
    if pos.x > 1.0 - edge  { boundary.x -= (pos.x - (1.0 - edge)) * bstr; }
    if pos.x < -1.0 + edge { boundary.x -= (pos.x + 1.0 - edge) * bstr; }
    if pos.y > 1.0 - edge  { boundary.y -= (pos.y - (1.0 - edge)) * bstr; }
    if pos.y < -1.0 + edge { boundary.y -= (pos.y + 1.0 - edge) * bstr; }


    var vel = new_vel + boundary * dt;
    let prev_pos = pos;
    var new_pos = pos + vel * dt;

    // Obstacle collision
    let coll = apply_obstacle_collision(new_pos, vel, prev_pos);
    new_pos = coll.xy;
    vel = coll.zw;

    // --- Depth-based sizing ---
    let depth = 1.0 - (new_pos.y + 1.0) * 0.15;
    let init_size = p.pos_life.z;
    let base_size = init_size * depth;
    let density_mod = 1.0 + f32(knn_count) * 0.03;
    let size = base_size * density_mod * (1.0 + u.rms * drive * 0.2);

    // --- Alpha: opaque birds with fade in/out ---
    // Spawn fade: based on real wall-clock age (not staggered initial_age)
    let real_age = u.time - p.flags.w;
    let spawn_fade = smoothstep(0.0, 0.5, real_age);
    let fade_out = 1.0 - smoothstep(0.85, 1.0, life_frac);
    var alpha = 0.9 * spawn_fade * fade_out;
    alpha *= eval_opacity_curve(life_frac);

    // --- Color: dark silhouettes + rim lighting for depth ---
    // Re-derive base color each frame from gradient (not previous frame's output)
    // to prevent frame-over-frame accumulation that washes out to bright white
    var col: vec3f;
    let color_t = fract(f32(idx) * 0.6180339887);  // golden ratio hash, stable per particle
    if u.gradient_count > 0u {
        col = eval_color_gradient(color_t).rgb;
    } else {
        col = vec3f(0.04 + fract(f32(idx) * 0.123) * 0.03);
    }
    let rim_param = param(2u);  // color_mode: 0=silhouette, 1=full rim
    let rim_intensity = rim_param * (u.rms * drive * 0.6 + u.onset * 0.3);
    // Dominant chroma drives rim hue — musical key → flock edge color
    // Centroid still provides warm/cool bias as secondary influence
    let hue = u.dominant_chroma + u.centroid * 0.15;
    let rim_r = abs(fract(hue) * 6.0 - 3.0) - 1.0;
    let rim_g = 2.0 - abs(fract(hue) * 6.0 - 2.0);
    let rim_b = 2.0 - abs(fract(hue) * 6.0 - 4.0);
    let rim_color = clamp(vec3f(rim_r, rim_g, rim_b), vec3f(0.3), vec3f(1.0));
    col += rim_color * edge_factor * rim_intensity * 0.12;
    // Y-depth tint: "closer" (lower) birds slightly warmer
    let y_depth = smoothstep(-0.5, 0.5, -pos.y);
    col += vec3f(0.02, 0.01, 0.0) * y_depth * rim_param;
    col = clamp(col, vec3f(0.0), vec3f(0.35));

    p.pos_life = vec4f(new_pos, init_size, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = vec4f(col, alpha);
    p.flags = vec4f(new_age, max_life, new_heading, p.flags.w);  // preserve birth time

    write_particle(idx, p);
    mark_alive(idx);
}
