// Turing — particle simulation driven by reaction-diffusion field.
//
// Particles sample the R-D texture (group 4) for:
// - Gradient force: flow toward high-B concentration regions
// - Color: from B concentration via palette
// - Size: scales with local B value
// - Alpha: from B for compositing transparency
//
// Uses particle_lib.wgsl infrastructure (auto-prepended).

// Group 4: R-D texture (added by ParticleSystem when reaction_diffusion is present)
@group(4) @binding(0) var rd_tex: texture_2d<f32>;
@group(4) @binding(1) var rd_samp: sampler;

// --- Palette from B concentration + centroid shift ---
fn rd_palette(t: f32, centroid: f32) -> vec3f {
    // Shift palette hue with audio centroid
    let s = t + centroid * 0.5;

    // Organic palette: dark substrate → deep color → vivid → bright edge
    let c0 = vec3f(0.01, 0.005, 0.02);   // near-black substrate
    let c1 = vec3f(0.02, 0.12, 0.18);    // deep teal
    let c2 = vec3f(0.08, 0.45, 0.35);    // cyan-green
    let c3 = vec3f(0.6, 0.85, 0.5);      // bright green-gold edge

    if s < 0.25 {
        return mix(c0, c1, s / 0.25);
    } else if s < 0.6 {
        return mix(c1, c2, (s - 0.25) / 0.35);
    }
    return mix(c2, c3, clamp((s - 0.6) / 0.4, 0.0, 1.0));
}

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 7.31;

    // Spawn at random screen position
    let pos = rand_vec2(seed_base);

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(0.0, 0.0, 0.0, u.initial_size);
    p.color = vec4f(0.0, 0.0, 0.0, 0.0);
    p.flags = vec4f(0.0, u.lifetime, 0.0, 0.0);
    return p;
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

    // Age
    let new_age = age + u.delta_time;
    if new_age >= max_life {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    let life_frac = new_age / max_life;
    let dt = u.delta_time;
    let pos = p.pos_life.xy;

    // Map clip-space [-1,1] to UV [0,1] for R-D sampling
    let uv = pos * 0.5 + 0.5;

    // Sample R-D field at current position
    let rd = textureSampleLevel(rd_tex, rd_samp, uv, 0.0);
    let B_here = rd.g;

    // Compute gradient of B via finite differences (per-texel, not per-UV)
    let rd_dims = vec2f(textureDimensions(rd_tex));
    let dx = 1.0 / rd_dims.x;
    let dy = 1.0 / rd_dims.y;
    let B_right = textureSampleLevel(rd_tex, rd_samp, uv + vec2f(dx, 0.0), 0.0).g;
    let B_left  = textureSampleLevel(rd_tex, rd_samp, uv - vec2f(dx, 0.0), 0.0).g;
    let B_up    = textureSampleLevel(rd_tex, rd_samp, uv + vec2f(0.0, dy), 0.0).g;
    let B_down  = textureSampleLevel(rd_tex, rd_samp, uv - vec2f(0.0, dy), 0.0).g;
    // Raw per-texel gradient (not divided by texel size — avoids huge magnification)
    let grad_B = vec2f(B_right - B_left, B_up - B_down) * 0.5;

    // Gradient force: attract to high-B regions, onset pulses gradient strength
    let onset_boost = 1.0 + u.onset * 2.0;
    let grad_strength = param(0u) * 8.0 * onset_boost;
    let grad_len = length(grad_B);
    let force = select(vec2f(0.0), normalize(grad_B) * min(grad_len, 0.5) * grad_strength, grad_len > 0.0005);

    // Apply gradient force + drag
    var vel = p.vel_size.xy;
    vel += force * dt;

    // Beat velocity kick — brief speed boost on beat
    let beat_kick = 1.0 + u.beat * 0.5;
    vel *= beat_kick;

    // Drag: higher param = more drag
    let drag_amount = 1.0 - param(1u) * 0.15;
    vel *= pow(drag_amount, dt * 60.0);

    // Apply builtin forces (noise turbulence etc.)
    vel = apply_builtin_forces(pos, vel, dt);

    // Speed limit
    let speed = length(vel);
    if speed > 1.5 {
        vel = vel * (1.5 / speed);
    }

    // Integrate position with obstacle collision before wrapping
    let prev_pos = pos;
    var new_pos = pos + vel * dt;

    // Obstacle collision (before wrap to avoid teleport artifacts)
    let coll = apply_obstacle_collision(new_pos, vel, prev_pos);
    new_pos = coll.xy;
    vel = coll.zw;

    new_pos = fract(new_pos * 0.5 + 0.5) * 2.0 - 1.0;

    // Color from B concentration via palette
    let brightness = param(7u) + 0.3;
    let pal_t = clamp(B_here * 3.0, 0.0, 1.0);
    var col = rd_palette(pal_t, u.centroid) * brightness;

    // Audio brightness modulation
    col *= 1.0 + u.rms * 0.8;

    // Size: larger in pattern areas, tiny in substrate; bass pumps size
    let bass_pump = 1.0 + u.bass * 0.6;
    let size = u.initial_size * (0.15 + B_here * 3.0) * eval_size_curve(life_frac) * bass_pump;

    // Alpha: transparent in substrate, opaque in pattern
    // This is the key to making R-D structure visible with additive particles
    let pattern_alpha = smoothstep(0.02, 0.2, B_here);
    let fade = 1.0 - smoothstep(0.85, 1.0, life_frac);
    let alpha = pattern_alpha * fade * eval_opacity_curve(life_frac) * 0.6;

    // Write output
    p.pos_life = vec4f(new_pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = vec4f(col, alpha);
    p.flags.x = new_age;

    write_particle(idx, p);
    mark_alive(idx);
}
