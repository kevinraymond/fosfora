// Protea — mass pass: reintegration-tracking transport + signed Lenia reaction (Flow Lenia).
//   feedback() = own mass field, previous frame (rgb = species A/B/C, a = total)
//   input0     = potential/growth, this frame (rgb = per-species SIGNED growth G, a = mean U)
//
// Two coupled dynamics on a mass-carrying field:
//
//  1) Transport (mass-CONSERVING). Every source cell moves its mass one sub-texel step
//     along its flow F = (1-alpha)*grad(G) - alpha*grad(A): toward higher growth when
//     sparse, spreading down its own gradient when overcrowded (alpha rises with local
//     total mass). Each destination gathers the 3x3 source window and sums the analytic
//     overlap of each moved unit-square with this cell, so a source's mass lands exactly
//     once — matter is conserved and creatures keep persistent bodies as they flow.
//
//  2) Reaction (Lenia growth). m += dt*metabolism*G*m with SIGNED G: mass grows where the
//     sensed density is optimal (G>0) and DIES where it is empty or overcrowded (G<0).
//     That death is what carves empty water between creatures — without it the field
//     fills to a uniform wash. Metabolism is driven by loudness, so silence stops growth
//     while a steady maintenance decay starves the ecosystem until it visibly shrinks.
//
// A light box diffusion keeps the field smooth enough for the ring kernel to sense
// coherent bodies instead of per-pixel speckle. Feedback pass at scale 0.5.

fn mass_at(p: vec2i, dims: vec2i) -> vec3f {
    return textureLoad(prev_frame, clamp(p, vec2i(0), dims - 1), 0).rgb;
}

fn growth_at(p: vec2i, dims: vec2i) -> vec3f {
    return textureLoad(input0_tex, clamp(p, vec2i(0), dims - 1), 0).rgb;
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2i(textureDimensions(prev_frame));
    let dest = vec2i(frag_coord.xy);

    let sim_speed = 0.3 + 1.2 * param(0u);  // p0
    let food_p = 0.4 + param(4u) * 1.6;     // p4 metabolism gain
    let injection = param(6u) * 2.0;        // p6 onset droplet mass
    let viscosity = param(7u);              // p7 flow damping

    let dt = clamp(u.delta_time, 0.0, 0.05) * 60.0;
    // Beat strength thins the medium — creatures surge with the groove.
    let speed = dt * sim_speed * mix(1.2, 0.4, viscosity) * (1.0 + 0.6 * u.beat_strength);

    // --- gather: 3x3 reintegration transport + neighbourhood average for diffusion ---
    var m = vec3f(0.0);
    var avg = vec3f(0.0);
    var navg = 0.0;
    for (var oy = -1; oy <= 1; oy = oy + 1) {
        for (var ox = -1; ox <= 1; ox = ox + 1) {
            let s = dest + vec2i(ox, oy);
            if (any(s < vec2i(0)) || any(s >= dims)) {
                continue; // borders absorb
            }
            let m_s = mass_at(s, dims);
            avg += m_s;
            navg += 1.0;
            if (m_s.r + m_s.g + m_s.b < 1e-5) {
                continue;
            }
            // Central differences of G and mass at the source (all three species per tap).
            let gl = growth_at(s - vec2i(1, 0), dims);
            let gr = growth_at(s + vec2i(1, 0), dims);
            let gb = growth_at(s - vec2i(0, 1), dims);
            let gt = growth_at(s + vec2i(0, 1), dims);
            let ml = mass_at(s - vec2i(1, 0), dims);
            let mr = mass_at(s + vec2i(1, 0), dims);
            let mb = mass_at(s - vec2i(0, 1), dims);
            let mt = mass_at(s + vec2i(0, 1), dims);
            // Overcrowding pressure: growth-seeking when sparse, self-spreading near capacity.
            let total_s = m_s.r + m_s.g + m_s.b;
            let alpha = clamp(total_s * total_s, 0.0, 1.0);
            // Per species (x/y component triples). Clamp each axis below one texel so the
            // moved square stays inside the gathered 3x3 window.
            let fx = ((gr - gl) * (1.0 - alpha) * 10.0 - (mr - ml) * alpha * 3.0) * 0.5;
            let fy = ((gt - gb) * (1.0 - alpha) * 10.0 - (mt - mb) * alpha * 3.0) * 0.5;
            let step_x = clamp(fx * speed, vec3f(-0.9), vec3f(0.9));
            let step_y = clamp(fy * speed, vec3f(-0.9), vec3f(0.9));
            let fs = vec2f(s) - vec2f(dest);
            let wx = max(vec3f(0.0), vec3f(1.0) - abs(vec3f(fs.x) + step_x));
            let wy = max(vec3f(0.0), vec3f(1.0) - abs(vec3f(fs.y) + step_y));
            m += m_s * wx * wy;
        }
    }
    // Light diffusion — smooth the field so the kernel senses bodies, not speckle.
    avg /= max(navg, 1.0);
    m = mix(m, avg, 0.14);

    // --- reaction: signed Lenia growth, driven by loudness ---
    // A gentle reaction timestep keeps explicit Euler stable: at dt=1 a raw m*(1.08*G)
    // step with G=-1 would drive mass negative in one frame and annihilate the field
    // before it can establish. rdt<1 makes negative growth DECAY mass instead of clearing
    // it, so creatures can grow through low density on their way to a stable body.
    let rdt = dt * 0.3;
    let g_here = growth_at(dest, dims);
    let food_env = clamp(u.rms * 3.0, 0.0, 1.0);
    let metabolism = food_p * food_env;               // 0 when silent → no growth
    let maintenance = 0.05 + 0.10 * (1.0 - food_env); // starvation: a shrink in a pause,
    m += rdt * m * (metabolism * g_here - maintenance); // near-total over a long silence

    // Onset droplets: mass rain at hash-jittered sites that re-seat every ~2s.
    if (u.onset > 0.02) {
        let cell = floor(u.time * 0.53);
        let px = vec2f(frag_coord.xy);
        let min_dim = f32(min(dims.x, dims.y));
        let rr = 0.028 * min_dim;
        for (var k = 0u; k < 5u; k = k + 1u) {
            let site = vec2f(
                0.08 + 0.84 * phosphor_hash2(vec2f(f32(k) * 13.7 + 1.0, cell)),
                0.08 + 0.84 * phosphor_hash2(vec2f(cell, f32(k) * 7.3 + 41.0)),
            ) * vec2f(dims);
            let d = px - site;
            m[k % 3u] += u.onset * injection * exp(-dot(d, d) / (rr * rr));
        }
    }

    // Smooth sparse nutrient seeding — one species per low-frequency blob, so the field
    // has coherent structure at the creature scale for the kernel to organise. Gated on
    // headroom and loudness: bootstraps an empty field and leaves faint embers in silence.
    let total = m.r + m.g + m.b;
    let headroom = clamp(1.0 - total / 1.5, 0.0, 1.0);
    let world = vec2f(frag_coord.xy) / f32(min(dims.x, dims.y));
    let seed = vec3f(
        smoothstep(0.74, 0.88, phosphor_noise3(vec3f(world * 4.0, u.time * 0.04))),
        smoothstep(0.74, 0.88, phosphor_noise3(vec3f(world * 4.0 + 23.0, u.time * 0.04))),
        smoothstep(0.74, 0.88, phosphor_noise3(vec3f(world * 4.0 + 61.0, u.time * 0.04))),
    );
    m += dt * 0.12 * (0.05 + 0.95 * food_env) * seed * headroom;

    // Feedback-loop safety clamp.
    m = clamp(m, vec3f(0.0), vec3f(1.6));
    return vec4f(m, m.r + m.g + m.b);
}
