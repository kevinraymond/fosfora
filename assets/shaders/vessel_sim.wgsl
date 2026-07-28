// Vessel particle simulation — body-as-container fill-and-release (#1797)
//
// The performer's silhouette (obstacle field: webcam / MiDaS depth / image)
// slowly fills with trapped light as a musical buildup rises; on the drop the
// accumulated particles burst outward carrying their stored velocity. With no
// obstacle armed, a centered SDF "amphora" takes the body's place.
//
// Containment is hand-rolled (obstacle_alpha / vessel_inward, the Contain-mode
// idiom from apply_obstacle_collision) rather than calling the canonical
// collision entry point: it must not depend on the obstacle-mode panel state —
// a .pfx cannot seed obstacle mode, and Vessel has to trap under ANY mode.
// The release latch is per-particle (flags.z): u.drop is a one-frame impulse
// counter-latched upstream, so every alive particle sees it in the same
// dispatch, flips its own flag, and flies free until death. Respawns are born
// contained — the vessel refills naturally during the drop refractory.
//
// --- Param mapping ---
// param(0) = fill_rate    (emission gate gain; u.buildup multiplies on top)
// param(1) = liquidity    (1 = liquid light: gravity pooling; 0 = trapped stars: curl drift)
// param(2) = swirl        (curl drift strength)
// param(3) = pressure     (interior jitter gain, charges with u.buildup)
// param(4) = burst_power  (outward impulse on release)
// param(5) = hue          (palette anchor offset, shared with bg)
// param(6) = glow         (bg shader: interior glow + feedback persistence)
// param(7) = release      (manual VJ trigger: > 0.5 forces release, OR'd with u.drop)

const TAU: f32 = 6.2831853;
// Soft half-width of the amphora's pseudo-alpha edge band.
const VESSEL_EDGE: f32 = 0.03;
// Amphora spine + radius in aspect-corrected screen space (y spans only
// +-res.y/res.x there — ~+-0.56 on 16:9 — so these read smaller than clip).
const AMPHORA_A: vec2f = vec2f(0.0, -0.20);
const AMPHORA_B: vec2f = vec2f(0.0, 0.10);
const AMPHORA_R: f32 = 0.22;

// Fallback vessel: rounded capsule in aspect-corrected screen space.
fn amphora_sd(pos: vec2f) -> f32 {
    let ps = pos * obstacle_aspect();
    return phosphor_sd_segment2(ps, AMPHORA_A, AMPHORA_B) - AMPHORA_R;
}

// Unified containment field: obstacle alpha when armed, else the amphora as a
// pseudo-alpha (1 deep inside, 0 outside, soft VESSEL_EDGE band). Branches on
// the enable flag, NOT on alpha — a disabled obstacle reads 0 everywhere.
fn vessel_alpha(pos: vec2f) -> f32 {
    if u.obstacle_enabled > 0.5 { return obstacle_alpha(pos); }
    return 1.0 - smoothstep(-VESSEL_EDGE, VESSEL_EDGE, amphora_sd(pos));
}

fn vessel_threshold() -> f32 {
    if u.obstacle_enabled > 0.5 { return u.obstacle_threshold; }
    return 0.5;
}

// Inward unit normal in SCREEN space (y-up). Obstacle path: the lib normal
// points away from high alpha, so negate. Amphora path: away from the nearest
// point on the capsule spine, negated.
fn vessel_inward(pos: vec2f) -> vec2f {
    if u.obstacle_enabled > 0.5 { return -obstacle_normal(pos); }
    let ps = pos * obstacle_aspect();
    let ba = AMPHORA_B - AMPHORA_A;
    let pa = ps - AMPHORA_A;
    let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    let d = pa - ba * h;
    let len = length(d);
    if len < 0.001 { return vec2f(0.0, 1.0); }
    return -d / len;
}

fn vessel_color(heat: f32, released: f32) -> vec3f {
    // Key-locked anchor; centroid tilts warm/cool (tide_color idiom).
    let hue_t = fract(u.dominant_chroma * 0.15 + param(5u) * 0.4 + 0.08);
    var col = phosphor_audio_palette(hue_t, 0.55 + 0.35 * clamp(u.centroid, 0.0, 1.0), u.time * 0.02);
    if released > 0.5 {
        // Burst flash spending the stored charge: near-white at full heat,
        // cooling to a key-colored ember as heat decays in flight.
        return mix(col, vec3f(1.0, 0.97, 0.90), 0.55 * heat) * (0.35 + 1.6 * heat);
    }
    // Contained simmer: dim, warming toward white-gold as the charge grows.
    col = mix(col, vec3f(1.0, 0.85, 0.60), heat * 0.35);
    return col * (0.07 + 0.10 * u.rms + 0.30 * heat);
}

// Interior rejection sampling: hashed full-screen candidates, accept the first
// safely inside. The +0.10 spawn margin keeps births off the boundary AND
// biases toward the true body on continuous MiDaS depth fields, where a back
// wall reads just above threshold (Tide's threshold-band reasoning). All
// candidates outside -> pos_life.w stays 0: the claimed emit slot is wasted,
// which is exactly the throttle a small silhouette needs.
fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    p.pos_life = vec4f(0.0);
    let seed_base = u.seed + f32(idx) * 17.31;
    let thr = vessel_threshold() + 0.10;
    for (var k = 0u; k < 6u; k++) {
        // Integer hash: these are the rejection sampler's CANDIDATES, so their
        // coverage of the silhouette is the spawn distribution. fract-sin banded a
        // contiguous range of slots onto near-identical candidates, so those slots
        // probed the same spot six times over — clustering births where it landed
        // inside the form, and permanently wasting the slot where it landed
        // outside. Same failure cymatics had, where whole regions went unseeded.
        let c = rand_vec2_u(seed_base + f32(k) * 2.0);
        if vessel_alpha(c) >= thr {
            let init_size = u.initial_size * (0.7 + 0.6 * hash(seed_base + 4.0));
            let life = u.lifetime * (0.7 + 0.6 * hash(seed_base + 5.0));
            p.pos_life = vec4f(c, 0.0, 1.0);
            p.vel_size = vec4f(rand_vec2(seed_base + 13.0) * u.initial_speed, init_size, init_size);
            p.color = vec4f(vessel_color(0.0, 0.0), 0.0);
            p.flags = vec4f(0.0, life, 0.0, 0.0); // (age, lifetime, released, heat)
            return p;
        }
    }
    return p;
}

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles { return; }

    var p = read_particle(idx);

    if p.pos_life.w <= 0.0 {
        let slot = emit_claim();
        // Fill gate: buildup drives the real spawn rate; emit_rate is only the
        // ceiling, and declining a claimed slot wastes it — that IS the rate
        // modulation. The 0.15 floor keeps an ambient simmer in silence.
        let gate = (0.15 + 0.85 * u.buildup) * (0.3 + 0.7 * param(0u));
        // Integer hash: fract-sin banded a range of slots onto near-zero, so they
        // passed even the 0.15 silence floor every frame (a permanent, audio-deaf
        // fountain) while the complementary band never spawned at all. Re-rolls
        // per frame via u.seed, as before.
        if slot < u.emit_count && uhash_f(idx + uhash(u32(u.seed * 256.0))) < gate {
            p = emit_particle(idx);
            if p.pos_life.w > 0.0 {
                // Clear the trail ring so ribbons never connect the previous
                // life's death point to the new spawn.
                if u.trail_length >= 2u {
                    for (var s = 0u; s < u.trail_length; s++) {
                        trail_buffer[idx * u.trail_length + s] = vec4f(p.pos_life.xy, p.vel_size.w, 0.0);
                    }
                }
                write_particle(idx, p);
                mark_alive(idx);
                return;
            }
        }
        write_particle(idx, p);
        return;
    }

    let age = p.flags.x;
    let max_life = p.flags.y;
    var released = p.flags.z;
    var heat = p.flags.w;

    // Released debris ages 1.5x so the burst clears well inside the drop
    // refractory and the refill reads.
    let new_age = age + u.delta_time * (1.0 + released * 0.5);
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
    let liq = param(1u);
    let asp = obstacle_aspect();

    // --- Release trigger ---
    // age > 0.2 s so a HELD manual release reads as a fountain (particles
    // charge briefly, then vent) instead of an always-empty vessel.
    if released < 0.5 && new_age > 0.2 && (u.drop > 0.5 || param(7u) > 0.5) {
        released = 1.0;
        // Radial burst from screen center, hashed at dead center. NOT
        // obstacle_normal: deep inside a solid silhouette its gradient is
        // degenerate and the fallback points TOWARD screen center — exactly
        // backwards for a burst.
        var r_s = pos * asp;
        if length(r_s) < 0.03 { r_s = rand_vec2(f32(idx) * 7.7); }
        let burst_dir = normalize(normalize(r_s) + rand_vec2(f32(idx) * 11.3 + u.seed) * 0.35);
        let power = param(4u);
        heat = max(heat, 0.8);
        vel = vel * (1.0 + 2.0 * power) + (burst_dir * (0.4 + 1.2 * power) * (0.5 + heat)) / asp;
    }

    if released < 0.5 {
        // Contained physics: liquidity morphs pooling liquid <-> drifting
        // stars. Gravity accumulates (the fill line IS piled particles);
        // the star field is a relaxation target so drift stays coherent.
        vel.y -= 1.4 * liq * dt;
        let v_drift = curl_noise_2d(pos * 2.2 + vec2f(0.05, 0.07) * u.time)
            * (0.05 + 0.30 * param(2u)) * mix(1.0, 0.3, liq);
        let k_star = 2.5 * (1.0 - liq);
        vel = mix(vel, v_drift, 1.0 - exp(-k_star * dt));
        // Interior pressure: hashed shiver charging with the buildup.
        vel += rand_vec2(f32(idx) * 5.3 + floor(u.time * 30.0)) * 3.0 * param(3u) * u.buildup * dt;
        // Bar-clock slosh, liquid only.
        vel.x += sin(u.bar_phase * TAU) * 0.5 * liq * dt;
        heat = min(1.0, heat + u.buildup * dt * 0.25);
    } else {
        heat *= exp(-dt * 0.8);
    }
    vel *= pow(u.drag, dt * 60.0);

    let prev_pos = pos;
    pos += vel * dt;

    // --- Hand-rolled containment (contained particles only) ---
    var stranded = 0.0;
    if released < 0.5 {
        let thr = vessel_threshold();
        if vessel_alpha(pos) < thr {
            let n_in = vessel_inward(pos); // screen space, y-up
            if vessel_alpha(prev_pos) >= thr {
                // Crossed the wall this step: binary-search the boundary
                // (the apply_obstacle_collision Contain idiom), park just
                // inside, reflect if still moving outward. Response math in
                // screen space so bounces look right on any viewport.
                var lo = 0.0;
                var hi = 1.0;
                for (var i = 0; i < 4; i++) {
                    let mid = (lo + hi) * 0.5;
                    if vessel_alpha(mix(prev_pos, pos, mid)) < thr { hi = mid; } else { lo = mid; }
                }
                pos = mix(prev_pos, pos, lo) + normalize(n_in / asp) * 0.002;
                let v_s = vel * asp;
                let v_dot_n = dot(v_s, n_in);
                if v_dot_n < 0.0 {
                    // Stars ping off the wall; liquid plops against it.
                    vel = ((v_s - n_in * 2.0 * v_dot_n) * mix(0.55, 0.15, liq)) / asp;
                }
            } else {
                // Stranded outside — the silhouette moved off us. Soft
                // recapture spring plus fast-aging below, so a stubborn halo
                // drains instead of orbiting the body (Tide stall-drain idiom).
                vel = (vel * asp + n_in * 3.0 * dt) / asp;
                stranded = 1.0;
            }
        }
    }

    if abs(pos.x) > 1.3 || abs(pos.y) > 1.3 {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    // --- Size / alpha / color ---
    let size = init_size * eval_size_curve(life_frac) * (1.0 + released * 0.5);
    let fade_in = smoothstep(0.0, 0.06, life_frac);
    let fade_out = 1.0 - smoothstep(0.85, 1.0, life_frac);
    // Contained alpha stays low: the fill reads through additive DENSITY of
    // the trapped population, not per-particle brightness.
    let alpha = mix(0.10, 0.16, released) * fade_in * fade_out * eval_opacity_curve(life_frac);
    let col = vessel_color(heat, released);

    let drained_age = new_age + stranded * dt * 3.0;
    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, init_size, size);
    p.color = vec4f(col, alpha);
    p.flags = vec4f(drained_age, max_life, released, heat);
    write_particle(idx, p);
    mark_alive(idx);
    trail_write(idx, vec4f(pos, size, alpha));
}
