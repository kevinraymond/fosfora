// Vessel background — charge glow + drop-shock plate (#1797)
//
// The fragment side has had buildup/drop since A18 landed, so the plate
// choreographs directly: a key-colored glow pools up from the bottom as the
// charge grows, and the one-frame drop impulse is INJECTED into the feedback
// loop so the flash decays over the following frames instead of lasting a
// single frame. Feedback drifts gently upward while filling (embers rise) and
// stills as the charge completes.
//
// param(5) = hue   (palette anchor offset, shared with the sim)
// param(6) = glow  (interior glow gain + feedback persistence)

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let decay = mix(0.78, 0.93, param(6u));

    // Upward drift: sample the pixel below (uv y is down-screen) so echoes
    // rise; the drift freezes as the buildup completes — held breath.
    let rise = 0.0012 * (1.0 - u.buildup);
    let prev = feedback(clamp(uv + vec2f(0.0, rise), vec2f(0.001), vec2f(0.999)));
    let trail = prev.rgb * decay * vec3f(0.97, 0.99, 1.02);

    // Key-locked charge floor rising with the buildup (the liquid metaphor on
    // the back wall). Kept faint — the fill story belongs to the particles.
    let hue = fract(u.dominant_chroma * 0.15 + param(5u) * 0.4 + 0.08);
    let r = clamp(abs(hue * 6.0 - 3.0) - 1.0, 0.0, 1.0);
    let g = clamp(2.0 - abs(hue * 6.0 - 2.0), 0.0, 1.0);
    let b = clamp(2.0 - abs(hue * 6.0 - 4.0), 0.0, 1.0);
    let key_col = mix(vec3f(r, g, b), vec3f(0.9, 0.7, 0.4), 0.25);
    let floor_glow = smoothstep(0.35, 1.0, uv.y) * 0.012 * param(6u) * u.buildup;

    // Drop shock: one-frame impulse into the feedback plate.
    let shock = u.drop * (0.25 + 0.5 * param(6u));

    let result = min(trail + key_col * floor_glow + vec3f(0.95, 0.97, 1.0) * shock, vec3f(1.5));
    let alpha = clamp(max(result.r, max(result.g, result.b)) * 2.0, 0.0, 1.0);
    return vec4f(result, alpha);
}
