// Phosphor history (#1482 Chronoflow) — the whole effect: long-exposure
// particle trails advected along the tubes' own motion; the compute-raster
// particles composite on top of this pass's output each frame.
//   feedback() = last frame's final image (trails + particles)
//   input0     = chronoflow velocity field (uv-space, per second)
// Params: 0 trail_decay (exposure), 1 beat_snap, 2 flow_stretch.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(prev_frame));
    let uv = frag_coord.xy / dims;
    let dt = clamp(u.delta_time, 1e-4, 0.05);

    let stretch = 0.5 + param(2u) * 2.5;
    let v = input0(uv).xy * stretch;
    let hist = feedback(uv - v * dt).rgb;

    let keep = chrono_keep(param(0u), param(1u));
    let col = min(hist * keep, vec3f(1.5));
    let alpha = clamp(max(col.r, max(col.g, col.b)) * 2.0, 0.0, 1.0);
    return vec4f(col, alpha);
}
