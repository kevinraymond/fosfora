/// Fullscreen triangle vertex shader snippet (WGSL).
/// Uses the vertex_index trick: 3 vertices cover the entire screen
/// without needing a vertex buffer.
pub const FULLSCREEN_TRIANGLE_VS: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4f {
    let x = f32(i32(vi & 1u) * 4) - 1.0;
    let y = f32(i32(vi & 2u) * 2) - 1.0;
    return vec4f(x, y, 0.0, 1.0);
}
"#;

/// Fullscreen triangle vertex shader that also outputs UVs.
/// Used by blit and post-processing passes that need texture coordinates.
pub const FULLSCREEN_TRIANGLE_VS_WITH_UV: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let x = f32(i32(vi & 1u) * 4) - 1.0;
    let y = f32(i32(vi & 2u) * 2) - 1.0;
    var out: VertexOutput;
    out.position = vec4f(x, y, 0.0, 1.0);
    // Map clip coords [-1,1] to UVs [0,1], flip Y for texture sampling
    out.uv = vec2f((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}
"#;
