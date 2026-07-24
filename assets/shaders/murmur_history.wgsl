// Murmur history (#1482 Chronoflow) — long-exposure streaks of the flock.
// The birds are DARK silhouettes alpha-blended over a bright sky, so the
// trail signal is signed: streaks carve darkness into the sky rather than
// adding light. `hist - bg` keeps that sign.
//   feedback() = last frame's final image (sky + birds)
//   input0     = chronoflow velocity field (uv-space, per second)
//   input1     = background pass (twilight sky, this frame)
// Params: 8 trail_exposure, 9 beat_snap, 10 flow_stretch.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(prev_frame));
    let uv = frag_coord.xy / dims;
    let dt = clamp(u.delta_time, 1e-4, 0.05);

    // flow_stretch is a dry/wet for the Chronoflow transport with a squared
    // response: 0 = anchored ghosting, the lower third stays essentially
    // anchored, 1 = 1.75× raw flock velocity (matches the Flux tuning, #1482).
    let s10 = param(10u);
    let stretch = s10 * s10 * 1.75;
    let v = input0(uv).xy * stretch;
    // Musical advection envelope: the flock's smear holds still in quiet
    // passages and surges into flow with loudness.
    let surge = 0.2 + 0.8 * clamp(u.rms * 1.6, 0.0, 1.0);
    let hist = feedback(uv - v * dt * surge).rgb;

    let bg = input1(uv).rgb;
    var trail = hist - bg;
    // Saturation knee on the positive (bright plume) side only — accumulated
    // white can't wash out, while the signed dark streaks pass untouched.
    let tl = max(max(trail.r, trail.g), trail.b);
    trail *= 1.0 / (1.0 + 0.35 * max(tl, 0.0));
    // Squared beat_snap: a hard per-beat collapse strobes like a failing
    // fluorescent tube (see flux_history.wgsl).
    let sp = param(9u);
    let keep = chrono_keep(param(8u), sp * sp * 0.75);
    let col = clamp(bg + trail * keep, vec3f(0.0), vec3f(1.5));

    return vec4f(col, 1.0);
}
