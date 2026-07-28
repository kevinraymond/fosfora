// Frame-rate-independent feedback decay.
//
// Trail and feedback effects keep the previous frame and multiply it down:
// `prev.rgb * decay`. Written that way, `decay` is a per-FRAME factor, so the
// trail's length is measured in frames rather than seconds — at 120 fps it decays
// twice as fast in wall-clock terms as at 60, and the picture visibly dims.
//
// That is not hypothetical. On this X11 setup, clicking or focusing ANY window
// produces a brief FPS spike from the event burst (recorded 2026-03-09), so every
// click made the image pulse. Same family as the Tide flash (#1796), which came
// from unclamped `dt` integration instead.
//
// `frame_decay` reinterprets the existing constant as "the factor I want at
// 60 fps" and rescales it to the real frame time, so the half-life is fixed in
// seconds. At exactly 60 fps the exponent is 1.0 and the value is unchanged —
// which is why every shipped look and every offscreen probe (all of which run at
// dt = 1/60) is preserved bit-for-bit.
//
// The idiom is Chronoflow's (lib/chronoflow.wgsl), generalised.

/// Rescale a per-frame-at-60fps decay factor to this frame's actual delta time.
/// `d60` is clamped to [0,1]: a factor above 1 would amplify feedback into a
/// blowout, and the exponent turns that into a much faster one.
fn frame_decay(d60: f32) -> f32 {
    let d = clamp(d60, 0.0, 1.0);
    // A zero or negative delta_time must NOT be read as "no time passed, so keep
    // everything" — that drives the exponent to 0, makes the factor 1.0, and turns
    // a feedback buffer into an accumulator that saturates the screen to white
    // within a second. Any caller that has not filled `delta_time` gets exactly the
    // historical per-frame behaviour instead, which is safe by construction.
    if u.delta_time <= 0.0 {
        return d;
    }
    return pow(d, u.delta_time * 60.0);
}
