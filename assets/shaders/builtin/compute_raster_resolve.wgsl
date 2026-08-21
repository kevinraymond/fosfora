// Compute raster resolve: fullscreen triangle reads atomic framebuffer, decodes, tonemaps.
// Outputs to render target with hardware blend state (LoadOp::Load).

struct ResolveUniforms {
    width: u32,
    height: u32,
    mode: u32,       // 0 = additive (tonemap), 1 = alpha blend, 2 = weighted-average OIT (#1800)
    // Frame-rate correction for the source gain of the additive composite (#2349).
    // 1.0 at 60 fps and for every effect with no feedback background.
    //
    // ADDITIVE ONLY, deliberately. Additive compositing is out = a*col + k*prev
    // with a and k independent, so holding the steady state a/(1-k) is exactly a
    // scale on the source. Alpha blending is out = c*alpha + prev*k*(1-alpha):
    // source and destination weights are coupled through alpha, its steady state
    // is c*alpha/(1 - k*(1-alpha)), and no scale on c reproduces the correction.
    // Mode 1 and mode 2 (Splat OIT) therefore ignore this rather than apply
    // something that is wrong in a different direction.
    composite_gain: f32,
}

// Mode 2 coverage: opacity from the accumulated weight sum. The sim folds an
// OIT_ALPHA_SCALE of 0.125 into every per-splat weight (i32 overflow headroom
// at 3M splats); this gain compensates it (8×) plus a density factor so a
// solid region of overlapping splats saturates to fully opaque instead of the
// ~55% translucency the old 12.0 left. Only the mode-2 (Splat #1800) branch
// reads this — Splat is the sole "blend":"oit" effect.
const COVERAGE_GAIN: f32 = 40.0;

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
        // Additive mode: Reinhard tonemap to prevent clipping.
        // The gain applies to the LINEAR accumulation, before the tonemap — it
        // scales how much light this frame's particles contribute, and the
        // tonemap is a display transform on the result. Applying it after would
        // scale an already-compressed value. Note the correction is exact only
        // while the tonemap is in its linear regime; where color >> 1 it
        // saturates and a bright core moves less than the formula predicts.
        let color = vec3f(r, g, b) * u.composite_gain;
        let mapped = color / (1.0 + color);
        return vec4f(mapped, clamp(a, 0.0, 1.0));
    } else if u.mode == 2u {
        // Weighted-average OIT (Splat #1800): the accumulator already holds
        // fb_rgb = Σ color·weight and fb_a = Σ weight (weight = α · depth
        // factor · OIT_ALPHA_SCALE, folded into color.a by the sim), so the
        // order-independent average is one division — no sorting, no WBOIT
        // composition. Empty pixels output a = 0 and the (SrcAlpha,
        // 1−SrcAlpha) blend preserves the background pass untouched.
        if a <= 1e-6 {
            return vec4f(0.0, 0.0, 0.0, 0.0);
        }
        let avg = vec3f(r, g, b) / a; // scale-invariant: OIT_ALPHA_SCALE cancels
        let coverage = 1.0 - exp(-a * COVERAGE_GAIN);
        return vec4f(clamp(avg, vec3f(0.0), vec3f(1.0)), coverage);
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
