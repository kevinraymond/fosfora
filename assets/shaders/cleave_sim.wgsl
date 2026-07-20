// Cleave particle simulation — HPSS percussive/harmonic duet (#1798)
//
// Two visual voices share one population, split into fixed index cohorts:
// ~30% SHARD slots (crystalline needles stabbing radially from the fracture
// origin on drum transients) and ~70% RIBBON slots (long-lived threads
// relaxed toward a shared curl field, swelling with melody and pads). The
// voice id is stamped in flags.z at emit; the mostly-dead shard pool IS the
// burst reservoir, so a long pad section can never convert the population
// into 6 s ribbons and leave nothing to burst with.
//
// Emission is hard-partitioned so the voices can never starve each other:
// ribbons claim via emit_claim() (counters[2]), shards via a second atomic
// on counters[3] — reserved, zeroed every dispatch, read by nothing else.
// Each voice compares its slot against its own share of u.emit_count; the
// share follows harmonic_ratio (dominance) plus the manual balance param.
//
// Shard bursts gate on u.percussive_energy MULTIPLICATIVELY (#1836: the
// kick band false-fires ~80% on sustained pads; HPSS percussive energy
// reads ~0 there — it is the pad/drum discriminator this effect teaches).
// onset+flux only sharpen the attack, they can never open the gate alone.
//
// Both voices ride the ribbon trail renderer: a shard is a short-lived,
// fast, ballistic particle whose 16-frame trail polyline IS the needle.
//
// --- Param mapping ---
// param(0) = shard_force     (burst probability + needle speed)
// param(1) = ribbon_drift    (curl field speed)
// param(2) = ribbon_glow     (ribbon alpha)
// param(3) = balance         (manual dominance bias; 0.5 = follow audio)
// param(4) = fracture_spread (shard spawn radius)
// param(5) = hue             (palette anchor, shared with bg)
// param(6) = shatter_glint   (bg shader: hit glint + radial echo)
// param(7) = trail_decay     (bg shader: feedback decay)

const SHARD_POOL_FRAC: f32 = 0.3;
const N_THREADS: f32 = 8.0;

// Integer hash (lowbias32). The lib's fract-sin hash() degrades on GPU for
// arguments beyond ~1e4 — with idx-scaled args (f32(idx)*7.7 + time ≈ 2.4M)
// a band of indices rolls near-constant tiny values, passing ANY spawn
// threshold every re-roll: an immortal audio-independent starburst (live
// finding). All per-index randomness here uses exact u32 mixing instead.
fn uhash(x: u32) -> u32 {
    var h = x;
    h = h ^ (h >> 16u);
    h = h * 0x7feb352du;
    h = h ^ (h >> 15u);
    h = h * 0x846ca68bu;
    h = h ^ (h >> 16u);
    return h;
}

fn uhash_f(x: u32) -> f32 {
    return f32(uhash(x)) / 4294967296.0;
}

// Fixed structural cohort: hashed by index only (no seed), so a slot keeps
// its voice across respawns and the burst reservoir is always recyclable
// within one shard lifetime (~0.5 s).
fn is_shard_slot(idx: u32) -> bool {
    return uhash_f(idx * 0x9e3779b9u) < SHARD_POOL_FRAC;
}

// Dominance with manual bias: 0 = percussion-dominant, 1 = harmonic-dominant.
// The 2.0 swing makes the extremes a hard override: harmonic_ratio dips to
// ~0 at the instant of a drum hit, so anything less leaves the strike gate
// partially open when balance is slammed to full-harmonic (live finding).
fn eff_ratio() -> f32 {
    return clamp(u.harmonic_ratio + (param(3u) - 0.5) * 2.0, 0.0, 1.0);
}

// Transient strength. percussive_energy is the GATE (multiplicative);
// onset/flux sharpen the attack inside the HPSS ~150 ms release envelope.
// BUT percussive_energy is adaptively normalized: on a pure pad (no drums
// anywhere in the ~4 s ranging window) the normalizer stretches tiny
// percussive residue toward 1 (live finding — refines #1836: the RAW HPSS
// value is ~0 there, the shipped uniform is not). harmonic_ratio is
// Passthrough (level-invariant), so IT is the discriminator that survives
// normalization: fade strikes out entirely as the mix goes harmonic-pure.
fn strike() -> f32 {
    let pad_guard = 1.0 - smoothstep(0.60, 0.85, eff_ratio());
    return u.percussive_energy * pad_guard * (0.4 + 0.6 * clamp(u.onset + u.flux, 0.0, 1.0));
}

// Shard share of this frame's emit budget; ribbons compare against the rest.
// Always < u.emit_count, so the ribbon share never underflows.
fn shard_budget() -> u32 {
    let share = mix(0.70, 0.15, smoothstep(0.15, 0.85, eff_ratio()));
    return u32(f32(u.emit_count) * share);
}

// Shared divergence-free drift — the ribbon primitive. phase selects one of
// N_THREADS quantized streamline families: coherent strands with variety,
// never per-particle dither (Tide lane lesson). Stiffens and quickens with
// harmonic content.
fn ribbon_field(pos: vec2f, phase: f32) -> vec2f {
    let speed = (0.10 + 0.40 * param(1u)) * (0.5 + 0.5 * u.harmonic_energy);
    let p = pos * vec2f(1.6, 1.1) + vec2f(u.time * 0.04, phase * 4.0);
    return fbm_curl_2d(p, 2u, 2.0, 0.5) * speed;
}

fn ribbon_color(vel: vec2f) -> vec3f {
    // Warm key-locked anchor; centroid tilts warm/cool (tide_color idiom).
    let hue_t = fract(u.dominant_chroma * 0.15 + param(5u) * 0.3 + 0.05);
    var col = phosphor_audio_palette(hue_t, 0.55 + 0.35 * clamp(u.centroid, 0.0, 1.0), u.time * 0.02);
    col = mix(col, vec3f(0.95, 0.75, 0.45), 0.25);
    let speed_glow = clamp(length(vel) * 2.0, 0.0, 1.0);
    // rms term kept small: the additive stack + bloom amplify from here
    // (Tide's white-out lesson on loud material).
    return col * (0.08 + 0.30 * speed_glow + 0.05 * u.rms);
}

fn shard_color(sw: f32) -> vec3f {
    // Cold ice: key-anchored but pulled hard toward blue-white; brightness
    // spends the strike strength captured at birth.
    let hue_t = fract(u.dominant_chroma * 0.15 + param(5u) * 0.3 + 0.45);
    var col = phosphor_audio_palette(hue_t, 0.35, u.time * 0.02);
    col = mix(col, vec3f(0.85, 0.93, 1.0), 0.65);
    return col * (0.5 + 1.1 * sw);
}

// Radial starburst from the fracture origin (u.emitter_pos — draggable).
// Direction is uniform in SCREEN space so the burst is round on 16:9.
fn emit_shard(idx: u32, sw: f32) -> Particle {
    var p: Particle;
    let sb = uhash(idx + uhash(u32(u.seed * 4096.0) + u32(u.time * 977.0)));
    let ang = uhash_f(sb) * 6.2831853;
    let dir_s = vec2f(cos(ang), sin(ang));
    // Birth point pushed outward along the needle's own direction: thousands
    // of max-alpha heads at r~0 fused into a solid white core on the first
    // live look — the offset hollows it into a radiant burst.
    let spread = 0.05 + (0.10 + 0.25 * param(4u)) * uhash_f(sb ^ 0x68bc21ebu);
    let pos = u.emitter_pos + to_clip(dir_s * spread);
    let speed = (0.7 + 1.5 * param(0u)) * (0.5 + sw) * (0.85 + 0.3 * uhash_f(sb ^ 0x02e5be93u));
    let init_size = u.initial_size * 0.6 * (0.5 + 0.6 * sw);
    let life = u.lifetime * (0.05 + 0.06 * uhash_f(sb ^ 0x967a889bu)); // 0.3-0.66 s
    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(to_clip(dir_s * speed), init_size, init_size);
    p.color = vec4f(shard_color(sw), 0.0);
    p.flags = vec4f(0.0, life, 1.0, sw); // (age, lifetime, voice=shard, strike)
    return p;
}

fn emit_ribbon(idx: u32) -> Particle {
    var p: Particle;
    let sb = uhash(idx + uhash(u32(u.seed * 4096.0) + u32(u.time * 977.0) + 0x2545f491u));
    // Hashed uniform over the frame with 5% overscan — threads are already
    // mid-field at birth, no visible spawn front.
    let pos = (vec2f(uhash_f(sb), uhash_f(sb ^ 0x1b873593u)) * 2.0 - 1.0) * 1.05;
    let phase = floor(uhash_f(sb ^ 0xcc9e2d51u) * N_THREADS) / N_THREADS;
    // Birth-sampled field velocity: immediate coherence, no settle-in drift.
    let vel = ribbon_field(pos, phase);
    let init_size = u.initial_size * (0.7 + 0.6 * uhash_f(sb ^ 0x85ebca6bu));
    let life = u.lifetime * (0.75 + 0.5 * uhash_f(sb ^ 0xe6546b64u)); // 4.5-7.5 s
    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, init_size, init_size);
    p.color = vec4f(ribbon_color(vel), 0.0);
    p.flags = vec4f(0.0, life, 0.0, phase); // (age, lifetime, voice=ribbon, phase)
    return p;
}

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles { return; }

    var p = read_particle(idx);

    if p.pos_life.w <= 0.0 {
        var emitted = false;
        if is_shard_slot(idx) {
            // Gate BEFORE claim: quiet frames never consume shard budget.
            // The probability makes burst TIMING; counters[3] caps burst SIZE.
            let s = strike();
            let dominance_damp = 0.25 + 0.75 * (1.0 - smoothstep(0.3, 0.9, eff_ratio()));
            let p_burst = smoothstep(0.12, 0.70, s) * dominance_damp * (0.4 + 0.6 * param(0u));
            // Ambient glint floor: ~50 needles/s across the ~75K-slot pool at
            // the 30 Hz re-roll (pool × p × 30) — sparse sparks, not a
            // standing starburst (first live look: 0.0008 was 1800/s). The
            // floor follows dominance: at full-harmonic (pad bridge, or
            // balance slammed) even 50/s reads as a stuck mini-burst. And it
            // follows presence: in TRUE silence harmonic_ratio parks at
            // neutral 0.5, so dominance alone leaves the floor wide open and
            // the fracture point strobes on a still frame (user live look) —
            // RMS is silence-gated to 0, making it the honest quiet signal.
            let floor_damp = 1.0 - 0.85 * smoothstep(0.5, 0.9, eff_ratio());
            let presence = 0.08 + 0.92 * smoothstep(0.02, 0.12, u.rms);
            let p_spawn = max(p_burst, 0.000025 * floor_damp * presence);
            if uhash_f(idx + uhash(u32(u.time * 30.0))) < p_spawn {
                let slot = atomicAdd(&counters[3], 1u); // shard sub-budget (see header)
                if slot < shard_budget() {
                    p = emit_shard(idx, max(s, 0.08));
                    emitted = true;
                }
            }
        } else {
            // Steady voice. The gate is the density governor: ~170K alive in
            // silence, saturating to the full pool on sustained harmonic
            // content — the swell. The ribbon look is DENSITY-based: many dim
            // particles braided into filaments by the ridge drift below (a
            // 16-frame trail at drift speed is a dot — strokes come from
            // overlap, not individual trails).
            let gate = 0.10 + 0.45 * smoothstep(0.05, 0.6, u.harmonic_energy);
            if uhash_f(idx + uhash(u32(u.time * 7.0) ^ 0x517cc1b7u)) < gate {
                let slot = emit_claim();
                if slot < u.emit_count - shard_budget() {
                    p = emit_ribbon(idx);
                    emitted = true;
                }
            }
        }
        if emitted {
            // Clear this particle's trail ring so ribbons never connect the
            // previous life's death point to the new spawn.
            if u.trail_length >= 2u {
                for (var s = 0u; s < u.trail_length; s++) {
                    trail_buffer[idx * u.trail_length + s] = vec4f(p.pos_life.xy, p.vel_size.w, 0.0);
                }
            }
            write_particle(idx, p);
            mark_alive(idx);
            return;
        }
        write_particle(idx, p);
        return;
    }

    let age = p.flags.x;
    let max_life = p.flags.y;
    let voice = p.flags.z;
    let new_age = age + u.delta_time;
    if new_age >= max_life {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    let dt = u.delta_time;
    let life_frac = new_age / max_life;
    var pos = p.pos_life.xy;
    var vel = p.vel_size.xy;
    let init_size = p.vel_size.z;
    var size = init_size;
    var alpha = 0.0;
    var col = vec3f(0.0);

    if voice > 0.5 {
        // --- SHARD: ballistic needle ---
        let sw = p.flags.w;
        vel *= pow(u.drag, dt * 60.0);
        let prev_pos = pos;
        pos += vel * dt;
        let coll = apply_obstacle_collision(pos, vel, prev_pos);
        pos = coll.xy;
        vel = coll.zw;
        if abs(pos.x) > 1.25 || abs(pos.y) > 1.25 {
            p.pos_life.w = 0.0;
            write_particle(idx, p);
            return;
        }
        size = init_size * (1.0 - 0.4 * life_frac);
        // Bright head, fast taper — closed form, no curve LUTs. Brief fade-in
        // so a burst's opening frame doesn't slam the additive stack.
        alpha = (0.08 + 0.25 * sw) * pow(1.0 - life_frac, 1.5) * smoothstep(0.0, 0.08, life_frac);
        col = shard_color(sw);
    } else {
        // --- RIBBON: relax toward the shared curl field ---
        let phase = p.flags.w;
        let v_field = ribbon_field(pos, phase);
        // Pads stiffen threads into glass (Tide relaxation idiom).
        let k = mix(1.5, 6.0, clamp(u.harmonic_energy, 0.0, 1.0));
        vel = mix(vel, v_field, 1.0 - exp(-k * dt));
        // Slight compressibility: drift toward ridges of a slow noise field
        // so uniform spawn braids into filaments — divergence-free curl alone
        // keeps density flat forever (the pre-integer-hash 'threads' were a
        // spawn-clustering artifact). Stronger with harmonic content.
        let e = 0.02;
        let np = pos * 1.3 + vec2f(phase * 3.7, u.time * 0.03);
        let g = vec2f(
            phosphor_noise2(np + vec2f(e, 0.0)) - phosphor_noise2(np - vec2f(e, 0.0)),
            phosphor_noise2(np + vec2f(0.0, e)) - phosphor_noise2(np - vec2f(0.0, e)));
        vel += g * (8.0 + 10.0 * u.harmonic_energy) * dt;
        vel *= pow(u.drag, dt * 60.0);
        let prev_pos = pos;
        pos += vel * dt;
        let coll = apply_obstacle_collision(pos, vel, prev_pos);
        pos = coll.xy;
        vel = coll.zw;
        if abs(pos.x) > 1.3 || abs(pos.y) > 1.3 {
            p.pos_life.w = 0.0;
            write_particle(idx, p);
            return;
        }
        size = init_size * eval_size_curve(life_frac) * (0.75 + 0.7 * u.harmonic_energy);
        let fade_in = smoothstep(0.0, 0.06, life_frac);
        let fade_out = 1.0 - smoothstep(0.85, 1.0, life_frac);
        // Dominance floor 0.55: ribbons dim during breaks, never vanish.
        let dom = 0.55 + 0.45 * smoothstep(0.2, 0.8, eff_ratio());
        alpha = (0.06 + 0.10 * param(2u)) * fade_in * fade_out * eval_opacity_curve(life_frac) * dom;
        col = ribbon_color(vel);
    }

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, init_size, size);
    p.color = vec4f(col, alpha);
    p.flags = vec4f(new_age, max_life, voice, p.flags.w);
    write_particle(idx, p);
    mark_alive(idx);
    trail_write(idx, vec4f(pos, size, alpha));
}
