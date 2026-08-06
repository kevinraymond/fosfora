// Overlay primitives (overlay effect family): staggering, trigger envelopes,
// grid reveals, flood fills, and stroke/bracket/crosshair SDFs.
//
// Rules of this lib (it is prepended to EVERY effect shader, fragment AND
// compute — the compute side has ParticleUniforms, not PhosphorUniforms):
//   - no `u.` references: all phases arrive as parameters. Callers pass
//     beat/bar phases (and `u.bar_index + u.bar_phase` derived cycles) in.
//   - every name carries the `ovl_` prefix (bare `hash`/`hash2` collide with
//     particle_lib.wgsl in compute shaders).
//   - pure functions only: no textures, no state, no derivatives (fwidth is
//     fragment-only and this lib must compile in compute modules).
// Hashing builds on `fosfora_ihash` from noise.wgsl.

// u32 hash → [0, 1).
fn ovl_hash01(x: u32) -> f32 {
    return f32(fosfora_ihash(x)) * (1.0 / 4294967296.0);
}

// Combine an element index with a float seed (the effect's `seed` param) into
// a stable hash in [0, 1). bitcast, not truncation: fractional seeds count.
fn ovl_cell_hash(cell: u32, seed: f32) -> f32 {
    return ovl_hash01(cell ^ fosfora_ihash(bitcast<u32>(seed)));
}

// Per-element deterministic stagger: stable phase offset in [0, max_offset).
fn ovl_stagger(index: u32, seed: f32, max_offset: f32) -> f32 {
    return ovl_cell_hash(index, seed) * max_offset;
}

// Trigger envelope on a cyclic phase in [0, 1): 0 before `trigger_at`, a
// shaped snap-in over `attack`, 1.0 for `hold`, a linear fall over `release`,
// then 0 until the phase wraps around to the trigger again. All durations are
// in phase units.
fn ovl_trigger(phase: f32, trigger_at: f32, attack: f32, hold: f32, release: f32) -> f32 {
    let t = fract(phase - trigger_at);
    let a = max(attack, 1e-4);
    if t < a {
        let r = t / a;
        return r * r * (3.0 - 2.0 * r); // smooth snap-in
    }
    if t < a + hold {
        return 1.0;
    }
    let rel = max(release, 1e-4);
    return clamp(1.0 - (t - a - hold) / rel, 0.0, 1.0);
}

// Grid helpers. Cell ids are row-major; uv outside [0,1) clamps to the edge
// cells so ids stay in range.
fn ovl_cell_id(uv: vec2f, cols: u32, rows: u32) -> u32 {
    let g = vec2f(f32(cols), f32(rows));
    let c = min(vec2u(clamp(uv, vec2f(0.0), vec2f(0.9999)) * g), vec2u(cols - 1u, rows - 1u));
    return c.y * cols + c.x;
}

fn ovl_cell_uv(uv: vec2f, cols: u32, rows: u32) -> vec2f {
    return fract(uv * vec2f(f32(cols), f32(rows)));
}

// Threshold reveal: an element with hash h turns on as `phase` passes h, with
// a `softness`-wide smooth edge — the stateless render-bucket / dissolve
// primitive. phase 0 → nothing on; phase 1 → everything on.
fn ovl_reveal(cell_hash: f32, phase: f32, softness: f32) -> f32 {
    let s = max(softness, 1e-4);
    return clamp((phase - cell_hash) / s + 0.5, 0.0, 1.0);
}

// Flood-fill approximation: growth from up to 8 seed points, hash-jittered
// wavefront thresholded by `phase` (0 = nothing, 1 = frame fully covered).
// A distance field with noise on the front — NOT a cellular automaton; no
// state allowed under the phase-locked contract.
fn ovl_flood(uv: vec2f, seeds: array<vec2f, 8>, n: u32, phase: f32, jitter: f32, seed: f32) -> f32 {
    var dmin = 1e9;
    for (var i = 0u; i < min(n, 8u); i++) {
        dmin = min(dmin, distance(uv, seeds[i]));
    }
    let j = (fosfora_hash2(uv * 41.0 + vec2f(seed * 0.013, seed * 0.007)) - 0.5) * jitter;
    // 1.6 ≳ the farthest a corner can sit from any seed in unit uv space.
    let front = phase * 1.6;
    return smoothstep(-0.02, 0.02, front - (dmin + j));
}

// Signed distance to an axis-aligned rectangle (negative inside).
fn ovl_rect_sdf(uv: vec2f, center: vec2f, half_size: vec2f) -> f32 {
    let d = abs(uv - center) - half_size;
    return length(max(d, vec2f(0.0))) + min(max(d.x, d.y), 0.0);
}

// Stroked rectangle outline: coverage of a `thickness`-wide band on the
// rectangle's edge, with a soft edge scaled to the thickness (no derivatives).
fn ovl_rect_stroke(uv: vec2f, center: vec2f, half_size: vec2f, thickness: f32) -> f32 {
    let e = max(thickness * 0.25, 1e-4);
    return 1.0 - smoothstep(thickness * 0.5 - e, thickness * 0.5 + e, abs(ovl_rect_sdf(uv, center, half_size)));
}

// Corner brackets: the rectangle outline masked to within `arm` of each
// corner — the classic HUD frame element.
fn ovl_bracket(uv: vec2f, center: vec2f, half_size: vec2f, arm: f32, thickness: f32) -> f32 {
    let stroke = ovl_rect_stroke(uv, center, half_size, thickness);
    let q = abs(uv - center);
    let near_corner = max(
        step(half_size.x - arm, q.x),
        step(half_size.y - arm, q.y),
    );
    return stroke * near_corner;
}

// Crosshair: four `arm`-long bars of width `thickness` around `center`,
// leaving a `gap`-radius hole in the middle.
fn ovl_cross(uv: vec2f, center: vec2f, arm: f32, gap: f32, thickness: f32) -> f32 {
    let q = abs(uv - center);
    let e = max(thickness * 0.25, 1e-4);
    let half_t = thickness * 0.5;
    let in_reach_x = smoothstep(gap - e, gap + e, q.x) * (1.0 - smoothstep(arm - e, arm + e, q.x));
    let in_reach_y = smoothstep(gap - e, gap + e, q.y) * (1.0 - smoothstep(arm - e, arm + e, q.y));
    let bar_h = (1.0 - smoothstep(half_t - e, half_t + e, q.y)) * in_reach_x;
    let bar_v = (1.0 - smoothstep(half_t - e, half_t + e, q.x)) * in_reach_y;
    return max(bar_h, bar_v);
}

// Stroked line segment from `a` to `b`.
fn ovl_segment(uv: vec2f, a: vec2f, b: vec2f, thickness: f32) -> f32 {
    let e = max(thickness * 0.25, 1e-4);
    let pa = uv - a;
    let ba = b - a;
    let t = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-8), 0.0, 1.0);
    let d = length(pa - ba * t);
    return 1.0 - smoothstep(thickness * 0.5 - e, thickness * 0.5 + e, d);
}

// Stroked circle of radius `radius`.
fn ovl_ring(uv: vec2f, center: vec2f, radius: f32, thickness: f32) -> f32 {
    let e = max(thickness * 0.25, 1e-4);
    let d = abs(length(uv - center) - radius);
    return 1.0 - smoothstep(thickness * 0.5 - e, thickness * 0.5 + e, d);
}

// Partial ring: `start` and `sweep` in turns (0..1), so rotations driven by a
// cycle phase loop exactly. Sweep >= 1 is a full ring.
fn ovl_arc(uv: vec2f, center: vec2f, radius: f32, start: f32, sweep: f32, thickness: f32) -> f32 {
    let d = uv - center;
    // atan2 -> turns in [0,1)
    let ang = fract(atan2(d.y, d.x) * 0.15915494 - start);
    let soft = 0.01;
    let mask = smoothstep(0.0, soft, ang) * (1.0 - smoothstep(sweep - soft, sweep, ang));
    return ovl_ring(uv, center, radius, thickness) * select(mask, 1.0, sweep >= 1.0);
}

// `count` radial tick marks on a ring: from `radius` outward by `len`.
fn ovl_ticks_ring(uv: vec2f, center: vec2f, radius: f32, count: f32, len: f32, thickness: f32) -> f32 {
    let d = uv - center;
    let r = length(d);
    let e = max(len * 0.2, 1e-4);
    let radial = smoothstep(radius - e, radius, r) * (1.0 - smoothstep(radius + len, radius + len + e, r));
    // Angular slot width from arc-length: thickness / circumference, in turns.
    let ang = fract(atan2(d.y, d.x) * 0.15915494);
    let slot = fract(ang * count);
    let w = thickness * count * 0.15915494 / max(r, 1e-4);
    let sw = max(w * 0.35, 1e-3);
    let tick = 1.0 - smoothstep(w - sw, w + sw, min(slot, 1.0 - slot) * 2.0);
    return radial * tick;
}

// ---- Deprecated aliases (pre-rename API, kept so user custom effects keep
// compiling). Do not use in new code; may be removed in a future major release. ----

