// Plateau — the motion aftereffect, timed to the music.
//
// Watch steady motion for half a minute and then stop it, and the still picture
// seems to flow the other way: the motion detectors that adapted to the drift
// are suppressed, and their opposites win for several seconds (Plateau's
// spiral, 1850; Addams' waterfall, 1834). Plateau drifts a high-contrast
// grating while the kick plays and freezes it dead when a breakdown takes the
// kick away (gate in plateau_state.wgsl), so the illusion lands on the quiet.
//
// Patterns: 0 a rotating logarithmic spiral, which reads as steady expansion
// and contracts when it stops; 1 rings zooming outward; 2 the waterfall, bars
// falling down the screen. The log forms are scale-invariant, so the drift is
// the same everywhere on screen rather than racing at the edges.
//
// Everything is gray: the aftereffect is a motion illusion and needs contrast,
// not hue. Nothing else on screen moves while the grating is frozen, since any
// real motion masks the illusory one. A fixation dot helps the eye hold still.

const TAU: f32 = 6.28318531;

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let px = 1.0 / u.resolution.y;
    // Height units, origin at the center.
    let p = (frag_coord.xy - 0.5 * u.resolution) * px;
    let r = max(length(p), px);
    let theta = atan2(p.y, p.x);

    let pattern = u32(clamp(floor(param(0u) + 0.5), 0.0, 2.0));
    let arms = clamp(floor(param(2u) + 0.5), 1.0, 12.0);
    let density = param(3u);
    let contrast = clamp(param(4u), 0.0, 1.0);
    let fixation = param(7u) > 0.5;

    let phase = phase_unpack(textureLoad(input0_tex, vec2i(0, 0), 0));

    // `s` counts grating cycles; `w` is how many cycles one pixel spans,
    // worked out analytically because atan2's seam would spike fwidth.
    var s: f32;
    var w: f32;
    if pattern == 0u {
        s = arms * theta / TAU + density * log(r) - phase;
        w = px / r * length(vec2f(arms / TAU, density));
    } else if pattern == 1u {
        s = density * log(r) - phase;
        w = px / r * abs(density);
    } else {
        s = density * 4.0 * p.y - phase;
        w = px * abs(density) * 4.0;
    }

    // A square wave with ~1.5 px edges, faded to gray where the grating gets
    // finer than the pixels can show (near the spiral's center).
    let edge = clamp(sin(TAU * s) / (TAU * max(w, 1e-6) * 1.5), -1.0, 1.0);
    let resolvable = 1.0 - smoothstep(0.2, 0.45, w);
    var lum = 0.5 + 0.5 * contrast * edge * resolvable;

    if fixation {
        let ring = smoothstep(0.012 + px, 0.012, r);
        let pip = smoothstep(0.005 + px, 0.005, r);
        lum = mix(lum, 0.0, ring);
        lum = mix(lum, 1.0, pip);
    }

    return vec4f(vec3f(lum), 1.0);
}
