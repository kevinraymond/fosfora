// Mycelium — swarming tendril particle system with chain growth and branching.
//
// Leader particles follow curl noise flow fields, depositing follower particles
// that form spring-connected chains. Chains branch probabilistically on audio
// onset, creating a living mycelial network that grows at tips and decays at roots.
//
// --- Param mapping ---
// param(0) = trail_decay    (bg shader only)
// param(1) = curl_scale     (0→1 maps to 3.0–15.0)
// param(2) = curl_speed     (0→1 maps to 0.02–0.2)
// param(3) = branch_rate    (0→1 base branch probability)
// param(4) = spring_k       (0→1 maps to 2.0–20.0)
// param(5) = chain_depth    (0→1 maps to 10–80)
// param(6) = growth_speed   (0→1 maps to 0.5–3.0)
// param(7) = color_mode     (4 modes: depth/generation/velocity/age)
//
// --- Particle field usage ---
// pos_life:  xy = screen position [-1,1], z = generation, w = life (1/0)
// vel_size:  xy = velocity, z = unused, w = display size
// color:     rgba
// flags:     x = age, y = max_lifetime, z = chain_id (as f32), w = chain_depth

const MAX_CHAIN_DEPTH: u32 = 80u;
const INITIAL_LEADERS: u32 = 50u;
const SEGMENT_DIST: f32 = 0.01;
const BASE_SPEED: f32 = 0.10;
const BASE_LIFETIME: f32 = 15.0;

// ============================================================
// Mapped params
// ============================================================

fn curl_scale() -> f32 { return mix(3.0, 15.0, param(1u)); }
fn curl_speed() -> f32 { return mix(0.02, 0.2, param(2u)); }
fn branch_rate() -> f32 { return param(3u); }
fn spring_stiffness() -> f32 { return mix(2.0, 20.0, param(4u)); }
fn max_depth() -> u32 { return u32(mix(10.0, 80.0, param(5u))); }
fn growth_speed() -> f32 { return mix(0.5, 3.0, param(6u)); }

fn segment_interval() -> f32 {
    let speed = BASE_SPEED * growth_speed() * (1.0 + u.bass * 0.8);
    return SEGMENT_DIST / max(speed, 0.001);
}

// ============================================================
// Color helpers
// ============================================================

fn depth_palette(t: f32) -> vec3f {
    // Root teal-blue → mid forest green → tip bright phosphorescent green
    let c0 = vec3f(0.1, 0.3, 0.5);   // teal-blue (root)
    let c1 = vec3f(0.05, 0.5, 0.2);  // forest green (mid)
    let c2 = vec3f(0.3, 0.9, 0.2);   // phosphorescent green (tip)
    if t < 0.5 {
        return mix(c0, c1, t * 2.0);
    }
    return mix(c1, c2, (t - 0.5) * 2.0);
}

fn generation_palette(gen: f32) -> vec3f {
    if gen < 0.5 { return vec3f(0.1, 0.6, 0.5); }      // original: teal
    if gen < 1.5 { return vec3f(0.7, 0.5, 0.1); }       // 1st branch: gold
    return vec3f(0.6, 0.15, 0.5);                         // 2nd+: magenta
}

fn velocity_palette(speed: f32) -> vec3f {
    let t = clamp(speed * 5.0, 0.0, 1.0);
    let slow = vec3f(0.05, 0.1, 0.4);   // dim blue
    let fast = vec3f(0.2, 0.8, 0.3);    // bright green
    return mix(slow, fast, t);
}

fn age_palette(life_frac: f32) -> vec3f {
    let young = vec3f(0.3, 0.8, 0.2);   // bright green
    let old = vec3f(0.2, 0.1, 0.05);    // dark brown
    return mix(young, old, clamp(life_frac, 0.0, 1.0));
}

fn compute_color(depth_norm: f32, generation: f32, speed: f32, life_frac: f32) -> vec3f {
    let mode = param(7u);
    if mode < 0.25 {
        return depth_palette(depth_norm);
    } else if mode < 0.5 {
        return generation_palette(generation);
    } else if mode < 0.75 {
        return velocity_palette(speed);
    }
    return age_palette(life_frac);
}

// ============================================================
// Tip movement — curl noise flow field
// ============================================================

fn update_tip(pos: vec2f, vel: vec2f, dt: f32) -> vec4f {
    let scale = curl_scale();
    let speed_param = curl_speed();

    // Curl noise at position — circular time scroll to avoid directional drift
    let phase = u.time * speed_param;
    let noise_pos = pos * scale + vec2f(cos(phase * 0.7), sin(phase * 0.6)) * 2.0;
    let curl = curl_noise_2d(noise_pos);

    let move_speed = BASE_SPEED * growth_speed() * (1.0 + u.bass * 0.8);
    let curl_intensity = 1.0 + u.mid * 1.5;

    let desired = normalize(curl * curl_intensity + vec2f(0.001)) * move_speed;
    // Slow blending for smooth, persistent curves (not jittery)
    let new_vel = mix(vel, desired, 0.08);
    var new_pos = pos + new_vel * dt;

    // Soft containment — gentle nudge only past screen edge
    let aspect_r = u.resolution.x / u.resolution.y;
    let edge_x = abs(new_pos.x) - aspect_r;
    let edge_y = abs(new_pos.y) - 1.0;
    if edge_x > 0.0 {
        new_pos.x -= sign(new_pos.x) * edge_x * 1.5 * dt;
    }
    if edge_y > 0.0 {
        new_pos.y -= sign(new_pos.y) * edge_y * 1.5 * dt;
    }

    return vec4f(new_pos, new_vel);
}

// ============================================================
// Follower spring physics
// ============================================================

fn spring_force(origin: vec2f, toward: vec2f, rest_len: f32, k: f32) -> vec2f {
    let delta = toward - origin;
    let dist = length(delta);
    // When particles overlap exactly, nudge with a small deterministic offset
    if dist < 0.0001 {
        return vec2f(0.001, 0.0007) * k;
    }
    let stretch = dist - rest_len;
    return normalize(delta) * stretch * k;
}

fn update_follower(pos: vec2f, vel: vec2f, dt: f32, chain_start: u32, depth: u32, md: u32) -> vec4f {
    let k = spring_stiffness();
    var force = vec2f(0.0);

    // Spring toward child (depth+1, toward tip) — follower chases the chain forward
    if depth + 1u < md {
        if pos_life_in[chain_start + depth + 1u].w > 0.0 {
            let child = pos_life_in[chain_start + depth + 1u].xy;
            force += spring_force(pos, child, SEGMENT_DIST, k);
        }
    }

    // Spring toward parent (depth-1, toward root) — maintains spacing
    if depth > 0u {
        if pos_life_in[chain_start + depth - 1u].w > 0.0 {
            let parent = pos_life_in[chain_start + depth - 1u].xy;
            force += spring_force(pos, parent, SEGMENT_DIST, k * 0.7);
        }
    }

    let new_vel = (vel + force * dt) * 0.90; // damping
    var new_pos = pos + new_vel * dt;

    // Soft rectangular containment — no petri dish
    let aspect_r = u.resolution.x / u.resolution.y;
    new_pos.x = clamp(new_pos.x, -aspect_r * 1.1, aspect_r * 1.1);
    new_pos.y = clamp(new_pos.y, -1.1, 1.1);

    return vec4f(new_pos, new_vel);
}

// ============================================================
// Emission — initial root activation
// ============================================================

fn emit_root(idx: u32, chain_id: u32) -> Particle {
    var p: Particle;
    let seed_val = u.seed + f32(idx) * 13.71;
    let h0 = hash(seed_val);
    let h1 = hash(seed_val + 1.0);
    let h2 = hash(seed_val + 2.0);

    // Random screen position — spread wide to avoid center blob
    let px = (h0 - 0.5) * 1.8;
    let py = (h1 - 0.5) * 1.8;

    // Random initial velocity direction
    let angle = h2 * 6.2831853;
    let speed = BASE_SPEED * growth_speed() * 0.5;
    let vx = cos(angle) * speed;
    let vy = sin(angle) * speed;

    let lifetime_var = 1.0 + (hash(seed_val + 3.0) - 0.5) * 0.4;

    p.pos_life = vec4f(px, py, 0.0, 1.0); // generation=0
    p.vel_size = vec4f(vx, vy, 0.0, u.initial_size * 2.0);
    p.color = vec4f(0.05, 0.15, 0.05, 0.1);
    p.flags = vec4f(0.0, BASE_LIFETIME * lifetime_var, f32(chain_id), 0.0); // depth=0
    return p;
}

// ============================================================
// Main compute shader
// ============================================================

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    let max_p = u.max_particles;
    if idx >= max_p { return; }

    let md = max_depth();
    let chain_id = idx / MAX_CHAIN_DEPTH;
    let depth = idx % MAX_CHAIN_DEPTH;
    let chain_start = chain_id * MAX_CHAIN_DEPTH;
    let num_chains = max_p / MAX_CHAIN_DEPTH;

    var p = read_particle(idx);
    let is_alive = p.pos_life.w > 0.0;

    // ---- DEAD PARTICLE ----
    if !is_alive {
        // Depth exceeds current max_depth — stay dead
        if depth >= md {
            write_particle(idx, p);
            return;
        }

        // Root (depth 0): try initial activation or branch spawn
        if depth == 0u {
            if chain_id < INITIAL_LEADERS {
                // Initial leaders: use standard emission system
                let slot = emit_claim();
                if slot < u.emit_count {
                    p = emit_root(idx, chain_id);
                    write_particle(idx, p);
                    mark_alive(idx);
                } else {
                    write_particle(idx, p);
                }
                return;
            }

            // Reserve chains: try branch activation
            let h0 = hash(f32(chain_id) * 13.7 + floor(u.time * 60.0));
            let branch_prob = branch_rate() * 0.002 * (1.0 + u.onset * 15.0);
            if h0 > branch_prob {
                write_particle(idx, p);
                return;
            }

            // Pick random source chain to branch from
            let source_hash = hash(f32(chain_id) * 7.3 + u.time);
            let source_id = u32(source_hash * f32(min(num_chains, INITIAL_LEADERS * 5u)));
            let source_start = source_id * MAX_CHAIN_DEPTH;

            // Check if source root is alive
            let source_root = pos_life_in[source_start];
            if source_root.w <= 0.0 {
                write_particle(idx, p);
                return;
            }

            // Estimate source tip depth from root age
            let source_root_age = flags_in[source_start].x;
            let seg_int = segment_interval();
            let est_tip = min(u32(source_root_age / max(seg_int, 0.001)), md - 1u);
            let tip_data = pos_life_in[source_start + est_tip];
            if tip_data.w <= 0.0 {
                write_particle(idx, p);
                return;
            }

            // Branch: activate at tip position with angled velocity
            let tip_vel = vel_size_in[source_start + est_tip].xy;
            let tip_speed = length(tip_vel);
            let base_angle = select(
                hash(f32(chain_id) * 3.14) * 6.2831853,
                atan2(tip_vel.y, tip_vel.x),
                tip_speed > 0.001
            );
            let branch_angle = 0.5 + u.brilliance * 0.5;
            let sign_h = hash(f32(chain_id) * 11.3 + u.time);
            let angle_offset = select(-branch_angle, branch_angle, sign_h > 0.5);
            let new_angle = base_angle + angle_offset;
            let new_speed = BASE_SPEED * growth_speed() * 0.8;

            let source_gen = tip_data.z; // generation of source
            let lifetime_var = 1.0 + (hash(f32(chain_id) * 5.7 + u.time) - 0.5) * 0.3;

            p.pos_life = vec4f(tip_data.xy, source_gen + 1.0, 1.0);
            p.vel_size = vec4f(cos(new_angle) * new_speed, sin(new_angle) * new_speed, 0.0, u.initial_size * 2.0);
            p.color = vec4f(0.05, 0.15, 0.05, 0.1);
            p.flags = vec4f(0.0, BASE_LIFETIME * lifetime_var * 0.7, f32(chain_id), 0.0);
            write_particle(idx, p);
            mark_alive(idx);
            return;
        }

        // Non-root depth: try chain growth (self-activation)
        if depth > 0u && depth < md {
            // Check predecessor is alive
            let pred = pos_life_in[chain_start + depth - 1u];
            if pred.w <= 0.0 {
                write_particle(idx, p);
                return;
            }

            // Check timing: root age must be enough for this depth
            let root_age = flags_in[chain_start].x;
            let seg_int = segment_interval();
            let needed_age = f32(depth) * seg_int;
            if root_age < needed_age {
                write_particle(idx, p);
                return;
            }

            // Self-activate slightly behind predecessor (small offset prevents
            // zero-distance spring deadlock and creates immediate trail separation)
            let pred_vel = vel_size_in[chain_start + depth - 1u].xy;
            let pred_speed = length(pred_vel);
            var offset = vec2f(0.0);
            if pred_speed > 0.0001 {
                // Place behind predecessor along its movement direction
                offset = -normalize(pred_vel) * SEGMENT_DIST * 0.5;
            } else {
                // Random tiny offset when velocity is near zero
                let h = hash(f32(idx) * 7.7 + u.time);
                offset = vec2f(cos(h * 6.283), sin(h * 6.283)) * SEGMENT_DIST * 0.3;
            }

            let root_gen = pos_life_in[chain_start].z;
            let root_max_life = flags_in[chain_start].y;

            p.pos_life = vec4f(pred.xy + offset, root_gen, 1.0);
            p.vel_size = vec4f(pred_vel * 0.3, 0.0, u.initial_size);
            p.color = vec4f(0.05, 0.15, 0.05, 0.1);
            p.flags = vec4f(0.0, root_max_life, f32(chain_id), f32(depth));
            write_particle(idx, p);
            mark_alive(idx);
            return;
        }

        write_particle(idx, p);
        return;
    }

    // ---- ALIVE PARTICLE ----
    let dt = u.delta_time;
    let age = p.flags.x;
    let max_life = p.flags.y;
    let generation = p.pos_life.z;
    let pos = p.pos_life.xy;
    let vel = p.vel_size.xy;
    let prev_pos = pos;

    // Age the particle (flux accelerates root aging)
    var age_rate = 1.0;
    if depth == 0u {
        age_rate += u.flux * 2.0;
    }
    let new_age = age + dt * age_rate;

    // Death check
    if new_age >= max_life {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    // Death cascade: if parent is dead, die after short delay
    if depth > 0u {
        let parent_alive = pos_life_in[chain_start + depth - 1u].w;
        if parent_alive <= 0.0 {
            let death_delay = f32(depth) * 0.05;
            let parent_death_age = flags_in[chain_start + depth - 1u].x;
            if new_age > parent_death_age + death_delay || new_age >= max_life * 0.95 {
                p.pos_life.w = 0.0;
                write_particle(idx, p);
                return;
            }
        }
    }

    let life_frac = new_age / max_life;

    // Determine if this particle is the tip (highest alive depth in chain)
    var is_tip = true;
    if depth + 1u < md {
        let next = pos_life_in[chain_start + depth + 1u];
        if next.w > 0.0 {
            is_tip = false;
        }
    }

    // Movement
    var new_pos: vec2f;
    var new_vel: vec2f;
    if is_tip {
        let result = update_tip(pos, vel, dt);
        new_pos = result.xy;
        new_vel = result.zw;
    } else {
        let result = update_follower(pos, vel, dt, chain_start, depth, md);
        new_pos = result.xy;
        new_vel = result.zw;
    }

    // Obstacle collision
    let coll = apply_obstacle_collision(new_pos, new_vel, prev_pos);
    new_pos = coll.xy;
    new_vel = coll.zw;

    // NaN guard
    if any(new_pos != new_pos) || any(new_vel != new_vel) {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    // --- Size ---
    let depth_norm = f32(depth) / max(f32(md - 1u), 1.0);
    let depth_size = mix(2.0, 0.5, depth_norm); // thick roots, thin tips
    var size = u.initial_size * depth_size * eval_size_curve(life_frac);
    if is_tip {
        size *= 2.0 + u.onset * 0.8; // bright tip beacon
    }
    size *= 1.0 + u.rms * 0.3;

    // --- Color ---
    let speed = length(new_vel);
    var col = compute_color(depth_norm, generation, speed, life_frac);

    // Per-particle brightness — visible chains but no blowout with feedback
    col *= 0.06;

    // Tip extra glow — bright leading point
    if is_tip {
        col *= 2.5;
    }

    // Beat-phase traveling wave
    let wave = fract(u.beat_phase - depth_norm * 0.5);
    let glow = smoothstep(0.0, 0.15, wave) * (1.0 - smoothstep(0.15, 0.3, wave));
    col *= 1.0 + glow * u.rms * 2.0;

    // Audio brightness
    col *= 1.0 + u.rms * 0.5;
    col += vec3f(0.1, 0.4, 0.1) * u.onset * 0.05; // green-tinted onset flash

    // Centroid hue shift
    col *= vec3f(1.0 - u.centroid * 0.1, 1.0, 1.0 + u.centroid * 0.15);

    // Opacity
    let fade_in = smoothstep(0.0, 0.05, life_frac);
    let fade_out = 1.0 - smoothstep(0.85, 1.0, life_frac);
    let alpha = fade_in * fade_out * eval_opacity_curve(life_frac) * 0.25;

    // --- Write ---
    p.pos_life = vec4f(new_pos, generation, 1.0);
    p.vel_size = vec4f(new_vel, 0.0, size);
    p.color = vec4f(col, alpha);
    p.flags = vec4f(new_age, max_life, f32(chain_id), f32(depth));
    write_particle(idx, p);
    mark_alive(idx);
}
