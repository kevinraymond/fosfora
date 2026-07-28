// Genesis background shader — dark void with feedback trails.
// Particles are the visual focus; background provides trail persistence
// and subtle organic smearing for creature afterimages.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let p = uv * 2.0 - 1.0;

    // Feedback (previous frame) with configurable decay
    let decay = param(0u); // trail_decay
    let prev = feedback(uv);

    // Subtle UV distortion on feedback — organic smearing of creature trails
    let warp_str = 0.0015 + u.rms * 0.001;
    let warp_x = phosphor_noise2(p * 2.5 + vec2f(u.time * 0.03, 0.0)) - 0.5;
    let warp_y = phosphor_noise2(p * 2.5 + vec2f(0.0, u.time * 0.025)) - 0.5;
    let warped_uv = uv + vec2f(warp_x, warp_y) * warp_str;
    let warped_prev = feedback(clamp(warped_uv, vec2f(0.001), vec2f(0.999)));

    // Blend warped and straight feedback for smoother trails
    let trail = mix(prev.rgb, warped_prev.rgb, 0.5) * decay;

    // Very dark base with slight blue-green tint (deep ocean / primordial void)
    let base = vec3f(0.005, 0.008, 0.012);

    let result = max(trail, base);
    let result_clamped = min(result, vec3f(1.5)); // HDR clamp for feedback stability
    let alpha = max(result_clamped.r, max(result_clamped.g, result_clamped.b)) * 2.0;
    return vec4f(result_clamped, clamp(alpha, 0.0, 1.0));
}
