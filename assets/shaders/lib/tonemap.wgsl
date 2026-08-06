// Fosfora tonemapping library — ported from spectral-senses-old tonemap.glsl

// ACES filmic tonemapping
fn fosfora_aces_tonemap(x: vec3f) -> vec3f {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3f(0.0), vec3f(1.0));
}

// Linear to sRGB gamma
fn fosfora_linear_to_srgb(c: vec3f) -> vec3f {
    return pow(max(c, vec3f(0.0)), vec3f(1.0 / 2.2));
}

// ---- Deprecated aliases (pre-rename API, kept so user custom effects keep
// compiling). Do not use in new code; may be removed in a future major release. ----
fn phosphor_aces_tonemap(x: vec3f) -> vec3f {
    return fosfora_aces_tonemap(x);
}
fn phosphor_linear_to_srgb(c: vec3f) -> vec3f {
    return fosfora_linear_to_srgb(c);
}
