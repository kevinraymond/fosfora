// Fluvid — project + advect + forces; owns the velocity field. Sumi's solver
// (sumi_velocity.wgsl) with the camera's motion as the force.
//   feedback() = own velocity, previous frame
//   input0     = pressure, this frame (solved from the divergence of the previous velocity)
//   input1     = motion, this frame (xy = push direction × gate, z = gate)

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

fn fluvid_vel(uv: vec2f) -> vec2f { return feedback(uv).xy; }
fn fluvid_p(uv: vec2f) -> f32 { return input0(uv).x; }

// Projected velocity at uv: V_prev(uv) − grad(pressure)(uv).
fn fluvid_proj(uv: vec2f, texel: vec2f) -> vec2f {
    let pl = fluvid_p(uv - vec2f(texel.x, 0.0));
    let pr = fluvid_p(uv + vec2f(texel.x, 0.0));
    let pb = fluvid_p(uv - vec2f(0.0, texel.y));
    let pt = fluvid_p(uv + vec2f(0.0, texel.y));
    return fluvid_vel(uv) - 0.5 * vec2f(pr - pl, pt - pb);
}

fn fluvid_curl(uv: vec2f, texel: vec2f) -> f32 {
    let vl = fluvid_vel(uv - vec2f(texel.x, 0.0));
    let vr = fluvid_vel(uv + vec2f(texel.x, 0.0));
    let vb = fluvid_vel(uv - vec2f(0.0, texel.y));
    let vt = fluvid_vel(uv + vec2f(0.0, texel.y));
    return 0.5 * ((vr.y - vl.y) - (vt.x - vb.x));
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
    let force = param(1u) * 3.0;               // p1 splat force
    let vort_amt = param(9u) * 0.6;            // p9 vorticity confinement
    let audio = param(13u);                    // p13 music reactivity

    let dt = clamp(u.delta_time, 0.0, 0.05) * 60.0 * energy;

    // 1) self-advect the projected field
    let v_here = fluvid_proj(uv, texel);
    var vel = fluvid_proj(uv - dt * v_here * texel, texel);

    // 2) vorticity confinement, sharpened by spectral flux
    let w = fluvid_curl(uv, texel);
    let wl = abs(fluvid_curl(uv - vec2f(texel.x, 0.0), texel));
    let wr = abs(fluvid_curl(uv + vec2f(texel.x, 0.0), texel));
    let wb = abs(fluvid_curl(uv - vec2f(0.0, texel.y), texel));
    let wt = abs(fluvid_curl(uv + vec2f(0.0, texel.y), texel));
    let eta = 0.5 * vec2f(wr - wl, wt - wb);
    let n = eta / (length(eta) + 1e-5);
    vel += dt * vort_amt * (0.3 + audio * u.flux) * w * vec2f(n.y, -n.x);

    // 3) the camera's push. A per-frame force scaled by dt, so the same wave
    // stirs the same amount at any frame rate; hits and beats kick it harder.
    let m = input1(uv);
    let kick = 1.0 + audio * (1.5 * u.onset + u.beat);
    vel += dt * m.xy * force * kick;

    // 4) damping + magnitude clamp (feedback-loop safety)
    vel *= frame_decay(0.992);
    let sp = length(vel);
    if (sp > 6.0) { vel *= 6.0 / sp; }

    return fluvid_clean(vec4f(vel, 0.0, 1.0), vec4f(0.0, 0.0, 0.0, 1.0));
}
