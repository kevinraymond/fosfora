// Protea — mass pass: condensing transport + clump-gated growth (Flow Lenia core).
//   feedback() = own mass field, previous frame (rgb = species A/B/C, a = total)
//   input0     = scent field, this frame (rgb = per-species ring density, a = total density)
//
// Two coupled dynamics on a mass-carrying field:
//
//  1) Transport (mass-CONSERVING). Every source cell moves its mass one sub-texel step
//     UP its own scent gradient (toward prey and its own density → condensation and
//     chasing) minus a pressure term down the total-density gradient (so dense bodies
//     don't collapse to a point). Each destination gathers the 3x3 source window and sums
//     the analytic overlap of each moved unit-square with this cell, so a source's mass
//     lands exactly once — matter is conserved and bodies stay coherent as they flow.
//
//  2) Growth (clump-GATED). m += rdt*m*(metabolism*clumped - maintenance). `clumped` is 1
//     only where a species' scent is already high (inside a body) and 0 in thin water, so
//     mass sustains and grows inside clumps and DIES between them. That is what carves the
//     empty water — without it the field tiles the plane. Metabolism is driven by loudness,
//     so silence stops growth while an always-on cost starves the colony until it shrinks.
//
// A light box diffusion keeps bodies smooth. Feedback pass at scale 0.5.

// Flow gains.
const ATTRACT: f32 = 5.0;  // pull up the scent gradient (condensation + chasing)
const PRESS: f32 = 3.0;    // push down the density gradient at high density (anti-collapse)
// Clump gate: a species only grows where its scent exceeds THR_HI; below THR_LO it is thin
// water and only decays. CAP_K is the per-pixel carrying capacity — growth saturates as a
// body fills up, so clumps reach a stable density and stop spreading instead of blowing out.
const THR_LO: f32 = 0.15;
const THR_HI: f32 = 0.37;
const CAP_K: f32 = 1.2;

fn mass_at(p: vec2i, dims: vec2i) -> vec3f {
    return textureLoad(prev_frame, clamp(p, vec2i(0), dims - 1), 0).rgb;
}

fn scent_at(p: vec2i, dims: vec2i) -> vec4f {
    return textureLoad(input0_tex, clamp(p, vec2i(0), dims - 1), 0);
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2i(textureDimensions(prev_frame));
    let dest = vec2i(frag_coord.xy);

    let sim_speed = 0.3 + 1.2 * param(0u);  // p0
    let food_p = 0.7 + param(4u) * 1.8;     // p4 metabolism gain
    let injection = param(6u) * 2.0;        // p6 onset droplet mass
    let viscosity = param(7u);              // p7 flow damping

    let dt = clamp(u.delta_time, 0.0, 0.05) * 60.0;
    // Beat strength thins the medium — creatures surge with the groove.
    let speed = dt * sim_speed * mix(1.2, 0.4, viscosity) * (1.0 + 0.6 * u.beat_strength);

    // --- gather: 3x3 condensing transport + neighbourhood average for diffusion ---
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
            // Scent + total gradients at the source (central differences).
            let sl = scent_at(s - vec2i(1, 0), dims);
            let sr = scent_at(s + vec2i(1, 0), dims);
            let sb = scent_at(s - vec2i(0, 1), dims);
            let st = scent_at(s + vec2i(0, 1), dims);
            let total_s = m_s.r + m_s.g + m_s.b;
            let alpha = clamp(total_s * total_s, 0.0, 1.0);
            // Per species (x/y triples): up own scent gradient, down the total-density
            // gradient at high local density. Clamp each axis below one texel so the moved
            // square stays inside the gathered 3x3 window.
            let dSx = (sr.rgb - sl.rgb) * 0.5;
            let dSy = (st.rgb - sb.rgb) * 0.5;
            let dTx = (sr.a - sl.a) * 0.5;
            let dTy = (st.a - sb.a) * 0.5;
            let fx = dSx * ATTRACT - vec3f(dTx) * PRESS * alpha;
            let fy = dSy * ATTRACT - vec3f(dTy) * PRESS * alpha;
            let step_x = clamp(fx * speed, vec3f(-0.9), vec3f(0.9));
            let step_y = clamp(fy * speed, vec3f(-0.9), vec3f(0.9));
            let fs = vec2f(s) - vec2f(dest);
            let wx = max(vec3f(0.0), vec3f(1.0) - abs(vec3f(fs.x) + step_x));
            let wy = max(vec3f(0.0), vec3f(1.0) - abs(vec3f(fs.y) + step_y));
            m += m_s * wx * wy;
        }
    }
    // Light diffusion — keep bodies smooth, not speckled.
    avg /= max(navg, 1.0);
    m = mix(m, avg, 0.16);

    // --- growth: clump-gated with a carrying cap, driven by loudness ---
    // A gentle reaction timestep keeps explicit Euler stable (a raw m*growth step at dt=1
    // would drive mass negative in one frame). `gate` (scent above THR_HI) is 1 in a body
    // and 0 in thin water, and `cap` saturates growth as the pixel fills toward CAP_K. So
    // growth adds mass only inside clumps and only until they reach a stable density, while
    // the always-on maintenance cost clears the thin water between them and the attraction
    // above vacuums it into the bodies — together, the empty water between creatures.
    let total_pre = m.r + m.g + m.b;
    let rdt = dt * 0.3;
    let S = scent_at(dest, dims).rgb;
    let gate = smoothstep(vec3f(THR_LO), vec3f(THR_HI), S);
    let cap = clamp(1.0 - total_pre / CAP_K, 0.0, 1.0);
    let food_env = clamp(u.rms * 3.0, 0.0, 1.0);
    let metabolism = food_p * food_env;                // 0 when silent → no growth
    let maintenance = 0.16 + 0.14 * (1.0 - food_env);  // always-on cost; silence starves
    m += rdt * m * (metabolism * gate * cap - maintenance);

    let total = m.r + m.g + m.b;

    // Onset droplets: feed existing creatures and spark new ones on beats, at hash-jittered
    // sites that re-seat every ~2s.
    if (u.onset > 0.02) {
        let cell = floor(u.time * 0.53);
        let px = vec2f(frag_coord.xy);
        let min_dim = f32(min(dims.x, dims.y));
        let drop_r = 0.030 * min_dim;
        for (var k = 0u; k < 4u; k = k + 1u) {
            let site = vec2f(
                0.10 + 0.80 * fosfora_hash2(vec2f(f32(k) * 13.7 + 1.0, cell)),
                0.10 + 0.80 * fosfora_hash2(vec2f(cell, f32(k) * 7.3 + 41.0)),
            ) * vec2f(dims);
            let d = px - site;
            m[k % 3u] += u.onset * injection * exp(-dot(d, d) / (drop_r * drop_r));
        }
    }

    // Ambient nucleation — rare, concentrated new creatures born ONLY in dead water. A
    // uniform trickle everywhere refills the voids and fills the plane; instead, coarse
    // cells fire on a slow clock with low probability, each dropping one species as a
    // kernel-sized blob (dense enough to read as clumped and survive) ONLY where the water
    // is currently empty. Established bodies and the water around them are left alone, so
    // the colony stays a scatter of distinct creatures. Also bootstraps a cleared field.
    let cell_sz = 0.06 * f32(min(dims.x, dims.y));
    let gcell = floor(vec2f(frag_coord.xy) / cell_sz);
    let tick = floor(u.time * 0.7);
    let fire = fosfora_hash2(gcell * 1.7 + vec2f(tick * 2.3 + 0.5, tick * 5.1 + 0.5));
    if (fire > 0.972 && total < 0.05) {
        let center = (gcell + vec2f(0.5)) * cell_sz;
        let d = vec2f(frag_coord.xy) - center;
        let nr = cell_sz * 0.44;
        let g = exp(-dot(d, d) / (nr * nr));
        let pick = fosfora_hash2(gcell + vec2f(9.1, 3.7));
        var mask = vec3f(0.0);
        if (pick < 0.34) { mask.x = 1.0; } else if (pick < 0.67) { mask.y = 1.0; } else { mask.z = 1.0; }
        m += mask * g * (0.4 + 0.6 * food_env) * 0.4;
    }

    // Feedback-loop safety clamp.
    m = clamp(m, vec3f(0.0), vec3f(1.6));
    return vec4f(m, m.r + m.g + m.b);
}
