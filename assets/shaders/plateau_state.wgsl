// Plateau — state pass: the drift phase and whether the kick is playing.
//
// Texel (1, 0): r is a kick peak-hold, jumping to 1 on each kick (`u.kick`
// crossing 0.5, as in strand_state.wgsl) and fading over ~1.2 s; g is the
// motion gate, opened by a kick and closed once the hold falls below 0.3, about
// 1.4 s after the last one. The gap allows half-time kicks at 120 BPM; a
// breakdown closes it.
//
// Texel (0, 0): the drift phase in cycles, integrated as `rate * dt` (#2984).
// The gate switches the rate between full and zero with no easing: the
// aftereffect is strongest when the motion stops dead, and an eased stop lets
// the eye adapt down with it. It reads last frame's gate, one frame late.
//
// The main pass reads both with textureLoad, so the pass needs to be at least
// two texels wide: 1/32 scale gives that from a 64 px wide output up.

const WRAP: f32 = 64.0; // cycles; the pattern only uses the phase through fract/sin
const KICK_HOLD_TAU: f32 = 1.2; // seconds
const GATE_CLOSE: f32 = 0.3;

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let texel = vec2i(frag_coord.xy);
    let kick_state = textureLoad(prev_frame, vec2i(1, 0), 0);

    if texel.x == 0 {
        let follow_kick = param(5u) > 0.5;
        let tempo_lock = param(6u) > 0.5;
        var rate = param(1u);
        if tempo_lock && u.bpm > 0.0 {
            rate *= u.bpm / 120.0;
        }
        if follow_kick {
            rate *= kick_state.g;
        }
        // `drift` off stops it by hand, the same hard stop the gate makes.
        if param(8u) < 0.5 {
            rate = 0.0;
        }
        let phase = phase_unpack(textureLoad(prev_frame, vec2i(0, 0), 0)) + rate * u.delta_time;
        return phase_pack(phase, WRAP);
    }

    let dt = max(u.delta_time, 0.0);
    let kicked = step(0.5, u.kick);
    let hold = max(kick_state.r * exp(-dt / KICK_HOLD_TAU), kicked);
    var gate = kick_state.g;
    if kicked > 0.0 {
        gate = 1.0;
    } else if hold < GATE_CLOSE {
        gate = 0.0;
    }
    return vec4f(hold, gate, 0.0, 1.0);
}
