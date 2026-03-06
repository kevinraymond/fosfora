// Chaos background shader — feedback trails with mild centripetal warp.
//
// Trails drift gently toward center (attractor focus) with differential
// RGB decay: red fades fastest, shifting aging trails from warm (fresh)
// to cool blue-violet (old). This creates the classic strange attractor
// trail aesthetic where trajectory history builds up the shape.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let p = uv * 2.0 - 1.0;

    let dist = length(p);
    let to_center = vec2f(0.5) - uv;

    // Mild centripetal warp — trails drift toward center
    let radial_falloff = smoothstep(0.0, 0.6, dist);
    let inward = 0.002 + u.bass * 0.002;
    let warp = to_center * inward * (0.3 + radial_falloff);
    let warped_uv = clamp(uv + warp, vec2f(0.001), vec2f(0.999));

    // Differential decay: red fades fastest, blue persists longest
    let decay = param(0u);
    let prev = feedback(warped_uv).rgb;
    let trail = prev * vec3f(decay * 0.96, decay * 0.99, decay);

    // HDR clamp to prevent blowout
    let result = min(trail, vec3f(1.2));
    let alpha = clamp(max(result.r, max(result.g, result.b)) * 2.0, 0.0, 1.0);
    return vec4f(result, alpha);
}
