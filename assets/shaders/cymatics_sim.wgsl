// Cymatics particle simulation — Chladni pattern attraction.
// Particles are attracted to nodal lines of standing wave patterns:
// cos(npi*x)cos(mpi*y) - cos(mpi*x)cos(npi*y) = 0
// Audio frequency bands select mode numbers, creating evolving geometric patterns.
// Smooth crossfade between modes on audio changes.
// Rotation, symmetry folding, and glow params for visual variety.
// Structs, bindings, and helpers are in particle_lib.wgsl (auto-prepended).

const PI: f32 = 3.14159265;

fn chladni(p: vec2f, n: f32, m: f32) -> f32 {
    return cos(n * PI * p.x) * cos(m * PI * p.y) - cos(m * PI * p.x) * cos(n * PI * p.y);
}

// Analytical gradient of Chladni function (exact, no numerical differentiation)
fn chladni_gradient(p: vec2f, n: f32, m: f32) -> vec2f {
    let npi = n * PI;
    let mpi = m * PI;
    let dx = -npi * sin(npi * p.x) * cos(mpi * p.y) + mpi * sin(mpi * p.x) * cos(npi * p.y);
    let dy = -mpi * cos(npi * p.x) * sin(mpi * p.y) + npi * cos(mpi * p.x) * sin(npi * p.y);
    return vec2f(dx, dy);
}

// Rotate a 2D point
fn rotate2d(p: vec2f, angle: f32) -> vec2f {
    let c = cos(angle);
    let s = sin(angle);
    return vec2f(p.x * c - p.y * s, p.x * s + p.y * c);
}

// Apply symmetry folding: 0=none, 0.5=bilateral, 1.0=quad
fn fold_symmetry(p: vec2f, sym: f32) -> vec2f {
    var q = p;
    if sym > 0.25 {
        q.x = abs(q.x); // bilateral mirror
    }
    if sym > 0.75 {
        q.y = abs(q.y); // quad mirror
    }
    return q;
}

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 13.37;

    let pos = rand_vec2(seed_base) * 0.9;
    let angle = hash(seed_base + 2.0) * 6.2831853;
    let speed = u.initial_speed * (0.3 + 0.7 * hash(seed_base + 3.0));
    let vel = vec2f(cos(angle), sin(angle)) * speed;

    // Color from gradient or default cool tones
    var col: vec3f;
    if u.gradient_count > 0u {
        let t = hash(seed_base + 5.0);
        let grad = eval_color_gradient(t);
        col = grad.rgb;
    } else {
        let hue = fract(0.55 + hash(seed_base + 5.0) * 0.2 + u.centroid * 0.2);
        let r_c = abs(hue * 6.0 - 3.0) - 1.0;
        let g_c = 2.0 - abs(hue * 6.0 - 2.0);
        let b_c = 2.0 - abs(hue * 6.0 - 4.0);
        let brightness = 0.5 + u.rms * 0.2;
        col = clamp(vec3f(r_c, g_c, b_c), vec3f(0.0), vec3f(1.0)) * brightness;
    }

    let initial_age = hash(seed_base + 9.0) * u.lifetime * 0.3;
    let init_size = u.initial_size * (0.7 + hash(seed_base + 6.0) * 0.6);
    p.pos_life = vec4f(pos, init_size, 1.0);
    p.vel_size = vec4f(vel, 0.0, init_size);
    p.color = vec4f(col, 0.6 + hash(seed_base + 7.0) * 0.2);
    p.flags = vec4f(initial_age, u.lifetime, 0.0, 0.0);
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

    let new_age = age + u.delta_time;
    if new_age >= max_life {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    let life_frac = new_age / max_life;
    let dt = u.delta_time;
    var vel = p.vel_size.xy;
    let pos = p.pos_life.xy;

    // Params
    let rotation_param = param(4u);   // rotation speed
    let symmetry_param = param(5u);   // symmetry folding
    let glow_param = param(6u);       // glow multiplier

    // Transform position for Chladni evaluation: rotate + fold
    let rot_angle = u.time * rotation_param * 0.5;
    var eval_pos = rotate2d(pos, rot_angle);
    eval_pos = fold_symmetry(eval_pos, symmetry_param);

    // Curated (n,m) mode pairs: varying the ratio gives qualitatively different shapes.
    // (2,1)=asymmetric star, (3,2)=triangular, (1,1)=cross, (4,1)=radial spokes, etc.
    let MODE_N = array<f32, 12>(2.0, 3.0, 1.0, 4.0, 2.0, 3.0, 5.0, 1.0, 4.0, 2.0, 5.0, 3.0);
    let MODE_M = array<f32, 12>(1.0, 2.0, 1.0, 1.0, 2.0, 1.0, 2.0, 3.0, 3.0, 3.0, 1.0, 3.0);

    // dominant_chroma (0-1) selects mode pair; crossfade between adjacent pairs
    let chroma_idx = u.dominant_chroma * 11.0;
    let ci_lo = u32(floor(chroma_idx));
    let ci_hi = min(ci_lo + 1u, 11u);
    let ci_frac = chroma_idx - floor(chroma_idx);

    // Evaluate both mode pairs and crossfade (no interpolating n/m individually)
    let val_lo = chladni(eval_pos, MODE_N[ci_lo], MODE_M[ci_lo]);
    let val_hi = chladni(eval_pos, MODE_N[ci_hi], MODE_M[ci_hi]);
    let val = mix(val_lo, val_hi, ci_frac);

    let grad_lo = chladni_gradient(eval_pos, MODE_N[ci_lo], MODE_M[ci_lo]);
    let grad_hi = chladni_gradient(eval_pos, MODE_N[ci_hi], MODE_M[ci_hi]);
    var grad = mix(grad_lo, grad_hi, ci_frac);

    // Un-rotate gradient back to world space
    grad = rotate2d(grad, -rot_angle);
    let grad_len = length(grad);

    // Attraction toward nodal lines — mfcc(0) drives vibrational energy
    let attract_k = u.attraction_strength * (0.5 + mfcc(0u) * 1.0);
    if grad_len > 0.01 {
        vel -= normalize(grad) * val * attract_k * dt;
    }

    // Organic diffusion — chroma peakedness controls pattern clarity
    // Compute chroma flatness: max/mean ratio → 0=uniform (noise), 1=peaked (tone)
    var chroma_sum = 0.0;
    var chroma_max = 0.0;
    for (var ci = 0u; ci < 12u; ci++) {
        let cv = chroma_val(ci);
        chroma_sum += cv;
        chroma_max = max(chroma_max, cv);
    }
    let chroma_mean = chroma_sum / 12.0;
    // tonality: 0 = all chroma equal (noise), 1 = one dominant (tonal)
    let tonality = select(0.0, 1.0 - chroma_mean / chroma_max, chroma_max > 0.01);
    // Tonal → crisp pattern (low diffusion), noise → blurred (high diffusion)
    let turb = (hash(f32(idx) * 0.37 + u.time * 2.0) - 0.5) * 6.28318;
    let diffusion = 0.02 * (0.3 + (1.0 - tonality) * 1.7);
    vel += vec2f(cos(turb), sin(turb)) * diffusion * dt;

    // Onset: scatter
    if u.onset > 0.3 {
        let scatter_dir = normalize(pos + vec2f(0.001, 0.001));
        vel += scatter_dir * u.onset * 0.08 * dt;
    }

    // Beat: snap to pattern
    if u.beat > 0.5 {
        vel *= 0.5;
    }

    // Drag
    vel *= 1.0 - (1.0 - u.drag) * dt * 60.0;

    // Soft boundary
    let prev_pos = pos;
    var new_pos = pos + vel * dt;
    let edge = 0.9;
    let bstr = 3.0;
    if new_pos.x > edge  { vel.x -= (new_pos.x - edge) * bstr * dt; }
    if new_pos.x < -edge { vel.x -= (new_pos.x + edge) * bstr * dt; }
    if new_pos.y > edge  { vel.y -= (new_pos.y - edge) * bstr * dt; }
    if new_pos.y < -edge { vel.y -= (new_pos.y + edge) * bstr * dt; }
    new_pos = clamp(new_pos, vec2f(-1.05), vec2f(1.05));

    // Obstacle collision
    let coll = apply_obstacle_collision(new_pos, vel, prev_pos);
    new_pos = coll.xy;
    vel = coll.zw;

    // Size: larger on nodal lines, with stronger contrast
    let on_line = exp(-abs(val) * 10.0);
    let init_size = p.pos_life.z;
    let base_size = mix(init_size, u.size_end, life_frac * 0.3);
    let size = base_size * (0.6 + 0.8 * on_line) * eval_size_curve(life_frac) * (1.0 + u.rms * 0.2);

    // Alpha: brighter on nodal lines, boosted by glow param
    let fade_in = smoothstep(0.0, 0.05, life_frac);
    let fade_out = 1.0 - smoothstep(0.75, 1.0, life_frac);
    let glow_mult = 0.5 + glow_param * 1.5; // 0.5 to 2.0 range
    let alpha = p.color.a * fade_in * fade_out * (0.5 + 0.5 * on_line) * eval_opacity_curve(life_frac) * glow_mult;

    let col = p.color.rgb;

    p.pos_life = vec4f(new_pos, init_size, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = vec4f(col, alpha);
    p.flags.x = new_age;

    write_particle(idx, p);
    mark_alive(idx);
}
