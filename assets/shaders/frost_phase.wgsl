// Frost — wander phase accumulator (#2984).
//
// Frost's crystal cells wander at a rate the MUSIC sets:
//
//     agitation = 0.15 + m * (0.6 + zcr_x * 2.0)   // calm ice -> fizzing sand
//
// The main pass used to spend that as `t * agitation`, which is a rate times an
// ABSOLUTE clock: every change in agitation is multiplied by how long the app
// has been running, so a frame where the music moves jumps the wander angle by
// (change x uptime) instead of advancing it. Measured on the GPU, a 0.01 move in
// `u.zcr` — far less than real music moves between frames — cost 176x more at a
// 300 s clock than at a fresh one, and moved the picture 19x further than a whole
// frame of ordinary motion. Frost is the one effect in the #2984 census that
// does this with no binding involved, so it degraded on ordinary playback.
//
// The fix is to integrate instead of multiply: this pass advances the phase by
// `agitation * dt` once per frame and hands the running total to the main pass.
// Frost's rate depends on audio the engine has not seen yet, so it cannot be
// pre-integrated on the CPU the way the other eight census effects can; keeping
// the formula here, in WGSL, is also what stops it drifting from a second copy.
//
// STORED IN THREE PARTS because pass targets are Rgba16Float: a half float has
// ~0.0005 of precision near 1.0 but ~0.25 near 300, so a raw running total stops
// registering a 1/60 s step within a couple of minutes — the exact failure this
// pass exists to remove. r holds floor(phase), an integer, exact in f16 well past
// the wrap. g holds the fraction rounded to f16, and b holds what that rounding
// threw away. Two parts are NOT enough: under steady audio the step is the same
// every frame, so the fraction rounds with the same bias every frame and the
// error ACCUMULATES rather than cancelling — measured at 1% slow over a minute.
// Carrying the remainder in b takes the bias to ~1e-7 a frame.
//
// WRAPPED AT 200*PI because the main pass uses the phase two ways, as `sin(ang)`
// and as `cos(ang * 1.31 + ...)`. 200*PI is 100 turns of the first and exactly
// 131 turns of the second, so wrapping there is invisible in both while keeping
// the integer half small enough to stay exact.

const WRAP: f32 = 628.3185307; // 200*PI

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    // Mirrors the main pass's morph, which is what sets the wander rate.
    let morph_bias = param(1u);
    let reactivity = param(7u);
    let zcr_x = min(u.zcr * 2.5, 1.0);
    let m = clamp(mix(0.5, u.flatness, reactivity) + morph_bias, 0.0, 1.0);
    let agitation = 0.15 + m * (0.6 + zcr_x * 2.0);

    // Every texel of this pass carries the same phase; sampling anywhere works.
    let prev = feedback(vec2f(0.5, 0.5));
    var phase = prev.r + prev.g + prev.b + agitation * u.delta_time;
    phase = phase - floor(phase / WRAP) * WRAP;

    let hi = floor(phase);
    let frac = phase - hi;
    let coarse = quantizeToF16(frac);
    return vec4f(hi, coarse, frac - coarse, 1.0);
}
