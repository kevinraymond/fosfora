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

    let stretch = 0.5 + param(10u) * 2.5;
    let v = input0(uv).xy * stretch;
    let hist = feedback(uv - v * dt).rgb;

    let bg = input1(uv).rgb;
    let keep = chrono_keep(param(8u), param(9u));
    let col = clamp(bg + (hist - bg) * keep, vec3f(0.0), vec3f(1.5));

    return vec4f(col, 1.0);
}
