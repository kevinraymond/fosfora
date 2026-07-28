// Mycelium background — feedback trails with organic warp.
//
// Differential decay: green persists longest (bioluminescent aging).
// Low-frequency noise warp creates organic drift in the trail network.
// Dark near-black base with slight green cast (forest floor).

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let p = uv * 2.0 - 1.0;

    // Organic noise warp — low-frequency distortion for living feel
    let warp_str = 0.001 + u.mid * 0.001;
    let noise_uv = uv * 3.0 + vec2f(u.time * 0.02, u.time * 0.015);
    let warp_x = phosphor_noise2(noise_uv) - 0.5;
    let warp_y = phosphor_noise2(noise_uv + vec2f(31.7, 47.3)) - 0.5;
    let warped_uv = clamp(uv + vec2f(warp_x, warp_y) * warp_str, vec2f(0.001), vec2f(0.999));

    // Differential RGB decay: green persists longest (bioluminescent)
    let decay = param(0u);
    let prev = feedback(warped_uv).rgb;
    let trail = prev * vec3f(decay * 0.94, decay, decay * 0.97);

    // HDR clamp to prevent blowout
    let result = min(trail, vec3f(1.2));

    // Alpha for compositing (empty-space pattern)
    let alpha = clamp(max(result.r, max(result.g, result.b)) * 2.0, 0.0, 1.0);
    return vec4f(result, alpha);
}
