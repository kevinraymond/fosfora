// Morph background — dark field with subtle feedback fade.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;

    // Feedback from previous frame
    let prev = feedback(uv);
    let decay = 0.85;

    // Dark background with radial gradient
    let center = uv - 0.5;
    let dist = length(center);
    let bg = mix(vec3f(0.015, 0.015, 0.025), vec3f(0.005, 0.005, 0.01), dist * 1.5);

    // Subtle chroma-driven tint — musical key colors the background
    let hue = u.dominant_chroma;
    let tint_r = abs(fract(hue) * 6.0 - 3.0) - 1.0;
    let tint_g = 2.0 - abs(fract(hue) * 6.0 - 2.0);
    let tint_b = 2.0 - abs(fract(hue) * 6.0 - 4.0);
    let tint = clamp(vec3f(tint_r, tint_g, tint_b), vec3f(0.0), vec3f(1.0));
    let radial_blend = 1.0 - dist * 1.5; // strongest at center
    let bg_tinted = bg + tint * max(radial_blend, 0.0) * u.rms * 0.04;

    // Blend feedback with background
    let faded = prev.rgb * decay;
    let color = max(faded, bg_tinted);

    return vec4f(color, 1.0);
}
