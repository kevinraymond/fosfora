// WBOIT composite pass: fullscreen triangle reads accumulation + revealage textures,
// composites transparent particles onto the opaque scene with SrcAlpha/OneMinusSrcAlpha blend.

@group(0) @binding(0) var accum_tex: texture_2d<f32>;
@group(0) @binding(1) var reveal_tex: texture_2d<f32>;
@group(0) @binding(2) var tex_sampler: sampler;

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
    let accum = textureSample(accum_tex, tex_sampler, in.uv);
    let reveal = textureSample(reveal_tex, tex_sampler, in.uv).r;

    // No transparent fragments here
    if accum.a < 1e-5 {
        discard;
    }

    let avg_color = accum.rgb / max(accum.a, 1e-5);
    return vec4f(avg_color, 1.0 - reveal);
}
