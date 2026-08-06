// Fosfora SDF library — ported from spectral-senses-old sdf-lib.glsl

// Primitives
fn fosfora_sd_sphere(p: vec3f, r: f32) -> f32 {
    return length(p) - r;
}

fn fosfora_sd_torus(p: vec3f, t: vec2f) -> f32 {
    let q = vec2f(length(p.xz) - t.x, p.y);
    return length(q) - t.y;
}

fn fosfora_sd_box(p: vec3f, b: vec3f) -> f32 {
    let q = abs(p) - b;
    return length(max(q, vec3f(0.0))) + min(max(q.x, max(q.y, q.z)), 0.0);
}

fn fosfora_sd_cylinder(p: vec3f, h: f32, r: f32) -> f32 {
    let d = abs(vec2f(length(p.xz), p.y)) - vec2f(r, h);
    return min(max(d.x, d.y), 0.0) + length(max(d, vec2f(0.0)));
}

fn fosfora_sd_plane(p: vec3f, n: vec3f, h: f32) -> f32 {
    return dot(p, n) + h;
}

// Boolean operations
fn fosfora_op_union(d1: f32, d2: f32) -> f32 {
    return min(d1, d2);
}

fn fosfora_op_subtract(d1: f32, d2: f32) -> f32 {
    return max(-d1, d2);
}

fn fosfora_op_intersect(d1: f32, d2: f32) -> f32 {
    return max(d1, d2);
}

// Smooth min (polynomial)
fn fosfora_smin(a: f32, b: f32, k: f32) -> f32 {
    let h = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0);
    return mix(b, a, h) - k * h * (1.0 - h);
}

// Smooth subtraction
fn fosfora_smax(a: f32, b: f32, k: f32) -> f32 {
    let h = clamp(0.5 - 0.5 * (b + a) / k, 0.0, 1.0);
    return mix(b, -a, h) + k * h * (1.0 - h);
}

// Domain operations
fn fosfora_op_rep(p: vec3f, c: vec3f) -> vec3f {
    return ((p + 0.5 * c) % c) - 0.5 * c;
}

fn fosfora_op_rep_lim(p: vec3f, c: f32, l: vec3f) -> vec3f {
    return p - c * clamp(round(p / c), -l, l);
}

// Twist around Y axis
fn fosfora_op_twist(p: vec3f, k: f32) -> vec3f {
    let c = cos(k * p.y);
    let s = sin(k * p.y);
    let xz = vec2f(c * p.x - s * p.z, s * p.x + c * p.z);
    return vec3f(xz.x, p.y, xz.y);
}

// --- 2D primitives (screen-space; used by line/beam effects) ---

// Distance from point p to the line segment a->b. Endpoints must be distinct;
// callers (e.g. the Beam scope trace) always pass neighboring samples that differ.
fn fosfora_sd_segment2(p: vec2f, a: vec2f, b: vec2f) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}

// Signed distance to a circle of radius r centered at the origin.
fn fosfora_sd_circle2(p: vec2f, r: f32) -> f32 {
    return length(p) - r;
}

// ---- Deprecated aliases (pre-rename API, kept so user custom effects keep
// compiling). Do not use in new code; may be removed in a future major release. ----
fn phosphor_sd_sphere(p: vec3f, r: f32) -> f32 {
    return fosfora_sd_sphere(p, r);
}
fn phosphor_sd_torus(p: vec3f, t: vec2f) -> f32 {
    return fosfora_sd_torus(p, t);
}
fn phosphor_sd_box(p: vec3f, b: vec3f) -> f32 {
    return fosfora_sd_box(p, b);
}
fn phosphor_sd_cylinder(p: vec3f, h: f32, r: f32) -> f32 {
    return fosfora_sd_cylinder(p, h, r);
}
fn phosphor_sd_plane(p: vec3f, n: vec3f, h: f32) -> f32 {
    return fosfora_sd_plane(p, n, h);
}
fn phosphor_op_union(d1: f32, d2: f32) -> f32 {
    return fosfora_op_union(d1, d2);
}
fn phosphor_op_subtract(d1: f32, d2: f32) -> f32 {
    return fosfora_op_subtract(d1, d2);
}
fn phosphor_op_intersect(d1: f32, d2: f32) -> f32 {
    return fosfora_op_intersect(d1, d2);
}
fn phosphor_smin(a: f32, b: f32, k: f32) -> f32 {
    return fosfora_smin(a, b, k);
}
fn phosphor_smax(a: f32, b: f32, k: f32) -> f32 {
    return fosfora_smax(a, b, k);
}
fn phosphor_op_rep(p: vec3f, c: vec3f) -> vec3f {
    return fosfora_op_rep(p, c);
}
fn phosphor_op_rep_lim(p: vec3f, c: f32, l: vec3f) -> vec3f {
    return fosfora_op_rep_lim(p, c, l);
}
fn phosphor_op_twist(p: vec3f, k: f32) -> vec3f {
    return fosfora_op_twist(p, k);
}
fn phosphor_sd_segment2(p: vec2f, a: vec2f, b: vec2f) -> f32 {
    return fosfora_sd_segment2(p, a, b);
}
fn phosphor_sd_circle2(p: vec2f, r: f32) -> f32 {
    return fosfora_sd_circle2(p, r);
}
