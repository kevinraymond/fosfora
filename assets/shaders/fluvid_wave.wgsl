// Fluvid — a water surface: the wave equation, driven by the camera's motion.
//   feedback() = own previous sub-step: r = height now, g = height one step ago
//   input0     = motion, this frame (z = gate)
//
// Leapfrog integration, FLUVID_WAVE_STEPS identical sub-steps a frame. A moving
// edge presses the surface down under it, so a hand drags a wake and a sudden
// move throws rings that spread, bounce off the frame's edges and come back.

// MUST equal the wave pass's "iterations" in fluvid.pfx.
const FLUVID_WAVE_STEPS: f32 = 16.0;

// Non-finite guard: see fluvid_cam.wgsl. This pass feeds back on itself.
fn fluvid_finite(x: f32) -> bool {
    return (bitcast<u32>(x) & 0x7f800000u) != 0x7f800000u;
}

fn fluvid_clean(v: vec4f, rest: vec4f) -> vec4f {
    return select(rest, v, vec4<bool>(fluvid_finite(v.x), fluvid_finite(v.y), fluvid_finite(v.z), fluvid_finite(v.w)));
}

fn fluvid_h(uv: vec2f) -> f32 {
    return fluvid_clean(feedback(uv), vec4f(0.0)).r;
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(prev_frame));
    let texel = 1.0 / dims;
    let uv = frag_coord.xy / dims;

    let here = fluvid_clean(feedback(uv), vec4f(0.0));
    let h = here.r;
    let h_prev = here.g;
    let lap = fluvid_h(uv + vec2f(texel.x, 0.0)) + fluvid_h(uv - vec2f(texel.x, 0.0))
        + fluvid_h(uv + vec2f(0.0, texel.y)) + fluvid_h(uv - vec2f(0.0, texel.y)) - 4.0 * h;

    // Wave speed: 0.3 frame heights a second, so ripples cross the frame in the
    // same time at any resolution. Courant number per 60 fps sub-step, capped at
    // 0.35 so that frame_steps (at most 2) keeps it under the 2D leapfrog limit
    // of 1/sqrt(2) at 30 fps.
    let c60 = min(0.3 * dims.y / (60.0 * FLUVID_WAVE_STEPS), 0.35);
    let c = c60 * frame_steps();

    // Two losses. Motion damping lets about 90% of a wave's swing survive each
    // second. The surface also relaxes toward flat (about 38% of any height
    // left after a second): the wave equation has no restoring force on the
    // level itself, so a hand that keeps moving would otherwise keep pushing
    // the whole surface down until it hit the clamp.
    let keep = frame_decay(0.99989);
    let relax = frame_decay(0.999);
    var h_next = (h + (h - h_prev) * keep + c * c * lap) * relax;

    // The camera's motion presses the surface, harder on hits.
    let surge = 1.0 + param(13u) * 1.5 * u.onset;
    h_next -= input0(uv).z * 0.0015 * surge * frame_steps();

    h_next = clamp(h_next, -1.0, 1.0);
    return fluvid_clean(vec4f(h_next, h, 0.0, 1.0), vec4f(0.0, 0.0, 0.0, 1.0));
}
