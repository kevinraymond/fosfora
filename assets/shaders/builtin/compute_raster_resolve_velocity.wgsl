// Velocity resolve (#1482 Chronoflow): decode the atomic velocity channels into
// a texture the pass graph exposes as the `@particles.velocity` input.
// Output per pixel: (vx, vy, coverage, 0) where vx/vy is the alpha-weighted mean
// particle velocity in NDC units/sec (y up — consumers flip y for uv space) and
// coverage is the clamped alpha sum (how strongly particles claim this pixel).
// Runs at the end of ParticleSystem::dispatch, before the fragment passes, so
// the passes read same-frame velocity.

struct VelResolveUniforms {
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<storage, read> fb_vx: array<i32>;
@group(0) @binding(1) var<storage, read> fb_vy: array<i32>;
@group(0) @binding(2) var<storage, read> fb_a: array<i32>;
@group(0) @binding(3) var<uniform> u: VelResolveUniforms;

const INV_PRECISION: f32 = 1.0 / 4096.0;    // alpha channel fixed point
const INV_V_PRECISION: f32 = 1.0 / 65536.0; // velocity channel fixed point

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    // Fullscreen triangle (same pattern as compute_raster_resolve.wgsl)
    var out: VertexOutput;
    let x = f32(i32(vi & 1u)) * 4.0 - 1.0;
    let y = f32(i32(vi >> 1u)) * 4.0 - 1.0;
    out.position = vec4f(x, y, 0.0, 1.0);
    out.uv = vec2f((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let ix = u32(floor(in.uv.x * f32(u.width)));
    let iy = u32(floor(in.uv.y * f32(u.height)));

    if ix >= u.width || iy >= u.height {
        return vec4f(0.0);
    }

    let idx = iy * u.width + ix;

    let wsum = f32(fb_a[idx]) * INV_PRECISION;
    if wsum <= 1e-5 {
        return vec4f(0.0);
    }

    // Weighted mean: Σ(v·w·V_PREC) / Σ(w·PREC), rescaled to plain NDC/s.
    let inv = 1.0 / wsum;
    let vx = f32(fb_vx[idx]) * INV_V_PRECISION * inv;
    let vy = f32(fb_vy[idx]) * INV_V_PRECISION * inv;

    return vec4f(vx, vy, clamp(wsum, 0.0, 1.0), 0.0);
}
