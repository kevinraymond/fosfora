// Fosfora palette library — ported from spectral-senses-old palette.glsl

// iq cosine palette: a + b * cos(2*pi*(c*t + d))
fn fosfora_palette(t: f32, a: vec3f, b: vec3f, c: vec3f, d: vec3f) -> vec3f {
    return a + b * cos(6.28318 * (c * t + d));
}

// Audio-driven palette interpolating warm (low centroid) to cool (high centroid)
fn fosfora_audio_palette(t: f32, cent: f32, phase: f32) -> vec3f {
    // Warm palette: deep reds, oranges, golds
    let a_warm = vec3f(0.5, 0.3, 0.2);
    let b_warm = vec3f(0.5, 0.3, 0.2);
    let c_warm = vec3f(1.0, 0.7, 0.4);
    let d_warm = vec3f(0.0, 0.15, 0.2);

    // Cool palette: electric blues, cyans, magentas
    let a_cool = vec3f(0.3, 0.4, 0.6);
    let b_cool = vec3f(0.4, 0.3, 0.4);
    let c_cool = vec3f(1.0, 1.0, 1.2);
    let d_cool = vec3f(0.0, 0.33, 0.67);

    let a = mix(a_warm, a_cool, cent);
    let b = mix(b_warm, b_cool, cent);
    let c = mix(c_warm, c_cool, cent);
    let d = mix(d_warm, d_cool, cent);

    return a + b * cos(6.28318 * (c * t + d + phase));
}

// Simplified bioluminescent palette
fn fosfora_bioluminescent(t: f32, cent: f32) -> vec3f {
    let warm = vec3f(1.0, 0.3, 0.05) * (0.5 + 0.5 * sin(t * 3.14159));
    let cool = vec3f(0.1, 0.7, 1.0) * (0.5 + 0.5 * cos(t * 3.14159 + 1.0));
    return mix(warm, cool, cent);
}

// Hue shift via Rodrigues rotation around (1,1,1) axis
fn fosfora_hue_shift(c: vec3f, shift: f32) -> vec3f {
    let a = shift * 6.28318;
    let co = cos(a);
    let si = sin(a);
    let k = vec3f(0.57735);
    return c * co + cross(k, c) * si + k * dot(k, c) * (1.0 - co);
}

// Key-locked hue on the circle of fifths (A11 #1462, Chromatica #1477).
// `key_class` is the detected tonic pitch class / 11 (0=C .. 1.0=B); `is_minor` is 0/1.
// Recover the integer pitch class, map it AROUND THE CIRCLE OF FIFTHS so neighbouring keys
// are a perfect fifth apart (Newton/Scriabin-style), and return a hue in 0..1. Minor keys
// are nudged toward the cool side of the wheel. `key_class` is held on silence upstream, so
// a layer that ties its palette to this locks to the song's key rather than sweeping.
// Keep this in sync with the `audio.key_hue` binding source (bindings/sources.rs::key_hue).
fn fosfora_key_hue(key_class: f32, is_minor: f32) -> f32 {
    let pc = round(clamp(key_class, 0.0, 1.0) * 11.0); // 0..11 pitch class
    let fifths = (pc * 7.0) % 12.0;                     // circle-of-fifths position
    return fract(fifths / 12.0 - is_minor * 0.08);
}

// ---- Deprecated aliases (pre-rename API, kept so user custom effects keep
// compiling). Do not use in new code; may be removed in a future major release. ----
fn phosphor_palette(t: f32, a: vec3f, b: vec3f, c: vec3f, d: vec3f) -> vec3f {
    return fosfora_palette(t, a, b, c, d);
}
fn phosphor_audio_palette(t: f32, cent: f32, phase: f32) -> vec3f {
    return fosfora_audio_palette(t, cent, phase);
}
fn phosphor_bioluminescent(t: f32, cent: f32) -> vec3f {
    return fosfora_bioluminescent(t, cent);
}
fn phosphor_hue_shift(c: vec3f, shift: f32) -> vec3f {
    return fosfora_hue_shift(c, shift);
}
fn phosphor_key_hue(key_class: f32, is_minor: f32) -> f32 {
    return fosfora_key_hue(key_class, is_minor);
}
