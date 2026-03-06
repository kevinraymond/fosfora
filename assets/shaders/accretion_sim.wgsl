// Accretion — tiled shared-memory N-body gravitational simulation.
//
// Every particle feels the gravitational pull of every other particle (O(N^2)),
// parallelized via workgroup shared memory tiles (GPU Gems 3 Ch. 31).
// Audio injects massive "seed" particles that attract swarms into accretion
// discs, orbital systems, and slingshot ejections.
//
// --- Param mapping ---
// param(0) = trail_decay    (bg shader)
// param(1) = G              (gravitational constant, 0→1 maps to 1–80)
// param(2) = softening      (singularity prevention, 0→1 maps to 0.2–8.0)
// param(3) = seed_mass      (mass of audio-injected seeds, 0→1 maps to 20–400)
// param(4) = seed_lifetime  (how long seeds persist, 0→1 maps to 2–10s)
// param(5) = damping        (radial velocity retention, 0→1 maps to 0.997–1.000)
// param(6) = init_pattern   (disc/ring/two-body/collapse)
// param(7) = color_mode     (velocity/proximity/orbital energy)
//
// --- Particle field usage ---
// pos_life:  xy = position, z = initial_size (preserved), w = life (1/0)
// vel_size:  xy = velocity, z = mass, w = display_size
// color:     rgba
// flags:     x = age, y = max_lifetime, z = is_seed (0/1), w = initial_mass

const TAU: f32 = 6.2831853;
const TILE_SIZE: u32 = 256u;
const G_BASE: f32 = 0.0000001;  // raw N-body scaling for clip-space coords (tuned for 30K particles)

// Shared memory tile: xy=pos, z=mass, w=unused
var<workgroup> tile: array<vec4f, 256>;

// --- Parameter helpers ---

fn get_G() -> f32 {
    let raw = param(1u);
    return mix(1.0, 80.0, raw);
}

fn get_softening() -> f32 {
    let raw = param(2u);
    return mix(0.2, 8.0, raw);
}

fn get_seed_mass() -> f32 {
    let raw = param(3u);
    return mix(20.0, 400.0, raw);
}

fn get_seed_lifetime() -> f32 {
    let raw = param(4u);
    return mix(2.0, 10.0, raw);
}

fn get_damping() -> f32 {
    let raw = param(5u);
    return mix(0.997, 1.000, raw);
}

// --- Color helpers ---

fn velocity_color(speed: f32) -> vec3f {
    let t = clamp(speed * 2.5, 0.0, 1.0);
    // deep blue → cyan-blue → warm gold (never white — stays saturated)
    let c0 = vec3f(0.08, 0.08, 0.5);  // slow: deep blue
    let c1 = vec3f(0.1, 0.4, 0.9);    // medium: blue
    let c2 = vec3f(0.9, 0.5, 0.1);    // fast: warm orange
    let c3 = vec3f(1.1, 0.7, 0.2);    // very fast: hot gold (NOT white)
    if t < 0.33 {
        return mix(c0, c1, t / 0.33);
    } else if t < 0.66 {
        return mix(c1, c2, (t - 0.33) / 0.33);
    }
    return mix(c2, c3, (t - 0.66) / 0.34);
}

fn proximity_color(min_seed_dist: f32) -> vec3f {
    let t = clamp(1.0 - min_seed_dist * 2.0, 0.0, 1.0);
    // blue (far) → orange/white (near seed)
    let far = vec3f(0.1, 0.2, 0.7);
    let near = vec3f(1.2, 0.8, 0.4);
    return mix(far, near, t * t);
}

fn energy_color(kinetic: f32, potential: f32) -> vec3f {
    let total = kinetic + potential;
    // bound (negative total) → blue, escaping (positive) → red
    let t = clamp(total * 5.0 + 0.5, 0.0, 1.0);
    let bound = vec3f(0.1, 0.3, 0.9);
    let escape = vec3f(0.9, 0.2, 0.1);
    return mix(bound, escape, t);
}

// --- Emission patterns ---

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 17.31;
    let pattern = param(6u);

    let h0 = hash(seed_base);
    let h1 = hash(seed_base + 1.0);
    let h2 = hash(seed_base + 2.0);
    let h3 = hash(seed_base + 3.0);
    let h4 = hash(seed_base + 4.0);

    var pos = vec2f(0.0);
    var vel = vec2f(0.0);

    if pattern < 0.25 {
        // Disc: random radius, tangential velocity ∝ sqrt(r)
        let r = sqrt(h0) * 0.7;
        let theta = h1 * TAU;
        pos = vec2f(cos(theta), sin(theta)) * r;
        let tangent = vec2f(-sin(theta), cos(theta));
        let orbital_speed = sqrt(max(r, 0.01)) * 0.35;
        vel = tangent * orbital_speed * (0.8 + h2 * 0.4);
    } else if pattern < 0.5 {
        // Ring: tight ring at r≈0.5
        let r = 0.45 + h0 * 0.1;
        let theta = h1 * TAU;
        pos = vec2f(cos(theta), sin(theta)) * r;
        let tangent = vec2f(-sin(theta), cos(theta));
        let orbital_speed = 0.25;
        vel = tangent * orbital_speed * (0.9 + h2 * 0.2);
    } else if pattern < 0.75 {
        // Two-body: two clusters at ±0.35, wider spread
        let cluster = select(-1.0, 1.0, h0 > 0.5);
        let cx = cluster * 0.35;
        let cy = 0.0;
        let spread = 0.12;
        pos = vec2f(cx + (h1 - 0.5) * spread, cy + (h2 - 0.5) * spread);
        // Orbital velocity around partner cluster
        let tangent = vec2f(0.0, cluster);
        vel = tangent * 0.15 * (0.8 + h3 * 0.4);
    } else {
        // Collapse: random sphere, zero velocity — dramatic infall
        let r = sqrt(h0) * 0.8;
        let theta = h1 * TAU;
        pos = vec2f(cos(theta), sin(theta)) * r;
        vel = vec2f(0.0);
    }

    let init_size = u.initial_size * (0.8 + h3 * 0.4);
    let mass = 1.0; // normal particles have unit mass

    // Lifetime: 30s ± 30% variance
    let life_var = 1.0 + (h4 - 0.5) * 0.6;

    // Color: start dim, will be updated each frame based on color_mode
    let col = velocity_color(length(vel));

    p.pos_life = vec4f(pos, init_size, 1.0);
    p.vel_size = vec4f(vel, mass, init_size);
    p.color = vec4f(col * 0.04, 0.20);
    p.flags = vec4f(0.0, u.lifetime * life_var, 0.0, mass);
    return p;
}

// --- Main compute shader ---

@compute @workgroup_size(256)
fn cs_main(
    @builtin(global_invocation_id) gid: vec3u,
    @builtin(local_invocation_id) lid: vec3u,
) {
    let idx = gid.x;
    let local_id = lid.x;
    let max_p = u.max_particles;

    // --- Read this particle's data (or zero for out-of-range) ---
    // ALL threads must participate in the tiled loop for barrier safety.
    var my_pos = vec2f(0.0);
    var my_mass = 0.0;
    var is_valid = false;
    var is_alive = false;

    var p: Particle;
    if idx < max_p {
        is_valid = true;
        p = read_particle(idx);
        is_alive = p.pos_life.w > 0.0;
        if is_alive {
            my_pos = p.pos_life.xy;
            my_mass = p.vel_size.z;
        }
    }

    // --- Tiled N-body force accumulation ---
    // Every thread (even out-of-range) participates in all barrier calls.
    var accel = vec2f(0.0);
    var min_seed_dist = 10.0;  // for proximity color mode
    var nearest_seed_mass = 0.0;
    let G = get_G() * G_BASE * (1.0 + u.bass * 0.5);
    let softening = get_softening();
    let soft2 = softening * softening * 0.001;  // in clip-space units² (~0.08 epsilon at default)

    let num_tiles = (max_p + TILE_SIZE - 1u) / TILE_SIZE;

    for (var t = 0u; t < num_tiles; t++) {
        // Cooperatively load tile into shared memory
        let load_idx = t * TILE_SIZE + local_id;
        if load_idx < max_p {
            let other_pl = pos_life_in[load_idx];
            let other_vs = vel_size_in[load_idx];
            // Dead particles contribute zero mass (no branching needed later)
            let alive_mask = select(0.0, 1.0, other_pl.w > 0.0);
            tile[local_id] = vec4f(other_pl.xy, other_vs.z * alive_mask, 0.0);
        } else {
            tile[local_id] = vec4f(0.0);
        }
        workgroupBarrier();

        // Accumulate gravity from all entries in this tile
        if is_alive {
            let tile_end = min(TILE_SIZE, max_p - t * TILE_SIZE);
            for (var j = 0u; j < tile_end; j++) {
                let global_j = t * TILE_SIZE + j;
                let other_pos = tile[j].xy;
                let other_mass = tile[j].z;

                let r = other_pos - my_pos;
                let dist2 = dot(r, r) + soft2;
                let inv_dist = inverseSqrt(dist2);
                let inv_dist3 = inv_dist * inv_dist * inv_dist;

                // Branchless self-exclusion: zero force for self-interaction
                let not_self = select(0.0, 1.0, global_j != idx);
                let eff_mass = other_mass * not_self;

                accel += r * (G * eff_mass * inv_dist3);

                // Track nearest seed for proximity coloring
                let other_flags = flags_in[min(global_j, max_p - 1u)];
                let is_seed = other_flags.z;
                if is_seed > 0.5 && not_self > 0.5 {
                    let d = sqrt(dot(r, r));
                    if d < min_seed_dist {
                        min_seed_dist = d;
                        nearest_seed_mass = other_mass;
                    }
                }
            }
        }
        workgroupBarrier();
    }

    // --- From here, only valid particles proceed ---
    if !is_valid {
        return;
    }

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

    // --- Alive particle: integrate ---
    let dt = u.delta_time;
    let age = p.flags.x;
    let max_life = p.flags.y;
    let is_seed = p.flags.z;
    let initial_mass = p.flags.w;
    let init_size = p.pos_life.z;

    let new_age = age + dt;
    if new_age >= max_life {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    let life_frac = new_age / max_life;
    var pos = p.pos_life.xy;
    var vel = p.vel_size.xy;
    var mass = p.vel_size.z;

    // --- Seed decay ---
    if is_seed > 0.5 {
        let seed_life = get_seed_lifetime();
        let seed_frac = new_age / seed_life;
        if seed_frac >= 1.0 {
            // Seed expires → revert to normal particle
            mass = 1.0;
            p.flags.z = 0.0;
        } else {
            mass = initial_mass * (1.0 - seed_frac);
            mass = max(mass, 1.0);
        }
    }

    // --- Seed injection on onset ---
    // Seeds stay where they are in the disc (no repositioning!) and become
    // local attractors. Nearby particles form mini accretion structures.
    if is_seed < 0.5 && u.onset > 0.5 {
        let seed_chance = hash(f32(idx) * 3.17 + u.time * 100.0);
        if seed_chance < 0.0001 {
            // Become a seed — stay in place, keep orbital velocity
            let sm = get_seed_mass() * (0.5 + u.mid);
            mass = sm;
            p.flags.z = 1.0;
            p.flags.w = sm;
            p.flags.x = 0.0;  // reset age for seed lifetime
            p.flags.y = get_seed_lifetime();
            // Slow down slightly but keep orbiting
            vel *= 0.7;
        }
    }

    // --- Apply accumulated gravitational acceleration ---
    vel += accel * dt;

    // --- Central pressure support (prevents gravothermal core collapse) ---
    // Scales with G so it stays proportional at any gravity setting.
    let r_center = length(pos);
    let pressure_r = 0.15;
    if r_center < pressure_r && r_center > 0.001 {
        let t = 1.0 - r_center / pressure_r;
        vel += normalize(pos) * t * t * G * 30000.0 * dt;
    }

    // --- Angular-momentum-preserving damping ---
    // Only damp radial velocity; preserve tangential (orbital) velocity.
    // This is the key to disc stability: uniform damping destroys angular
    // momentum, causing all orbits to inspiral. Radial-only damping settles
    // elliptical orbits into clean circles while maintaining orbital speed.
    let base_damping = get_damping();
    let damping = mix(base_damping, 1.0, u.presence * 0.5);
    if r_center > 0.005 {
        let r_hat = normalize(pos);
        let t_hat = vec2f(-r_hat.y, r_hat.x);
        let v_rad = dot(vel, r_hat);
        let v_tan = dot(vel, t_hat);
        let damped_v_rad = v_rad * pow(damping, dt * 60.0);
        vel = r_hat * damped_v_rad + t_hat * v_tan;
    } else {
        vel *= pow(damping, dt * 60.0);
    }

    // --- Gentle centering (prevents disc from drifting off-screen) ---
    // Very weak harmonic potential; negligible vs gravity but keeps COM near origin.
    vel -= pos * 0.03 * dt;

    // --- Flux perturbation ---
    if u.flux > 0.1 {
        let perturb_angle = hash(f32(idx) * 5.37 + u.time * 50.0) * TAU;
        vel += vec2f(cos(perturb_angle), sin(perturb_angle)) * u.flux * 0.005;
    }

    // --- Speed clamp (numerical stability) ---
    let speed = length(vel);
    if speed > 2.0 {
        vel = vel * (2.0 / speed);
    }

    // --- Integrate position ---
    let prev_pos = pos;
    pos += vel * dt;

    // --- Obstacle collision ---
    let coll = apply_obstacle_collision(pos, vel, prev_pos);
    pos = coll.xy;
    vel = coll.zw;

    // --- Boundary kill (large for slingshot arcs) ---
    if length(pos) > 3.0 {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    // --- Size (bass breathing) ---
    var size = init_size * eval_size_curve(life_frac);
    size *= 1.0 + u.bass * 0.4;  // bass makes particles swell
    if is_seed > 0.5 {
        let mass_frac = mass / max(initial_mass, 1.0);
        size = init_size * (1.5 + mass_frac * 2.5) * (1.0 + u.rms * 0.3);
    }

    // --- Fade ---
    let fade_in = smoothstep(0.0, 0.05, life_frac);
    let fade_out = 1.0 - smoothstep(0.85, 1.0, life_frac);
    let alpha_base = fade_in * fade_out * eval_opacity_curve(life_frac);

    // --- Color ---
    let color_mode = param(7u);
    var col = vec3f(0.0);

    if is_seed > 0.5 {
        // Seeds: warm gold glow, visible focal points
        let mass_frac = mass / max(initial_mass, 1.0);
        col = mix(vec3f(1.0, 0.7, 0.3), vec3f(1.2, 1.0, 0.8), mass_frac);
        col *= 0.10 * (0.5 + mass_frac * 0.5);
    } else if color_mode < 0.33 {
        // Velocity mode (default)
        col = velocity_color(speed) * 0.05;
    } else if color_mode < 0.66 {
        // Proximity mode
        col = proximity_color(min_seed_dist) * 0.05;
    } else {
        // Orbital energy mode
        let kinetic = 0.5 * speed * speed;
        let potential = -G * nearest_seed_mass / max(min_seed_dist, 0.01);
        col = energy_color(kinetic, potential) * 0.05;
    }

    // Audio brightness: RMS glow + warm-tinted onset flash
    col *= 1.0 + u.rms * 1.0;
    col += vec3f(1.0, 0.5, 0.1) * u.onset * 0.04;  // warm flash, not white

    let alpha = alpha_base * select(0.20, 0.40, is_seed > 0.5);

    // --- Write out ---
    p.pos_life = vec4f(pos, init_size, 1.0);
    p.vel_size = vec4f(vel, mass, size);
    p.color = vec4f(col, alpha);
    p.flags.x = new_age;
    write_particle(idx, p);
    mark_alive(idx);
}
