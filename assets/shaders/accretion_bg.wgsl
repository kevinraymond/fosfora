// Accretion background shader — feedback trails with spiral UV warp.
//
// The warp has two components:
// 1. Inward pull — trails drift toward center (gravitational lensing)
// 2. Rotation — trails curve into spiral arms (angular momentum)
// Together they create galaxy-like spiral arm structure from particle trails.
//
// Differential decay: red fades faster than blue, shifting aging trails
// from warm gold (fresh emission) to cool blue (old spiral arms).
// This mimics real galaxy color: hot young stars in arms, cool old stars between.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let p = uv * 2.0 - 1.0;

    let dist = length(p);
    let to_center = vec2f(0.5) - uv;          // points toward center in UV space
    let tangent = vec2f(-to_center.y, to_center.x);  // perpendicular = rotational

    // Warp strengths: inward pull + rotation for spiral arms
    // Stronger at edges (longer lever arm), weaker near center
    let radial_falloff = smoothstep(0.0, 0.5, dist);
    let inward = 0.004 + u.bass * 0.003;
    let spin = 0.003 + u.mid * 0.002;

    let warp = (to_center * inward + tangent * spin) * (0.5 + radial_falloff);
    let warped_uv = clamp(uv + warp, vec2f(0.001), vec2f(0.999));

    // Differential decay: red fades fastest, blue persists longest.
    // This shifts aging trails warm→cool, preventing white-out and
    // creating the classic galaxy color gradient.
    let decay = param(0u);
    let prev = feedback(warped_uv).rgb;
    let trail = prev * vec3f(decay * 0.97, decay * 0.99, decay);

    // HDR clamp to prevent blowout
    let result = min(trail, vec3f(1.2));
    let alpha = clamp(max(result.r, max(result.g, result.b)) * 2.0, 0.0, 1.0);
    return vec4f(result, alpha);
}
