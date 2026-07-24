// Flux background shader — subtle ambient smoke glow, no feedback.
// Trail persistence moved to flux_history.wgsl (#1482 Chronoflow); this pass
// only provides the barely-visible key-anchored atmosphere under the trails.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let p = uv * 2.0 - 1.0;

    // Very subtle ambient glow based on audio — barely visible without particles
    let density_param = param(3u);
    let ambient_n = phosphor_noise2(p * 2.0 + vec2f(u.time * 0.05));
    let ambient = ambient_n * ambient_n * 0.015 * density_param * u.rms;

    // Color: muted ambient, anchored to musical key
    let color_shift = param(2u);
    let hue = u.dominant_chroma + color_shift * 0.3 + u.centroid * 0.1;
    let r = abs(hue * 6.0 - 3.0) - 1.0;
    let g = 2.0 - abs(hue * 6.0 - 2.0);
    let b = 2.0 - abs(hue * 6.0 - 4.0);
    let ambient_color = clamp(vec3f(r, g, b), vec3f(0.0), vec3f(1.0)) * ambient;

    let result = min(ambient_color, vec3f(1.5));
    let alpha = max(result.r, max(result.g, result.b)) * 2.0;
    return vec4f(result, clamp(alpha, 0.0, 1.0));
}
