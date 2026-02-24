// Instanced quad particle renderer.
// Vertex-pulling from storage buffer, 6 vertices per particle (two triangles).

struct Particle {
    pos_life: vec4f,
    vel_size: vec4f,
    color: vec4f,
    flags: vec4f,
}

struct RenderUniforms {
    resolution: vec2f,
    time: f32,
    _pad: f32,
}

@group(0) @binding(0) var<storage, read> particles: array<Particle>;
@group(0) @binding(1) var<uniform> ru: RenderUniforms;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) color: vec4f,
    @location(1) quad_uv: vec2f,
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let p = particles[instance_index];
    var out: VertexOutput;

    // Dead particles: collapse to degenerate triangle (GPU clips trivially)
    if p.pos_life.w <= 0.0 {
        out.position = vec4f(0.0, 0.0, 0.0, 1.0);
        out.color = vec4f(0.0);
        out.quad_uv = vec2f(0.0);
        return out;
    }

    // Quad corners: 2 triangles from 6 vertices
    // 0,1,2, 2,3,0 pattern
    var corner: vec2f;
    let vi = vertex_index % 6u;
    switch vi {
        case 0u: { corner = vec2f(-1.0, -1.0); }
        case 1u: { corner = vec2f( 1.0, -1.0); }
        case 2u: { corner = vec2f( 1.0,  1.0); }
        case 3u: { corner = vec2f( 1.0,  1.0); }
        case 4u: { corner = vec2f(-1.0,  1.0); }
        case 5u: { corner = vec2f(-1.0, -1.0); }
        default: { corner = vec2f(0.0); }
    }

    let size = p.vel_size.w;
    // Correct aspect ratio so particles are circular
    let aspect = ru.resolution.x / ru.resolution.y;
    let offset = corner * size * vec2f(1.0 / aspect, 1.0);

    let pos = p.pos_life.xy + offset;
    out.position = vec4f(pos, 0.0, 1.0);
    out.color = p.color;
    out.quad_uv = corner;

    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // Soft circle glow
    let dist = length(in.quad_uv);
    let glow = exp(-dist * dist * 3.0);

    // Discard fully transparent
    if glow < 0.01 {
        discard;
    }

    return vec4f(in.color.rgb * glow, in.color.a * glow);
}
