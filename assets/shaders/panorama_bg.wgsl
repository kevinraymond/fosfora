// Panorama background — stereo-field afterglow with a centre reference (#1801)
//
// Feedback is advected HORIZONTALLY outward from the centre line, so echoes
// trail the way the image reads: a band that swings left leaves its ghost to
// the left. Deliberately not isotropic — an even bloom would blur the very
// axis the effect exists to show.
//
// A faint centre line and edge markers give the stereo image a reference, the
// way a goniometer's graticule does. Without it "everything is slightly left"
// is invisible; with it, it is obvious.
//
// param(6) = centre_pull  (graticule strength)
// param(7) = trail_decay  (feedback decay)

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let decay = param(7u);

    // Outward advection: sample slightly toward the centre so echoes drift
    // away from it. Scales with the measured width, so a mono mix's afterglow
    // stays put and a wide one visibly spreads.
    let from_centre = uv.x - 0.5;
    let spread = (0.0006 + 0.0035 * u.stereo_width) * sign(from_centre);
    let prev = feedback(clamp(uv - vec2f(spread, 0.0), vec2f(0.001), vec2f(0.999)));

    // Chromatic decay: blue survives longest, so older light cools. Keeps a
    // busy field legible by separating "now" from "a moment ago" in hue as
    // well as brightness.
    var trail = prev.rgb * decay * vec3f(0.94, 0.965, 1.0);

    // Graticule: a centre line plus soft hard-left/hard-right markers. These
    // are the only fixed reference in the frame, so pan is readable as an
    // absolute position rather than only as motion.
    let grat = param(6u);
    let centre = exp(-abs(from_centre) * 260.0) * 0.055;
    let edges = (exp(-abs(uv.x - 0.02) * 200.0) + exp(-abs(uv.x - 0.98) * 200.0)) * 0.022;
    // Breathe the centre line with correlation: a mono mix lights it, a wide
    // one lets it fade, so the reference itself reports the field.
    let coh = clamp(abs(u.stereo_corr * 2.0 - 1.0), 0.0, 1.0);
    let guide = (centre * (0.35 + 0.9 * coh) + edges * (0.3 + 0.9 * u.stereo_width)) * grat;
    let guide_color = vec3f(0.35, 0.55, 0.85) * guide;

    let result = min(trail + guide_color, vec3f(1.5)); // HDR clamp
    let alpha = clamp(max(result.r, max(result.g, result.b)) * 2.0, 0.0, 1.0);
    return vec4f(result, alpha);
}
