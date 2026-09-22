// Fluvid — the colored ink the camera's motion leaves in the fluid.
//   feedback() = own dye, previous frame
//   input0     = velocity, this frame
//   input1     = motion, this frame (xy = push direction × gate, z = gate)
//   input2     = @backdrop (the camera), full resolution
//
// Color comes from a hue that turns with the direction of motion (so a hand sweeping
// one way and back leaves two tones), blended toward the camera's own colors by
// camera_color. Injection uses frame_gain against the same retention as the decay,
// so the ink's steady brightness does not move with frame rate.

fn fluvid_hsv(h: f32, s: f32, v: f32) -> vec3f {
    let k = vec3f(1.0, 0.6666667, 0.3333333);
    let p = abs(fract(vec3f(h) + k) * 6.0 - 3.0);
    return v * mix(vec3f(1.0), clamp(p - 1.0, vec3f(0.0), vec3f(1.0)), s);
}

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
    let uv = frag_coord.xy / dims;
    let sim_texel = 1.0 / vec2f(textureDimensions(input0_tex));

    let energy = fluvid_energy();                  // p0 flow speed
    let amount = 0.02 + param(3u) * 0.25;          // p3 dye amount
    let keep = mix(0.998, 0.94, param(4u));        // p4 dissipation
    let hue0 = param(10u);                         // p10
    let cam_mix = param(11u);                      // p11 camera color
    let audio = param(13u);                        // p13

    let dt = clamp(u.delta_time, 0.0, 0.05) * 60.0 * energy;

    let vel = input0(uv).xy;
    var col = feedback(uv - dt * vel * sim_texel).rgb * frame_decay(keep);

    let m = input1(uv);
    let gate = m.z;
    // atan2(0, 0) is implementation-defined (NaN on some GPUs), and most of the
    // frame has no motion; NaN times a zero gate is still NaN.
    var turn = 0.0;
    if (gate > 1e-4) {
        turn = atan2(m.y, m.x) / 6.28318;
    }
    let pal = fluvid_hsv(fract(hue0 + 0.12 * turn), 0.85, 1.0);

    // The camera's color, saturation pushed so a skin tone reads as a color and
    // not as beige, and lifted so a dim room still gives visible ink.
    let cam = clamp(fluvid_clean(input2(uv), vec4f(0.0)).rgb, vec3f(0.0), vec3f(4.0));
    let cl = dot(cam, vec3f(0.299, 0.587, 0.114));
    let cam_vivid = max(mix(vec3f(cl), cam, 1.8), vec3f(0.0)) / max(cl, 0.15);

    let src = mix(pal, cam_vivid, cam_mix);
    let surge = 1.0 + audio * 1.5 * u.onset;
    col += src * gate * surge * frame_gain(amount, keep);

    col = min(col, vec3f(4.0));
    return fluvid_clean(vec4f(col, 1.0), vec4f(0.0, 0.0, 0.0, 1.0));
}
