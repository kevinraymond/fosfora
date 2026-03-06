// Murmur background shader — twilight sky gradient, no feedback.
// Dark bird silhouettes get contrast from the bright sky alone.
// No trails needed: 40K particles provide frame-to-frame continuity.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;

    // Twilight sky gradient: deep blue top, warm amber horizon (brighter than before)
    let sky_top = vec3f(0.06, 0.08, 0.18);
    let sky_mid = vec3f(0.10, 0.08, 0.12);
    let sky_low = vec3f(0.18, 0.11, 0.06);
    let y = uv.y;
    var sky: vec3f;
    if y > 0.5 {
        sky = mix(sky_mid, sky_top, (y - 0.5) * 2.0);
    } else {
        sky = mix(sky_low, sky_mid, y * 2.0);
    }

    // Color mode: shift sky warmth (param 2 now, was param 3)
    let color_mode = param(2u);
    sky = mix(sky, sky.brg * 0.8, (color_mode - 0.5) * 0.3);

    // Audio-reactive sky glow
    sky *= 1.0 + u.rms * 0.3;

    // Subtle chroma-driven horizon tint — musical key colors the sky
    let hue = u.dominant_chroma;
    let tint_r = abs(fract(hue) * 6.0 - 3.0) - 1.0;
    let tint_g = 2.0 - abs(fract(hue) * 6.0 - 2.0);
    let tint_b = 2.0 - abs(fract(hue) * 6.0 - 4.0);
    let tint = clamp(vec3f(tint_r, tint_g, tint_b), vec3f(0.0), vec3f(1.0));
    let horizon_blend = smoothstep(0.5, 0.0, uv.y); // strongest at bottom
    sky += tint * horizon_blend * u.rms * 0.06;

    let center = uv - 0.5;
    let vignette = 1.0 - dot(center, center) * 1.2;
    let result = sky * max(vignette, 0.0);
    return vec4f(result, 1.0);
}
