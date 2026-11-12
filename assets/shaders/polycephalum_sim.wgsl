// Polycephalum — 12-species Jones physarum agent simulation.
//
// One slime-mold species per pitch class (0..11). Agents sense a multi-channel behavioral
// trail field (group 4), steer sense-rotate style, move, and deposit into their own species
// channel. Same-species trail attracts; semitone-neighbour species repel; fifth/fourth species
// cooperate — so the chromatic structure of the music becomes territorial warfare.
//
// Chroma drives per-species vigour: loud pitch classes move faster and deposit more, so they
// flood and conquer. dominant_chroma sharpens the reigning species' turning; flatness sets
// sensor reach (tonal = long clean highways, percussive = short chaotic scribbles); kick bursts
// deposits. Visuals ride the existing additive compute-raster render + feedback bg pass; this
// field is behaviour only.
//
// --- Particle field usage ---
// pos_life:  xy = position [-1,1], z = unused, w = life (1/0)
// vel_size:  xy = last heading dir, z = unused, w = display size
// color:     rgba (species-tinted, consumed by the render pass)
// flags:     x = unused, y = max_lifetime, z = species_id (0..11), w = heading angle (radians)

const PI: f32 = 3.1415927;
const TWO_PI: f32 = 6.2831853;
const SPECIES: u32 = 12u;

// Group 4: behavioral trail field (added by ParticleSystem when trail_field is present).
struct TrailUniforms {
    grid_w: u32,
    grid_h: u32,
    channels: u32,
    deposit_scale: f32,
    decay: f32,
    diffuse: f32,
    time: f32,
    _pad: f32,
}
@group(4) @binding(0) var<storage, read> trail: array<f32>;
@group(4) @binding(1) var<storage, read_write> deposit: array<atomic<i32>>;
@group(4) @binding(2) var<uniform> tu: TrailUniforms;

// Pitch-class coupling: how strongly species `s` is drawn to channel `c`'s trail.
// Same class attracts; semitone neighbours repel (they "fight"); perfect fourth/fifth
// (5 or 7 semitones) cooperate; everything else mildly repels.
fn coupling(s: u32, c: u32) -> f32 {
    if s == c { return 1.0; }
    let iv = (c + SPECIES - s) % SPECIES;
    let semis = min(iv, SPECIES - iv); // 1..6
    if semis == 1u { return -0.7; }
    if semis == 5u { return 0.35; }
    return -0.12;
}

// Integer PCG-style hash -> 0..1. Robust regardless of argument magnitude, unlike the sin-based
// hash() in particle_lib (whose precision — and per-agent decorrelation — collapses once the
// time seed grows large, which would turn per-agent wander into a coherent global drift over time).
fn pcg_rand(a: u32, b: u32) -> f32 {
    var h = a * 747796405u + b * 2891336453u + 1u;
    h = ((h >> 16u) ^ h) * 2654435769u;
    h = ((h >> 16u) ^ h) * 2654435769u;
    h = (h >> 16u) ^ h;
    return f32(h & 0xFFFFu) / 65535.0;
}

// Flat trail index (toroidal wrap) for a uv point and channel.
fn tf_texel(uv: vec2f) -> vec2u {
    let w = i32(tu.grid_w);
    let h = i32(tu.grid_h);
    let x = i32(floor(fract(uv.x) * f32(w)));
    let y = i32(floor(fract(uv.y) * f32(h)));
    let xx = ((x % w) + w) % w;
    let yy = ((y % h) + h) % h;
    return vec2u(u32(xx), u32(yy));
}

// Score a sensor point for `my_species`: sum of coupled trail across all channels.
fn tf_sense(uv: vec2f, my_species: u32) -> f32 {
    let t = tf_texel(uv);
    let base = (t.y * tu.grid_w + t.x) * SPECIES;
    var score = 0.0;
    for (var c = 0u; c < SPECIES; c++) {
        score += coupling(my_species, c) * trail[base + c];
    }
    return score;
}

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;

    // Spawn uniformly across the screen; fixed species per slot => even 1/12 split. Integer-hashed
    // so the initial position/heading stay uncorrelated even for large idx (agents are immortal
    // and never re-emit, so a banded spawn heading would seed permanent aligned lanes).
    let pos = vec2f(pcg_rand(idx, 11u), pcg_rand(idx, 22u)) * 2.0 - 1.0;
    let species = idx % SPECIES;
    let angle = pcg_rand(idx, 33u) * TWO_PI;
    let hue = f32(species) / f32(SPECIES);
    let col = fosfora_hsv_to_rgb(hue, 0.85, 1.0);

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(cos(angle), sin(angle), 0.0, u.initial_size);
    p.color = vec4f(col * 0.5, 0.5);
    p.flags = vec4f(0.0, u.lifetime, f32(species), angle);
    return p;
}

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles { return; }

    var p = read_particle(idx);
    let life = p.pos_life.w;

    // Dead -> emit (immortal thereafter; agents never die of age).
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

    let dt = u.delta_time;
    let pos = p.pos_life.xy;
    let my_species = u32(p.flags.z);
    var angle = p.flags.w;

    let uv = pos * 0.5 + 0.5;
    let chroma_here = chroma_val(my_species);

    // Sensor reach: tonal music (low flatness) => longer, cleaner highways; percussive => short.
    // Capped modestly — a very long look-ahead makes agents commit to dead-straight paths that
    // ossify into parallel lanes, so even the tonal end stays in the curling regime.
    let reach_texels = mix(6.5, 3.0, clamp(u.flatness, 0.0, 1.0));
    let sensor_dist = (reach_texels * (0.5 + param(1u))) / f32(tu.grid_w);
    let sensor_angle = 0.2 + param(0u) * 1.0;

    let fwd = vec2f(cos(angle), sin(angle));
    let lft = vec2f(cos(angle + sensor_angle), sin(angle + sensor_angle));
    let rgt = vec2f(cos(angle - sensor_angle), sin(angle - sensor_angle));

    let f_score = tf_sense(uv + fwd * sensor_dist, my_species);
    let l_score = tf_sense(uv + lft * sensor_dist, my_species);
    let r_score = tf_sense(uv + rgt * sensor_dist, my_species);

    // Turn rate — sharper for the currently dominant pitch class (it conquers cleaner).
    let dominant_species = u32(round(u.dominant_chroma * f32(SPECIES - 1u)));
    var turn = (0.15 + param(2u) * 0.6);
    if my_species == dominant_species {
        turn *= 1.6;
    }
    // Two independent per-agent, per-frame randoms for symmetry breaking. Integer-hashed on a
    // bounded frame counter so the decorrelation never degrades as time grows.
    let frame = u32(tu.time * 60.0);
    let rnd = pcg_rand(idx, frame);
    let rnd2 = pcg_rand(idx, frame + 0x9E37u);

    // Jones sense-rotate rule.
    if f_score >= l_score && f_score >= r_score {
        // Ahead is best — hold heading (the wander below still perturbs it).
    } else if f_score < l_score && f_score < r_score {
        // Both sides better than ahead: turn hard to a random side.
        angle += select(-turn, turn, rnd > 0.5);
    } else if l_score > r_score {
        angle += turn;
    } else {
        angle -= turn;
    }
    // Continuous random wander — the key anti-ossification term. Without it, an established
    // straight lane keeps winning its own front sensor and locks in, so the whole field decays
    // into static parallel highways. A small random turn every step keeps lanes curling and the
    // network perpetually reorganizing.
    angle += (rnd2 - 0.5) * turn * 0.9;

    // Move — loud pitch classes are faster, so they flood territory.
    let speed = (0.04 + param(6u) * 0.22) * (0.35 + chroma_here * 1.4);
    let dir = vec2f(cos(angle), sin(angle));
    var new_pos = pos + dir * speed * dt;
    // Toroidal wrap in clip space.
    new_pos = fract(new_pos * 0.5 + 0.5) * 2.0 - 1.0;

    // Deposit into own channel at the new position (chroma + kick weighted).
    let dep_uv = new_pos * 0.5 + 0.5;
    let t = tf_texel(dep_uv);
    let dep_idx = (t.y * tu.grid_w + t.x) * SPECIES + my_species;
    let weight = (0.3 + chroma_here) * (0.5 + param(3u)) * (1.0 + u.kick * 3.0);
    atomicAdd(&deposit[dep_idx], i32(weight * tu.deposit_scale));

    // Species-tinted color for the additive render pass. Kept dim + saturated so additive
    // accumulation along dense highways stays in-gamut and reads as the pitch-class hue rather
    // than blowing out to white (additive-in-feedback runaway).
    let hue = f32(my_species) / f32(SPECIES);
    let bright = 0.035 + chroma_here * 0.14;
    let col = fosfora_hsv_to_rgb(hue, 1.0, 1.0) * bright;
    let alpha = clamp(0.12 + chroma_here * 0.35, 0.08, 0.6);

    p.pos_life = vec4f(new_pos, 0.0, 1.0);
    p.vel_size = vec4f(dir, 0.0, u.initial_size);
    p.color = vec4f(col, alpha);
    p.flags = vec4f(0.0, p.flags.y, f32(my_species), angle);

    write_particle(idx, p);
    mark_alive(idx);
}
