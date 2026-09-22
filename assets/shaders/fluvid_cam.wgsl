// Fluvid — camera brightness history at quarter resolution.
//   feedback() = own previous frame: r = fast luma, g = slow luma, b = 1 once primed
//   input0     = @backdrop, the layers beneath (the camera), full resolution
//
// Motion downstream is fast − slow: a temporal high-pass that stays continuous when
// the camera delivers fewer frames than the app renders. A plain current − previous
// frame reads zero on every repeated camera frame (a 30 fps camera under a 60 Hz
// render repeats every other one), which would strobe the splats at the beat of the
// two rates. Both averages go through frame_decay, so their time constants are in
// seconds, not frames.

fn fluvid_luma(c: vec3f) -> f32 {
    return dot(c, vec3f(0.299, 0.587, 0.114));
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

// One tap of the layers below, finite and bounded: an HDR layer's highlights
// can run far past 1 and must not read as a huge brightness change.
fn fluvid_cam_tap(uv: vec2f) -> vec3f {
    return clamp(fluvid_clean(input0(uv), vec4f(0.0)).rgb, vec3f(0.0), vec3f(4.0));
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(prev_frame));
    let uv = frag_coord.xy / dims;

    // Four bilinear taps one source pixel either side of this texel's center
    // average a 4x4 block: the quarter-res box filter, which is also the first
    // line of defense against sensor noise.
    let o = 1.0 / vec2f(textureDimensions(input0_tex));
    let c = 0.25
        * (fluvid_cam_tap(uv + vec2f(-o.x, -o.y))
            + fluvid_cam_tap(uv + vec2f(o.x, -o.y))
            + fluvid_cam_tap(uv + vec2f(-o.x, o.y))
            + fluvid_cam_tap(uv + o));

    // A dim room: brighten the camera layer with a trama Levels node, which
    // lifts both what this sees and what the camera slider shows.
    let l = fluvid_luma(c);

    let prev = fluvid_clean(feedback(uv), vec4f(0.0));
    // First frame (cleared target): prime both averages to the picture, or the whole
    // frame reads as one giant movement from black.
    if (prev.b < 0.5) {
        return vec4f(l, l, 1.0, 1.0);
    }

    let fast = l;
    let slow = mix(fast, prev.g, frame_decay(0.85));
    return fluvid_clean(vec4f(fast, slow, 1.0, 1.0), vec4f(0.0));
}
