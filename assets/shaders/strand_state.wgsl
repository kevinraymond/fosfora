// Strand — state pass: the spin phase and the kick envelope.
//
// Texel (0, 0) holds the spin phase in turns. The spin rate is a slider times
// the bass, so it is integrated here as `rate * dt` (#2984), never spent as
// `u.time * rate`, which would jump the strands by (change x uptime) every time
// the bass moved. The direction flips every `flip_bars` bars; since the flip
// changes the rate, not the phase, the strands reverse without a jump.
//
// Texel (1, 0) holds whether the kick is playing: r jumps to 1 on each kick
// and fades over ~1.2 s, so it stays up while kicks keep coming and falls away
// a couple of seconds into a breakdown; g follows r over ~0.5 s, which is what
// the depth cue reads, so the picture eases into depth when the drums return
// instead of snapping on the first kick. Kicks, not `percussive_energy`: on the
// batida renders kick onsets ran ~4/s in full sections and ~0.1/s in breaks,
// while percussive_energy read 0.57 in the techno break against 0.58 at the
// peak. A kick is `u.kick` crossing 0.5, the threshold the Signal wire's
// drums onset uses.
//
// The main pass reads both with textureLoad, so the pass needs to be at least
// two texels wide: 1/32 scale gives that from a 64 px wide output up.

const WRAP: f32 = 64.0; // turns; the main pass only ever uses the phase times whole numbers
const KICK_HOLD_TAU: f32 = 1.2; // seconds
const CUE_TAU: f32 = 0.5; // seconds

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let texel = vec2i(frag_coord.xy);

    if texel.x == 0 {
        let spin = param(0u);
        let flip_bars = floor(param(9u) + 0.5);
        let reactivity = param(10u);

        var dir = 1.0;
        if flip_bars >= 1.0 {
            dir = 1.0 - 2.0 * (floor(u.bar_index / flip_bars) % 2.0);
        }
        let rate = spin * dir * (1.0 + reactivity * 1.5 * u.bass);

        let phase = phase_unpack(textureLoad(prev_frame, vec2i(0, 0), 0)) + rate * u.delta_time;
        return phase_pack(phase, WRAP);
    }

    let prev = textureLoad(prev_frame, vec2i(1, 0), 0);
    let dt = max(u.delta_time, 0.0);
    let hold = max(prev.r * exp(-dt / KICK_HOLD_TAU), step(0.5, u.kick));
    let eased = mix(prev.g, hold, 1.0 - exp(-dt / CUE_TAU));
    return vec4f(hold, eased, 0.0, 1.0);
}
