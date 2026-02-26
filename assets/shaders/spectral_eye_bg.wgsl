// Spectral Eye background pass — reads feedback, applies decay, subtle dark vignette.
// Particles render on top of this via LoadOp::Load.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;

    // Read previous frame (feedback)
    let prev = feedback(uv);

    // Decay factor from param (trail_decay = param(1))
    let decay = param(1u);

    // Apply decay
    var col = prev.rgb * decay;

    // CRITICAL: hard cap to prevent additive-blend feedback runaway.
    // Without this, overlapping particles accumulate to astronomical HDR values
    // through the feedback loop (each frame: prev * decay + additive particles).
    col = min(col, vec3f(1.5));

    // Subtle vignette to darken edges (keeps focus on center)
    let center = uv - 0.5;
    let vignette = 1.0 - dot(center, center) * 1.5;
    col *= max(vignette, 0.0);

    let alpha = clamp(max(col.r, max(col.g, col.b)) * 2.0, 0.0, 1.0);
    return vec4f(col, alpha);
}
