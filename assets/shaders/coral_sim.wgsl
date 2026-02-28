// Coral particle simulation — Turing pattern attraction.
// Particles are attracted to features of reaction-diffusion-like patterns:
// hexagonal spots that morph to labyrinthine stripes based on audio parameters.
// Audio frequency bands control pattern density and dissolution.
// Structs, bindings, and helpers are in particle_lib.wgsl (auto-prepended).

const PI: f32 = 3.14159265;

// Turing-like pattern function: sum of 3 cosine waves at 60° intervals
// creates hexagonal spot patterns (Gray-Scott analog).
// k controls spatial frequency (density), morph blends spots ↔ stripes.
fn turing(p: vec2f, k: f32, morph: f32) -> f32 {
    // Hexagonal directions at 0°, 60°, 120°
    let d1 = cos(k * p.x);
    let d2 = cos(k * (-0.5 * p.x + 0.866 * p.y));
    let d3 = cos(k * (-0.5 * p.x - 0.866 * p.y));

    // Spot pattern = all three (hexagonal lattice)
    let spots = (d1 + d2 + d3) / 3.0;

    // Stripe pattern = dominant direction only
    let stripes = d1;

    // Morph between spots and stripes (analog to feed/kill ratio)
    return mix(spots, stripes, morph);
}

// Gradient of Turing function via central differences
fn turing_gradient(p: vec2f, k: f32, morph: f32) -> vec2f {
    let eps = 0.003;
    let dx = turing(p + vec2f(eps, 0.0), k, morph) - turing(p - vec2f(eps, 0.0), k, morph);
    let dy = turing(p + vec2f(0.0, eps), k, morph) - turing(p - vec2f(0.0, eps), k, morph);
    return vec2f(dx, dy) / (2.0 * eps);
}

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 13.37;

    // Screen emitter
    let pos = rand_vec2(seed_base) * 0.9;

    // Small random velocity
    let angle = hash(seed_base + 2.0) * 6.2831853;
    let speed = u.initial_speed * (0.3 + 0.7 * hash(seed_base + 3.0));
    let vel = vec2f(cos(angle), sin(angle)) * speed;

    // Color: warm coral/organic tones
    let hue = fract(0.08 + hash(seed_base + 5.0) * 0.12 + u.centroid * 0.15);
    let r_c = abs(hue * 6.0 - 3.0) - 1.0;
    let g_c = 2.0 - abs(hue * 6.0 - 2.0);
    let b_c = 2.0 - abs(hue * 6.0 - 4.0);
    let brightness = 0.03 + u.rms * 0.03;
    let col = clamp(vec3f(r_c, g_c, b_c), vec3f(0.0), vec3f(1.0)) * brightness;

    let initial_age = hash(seed_base + 9.0) * u.lifetime * 0.3;

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, u.initial_size * (0.7 + hash(seed_base + 6.0) * 0.6));
    p.color = vec4f(col, 0.1 + hash(seed_base + 7.0) * 0.08);
    p.flags = vec4f(initial_age, u.lifetime, 0.0, 0.0);
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
    var vel = p.vel_size.xy;
    let pos = p.pos_life.xy;

    // Pattern parameters from audio
    // Bass controls spatial frequency (density): low bass = sparse spots, high bass = dense
    let k = 6.0 + u.bass * 10.0 + u.mid * 4.0;

    // Centroid controls morph: low = hexagonal spots, high = labyrinthine stripes
    let morph = clamp(u.centroid * 1.5, 0.0, 1.0);

    // Slowly evolving pattern via time-shifted k
    let time_shift = sin(u.time * 0.1) * 0.5;
    let k_eff = k + time_shift;

    // Gradient descent toward pattern features (high-concentration spots)
    let val = turing(pos, k_eff, morph);
    let grad = turing_gradient(pos, k_eff, morph);
    let grad_len = length(grad);

    // Attraction: particles seek pattern maxima (spots/stripes)
    // Positive gradient direction = toward maxima
    let attract_k = u.attraction_strength;
    if grad_len > 0.001 {
        // Move toward maxima: follow gradient when val < 0 (away from spots),
        // slow down near maxima (val > 0)
        let attract = normalize(grad) * (1.0 - val) * attract_k * dt;
        vel += attract;
    }

    // Organic random walk — mimics diffusion
    let turb_angle = (hash(f32(idx) * 0.37 + u.time * 1.5) - 0.5) * 6.28318;
    let turb_strength = 0.015 + u.onset * 0.04;
    vel += vec2f(cos(turb_angle), sin(turb_angle)) * turb_strength * dt;

    // Onset: dissolution — scatter particles away (kill rate analog)
    if u.onset > 0.3 {
        let scatter_dir = normalize(pos + vec2f(0.001, 0.001));
        vel += scatter_dir * u.onset * 0.08 * dt;
    }

    // Beat: growth pulse — attract more strongly to pattern
    if u.beat > 0.5 {
        if grad_len > 0.001 {
            vel += normalize(grad) * 0.05 * dt;
        }
        vel *= 0.7; // Settle into pattern
    }

    // Drag
    vel *= 1.0 - (1.0 - u.drag) * dt * 60.0;

    // Boundary: soft wrap
    var new_pos = pos + vel * dt;
    new_pos = clamp(new_pos, vec2f(-1.0), vec2f(1.0));

    // Size: particles on pattern features are larger
    let on_feature = clamp(val * 0.5 + 0.5, 0.0, 1.0); // 0=trough, 1=peak
    let base_size = mix(p.vel_size.w, u.size_end, life_frac * 0.3);
    let size = base_size * (0.6 + 0.8 * on_feature) * (1.0 + u.rms * 0.2);

    // Alpha: brighter on pattern features
    let fade_in = smoothstep(0.0, 0.05, life_frac);
    let fade_out = 1.0 - smoothstep(0.8, 1.0, life_frac);
    let alpha = p.color.a * fade_in * fade_out * (0.4 + 0.6 * on_feature);

    // Color: warm shift on pattern features, cool in troughs
    var col = p.color.rgb;
    col += vec3f(0.08, 0.04, -0.03) * on_feature;
    col -= vec3f(0.02, 0.0, -0.04) * (1.0 - on_feature);
    col = clamp(col, vec3f(0.0), vec3f(1.0));

    p.pos_life = vec4f(new_pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = vec4f(col, alpha);
    p.flags.x = new_age;

    particles_out[idx] = p;
    mark_alive(idx);
}
