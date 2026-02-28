// Murmuration particle simulation — Vicsek flocking model.
// Each particle aligns heading to average heading of nearest neighbors + noise.
// Audio-reactive noise_eta parameter creates order↔disorder phase transitions.
// Uses spatial hash (group 3) for efficient neighbor queries.
// Structs, bindings, and helpers are in particle_lib.wgsl (auto-prepended).

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 13.37;

    // Screen emitter with slight center bias
    let pos = rand_vec2(seed_base) * 0.8;

    // Random initial heading
    let angle = hash(seed_base + 2.0) * 6.2831853;
    let speed = u.initial_speed * (0.8 + 0.4 * hash(seed_base + 3.0));
    let vel = vec2f(cos(angle), sin(angle)) * speed;

    // Color: warm grays/whites like real starlings
    let tone = 0.04 + hash(seed_base + 5.0) * 0.03 + u.rms * 0.02;
    let hue_shift = u.centroid * 0.05;
    let col = vec3f(tone + hue_shift, tone, tone - hue_shift * 0.3);

    let initial_age = hash(seed_base + 9.0) * u.lifetime * 0.5;

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, u.initial_size * (0.7 + hash(seed_base + 6.0) * 0.6));
    p.color = vec4f(clamp(col, vec3f(0.0), vec3f(1.0)), 0.15 + hash(seed_base + 7.0) * 0.1);
    p.flags = vec4f(initial_age, u.lifetime, angle, 0.0); // flags.z stores heading angle
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
    let pos = p.pos_life.xy;
    let heading = p.flags.z; // current heading angle

    // --- Vicsek alignment: average heading of neighbors ---
    let my_cell = sh_pos_to_cell(pos);
    var sum_cos = 0.0;
    var sum_sin = 0.0;
    var neighbor_count = 0u;
    let max_neighbors = 7u; // K=7 topological

    // Search 3x3 cells around current position
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let cx = my_cell.x + dx;
            let cy = my_cell.y + dy;
            let range = sh_cell_range(cx, cy);
            let start = range.x;
            let count = range.y;

            for (var i = 0u; i < count && neighbor_count < max_neighbors; i++) {
                let ni = sh_sorted_indices[start + i];
                if ni == idx { continue; } // Skip self

                let np = particles_in[ni];
                if np.pos_life.w <= 0.0 { continue; } // Skip dead

                let dist = length(np.pos_life.xy - pos);
                if dist > 0.1 { continue; } // Interaction radius

                // Accumulate neighbor heading (stored in flags.z)
                let n_heading = np.flags.z;
                sum_cos += cos(n_heading);
                sum_sin += sin(n_heading);
                neighbor_count++;
            }
        }
    }

    // Average heading with self
    sum_cos += cos(heading);
    sum_sin += sin(heading);
    let avg_heading = atan2(sum_sin, sum_cos);

    // Noise parameter eta: high = disorder, low = ordered flocking
    // "order" param is inverted: 0=disordered, 1=ordered
    let order_param = u.emitter_radius; // repurpose emitter_radius as order control
    let base_eta = (1.0 - order_param) * 3.14159; // 0 = ordered, pi = max noise

    // Audio modulation: bass adds disorder, beat briefly forces order
    var eta = base_eta + u.bass * 1.5 + u.onset * 0.8;
    if u.beat > 0.5 {
        eta *= 0.2; // Beat snaps to order briefly
    }
    eta = clamp(eta, 0.0, 3.14159);

    // New heading = average + noise
    let noise = (hash(f32(idx) * 0.37 + u.time * 3.0) - 0.5) * 2.0 * eta;
    let new_heading = avg_heading + noise;

    // Flock speed: audio reactive
    let flock_speed = u.initial_speed * (1.0 + u.mid * 0.5);
    let new_vel = vec2f(cos(new_heading), sin(new_heading)) * flock_speed;

    // Boundary: soft repulsion from edges
    var boundary_force = vec2f(0.0);
    let edge_dist = 0.15;
    if pos.x > 1.0 - edge_dist { boundary_force.x -= (pos.x - (1.0 - edge_dist)) * 3.0; }
    if pos.x < -1.0 + edge_dist { boundary_force.x -= (pos.x - (-1.0 + edge_dist)) * 3.0; }
    if pos.y > 1.0 - edge_dist { boundary_force.y -= (pos.y - (1.0 - edge_dist)) * 3.0; }
    if pos.y < -1.0 + edge_dist { boundary_force.y -= (pos.y - (-1.0 + edge_dist)) * 3.0; }

    let vel = new_vel + boundary_force * dt;
    let new_pos = pos + vel * dt;

    // Size: slightly modulated by neighbor density
    let density_mod = 1.0 + f32(neighbor_count) * 0.05;
    let base_size = mix(p.vel_size.w, u.size_end, life_frac * 0.3);
    let size = base_size * density_mod * (1.0 + u.rms * 0.2);

    // Alpha
    let fade_in = smoothstep(0.0, 0.05, life_frac);
    let fade_out = 1.0 - smoothstep(0.8, 1.0, life_frac);
    let alpha = p.color.a * fade_in * fade_out;

    // Color: slight warmth when ordered (many aligned neighbors)
    var col = p.color.rgb;
    let alignment = clamp(f32(neighbor_count) / 7.0, 0.0, 1.0);
    col += vec3f(0.05, 0.02, -0.02) * alignment;
    col = clamp(col, vec3f(0.0), vec3f(1.0));

    p.pos_life = vec4f(new_pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = vec4f(col, alpha);
    p.flags = vec4f(new_age, max_life, new_heading, 0.0);

    particles_out[idx] = p;
    mark_alive(idx);
}
