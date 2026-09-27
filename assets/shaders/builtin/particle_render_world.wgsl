// World-space particle renderer for the XR build: sprites that live in meters
// in 3D, drawn once per eye through that eye's view and projection, depth
// tested against other geometry.
//
// Expects the world layout documented at `struct Particle` in particle_lib.wgsl:
// pos_life.xyz = position in meters relative to the effect anchor, w = life;
// vel_size.w = sprite radius in meters; color = rgba.
//
// Vertex pulling: one non-instanced draw of 3 * alive vertices (args written
// by particle_prepare_indirect_world.wgsl); the vertex index picks the
// particle and the corner. Each sprite is a single triangle circumscribing the
// unit disc, which is all the soft disc below needs. On a tiler (Quest 3's
// Adreno 740) this is ~1.5x cheaper than one instance per sprite, and three
// vertices instead of six another 1.25-1.6x.
//
// Mode 0 (the soft circle) only: sprite atlases, trails and spin are not
// supported on this path.

struct WorldCamera {
    view: mat4x4f,
    proj: mat4x4f,
    anchor: vec4f,   // xyz added to every position, w unused
    gain: f32,       // rgb gain, the 2D path's composite_gain
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

// Group 0 is the 2D renderer's bind group layout, unchanged; only the
// bindings this shader reads are declared.
@group(0) @binding(0) var<storage, read> pos_life: array<vec4f>;
@group(0) @binding(1) var<storage, read> vel_size: array<vec4f>;
@group(0) @binding(2) var<storage, read> color: array<vec4f>;
@group(0) @binding(5) var<storage, read> alive_indices: array<u32>;

@group(1) @binding(0) var<uniform> cam: WorldCamera;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) color: vec4f,
    @location(1) quad_uv: vec2f,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let particle_idx = alive_indices[vertex_index / 3u];
    let pl = pos_life[particle_idx];
    let radius = vel_size[particle_idx].w;

    // Triangle circumscribing the unit disc (inradius 1, centered on the origin).
    var corner: vec2f;
    switch vertex_index % 3u {
        case 0u: { corner = vec2f(-1.7320508, -1.0); }
        case 1u: { corner = vec2f( 1.7320508, -1.0); }
        default: { corner = vec2f( 0.0, 2.0); }
    }

    // Offset in view space so the sprite has a size in meters and faces the eye.
    let view_pos = cam.view * vec4f(pl.xyz + cam.anchor.xyz, 1.0);
    var out: VertexOutput;
    out.position = cam.proj * (view_pos + vec4f(corner * radius, 0.0, 0.0));
    out.color = color[particle_idx];
    out.quad_uv = corner;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // Same soft disc as particle_render.wgsl mode 0.
    let dist = length(in.quad_uv);
    let glow = exp(-dist * dist * 2.0);
    if glow < 0.01 {
        discard;
    }
    return vec4f(in.color.rgb * glow * cam.gain, in.color.a * glow);
}
