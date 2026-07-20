// Tide particle simulation — depth-occlusion flow-around-body water (#1796)
//
// A luminous waterfall sheet pours from the top edge, then parts, pools,
// and eddies around obstacle silhouettes (webcam / MiDaS depth / image).
// The laminar look comes from a deterministic shared velocity field plus
// exponential relaxation toward it: neighboring particles ride identical
// streamlines, so the sheet moves as water instead of smoke. Drums
// (percussive_energy) break the sheet into whitewater and spray; pads
// (harmonic_energy) stiffen it into glass. Works with obstacle Flow Around
// mode (2); the anticipatory steering below keeps the look under any mode.
// First consumer of the ribbon trail renderer (trail_write).
//
// --- Param mapping ---
// param(0) = flow_speed     (base fall speed)
// param(1) = coherence      (lane snap + relaxation stiffness)
// param(2) = whitewater     (percussive breakup + splash amount)
// param(3) = pooling        (anticipatory pile-up on silhouettes)
// param(4) = eddy_strength  (curl at silhouette edges)
// param(5) = water_hue      (palette anchor offset)
// param(6) = shimmer        (bg shader: refraction warp)
// param(7) = trail_decay    (bg shader: feedback decay)

const N_LANES: f32 = 64.0;

// Shared laminar velocity field: base fall + divergence-free meander +
// field-coherent whitewater breakup. Deterministic in position so the
// sheet forms streamlines, never per-particle dither.
fn tide_field(pos: vec2f, lane_x: f32, agitation: f32) -> vec2f {
    let flow_speed = (0.35 + param(0u) * 0.9) * (0.8 + u.rms * 0.6);
    var v = vec2f(0.0, -flow_speed);

    // Meander: low-freq curl noise with y compressed 2.5:1 so features
    // elongate vertically — streaks, not blobs. coherence snaps the sample
    // x toward the particle's lane -> discrete waterfall strands.
    let coher = param(1u);
    let mx = mix(pos.x, lane_x, coher);
    let meander = curl_noise_2d(vec2f(mx * 1.5, pos.y * 0.6 - u.time * 0.05));
    v += meander * 0.12 * (1.0 - 0.7 * coher) * (0.6 + 0.4 * u.harmonic_ratio);

    // Whitewater: finer curl scrolling down with the flow; gain from
    // percussive_energy (drums make the sheet break) + local agitation.
    let ww = curl_noise_2d(pos * vec2f(6.0, 2.5) + vec2f(0.0, -u.time * 1.2));
    let ww_amp = (0.02 + 0.10 * param(2u)) * (0.25 + 1.5 * u.percussive_energy + agitation);
    v += ww * ww_amp;
    return v;
}

fn tide_color(pos: vec2f, vel: vec2f, foamy: f32) -> vec3f {
    // Deep water anchored to the musical key, pulled hard toward blue-cyan;
    // centroid tilts the palette warm/cool.
    let hue_t = fract(u.dominant_chroma * 0.15 + param(5u) * 0.25 + 0.52 + pos.y * 0.05);
    var col = phosphor_audio_palette(hue_t, 0.6 + 0.4 * clamp(u.centroid, 0.0, 1.0), u.time * 0.02);
    col = mix(col, vec3f(0.10, 0.35, 0.60), 0.45);
    let speed_glow = clamp(length(vel) * 1.6, 0.0, 1.0);
    col *= 0.08 + 0.35 * speed_glow + 0.15 * u.rms;
    // Foam whitens — the bright contour where water meets a silhouette.
    return mix(col, vec3f(0.85, 0.92, 1.0) * 0.7, clamp(foamy, 0.0, 1.0) * 0.75);
}

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let seed_base = u.seed + f32(idx) * 17.31;
    let res_x = u32(max(u.resolution.x, 1.0));

    // Deterministic column + sub-pixel jitter: gap-free sheet along the top
    // edge (cascade's perimeter-emitter pattern, top edge only), staggered
    // just above the screen so the sheet has no hard birth line.
    let t = ((f32(idx % res_x) + hash(seed_base)) / f32(res_x)) * 2.0 - 1.0;
    let pos = vec2f(t, 1.0 + hash(seed_base + 1.0) * 0.06);

    // Lane id: quantized column -> coherent strands (see tide_field).
    let lane_x = (floor((t * 0.5 + 0.5) * N_LANES) + 0.5) / N_LANES * 2.0 - 1.0;

    // Beat surge: fresh water pours faster right after the beat, so a pulse
    // front travels down the falls.
    let surge = 1.0 + 0.3 * u.beat_strength * exp(-u.beat_phase * 6.0);
    let speed = u.initial_speed * (0.85 + 0.3 * hash(seed_base + 2.0)) * surge;
    let vel = vec2f((hash(seed_base + 3.0) - 0.5) * 0.02, -speed);

    let init_size = u.initial_size * (0.7 + 0.6 * hash(seed_base + 4.0));
    let life = u.lifetime * (0.8 + 0.4 * hash(seed_base + 5.0));

    p.pos_life = vec4f(pos, 0.0, 1.0); // z = billboard spin: none
    p.vel_size = vec4f(vel, init_size, init_size); // z = persistent init size
    p.color = vec4f(tide_color(pos, vel, 0.0), 0.10);
    p.flags = vec4f(0.0, life, lane_x, 0.0); // (age, lifetime, lane_x, foam)
    return p;
}

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles { return; }

    var p = read_particle(idx);

    if p.pos_life.w <= 0.0 {
        let slot = emit_claim();
        if slot < u.emit_count {
            p = emit_particle(idx);
            // Clear this particle's trail ring so ribbons never connect the
            // previous life's death point to the new spawn. Valid position
            // with alpha 0 -> renderer emits fully transparent segments
            // (discarded) instead of skipping validity, keeping taper sane.
            if u.trail_length >= 2u {
                for (var s = 0u; s < u.trail_length; s++) {
                    trail_buffer[idx * u.trail_length + s] = vec4f(p.pos_life.xy, p.vel_size.w, 0.0);
                }
            }
            write_particle(idx, p);
            mark_alive(idx);
        } else {
            write_particle(idx, p);
        }
        return;
    }

    let age = p.flags.x;
    let max_life = p.flags.y;
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
    let lane_x = p.flags.z;
    var foam = p.flags.w;

    // --- Anticipatory obstacle response: pooling / parting / eddy ---
    // Runs BEFORE the collision call so water decelerates and steers on
    // approach instead of slamming into the surface. Costs 2 alpha taps in
    // free fall, +4 (normal) only near silhouettes; zero when disabled.
    var agitation = 0.0;
    if u.obstacle_enabled > 0.5 {
        let dir = normalize(vel + vec2f(0.0, -1e-4));
        // Occupancy relative to the collision threshold, NOT raw alpha: a
        // MiDaS depth field is continuous (a background wall reads ~0.3-0.6),
        // so raw-alpha proximity foamed the entire frame white on live depth.
        // The smoothstep band just below threshold means "approaching a
        // surface the collision test would actually count" — identical
        // behavior on binary images, threshold-aware on depth fields.
        let thr = u.obstacle_threshold;
        let a1 = smoothstep(thr - 0.2, thr, obstacle_alpha(pos + dir * 0.05));
        let a2 = smoothstep(thr - 0.2, thr, obstacle_alpha(pos + dir * 0.12));
        let prox = max(a1, a2 * 0.6);
        if prox > 0.02 {
            let n = obstacle_normal(pos + dir * 0.05);
            let asp = obstacle_aspect();
            var v_s = vel * asp;
            let vn = dot(v_s, n);
            // Pooling: bleed off inward velocity on approach — water piles
            // on heads and shoulders. pool < 1 keeps residual inward motion
            // so Flow collision still engages at the surface.
            let pool = param(3u) * smoothstep(0.0, 0.6, prox) * 0.9;
            v_s -= n * min(vn, 0.0) * pool;
            // Parting: steer along the tangent matching current motion
            // (spills left on left slopes, right on right); hash tie-break
            // at the exact crest so streams split both ways.
            let tang = vec2f(-n.y, n.x);
            var tdot = dot(v_s, tang);
            if abs(tdot) < 0.01 { tdot = hash(f32(idx) * 3.1) - 0.5; }
            let tdir = select(-tang, tang, tdot >= 0.0);
            v_s += tdir * abs(min(vn, 0.0)) * pool * 0.8;
            // Eddy: small signed velocity rotation near edges.
            let eddy = param(4u) * prox * 0.5 * sign(tdot);
            let ca = cos(eddy);
            let sa = sin(eddy);
            v_s = vec2f(v_s.x * ca - v_s.y * sa, v_s.x * sa + v_s.y * ca);
            vel = v_s / asp;
            // Steering strength follows the anticipatory prox, but the FOAM
            // color band uses the near tap only — a tight contour at the
            // surface, not the whole 0.12-lookahead approach zone.
            agitation = a1;
        }
    }

    // --- Laminar relaxation toward the shared field (THE water mechanism).
    // Continuously re-injects downward velocity, which is what keeps Flow
    // collision engaged (it only fires on inward motion). harmonic_energy
    // stiffens the sheet (pads = glass); foam flies ballistic with gravity
    // so spray arcs like real droplets.
    let v_field = tide_field(pos, lane_x, agitation);
    let k = mix(3.0, 9.0, clamp(0.3 + 0.7 * u.harmonic_energy, 0.0, 1.0)) * (0.4 + 0.6 * param(1u));
    let blend = (1.0 - exp(-k * dt)) * (1.0 - foam * 0.85);
    vel = mix(vel, v_field, blend);
    vel.y -= 1.8 * foam * dt;
    vel *= pow(u.drag, dt * 60.0);

    // --- Percussive splash: a hashed subset sprays upward as foam on hits.
    // Gated on HPSS percussive energy ONLY — the kick band false-fires on
    // sustained low sines (pads), and HPSS is precisely the pad/drum
    // discriminator this effect is built around.
    let hit = u.percussive_energy;
    if hit > 0.6 && hash(f32(idx) * 7.7 + floor(u.time * 20.0)) < (hit - 0.6) * (0.1 + 0.3 * param(2u)) {
        let a = (hash(f32(idx) + u.time) - 0.5) * 2.6;
        vel += vec2f(sin(a) * 0.6, abs(cos(a))) * 0.35 * hit * (0.5 + agitation);
        foam = 1.0;
    }
    foam *= exp(-dt * 2.2);

    // --- Integrate + canonical collision as the hard guarantee ---
    let prev_pos = pos;
    pos += vel * dt;
    let coll = apply_obstacle_collision(pos, vel, prev_pos);
    pos = coll.xy;
    vel = coll.zw;

    if pos.y < -1.3 || abs(pos.x) > 1.3 {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    // --- Size / alpha / color ---
    let size = init_size * eval_size_curve(life_frac);
    let fade_in = smoothstep(0.0, 0.04, life_frac);
    let fade_out = 1.0 - smoothstep(0.85, 1.0, life_frac);
    let alpha = 0.09 * fade_in * fade_out * eval_opacity_curve(life_frac);
    let col = tide_color(pos, vel, foam + agitation * 0.6);

    // Stall drain: water pooled on a large silhouette otherwise sits out its
    // full lifetime and starves the top-edge emitter (live check saturated the
    // cap at 98%). Agitated-but-slow particles age extra fast and recycle.
    let stall = agitation * (1.0 - clamp(length(vel) * 4.0, 0.0, 1.0));
    let drained_age = new_age + stall * dt * 3.0;

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, init_size, size);
    p.color = vec4f(col, alpha);
    p.flags = vec4f(drained_age, max_life, lane_x, foam);
    write_particle(idx, p);
    mark_alive(idx);
    trail_write(idx, vec4f(pos, size, alpha));
}
