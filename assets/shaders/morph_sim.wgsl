// Morph — shape target morphing simulation.
//
// Particles store home positions for up to 4 target shapes in strided aux buffer.
// Morph between targets on beat drops with spring physics and turbulence.
//
// --- Aux buffer layout (strided, 4 targets per particle) ---
// aux[idx * 4u + target_idx].home: xy=position, z=packed RGBA, w=unused
//
// --- Param mapping ---
// param(0) = spring_k       (spring stiffness, 0.01-1.0)
// param(1) = damping         (velocity damping, 0.05-1.0)
// param(2) = turbulence      (noise displacement during transition, 0-1)
// param(3) = stagger         (per-particle transition stagger amount, 0-1)
// param(4) = trans_width     (transition stagger window width, 0.1-1.0)
// param(5) = color_mode      (0=target color, 1=velocity-tinted)
// param(6) = scatter         (random displacement from brilliance, 0-1)
// param(7) = style           (0-1 maps to transition style, CPU-managed)
//
// --- Uniform fields ---
// morph_progress: 0.0-1.0 global transition progress
// morph_source: source target index (0-3)
// morph_dest: destination target index (0-3)
// morph_flags: bit 0 = transitioning, bits 1-3 = transition_style

const PI: f32 = 3.1415927;
const MORPH_STRIDE: u32 = 4u;

fn unpack_rgba(packed: f32) -> vec4f {
    let bits = bitcast<u32>(packed);
    return vec4f(
        f32(bits & 0xFFu) / 255.0,
        f32((bits >> 8u) & 0xFFu) / 255.0,
        f32((bits >> 16u) & 0xFFu) / 255.0,
        f32((bits >> 24u) & 0xFFu) / 255.0,
    );
}

// Cubic ease in-out for smooth transitions.
fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 {
        return 4.0 * t * t * t;
    }
    return 1.0 - pow(-2.0 * t + 2.0, 3.0) * 0.5;
}

// Read a morph target's aux data for a particle.
fn read_target(idx: u32, tgt: u32) -> vec4f {
    return aux[idx * MORPH_STRIDE + tgt].home;
}

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

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles {
        return;
    }

    // Read params
    let spring_k   = mix(5.0, 50.0, param(0u));
    let damping    = mix(0.7, 0.98, param(1u));
    let turb_amt   = param(2u);
    let stagger    = param(3u);
    let trans_w    = mix(0.1, 1.0, param(4u));
    let color_mode = param(5u);
    let scatter    = param(6u);

    // Read morph uniforms
    let progress   = u.morph_progress;
    let src_idx    = u.morph_source;
    let dst_idx    = u.morph_dest;
    let flags      = u.morph_flags;
    let is_transitioning = (flags & 1u) != 0u;
    let style      = (flags >> 1u) & 7u;

    // Read source and dest targets
    let src_home   = read_target(idx, src_idx);
    let dst_home   = read_target(idx, dst_idx);
    let src_pos    = src_home.xy;
    let dst_pos    = dst_home.xy;
    let src_color  = unpack_rgba(src_home.z);
    let dst_color  = unpack_rgba(dst_home.z);

    // Skip particles with no valid target data (transparent padding beyond image samples)
    let src_valid = src_color.a > 0.01;
    let dst_valid = dst_color.a > 0.01;
    if !src_valid && !dst_valid {
        var p = read_particle(idx);
        p.pos_life = vec4f(99.0, 99.0, 0.0, 0.0);
        p.color = vec4f(0.0);
        write_particle(idx, p);
        return;
    }

    var p = read_particle(idx);
    let dt = u.delta_time;

    // Initial emit: particles start at source target position
    if p.pos_life.w <= 0.0 {
        let slot = emit_claim();
        if slot < u.emit_count {
            let seed_base = u.seed + f32(idx) * 7.31;
            // Start at current destination (or source if not transitioning)
            let start_target = select(dst_idx, src_idx, !is_transitioning);
            let start_home = read_target(idx, start_target);
            let start_pos = start_home.xy;
            let start_color = unpack_rgba(start_home.z);

            p.pos_life = vec4f(start_pos + vec2f(hash(seed_base), hash(seed_base + 1.0)) * 0.005, 0.0, 1.0);
            p.vel_size = vec4f(0.0, 0.0, 0.0, u.initial_size);
            p.color = start_color;
            p.flags = vec4f(hash(seed_base + 2.0) * u.lifetime * 0.5, u.lifetime, 0.0, 0.0);
            write_particle(idx, p);
            mark_alive(idx);
        } else {
            write_particle(idx, p);
        }
        return;
    }

    let prev_pos = p.pos_life.xy;
    var pos = p.pos_life.xy;
    var vel = p.vel_size.xy;

    // MFCC(1) modulates transition speed: bright timbre accelerates, dark slows
    let mfcc1 = clamp(mfcc(1u) * 0.02, -0.5, 0.5);
    let speed_mod = 1.0 + mfcc1;
    let mod_progress = clamp(progress * speed_mod, 0.0, 1.0);

    // Per-particle staggered progress using hash for random offset
    let particle_hash = hash(f32(idx) * 1.618);
    var local_t: f32;

    if style == 3u {
        // Cascade: spatial stagger (left-to-right wave)
        let spatial_t = (src_pos.x + 1.0) * 0.5; // 0 at left, 1 at right
        let stagger_offset = spatial_t * stagger;
        local_t = clamp((mod_progress - stagger_offset) / max(trans_w, 0.01), 0.0, 1.0);
    } else {
        // Other styles: random per-particle stagger
        let stagger_offset = particle_hash * stagger;
        local_t = clamp((mod_progress - stagger_offset) / max(trans_w, 0.01), 0.0, 1.0);
    }

    let eased_t = ease_in_out_cubic(local_t);

    // Compute interpolated target position
    let target_pos = mix(src_pos, dst_pos, eased_t);

    // Apply transition-style-specific physics
    if style == 4u {
        // Direct: pure lerp, no physics
        pos = target_pos;
        vel = vec2f(0.0);
    } else if style == 1u {
        // Explode-reform: burst at start, delayed spring
        // MFCC(2) amplifies explosion scatter
        let mfcc2_explode = clamp(mfcc(2u) * 0.02, 0.0, 1.0);
        if is_transitioning {
            if local_t < 0.3 {
                let burst_seed = f32(idx) * 3.17 + u.time;
                let burst_dir = vec2f(hash(burst_seed) - 0.5, hash(burst_seed + 5.0) - 0.5);
                let burst_strength = (1.0 - local_t / 0.3) * 2.0 * (1.0 + u.bass) * (1.0 + mfcc2_explode);
                vel += burst_dir * burst_strength * dt * 10.0;
            } else {
                let reform_t = (local_t - 0.3) / 0.7;
                let spring_target = mix(pos, target_pos, reform_t * 0.3);
                let to_target = spring_target - pos;
                vel += to_target * spring_k * dt;
            }
            vel *= pow(damping, dt * 60.0);
            pos += vel * dt;
        } else {
            pos = mix(pos, target_pos, min(spring_k * dt * 0.5, 0.8));
            vel = vec2f(0.0);
        }
    } else if style == 2u {
        // Flow: strong curl noise, weak spring until late
        if is_transitioning {
            let noise_pos = pos * 3.0 + vec2f(u.time * 0.5);
            let curl = curl_noise_2d(noise_pos);
            let flow_strength = sin(local_t * PI) * 1.5;
            vel += curl * flow_strength * dt;

            let spring_blend = smoothstep(0.5, 1.0, local_t);
            let to_target = target_pos - pos;
            vel += to_target * spring_k * spring_blend * dt;

            vel *= pow(damping, dt * 60.0);
            pos += vel * dt;
        } else {
            pos = mix(pos, target_pos, min(spring_k * dt * 0.5, 0.8));
            vel = vec2f(0.0);
        }
    } else {
        // Spring (default, style 0) and Cascade (style 3, same physics)
        let to_target = target_pos - pos;

        if is_transitioning {
            // During transition: normal spring + turbulence
            vel += to_target * spring_k * dt;

            if turb_amt > 0.0 {
                // MFCC(2) drives mid-transition scatter: spectral complexity = more displacement
                let mfcc2_scatter = clamp(mfcc(2u) * 0.02, 0.0, 1.0);
                let turb_scale = sin(local_t * PI) * turb_amt * (1.0 + mfcc2_scatter * 2.0);
                let noise_pos = pos * 4.0 + vec2f(u.time * 0.7);
                let turb = curl_noise_2d(noise_pos);
                vel += turb * turb_scale * dt * 2.0;
            }

            vel *= pow(damping, dt * 60.0);
            pos += vel * dt;
        } else {
            // Holding shape: critically damped — snap to target fast
            let dist = length(to_target);
            if dist > 0.0005 {
                // Strong exponential convergence toward target
                pos = mix(pos, target_pos, min(spring_k * dt * 0.5, 0.8));
                vel = vec2f(0.0);
            } else {
                pos = target_pos;
                vel = vec2f(0.0);
            }
        }
    }

    // Audio reactivity (mostly during transitions — minimal when holding shape)
    let audio_gate = select(0.05, 1.0, is_transitioning);

    // Bass: spring excitement (vibrate around target)
    let bass_excitement = u.bass * 0.08 * audio_gate;
    if bass_excitement > 0.001 {
        let bass_seed = f32(idx) * 2.13 + u.time * 4.0;
        let vibrate = vec2f(hash(bass_seed) - 0.5, hash(bass_seed + 3.0) - 0.5);
        vel += vibrate * bass_excitement * dt;
    }

    // Mid: extra turbulence during transition only
    if is_transitioning && u.mid > 0.1 {
        let mid_noise_pos = pos * 5.0 + vec2f(u.time);
        let mid_turb = curl_noise_2d(mid_noise_pos);
        vel += mid_turb * u.mid * 0.15 * sin(local_t * PI) * dt;
    }

    // Brilliance: scatter from target positions (subtle)
    if u.brilliance > 0.2 && scatter > 0.0 {
        let scatter_seed = f32(idx) * 1.37 + floor(u.time * 8.0);
        let jitter = vec2f(hash(scatter_seed) - 0.5, hash(scatter_seed + 5.0) - 0.5);
        vel += jitter * u.brilliance * scatter * 0.5 * audio_gate * dt;
    }

    // Velocity cap
    let speed = length(vel);
    if speed > 3.0 {
        vel = vel * (3.0 / speed);
    }

    // Obstacle collision
    let coll = apply_obstacle_collision(pos, vel, prev_pos);
    pos = coll.xy;
    vel = coll.zw;

    // Interpolate color
    var color: vec4f;
    if color_mode < 0.5 {
        // Target color interpolation
        color = mix(src_color, dst_color, eased_t);
    } else {
        // Velocity-tinted: hue from velocity direction
        let vel_speed = length(vel);
        let hue = atan2(vel.y, vel.x) / (2.0 * PI) + 0.5;
        let sat = smoothstep(0.0, 0.5, vel_speed);
        let base = mix(src_color, dst_color, eased_t);
        let vel_color = vec4f(hsv2rgb(hue, sat * 0.7, 1.0), 1.0);
        color = mix(base, vel_color, sat * 0.5);
    }

    // Clamp color to prevent compute rasterizer accumulation blowout
    color = vec4f(clamp(color.rgb, vec3f(0.0), vec3f(1.0)), clamp(color.a, 0.0, 1.0));

    // Size: base + audio pulse
    let size = u.initial_size * (1.0 + u.bass * 0.2);

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = color;
    p.flags.x += dt;

    // Persistent particles (no death, wrap age)
    if p.flags.x >= p.flags.y {
        p.flags.x = 0.0;
    }

    write_particle(idx, p);
    mark_alive(idx);
}
