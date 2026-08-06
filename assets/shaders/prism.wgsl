// Prism — Kaleidoscopic N-fold mirror symmetry over FBM + geometric source
// Creates hypnotic kaleidoscope patterns with audio-reactive complexity.

fn kaleidoscope(p: vec2f, folds: f32) -> vec2f {
    var q = p;
    let angle = 6.28318 / folds;
    let a = atan2(q.y, q.x);
    let r = length(q);
    // Mirror fold
    var folded_a = ((a % angle) + angle) % angle;
    if (folded_a > angle * 0.5) {
        folded_a = angle - folded_a;
    }
    return vec2f(cos(folded_a), sin(folded_a)) * r;
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let aspect = res.x / res.y;
    let p = (uv - 0.5) * vec2f(aspect, 1.0);
    let t = u.time;

    // param(0) = fold_count, param(1) = rotation_speed, param(2) = zoom, param(3) = complexity
    let folds = floor(param(0u) * 10.0 + 3.0); // 3 to 13 folds
    let rot_speed = param(1u) * 2.0;
    let zoom = param(2u) * 3.0 + 1.0;
    let complexity = param(3u) * 4.0 + 2.0;

    // Audio modulation
    let rotation = t * rot_speed + u.beat_phase * 6.28318 * 0.25;
    let bass_pulse = 1.0 + u.bass * 0.3;
    let mid_complex = u.mid * 2.0;

    // Rotate
    let ca = cos(rotation);
    let sa = sin(rotation);
    let rotated = vec2f(p.x * ca - p.y * sa, p.x * sa + p.y * ca);

    // Apply kaleidoscope fold
    let kp = kaleidoscope(rotated, folds);

    // Zoom with bass pulse
    let zp = kp * zoom * bass_pulse;

    // Source pattern: FBM + geometric elements
    let fbm_val = fosfora_fbm2(zp * (complexity + mid_complex) + vec2f(t * 0.1, 0.0), 5, 0.55);

    // Geometric overlay: concentric hexagons
    let hex_r = length(zp);
    let hex_a = atan2(zp.y, zp.x);
    let hex_pattern = abs(sin(hex_r * 6.0 - t * 0.5)) * abs(sin(hex_a * 3.0 + t * 0.3));

    // Combine patterns
    let pattern = fbm_val * 0.7 + hex_pattern * 0.3;

    // Color
    let pal_t = pattern * 2.0 + t * 0.05 + hex_r * 0.3;
    var col = fosfora_audio_palette(pal_t, u.centroid, 0.0);
    col *= pattern * 1.5;

    // param(4) = sparkle toggle, param(5) = bass_pulse toggle, param(6) = beat_flash toggle

    // Presence sparkle: high-frequency detail
    let sparkle_on = param(4u);
    let sparkle_p = zp * 20.0 + vec2f(t * 2.0, t * 1.7);
    let sparkle = pow(fosfora_noise2(sparkle_p), 8.0) * u.presence * 3.0 * sparkle_on;
    col += vec3f(sparkle);

    // Bass radial pulse
    let pulse_on = param(5u);
    let radial_pulse = exp(-abs(hex_r - 0.5 - u.bass * 0.3) * 10.0) * u.bass * 0.5 * pulse_on;
    col += fosfora_audio_palette(t * 0.2, u.centroid, 0.3) * radial_pulse;

    // Beat flash at center
    let flash_on = param(6u);
    let center_glow = exp(-hex_r * hex_r * 8.0) * u.beat * 2.0 * flash_on;
    col += vec3f(center_glow);

    return vec4f(col, 1.0);
}
