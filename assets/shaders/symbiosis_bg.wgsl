// Symbiosis background — dark field, no feedback.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;

    // Deep dark background with subtle radial gradient
    let center = uv - 0.5;
    let dist = length(center);
    let bg = mix(vec3f(0.02, 0.02, 0.04), vec3f(0.005, 0.005, 0.01), dist * 1.5);

    return vec4f(bg, 1.0);
}
