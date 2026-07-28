// Tide background — falling water afterglow with refraction shimmer (#1796)
//
// Deliberately ANISOTROPIC feedback (vs Flux's isotropic smear): echoes are
// advected downward with the water and warped only in x, so the afterglow
// falls instead of blooming outward. Chromatic decay kills red fastest —
// water absorbs red first, so older light turns blue-green.
//
// param(5) = water_hue    (palette anchor offset, shared with the sim)
// param(6) = shimmer      (refraction warp amount)
// param(7) = trail_decay  (feedback decay)

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let decay = param(7u);

    // Downward advection: sample the pixel above (uv y is down-screen) so
    // echoes fall with the water; faster when the mix is loud.
    let fall = 0.0015 + 0.004 * u.rms;

    // Refraction shimmer: fine x-only warp scrolling down.
    let wx = (phosphor_noise2(vec2f(uv.x * 40.0, uv.y * 8.0 - u.time * 1.5)) - 0.5)
        * 0.004 * param(6u) * (0.5 + u.presence);
    let prev = feedback(clamp(uv + vec2f(wx, -fall), vec2f(0.001), vec2f(0.999)));

    // Chromatic decay: red dies fastest -> aged light goes blue-green.
    var trail = prev.rgb * decay * vec3f(0.90, 0.97, 1.0);

    // Faint vertical caustic curtains, swelling with sustained (harmonic)
    // content — barely visible alone, they give the falls a back-wall.
    let curtain_n = phosphor_noise2(vec2f(uv.x * 24.0, uv.y * 3.0 - u.time * 0.4));
    let curtain = curtain_n * curtain_n * 0.010 * u.harmonic_energy;
    let hue = fract(u.dominant_chroma * 0.15 + param(5u) * 0.25 + 0.55);
    let r = clamp(abs(hue * 6.0 - 3.0) - 1.0, 0.0, 1.0);
    let g = clamp(2.0 - abs(hue * 6.0 - 2.0), 0.0, 1.0);
    let b = clamp(2.0 - abs(hue * 6.0 - 4.0), 0.0, 1.0);
    let curtain_color = mix(vec3f(r, g, b), vec3f(0.2, 0.5, 0.8), 0.5) * curtain;

    let result = min(trail + curtain_color, vec3f(1.5)); // HDR clamp
    let alpha = clamp(max(result.r, max(result.g, result.b)) * 2.0, 0.0, 1.0);
    return vec4f(result, alpha);
}
