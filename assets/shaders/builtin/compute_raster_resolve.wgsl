// Compute raster resolve: fullscreen triangle reads atomic framebuffer, decodes, tonemaps.
// Outputs to render target with hardware blend state (LoadOp::Load).

struct ResolveUniforms {
    width: u32,
    height: u32,
    mode: u32,       // 0 = additive (tonemap), 1 = alpha blend
    _pad: u32,
}

@group(0) @binding(0) var<storage, read> fb_r: array<i32>;
@group(0) @binding(1) var<storage, read> fb_g: array<i32>;
@group(0) @binding(2) var<storage, read> fb_b: array<i32>;
@group(0) @binding(3) var<storage, read> fb_a: array<i32>;
@group(0) @binding(4) var<uniform> u: ResolveUniforms;

const INV_PRECISION: f32 = 1.0 / 4096.0;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    // Fullscreen triangle (same pattern as crossfade.wgsl)
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

    // Bounds check
    if ix >= u.width || iy >= u.height {
        return vec4f(0.0, 0.0, 0.0, 0.0);
    }

    let idx = iy * u.width + ix;

    // Decode fixed-point
    let r = f32(fb_r[idx]) * INV_PRECISION;
    let g = f32(fb_g[idx]) * INV_PRECISION;
    let b = f32(fb_b[idx]) * INV_PRECISION;
    let a = f32(fb_a[idx]) * INV_PRECISION;

    if u.mode == 0u {
        // Additive mode: Reinhard tonemap to prevent clipping
        let color = vec3f(r, g, b);
        let mapped = color / (1.0 + color);
        return vec4f(mapped, clamp(a, 0.0, 1.0));
    } else {
        // Alpha blend mode: clamp and pass through
        return vec4f(
            clamp(r, 0.0, 1.0),
            clamp(g, 0.0, 1.0),
            clamp(b, 0.0, 1.0),
            clamp(a, 0.0, 1.0),
        );
    }
}
