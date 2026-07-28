// Ascend background — horizon afterglow (#1801)
//
// Feedback advected slowly UPWARD, so the field leaves a rising wake as the
// horizon climbs and a compressed one as it sinks. Kept very dim: Ascend is
// designed to sit underneath another effect, and a bright background would
// wash out whatever is layered on top of it.
//
// param(7) = trail_decay  (feedback decay)

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let decay = param(7u);

    // uv.y is down-screen, so sampling below lifts the echo. Faster when the
    // sound is bright, which is the direction the horizon is moving anyway.
    let rise = 0.0008 + 0.0030 * clamp(u.rolloff, 0.0, 1.0);
    let prev = feedback(clamp(uv + vec2f(0.0, rise), vec2f(0.001), vec2f(0.999)));

    // Cool the wake as it ages so the live band stays the brightest, warmest
    // thing in frame.
    let trail = prev.rgb * decay * vec3f(0.92, 0.96, 1.0);

    let result = min(trail, vec3f(1.5)); // HDR clamp
    let alpha = clamp(max(result.r, max(result.g, result.b)) * 2.0, 0.0, 1.0);
    return vec4f(result, alpha);
}
