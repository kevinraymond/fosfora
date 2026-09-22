// Fluvid — the pressure solve ∇²p = div, by red-black successive over-relaxation.
//   feedback() = the previous iteration: r = pressure, g = checkerboard parity
//   input0     = divergence, fixed across the whole loop
//
// Sumi's Jacobi solve (sumi_pressure.wgsl) needs iterations in proportion to
// the grid's size squared to converge, and an unconverged solve is frame-rate
// dependent: the pass runs a fixed count per FRAME, so a faster frame rate
// solves more per second and the fluid behaves differently (#3077 measured
// 120 Hz 39% dimmer before the grid shrank, still ~6% after 96 iterations).
// Over-relaxed Gauss-Seidel converges in far fewer iterations.
//
// Gauss-Seidel must update the two colors of a checkerboard in turn, and this
// pass has no iteration index. So every texel carries the parity in g and
// flips it each iteration: they all start at 0 (a cleared target) and flip
// together, so the whole grid agrees on which color is updating. The pass's
// iteration count is even, so each frame starts on the same color.

// Over-relaxation factor: 2 / (1 + sin(π/N)) is optimal for an N-wide grid;
// 1.9 suits grids of a few hundred cells, and anything in (1, 2) converges.
const FLUVID_SOR: f32 = 1.9;

fn fluvid_finite(x: f32) -> bool {
    return (bitcast<u32>(x) & 0x7f800000u) != 0x7f800000u;
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(prev_frame));
    let texel = 1.0 / dims;
    let uv = frag_coord.xy / dims;

    let here = feedback(uv);
    var p = here.r;
    if (!fluvid_finite(p)) {
        p = 0.0;
    }
    let parity = select(0u, 1u, here.g > 0.5);
    let cell = vec2u(frag_coord.xy);

    if (((cell.x + cell.y) & 1u) == parity) {
        let pl = feedback(uv - vec2f(texel.x, 0.0)).r;
        let pr = feedback(uv + vec2f(texel.x, 0.0)).r;
        let pb = feedback(uv - vec2f(0.0, texel.y)).r;
        let pt = feedback(uv + vec2f(0.0, texel.y)).r;
        let gs = (pl + pr + pb + pt - input0(uv).x) * 0.25;
        if (fluvid_finite(gs)) {
            p = mix(p, gs, FLUVID_SOR);
        }
    }
    return vec4f(p, 1.0 - f32(parity), 0.0, 1.0);
}
