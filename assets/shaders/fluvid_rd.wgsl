// Fluvid — Gray-Scott reaction-diffusion living inside the ink.
//   feedback() = own previous sub-step: r = 1 − A, g = B (a cleared target is
//                therefore the rest state A = 1, B = 0, with nothing to prime)
//   input0     = velocity, this frame (sim grid texels/frame)
//   input1     = motion, this frame (z = gate)
//   input2     = dye, PREVIOUS frame
//
// The pass runs FLUVID_RD_STEPS times a frame with no iteration index, so every
// sub-step is identical: each carries the chemistry 1/N of the frame's advection
// (the fluid moves the maze) and one reaction step. Motion sparks B; outside the
// dye an extra kill term starves B, so the maze grows where the ink is and fades
// where it has gone.

// MUST equal the rd pass's "iterations" in fluvid.pfx.
const FLUVID_RD_STEPS: f32 = 32.0;

// Non-finite guard. A layer below can hand this effect infinity or NaN (an
// overflowing effect, an HDR source), and this pass feeds back on itself, so
// one bad texel would never wash out and the effect would stay black. Tested
// on the bit pattern (all exponent bits set) because a NaN self-comparison
// may be folded away by the compiler.
fn fluvid_finite(x: f32) -> bool {
    return (bitcast<u32>(x) & 0x7f800000u) != 0x7f800000u;
}

fn fluvid_clean(v: vec4f, rest: vec4f) -> vec4f {
    return select(rest, v, vec4<bool>(fluvid_finite(v.x), fluvid_finite(v.y), fluvid_finite(v.z), fluvid_finite(v.w)));
}

fn fluvid_ab(uv: vec2f) -> vec2f {
    let s = feedback(uv);
    return vec2f(1.0 - s.r, s.g);
}

// Energy (p0), −1..+1 → flow speed. 0 is the old default (1.2); the curve is
// exponential so each step of the slider is the same RATIO faster or slower:
// −1 slows it to 0.1 (smoke that lingers where it was breathed), +1 is 3.2.
fn fluvid_energy() -> f32 {
    let e = clamp(param(0u), -1.0, 1.0);
    return 1.2 * exp2(e * select(1.42, 3.58, e < 0.0));
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(prev_frame));
    let texel = 1.0 / dims;
    let uv = frag_coord.xy / dims;

    let energy = fluvid_energy();                  // p0 flow speed
    let audio = param(13u);                        // p13
    let dt = clamp(u.delta_time, 0.0, 0.05) * 60.0 * energy;

    // Advect by this sub-step's share of the frame. Velocity is in its own grid's
    // texels; convert through that grid's size.
    let vel_texel = 1.0 / vec2f(textureDimensions(input0_tex));
    let back = uv - (dt / FLUVID_RD_STEPS) * input0(uv).xy * vel_texel;

    let c = fluvid_ab(back);
    let lap = 0.25 * (fluvid_ab(back + vec2f(texel.x, 0.0)) + fluvid_ab(back - vec2f(texel.x, 0.0))
        + fluvid_ab(back + vec2f(0.0, texel.y)) + fluvid_ab(back - vec2f(0.0, texel.y))) - c;

    // Pattern size. Gray-Scott's wavelength is fixed in grid cells for fixed
    // diffusion; scaling both diffusion rates by k² scales it by k. RD scale (p5)
    // sets k, and so does this grid's height against 540 rows (this pass at 4K),
    // so the maze is the same fraction of the frame at 1080p as at 4K. A stride
    // in the stencil would scale it too, but an integer stride splits the grid
    // into independent interleaved sub-grids that never exchange chemistry.
    let k = mix(0.8, 2.4, param(5u)) * dims.y / 540.0;
    let da = 1.0 * k * k;
    let db = 0.5 * k * k;

    // Step size. Explicit Euler on this normalized stencil is stable while
    // h·D_A ≤ 1. The 60 fps step is at most 0.45/D_A and frame_steps is at most
    // 2, so no frame rate crosses the limit and the maze grows at the same speed
    // in seconds from 30 fps up. RD rate (p6) picks where in that range it runs.
    let h = mix(0.15, 0.45, param(6u)) / max(da, 1.0) * frame_steps();

    // Coral/labyrinth regime. Bass nudges the kill rate: the walls thicken and thin
    // with the low end without leaving the maze regime.
    let feed = 0.0545;
    let dye_l = dot(input2(uv).rgb, vec3f(0.299, 0.587, 0.114));
    let starve = (1.0 - smoothstep(0.003, 0.04, dye_l)) * 0.012;
    let kill = 0.062 + audio * (u.bass - 0.5) * 0.002 + starve;

    let a = c.x;
    let b = c.y;
    let react = a * b * b;
    var na = a + h * (da * lap.x - react + feed * (1.0 - a));
    var nb = b + h * (db * lap.y + react - (kill + feed) * b);

    // Motion NUCLEATES B: sparse sparks (about 3% of texels, re-rolled 30 times a
    // second whatever the frame rate) that the Turing instability grows into walls.
    // A uniform seed across the moving region pinned the chemistry at a fixed
    // point (A near 0, B high) for as long as the hand kept moving, and no maze
    // ever formed there.
    let tick = floor(u.time * 30.0);
    let spark = step(0.97, fosfora_hash3(vec3f(floor(uv * dims), tick)));
    nb += input1(uv).z * spark * 0.5 * h;

    na = clamp(na, 0.0, 1.0);
    nb = clamp(nb, 0.0, 1.0);
    return fluvid_clean(vec4f(1.0 - na, nb, 0.0, 1.0), vec4f(0.0, 0.0, 0.0, 1.0));
}
