// Fosfora particle simulation — two particle "worms" tracing an F (the
// effect traced a P, and was called Phosphor, while the app was).
// Each strand is a thick tube of particles that twist around the F spine.
// Cross-section is a filled disc; depth modulates brightness for 3D illusion.
// Structs, bindings, and helpers are in particle_lib.wgsl (auto-prepended).

// ---- F-shaped spine path ----
//
// An F is two strokes. The first runs up the stem and turns, round a soft
// corner, into the top bar; the second is the middle bar. `t` in [0, 1)
// covers both, split by length so the bars are as dense as the stem.

const F_SPLIT: f32 = 0.8; // t below this is stroke 1
// An F has nothing at its lower right, so it sits a little right of where
// the P's stem stood to look centered.
const F_SHIFT: f32 = 0.03;

fn eval_f(t: f32) -> vec2f {
    if t < F_SPLIT {
        let s = t / F_SPLIT;
        // The stem: slightly bowed, with the P's flourish at its foot.
        let stem_t = clamp(s / 0.685, 0.0, 1.0);
        let stem_bow = sin(stem_t * 3.14159) * 0.015;
        let stem_tail = (1.0 - stem_t) * (1.0 - stem_t) * (1.0 - stem_t) * 0.05;
        let stem_pos = vec2f(-0.14 + F_SHIFT + stem_bow - stem_tail, -0.37 + stem_t * 0.74);
        // The top bar, lifting a little toward its end.
        let bar_s = clamp((s - 0.685) / 0.315, 0.0, 1.0);
        let bar_pos = vec2f(
            -0.14 + F_SHIFT + 0.36 * bar_s,
            0.37 + sin(bar_s * 3.14159) * 0.012 + bar_s * bar_s * 0.03
        );
        // Blending the two across the join rounds the corner.
        let blend = smoothstep(0.63, 0.74, s);
        return mix(stem_pos, bar_pos, blend);
    }
    // The middle bar, a little above center and shorter than the top one.
    let m = (t - F_SPLIT) / (1.0 - F_SPLIT);
    return vec2f(
        -0.14 + F_SHIFT + 0.015 + 0.27 * m,
        0.03 + sin(m * 3.14159) * 0.012 + m * m * 0.015
    );
}

fn eval_f_tangent(t: f32) -> vec2f {
    // Sample within the stroke t is on: across the gap between strokes the
    // difference would point from the top bar's end to the middle bar.
    let eps = 0.003;
    var lo = 0.0;
    var hi = F_SPLIT - 0.0001;
    if t >= F_SPLIT {
        lo = F_SPLIT;
        hi = 0.999;
    }
    let a = eval_f(max(t - eps, lo));
    let b = eval_f(min(t + eps, hi));
    let d = b - a;
    let len = length(d);
    if len < 0.0001 {
        return vec2f(0.0, 1.0);
    }
    return d / len;
}

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 17.31;
    // Integer seed for the DECISIONS below — which strand, sparkle or not, flow
    // direction. fract-sin bands ~1/8 of indices onto near-zero at this scale,
    // which skewed all three. The cosmetic draws in this function stay on
    // hash(seed_base + k) on purpose: their banding is part of the shipped look,
    // so re-rolling them would re-tune the effect rather than fix a defect.
    let sb = uhash(idx + uhash(u32(u.seed * 4096.0)));

    // Which worm? ~50/50 split (teal=0, amber=1)
    let curve_id = step(0.5, uhash_f(sb ^ 0x9e3779b9u));
    let strand_phase = curve_id * 3.14159; // 0 or pi

    // Random position along the F
    let t = hash(seed_base);

    // Base spine position and tangent
    let base_pos = eval_f(t);
    let tangent = eval_f_tangent(t);
    let perp = vec2f(-tangent.y, tangent.x);

    // Scale
    let scale = u.emitter_radius / 0.30 * (1.0 + u.bass * 0.04);

    // Organic breathing
    let wobble = vec2f(
        sin(u.time * 0.3 + t * 8.0) * 0.010 + sin(u.time * 0.7 + t * 13.0) * 0.005,
        cos(u.time * 0.25 + t * 6.0) * 0.010 + cos(u.time * 0.6 + t * 11.0) * 0.005
    );

    // Helix: strand center orbits the spine
    let helix_angle = t * 3.0 * 6.28318 + u.time * 0.5 + strand_phase;
    let helix_offset = perp * cos(helix_angle) * 0.05;

    // Strand center in screen space
    let strand_center = base_pos * scale + wobble + helix_offset;

    // ---- Tube cross-section: filled disc around the strand center ----
    let tube_r = 0.012 + u.rms * 0.004; // tight tube
    let disc_angle = hash(seed_base + 2.0) * 6.28318;
    let disc_r = sqrt(hash(seed_base + 3.0)) * tube_r;

    // Screen-space offset (perpendicular to path)
    let tube_screen = perp * disc_r * cos(disc_angle);
    // Depth component (into screen) — modulates brightness
    let tube_depth = sin(disc_angle); // -1 to 1, normalized
    let depth_mod = 0.5 + 0.5 * tube_depth; // 0 (back) to 1 (front)

    // Sparkle: ~6% chance, scattered further out
    let is_sparkle = uhash_f(sb ^ 0x85ebca6bu) > 0.94;
    var screen_pos = strand_center + tube_screen;
    if is_sparkle {
        let halo_r = 0.015 + hash(seed_base + 10.0) * 0.035;
        let halo_a = hash(seed_base + 11.0) * 6.28318;
        screen_pos += vec2f(cos(halo_a), sin(halo_a)) * halo_r;
    }

    let pos = to_clip(screen_pos);

    // Velocity: tangential flow only — particles travel inside the tube
    let flow_dir = select(1.0, -1.0, uhash_f(sb ^ 0xc2b2ae35u) > 0.5);
    let vel = to_clip(tangent * 0.025 * flow_dir * scale);

    // Size: depth-modulated (front particles larger)
    var size: f32;
    if is_sparkle {
        size = 0.008 + hash(seed_base + 6.0) * 0.010;
    } else {
        size = u.initial_size * (0.7 + hash(seed_base + 6.0) * 0.6) * (0.6 + 0.4 * depth_mod);
    }

    // Color: vivid, depth-modulated
    let brightness = (0.45 + u.rms * 0.25) * (0.45 + 0.55 * depth_mod);
    var col: vec3f;
    if curve_id < 0.5 {
        col = vec3f(0.12, 1.0, 0.65) * brightness;
    } else {
        col = vec3f(1.0, 0.75, 0.25) * brightness;
    }
    if is_sparkle {
        col = mix(col, vec3f(1.1, 1.05, 0.95), 0.3) * 1.2;
    }

    let base_alpha = select(0.55, 0.80, is_sparkle) * (0.45 + 0.55 * depth_mod);
    let initial_age = hash(seed_base + 9.0) * u.lifetime * 0.08;

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = vec4f(col, base_alpha);
    p.flags = vec4f(initial_age, u.lifetime, curve_id, base_alpha);

    return p;
}

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
    var vel = p.vel_size.xy;

    // Gentle downward gravity
    vel += vec2f(0.0, -0.004) * dt;

    // Turbulence
    let turb = fosfora_noise2(p.pos_life.xy * 4.0 + vec2f(u.time * 0.4, u.time * 0.3));
    let turb_angle = turb * 6.28318;
    vel += vec2f(cos(turb_angle), sin(turb_angle)) * u.turbulence * 0.001 * dt;

    // Audio: onset gives a gentle push
    if u.onset > 0.3 {
        let dir = normalize(p.pos_life.xy + vec2f(0.001, 0.001));
        vel += dir * u.onset * 0.006 * dt;
    }

    // Beat: inward contraction pulse
    if u.beat > 0.5 {
        vel -= p.pos_life.xy * 0.06 * dt;
    }

    // Drag
    vel *= 1.0 - (1.0 - u.drag) * dt * 60.0;

    let prev_pos = p.pos_life.xy;
    var new_pos = p.pos_life.xy + vel * dt;

    // Obstacle collision
    let coll = apply_obstacle_collision(new_pos, vel, prev_pos);
    new_pos = coll.xy;
    vel = coll.zw;

    let size = mix(p.vel_size.w, u.size_end, life_frac * life_frac);
    let fade = 1.0 - smoothstep(0.6, 1.0, life_frac);
    let alpha = p.flags.w * fade;

    p.pos_life = vec4f(new_pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color.a = alpha;
    p.flags.x = new_age;

    write_particle(idx, p);
    mark_alive(idx);
}
