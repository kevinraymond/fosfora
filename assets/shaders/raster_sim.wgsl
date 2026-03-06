// Raster (Video Wall) particle simulation.
// Particles map to image pixel positions, colored by source image.
// Audio drives displacement: bass=Voronoi shards/swirl/radial, mids=2D wave, highs=scatter.
// Shard mode: 32 Voronoi cells with per-shard rotation + translation + depth,
// frayed edges where particles near cell boundaries break free.
// Spring-return physics pull particles back toward displaced home positions.
// Structs, bindings, and helpers are in particle_lib.wgsl (auto-prepended).

fn unpack_rgba(packed: f32) -> vec4f {
    let bits = bitcast<u32>(packed);
    return vec4f(
        f32(bits & 0xFFu) / 255.0,
        f32((bits >> 8u) & 0xFFu) / 255.0,
        f32((bits >> 16u) & 0xFFu) / 255.0,
        f32((bits >> 24u) & 0xFFu) / 255.0,
    );
}

fn luminance(c: vec3f) -> f32 {
    return dot(c, vec3f(0.299, 0.587, 0.114));
}

// --- Voronoi shard system ---

const N_SHARDS: u32 = 32u;

struct ShardResult {
    id: u32,
    center: vec2f,
    dist1_sq: f32,
    dist2_sq: f32,
}

// Generate a deterministic Voronoi seed point position for shard i.
fn shard_seed(i: u32) -> vec2f {
    let fi = f32(i);
    return vec2f(
        hash(fi * 127.1 + 5.3) * 1.8 - 0.9,
        hash(fi * 311.7 + 11.7) * 1.8 - 0.9
    );
}

// Find nearest and second-nearest Voronoi seed for a home position.
fn find_shard(home_pos: vec2f) -> ShardResult {
    var result: ShardResult;
    result.dist1_sq = 999.0;
    result.dist2_sq = 999.0;
    result.id = 0u;
    result.center = vec2f(0.0);

    for (var i = 0u; i < N_SHARDS; i++) {
        let seed = shard_seed(i);
        let diff = home_pos - seed;
        let d_sq = dot(diff, diff);
        if d_sq < result.dist1_sq {
            result.dist2_sq = result.dist1_sq;
            result.dist1_sq = d_sq;
            result.id = i;
            result.center = seed;
        } else if d_sq < result.dist2_sq {
            result.dist2_sq = d_sq;
        }
    }
    return result;
}

// Per-shard translation direction (consistent for all particles in shard).
fn shard_translate_dir(shard_id: u32) -> vec2f {
    let angle = hash(f32(shard_id) * 37.1 + 7.0) * 6.2831853;
    return vec2f(cos(angle), sin(angle));
}

// Per-shard rotation speed (can be positive or negative).
fn shard_rot_speed(shard_id: u32) -> f32 {
    return (hash(f32(shard_id) * 53.3 + 13.0) - 0.5) * 5.0;
}

// Per-shard depth multiplier (varies displacement magnitude, fakes Z separation).
fn shard_depth(shard_id: u32) -> f32 {
    return 0.5 + hash(f32(shard_id) * 71.7 + 23.0) * 1.0; // 0.5–1.5
}

// Rotate a 2D vector by angle.
fn rot2(v: vec2f, angle: f32) -> vec2f {
    let c = cos(angle);
    let s = sin(angle);
    return vec2f(v.x * c - v.y * s, v.x * s + v.y * c);
}

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles {
        return;
    }

    let home = aux[idx].home;
    let home_pos = home.xy;
    let home_color = unpack_rgba(home.z);

    // Skip transparent particles (padding beyond sampled image pixels)
    if home_color.a < 0.01 {
        var p = read_particle(idx);
        p.pos_life = vec4f(99.0, 99.0, 0.0, 0.0);
        p.color = vec4f(0.0);
        write_particle(idx, p);
        return;
    }

    var p = read_particle(idx);

    // Initial emit: particles start at home position
    if p.pos_life.w <= 0.0 {
        let slot = emit_claim();
        if slot < u.emit_count {
            let seed_base = u.seed + f32(idx) * 7.31;
            p.pos_life = vec4f(home_pos + vec2f(hash(seed_base), hash(seed_base + 1.0)) * 0.01, 0.0, 1.0);
            p.vel_size = vec4f(0.0, 0.0, 0.0, u.initial_size);
            p.color = home_color;
            p.flags = vec4f(hash(seed_base + 2.0) * u.lifetime * 0.5, u.lifetime, 0.0, 0.0);
            write_particle(idx, p);
            mark_alive(idx);
        } else {
            write_particle(idx, p);
        }
        return;
    }

    var pos = p.pos_life.xy;
    var vel = p.vel_size.xy;
    let dt = u.delta_time;

    // Read params
    let spring_k       = param(1u);  // spring stiffness (2-30)
    let bass_strength  = param(2u);  // bass displacement intensity (0-0.8)
    let mid_wave       = param(3u);  // wave displacement from mids (0-0.5)
    let high_scatter   = param(4u);  // random jitter from highs (0-0.5)
    let burst_force    = param(5u);  // beat onset impulse (0-1)
    let depth_scale    = param(6u);  // luminance → size multiplier (0-3)
    let bass_mode      = param(7u);  // 0=shards, 0.5=swirl, 1.0=radial

    // --- Audio displacement target ---
    var displacement = vec2f(0.0);
    var shard_depth_mult = 1.0; // used for size variation

    // Bass displacement (three modes)
    let radial_dir = normalize(home_pos + vec2f(0.001));
    if bass_mode < 0.33 {
        // --- Voronoi shard fragmentation ---
        let shard = find_shard(home_pos);
        let sid = shard.id;
        let depth_m = shard_depth(sid);
        shard_depth_mult = depth_m;

        let bass_amt = u.bass * bass_strength * depth_m;

        // Translation: each shard slides in its own direction
        let trans_dir = shard_translate_dir(sid);
        let translate = trans_dir * bass_amt * 0.5;

        // Rotation: twist increases with distance from shard center (flex, not rigid)
        let rot_spd = shard_rot_speed(sid);
        let offset = home_pos - shard.center;
        let dist_from_center = length(offset);
        let flex = 1.0 + dist_from_center * 1.8; // outer particles rotate more
        let rot_amount = bass_amt * rot_spd * flex;
        let rotated = rot2(offset, rot_amount) - offset;

        // Per-particle noise displacement within shard (breaks rigid lockstep)
        let inner_seed = f32(idx) * 3.91 + f32(sid) * 17.3;
        let inner_noise = vec2f(
            hash(inner_seed) - 0.5,
            hash(inner_seed + 2.7) - 0.5
        ) * bass_amt * 0.3;

        let shard_disp = translate + rotated + inner_noise;

        // Edge affinity: particles near Voronoi cell boundaries break free
        let d1 = sqrt(shard.dist1_sq);
        let d2 = sqrt(max(shard.dist2_sq, 0.0001));
        let edge_ratio = d1 / d2; // 0 at center, ~1 at boundary
        let affinity = smoothstep(0.85, 0.35, edge_ratio); // 1.0=core, 0.0=edge

        // Frayed edge particles get individual random jitter instead of shard motion
        let edge_seed = f32(idx) * 2.71 + floor(u.time * 4.0);
        let edge_jitter = vec2f(hash(edge_seed) - 0.5, hash(edge_seed + 3.0) - 0.5) * bass_amt * 2.5;

        displacement += mix(edge_jitter, shard_disp, affinity);

    } else if bass_mode < 0.66 {
        // Swirl: tangential displacement (perpendicular to radial)
        let tangent = vec2f(-radial_dir.y, radial_dir.x);
        displacement += tangent * u.bass * bass_strength;
    } else {
        // Radial: push away from center
        displacement += radial_dir * u.bass * bass_strength;
    }

    // Mids: 2D sinusoidal wave pattern across image
    let wave_freq = 6.0;
    let wave_y = sin(home_pos.x * wave_freq + u.time * 2.0);
    let wave_x = cos(home_pos.y * wave_freq * 0.7 + u.time * 1.6);
    displacement += vec2f(wave_x, wave_y) * u.mid * mid_wave;

    // Highs: per-particle random jitter driven by onset (spiky, visible)
    let scatter_seed = f32(idx) * 1.37 + floor(u.time * 8.0);
    let jitter = vec2f(hash(scatter_seed) - 0.5, hash(scatter_seed + 5.0) - 0.5);
    displacement += jitter * u.onset * high_scatter;

    // Spring force toward displaced home position
    let spring_target = home_pos + displacement;
    let to_target = spring_target - pos;
    vel += to_target * spring_k * dt;

    // Damping (frame-rate independent)
    vel *= pow(0.85, dt * 60.0);

    // Beat onset impulse
    if u.beat > 0.5 {
        let burst_seed = f32(idx) * 3.17 + u.time;
        let random_dir = vec2f(hash(burst_seed) - 0.5, hash(burst_seed + 7.0) - 0.5);
        var burst_dir: vec2f;
        var burst_mag = burst_force * (1.0 + u.kick);
        if bass_mode < 0.33 {
            // In shard mode, burst uses shard's direction + per-particle spread
            let shard = find_shard(home_pos);
            let trans_dir = shard_translate_dir(shard.id);
            burst_dir = normalize(trans_dir + random_dir * 0.8 + vec2f(0.001));
            // Outer particles in the shard fly further (flex, not rigid block)
            let dist_c = length(home_pos - shard.center);
            burst_mag *= (0.7 + dist_c * 1.2);
        } else {
            burst_dir = normalize(radial_dir + random_dir * 0.3 + vec2f(0.001));
        }
        vel += burst_dir * burst_mag;
    }

    // Velocity cap
    let speed = length(vel);
    if speed > 2.0 {
        vel = vel * (2.0 / speed);
    }

    // Integrate
    let prev_pos = pos;
    pos += vel * dt;

    // Obstacle collision
    let coll = apply_obstacle_collision(pos, vel, prev_pos);
    pos = coll.xy;
    vel = coll.zw;

    // Preserve original image color
    let color = home_color;

    // Gradient-based size modulation: smooth areas larger (fill gaps), edges neutral
    let gradient = home.w;
    let grad_norm = clamp(gradient / 80.0, 0.0, 1.0);
    let grad_size = mix(1.3, 1.0, grad_norm);

    // Size: luminance-based depth + bass pulse + shard depth variation + gradient
    let lum = luminance(home_color.rgb);
    var size = u.initial_size * (1.0 + lum * depth_scale)
             * (1.0 + u.bass * 0.15 * shard_depth_mult)
             * grad_size;

    // Sparkle boost: bright pixels at high-gradient locations (isolated stars, glints)
    // get an audio-reactive size pulse that creates active twinkling
    let sparkle = lum * grad_norm; // bright + high gradient = sparkle candidate
    if sparkle > 0.3 {
        let sparkle_strength = smoothstep(0.3, 0.8, sparkle);
        // Per-particle phase offset so they don't all pulse in sync
        let phase = hash(f32(idx) * 1.618);
        let twinkle = sin(u.time * 6.0 + phase * 6.2831853) * 0.5 + 0.5;
        // Audio modulates twinkle intensity: onset makes them flash, presence adds shimmer
        let audio_mod = 0.5 + u.onset * 0.8 + u.mid * 0.3;
        size *= 1.0 + sparkle_strength * twinkle * audio_mod * 0.8;
    }

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = color;
    p.flags.x += dt;

    // Wrap-around instead of death (image particles are persistent)
    if p.flags.x >= p.flags.y {
        p.flags.x = 0.0;
    }

    write_particle(idx, p);
    mark_alive(idx);
}
