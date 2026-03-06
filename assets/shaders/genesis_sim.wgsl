// Genesis particle simulation — Multi-Species Particle Lenia.
//
// Two species with complementary kernel/growth parameters interact through
// a 2×2 kernel matrix, creating predator/prey, symbiosis, or competition
// dynamics depending on cross_affinity.
//
// Energy-based formulation: E = R(x) - G(U(x))
// Particles minimize energy: dp/dt = -∇E = ∇G(U) - ∇R
//
// Reference: Mordvintsev et al., "Particle Lenia" (Google Research, 2023)
//
// Uses spatial hash (group 3) for efficient neighbor queries.
// Structs, bindings, and helpers are in particle_lib.wgsl (auto-prepended).
//
// --- Param mapping ---
// param(0) = trail_decay      (bg shader only)
// param(1) = radius           (interaction radius R, mapped 0.05–0.20)
// param(2) = kernel_peak      (species 0 μ_K; species 1 = 1.0 - peak)
// param(3) = kernel_width     (species 0 σ_K; species 1 = width × 0.7)
// param(4) = target_density   (species 0 μ_G; species 1 = 1.0 - target)
// param(5) = density_width    (shared σ_G)
// param(6) = step_size        (shared force multiplier)
// param(7) = cross_affinity   (inter-species interaction: 0=ignore, 0.5=neutral, 1=strong)

const CELL_SIZE: f32 = 0.05;
const MAX_NEIGHBORS: u32 = 256u;
const W_K: f32 = 0.02;        // Kernel weight (tuned for 3K particles: ~50 neighbors × avg K ~0.5 × 0.02 = U ≈ 0.5)
const MAX_SPEED: f32 = 0.25;

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 17.31;

    // 9 clusters in a 3×3 grid across the screen
    let cluster = u32(hash(seed_base) * 9.0) % 9u;
    let cx = f32(cluster % 3u) - 1.0;  // -1, 0, 1
    let cy = f32(cluster / 3u) - 1.0;  // -1, 0, 1
    let cluster_center = vec2f(cx, cy) * 0.55;

    let angle = hash(seed_base + 1.0) * 6.2831853;
    let r = sqrt(hash(seed_base + 2.0)) * 0.14;
    let pos = cluster_center + vec2f(cos(angle), sin(angle)) * r;

    // Small random velocity for symmetry breaking
    let vel = vec2f(hash(seed_base + 5.0) - 0.5, hash(seed_base + 6.0) - 0.5) * 0.005;

    let init_size = u.initial_size * (0.8 + hash(seed_base + 4.0) * 0.4);

    // Species assignment: 50/50 split
    let species = f32(idx % 2u);

    p.pos_life = vec4f(pos, init_size, 1.0);
    p.vel_size = vec4f(vel, species, init_size);
    p.color = vec4f(0.3, 0.25, 0.1, 0.1);
    p.flags = vec4f(0.0, u.lifetime, 0.0, 0.0);
    return p;
}

// Ring kernel: K(d) = w_K · exp(-((d-μ)/σ)²)
fn kernel_val(d: f32, mu: f32, sigma_sq: f32) -> f32 {
    let dd = d - mu;
    return W_K * exp(-(dd * dd) / sigma_sq);
}

// Kernel derivative: K'(d) = K · (-2(d-μ)/σ²)
fn kernel_deriv(K_val: f32, d: f32, mu: f32, sigma_sq: f32) -> f32 {
    let dd = d - mu;
    return K_val * (-2.0 * dd / sigma_sq);
}

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles { return; }

    var p = read_particle(idx);
    let life = p.pos_life.w;
    let age = p.flags.x;
    let max_life = p.flags.y;

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

    let dt = u.delta_time;
    let pos = p.pos_life.xy;
    var vel = p.vel_size.xy;
    let my_species = u32(p.vel_size.z);

    // ===================================================================
    // LENIA PARAMS + AUDIO — per-species from shared sliders
    // ===================================================================

    let R = 0.05 + param(1u) * 0.15;

    let cross_affinity = param(7u);

    // --- Species 0 kernel ---
    let mu_K_0 = clamp(param(2u) + u.bass * 0.12, 0.05, 0.95);
    let sigma_K_0 = max(param(3u) + (u.mid - 0.5) * 0.06, 0.03);
    let sigma_K_0_sq = sigma_K_0 * sigma_K_0;

    // --- Species 1 kernel: complementary ---
    let mu_K_1 = clamp(1.0 - param(2u) + u.bass * 0.12, 0.05, 0.95);
    let sigma_K_1 = max(param(3u) * 0.7 + (u.mid - 0.5) * 0.06, 0.03);
    let sigma_K_1_sq = sigma_K_1 * sigma_K_1;

    // --- Cross-species kernels ---
    let mu_K_cross = (mu_K_0 + mu_K_1) * 0.5;
    // sp0→sp1: broader awareness (1.2×), sp1→sp0: narrower (0.8×)
    let sigma_K_01 = max(param(3u) * 1.2, 0.03);
    let sigma_K_01_sq = sigma_K_01 * sigma_K_01;
    let sigma_K_10 = max(param(3u) * 0.8, 0.03);
    let sigma_K_10_sq = sigma_K_10 * sigma_K_10;

    // --- Growth per-species ---
    let mu_G_0 = clamp(param(4u) + u.sub_bass * 0.1, 0.05, 0.95);
    let sigma_G_0 = max(param(5u) + (u.centroid - 0.5) * 0.02, 0.02);
    let sigma_G_0_sq = sigma_G_0 * sigma_G_0;

    let mu_G_1 = clamp(1.0 - param(4u) + u.sub_bass * 0.1, 0.05, 0.95);
    let sigma_G_1 = max(param(5u) * 1.2 + (u.centroid - 0.5) * 0.02, 0.02);
    let sigma_G_1_sq = sigma_G_1 * sigma_G_1;

    // Select my growth params
    var mu_G: f32;
    var sigma_G_sq: f32;
    if my_species == 0u {
        mu_G = mu_G_0;
        sigma_G_sq = sigma_G_0_sq;
    } else {
        mu_G = mu_G_1;
        sigma_G_sq = sigma_G_1_sq;
    }

    let step = param(6u) * 0.10;

    // ===================================================================
    // NEIGHBOR LOOP — 2×2 kernel matrix
    // ===================================================================

    let cells_r = min(i32(ceil(R / CELL_SIZE)), 5);
    let my_cell = sh_pos_to_cell(pos);

    var U = 0.0;
    var grad_U = vec2f(0.0);
    var repulsion = vec2f(0.0);
    var neighbor_count = 0u;

    // Repulsion at 25% of R
    let d_rep = 0.25;
    let r_rep = d_rep * R;
    let c_rep_same = 0.12;
    let c_rep_cross = 0.06; // weaker cross-species repulsion

    for (var dy = -cells_r; dy <= cells_r; dy++) {
        for (var dx = -cells_r; dx <= cells_r; dx++) {
            let cx = my_cell.x + dx;
            let cy = my_cell.y + dy;
            let range = sh_cell_range(cx, cy);
            let start = range.x;
            let count = range.y;

            for (var i = 0u; i < count && neighbor_count < MAX_NEIGHBORS; i++) {
                let ni = sh_sorted_indices[start + i];
                if ni == idx { continue; }

                let n_pl = pos_life_in[ni];
                if n_pl.w <= 0.0 { continue; }

                let diff = pos - n_pl.xy;
                let r_ij = length(diff);
                if r_ij >= R || r_ij < 0.0001 { continue; }

                let d_ij = r_ij / R;
                let dir_ij = diff / r_ij; // FROM neighbor TO self

                // Read neighbor species
                let n_species = u32(vel_size_in[ni].z);

                // Select kernel from 2×2 matrix
                var K_val: f32;
                var dK_dd: f32;

                if my_species == n_species {
                    // Self-interaction: use own species kernel
                    if my_species == 0u {
                        K_val = kernel_val(d_ij, mu_K_0, sigma_K_0_sq);
                        dK_dd = kernel_deriv(K_val, d_ij, mu_K_0, sigma_K_0_sq);
                    } else {
                        K_val = kernel_val(d_ij, mu_K_1, sigma_K_1_sq);
                        dK_dd = kernel_deriv(K_val, d_ij, mu_K_1, sigma_K_1_sq);
                    }
                } else {
                    // Cross-interaction: scaled by cross_affinity
                    if my_species == 0u {
                        // sp0 sees sp1: broader awareness
                        K_val = kernel_val(d_ij, mu_K_cross, sigma_K_01_sq) * cross_affinity;
                        dK_dd = kernel_deriv(K_val, d_ij, mu_K_cross, sigma_K_01_sq);
                    } else {
                        // sp1 sees sp0: narrower awareness
                        K_val = kernel_val(d_ij, mu_K_cross, sigma_K_10_sq) * cross_affinity;
                        dK_dd = kernel_deriv(K_val, d_ij, mu_K_cross, sigma_K_10_sq);
                    }
                }

                U += K_val;

                // ∇U: K'(d)·dir/R
                grad_U += dK_dd * dir_ij / R;

                // Repulsion: weaker for cross-species (allows mixing)
                if r_ij < r_rep {
                    let overlap = 1.0 - r_ij / r_rep;
                    let c_rep = select(c_rep_cross, c_rep_same, my_species == n_species);
                    repulsion += dir_ij * overlap * c_rep;
                }

                neighbor_count++;
            }
        }
    }

    // ===================================================================
    // GROWTH FUNCTION + FORCE (per-species)
    // ===================================================================

    let dU_val = U - mu_G;
    let G_val = exp(-(dU_val * dU_val) / sigma_G_sq);
    let dG_dU = G_val * (-2.0 * dU_val / sigma_G_sq);

    // Growth force (radial: seeks optimal density)
    let growth_force = dG_dU * grad_U * step;

    var force = growth_force + repulsion;

    // --- Audio ---
    if u.onset > 0.3 {
        let jitter_angle = hash(f32(idx) * 5.17 + u.time) * 6.2831853;
        force += vec2f(cos(jitter_angle), sin(jitter_angle)) * u.onset * 0.04;
    }

    let breath = sin(u.beat_phase * 6.2831853) * 0.003;
    if length(pos) > 0.01 {
        force -= normalize(pos) * breath;
    }

    // Perpetual noise for symmetry breaking (paired hashes cancel systematic bias)
    let ns_t = u.time * 5.0;
    let nx = (hash(f32(idx) * 1.37 + ns_t) - hash(f32(idx) * 3.71 + ns_t)) * 0.5;
    let ny = (hash(f32(idx) * 2.91 + ns_t) - hash(f32(idx) * 4.53 + ns_t)) * 0.5;
    force += vec2f(nx, ny) * 0.025;

    // ===================================================================
    // VELOCITY UPDATE
    // ===================================================================

    vel += force * dt;
    vel *= pow(u.drag, dt * 60.0);

    let speed = length(vel);
    if speed > MAX_SPEED {
        vel *= MAX_SPEED / speed;
    }

    // ===================================================================
    // BEAT SEED DROP — alternate species per beat
    // ===================================================================

    let seed_hash = hash(f32(idx) * 7.13 + floor(u.time * 2.0));
    if u.beat > 0.5 && seed_hash < 0.03 {
        let seed_t = floor(u.time * 2.0);
        let sx = (hash(seed_t * 3.17 + 1.0) - 0.5) * 1.6;
        let sy = (hash(seed_t * 5.31 + 2.0) - 0.5) * 1.6;
        let seed_center = vec2f(sx, sy);

        let sa = hash(f32(idx) * 11.3 + seed_t) * 6.2831853;
        let sr = sqrt(hash(f32(idx) * 13.7 + seed_t)) * 0.12;
        let new_seed_pos = seed_center + vec2f(cos(sa), sin(sa)) * sr;

        let sv = vec2f(hash(f32(idx) * 17.1 + seed_t) - 0.5,
                       hash(f32(idx) * 19.3 + seed_t) - 0.5) * 0.01;

        // Alternate species per beat
        let seed_species = f32(u32(floor(u.time)) % 2u);

        p.pos_life = vec4f(new_seed_pos, p.pos_life.z, 1.0);
        p.vel_size = vec4f(sv, seed_species, p.vel_size.w);
        p.flags = vec4f(0.0, max_life, 0.0, 0.0); // reset age
        write_particle(idx, p);
        mark_alive(idx);
        return;
    }

    // ===================================================================
    // BOUNDARY + POSITION
    // ===================================================================

    // Soft radial containment — gentle spring past screen edge, hard wall off-screen
    let d_center = length(pos);
    let soft_edge = 0.9;
    if d_center > soft_edge {
        let overshoot = d_center - soft_edge;
        // Quadratic spring: gentle near edge, strong far off-screen
        let pull = overshoot * overshoot * 8.0;
        force -= normalize(pos) * pull;
    }

    let prev_pos = pos;
    var new_pos = pos + vel * dt;
    // Hard clamp well outside viewport as safety net
    let max_r = 1.5;
    let new_d = length(new_pos);
    if new_d > max_r {
        new_pos = new_pos / new_d * max_r;
        vel *= 0.5;
    }

    // Obstacle collision
    let coll = apply_obstacle_collision(new_pos, vel, prev_pos);
    new_pos = coll.xy;
    vel = coll.zw;

    // ===================================================================
    // RENDERING — species-based coloring
    // ===================================================================

    let init_size = p.pos_life.z;
    let density_size = 1.0 + clamp(U * 1.5, 0.0, 1.5);
    let size = init_size * density_size * (1.0 + u.rms * 0.15);

    // G_val drives organism visibility: high G = "alive" region
    let structure_t = clamp(dG_dU * 0.15 + 0.5, 0.0, 1.0);
    let density_t = clamp(U / max(mu_G * 1.5, 0.1), 0.0, 1.0);

    var col: vec3f;

    if my_species == 0u {
        // Species 0: teal/cyan bioluminescent (original palette)
        let outer = mix(vec3f(0.02, 0.05, 0.08), vec3f(0.06, 0.25, 0.2), density_t);
        let inner = mix(vec3f(0.08, 0.3, 0.25), vec3f(0.15, 0.5, 0.35), structure_t);
        col = mix(outer, inner, G_val);
    } else {
        // Species 1: magenta/amber warm complement
        let outer = mix(vec3f(0.08, 0.02, 0.05), vec3f(0.25, 0.08, 0.18), density_t);
        let inner = mix(vec3f(0.3, 0.1, 0.15), vec3f(0.5, 0.25, 0.12), structure_t);
        col = mix(outer, inner, G_val);
    }

    // Alpha for alpha blending: larger particles need stronger alpha
    let alpha = clamp(0.05 + G_val * 0.20, 0.05, 0.25);

    p.pos_life = vec4f(new_pos, init_size, 1.0);
    p.vel_size = vec4f(vel, f32(my_species), size);
    p.color = vec4f(col, alpha);
    p.flags = vec4f(new_age, max_life, U, G_val);

    write_particle(idx, p);
    mark_alive(idx);
}
