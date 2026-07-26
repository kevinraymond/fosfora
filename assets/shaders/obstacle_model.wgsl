// Obstacle-model depth pass (#1851 — "waterfall of roses / skulls").
//
// Renders a posed 3D model (triangle mesh or splat point cloud) into the
// per-layer obstacle field. The fragment writes a NEAR-BRIGHT normalized depth
// into all four channels; the particle sim samples the alpha channel as the
// collision height. Background (uncovered) texels stay at the cleared 0, so the
// model reads as a solid silhouette with a 2.5-D depth gradient across it.
//
// Projection is orthographic (fit to the model AABB), so framebuffer depth is
// linear in world Z — `d = 1 - depth` is a true near→far ramp.

struct ModelUniforms {
    // Split model-view and projection so splat billboards can offset in view
    // space (screen-facing quads of a world-space radius).
    mv: mat4x4<f32>,
    proj: mat4x4<f32>,
    radius_scale: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var<uniform> u: ModelUniforms;

// ---- Mesh path: indexed triangles ----------------------------------------

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_mesh(@location(0) pos: vec3<f32>) -> MeshOut {
    var out: MeshOut;
    out.clip = u.proj * (u.mv * vec4<f32>(pos, 1.0));
    return out;
}

// ---- Splat path: instanced screen-facing quads ---------------------------

// Unit-quad corners (two triangles), generated from vertex_index so no
// per-vertex buffer is needed — draw 6 verts × instance_count.
const QUAD = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0), vec2<f32>( 1.0, -1.0), vec2<f32>( 1.0,  1.0),
    vec2<f32>(-1.0, -1.0), vec2<f32>( 1.0,  1.0), vec2<f32>(-1.0,  1.0),
);

struct SplatOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_splat(
    @builtin(vertex_index) vi: u32,
    // instance: xyz = center, w = radius (world units)
    @location(0) inst: vec4<f32>,
) -> SplatOut {
    var out: SplatOut;
    let corner = QUAD[vi];
    var view_pos = u.mv * vec4<f32>(inst.xyz, 1.0);
    view_pos = vec4<f32>(view_pos.xy + corner * inst.w * u.radius_scale, view_pos.zw);
    out.clip = u.proj * view_pos;
    out.uv = corner;
    return out;
}

// ---- Shared fragment: near-bright depth ----------------------------------

// Front-surface relief exaggeration. The raw near→far ramp `1 - clip.z` packs a
// model's whole VISIBLE relief (a skull's brow→socket is only ~15-20% of its
// front-to-back depth) into a narrow high-d band; after 8-bit quantization that
// collapses to ~15 flat levels, so central-difference ∇h is ~0 across the face
// and spikes only at the silhouette — water hugged the outline and streamed
// straight through the interior. Remapping the front band [LO,HI]→[0,1] spreads
// the relief across the full range (~10x stronger gradients; sockets/temples,
// cheekbones and brow all carry real slope). These two constants are the tuning
// knobs — nudge LO up if the face washes to flat white, down if it stays grey.
// (A per-frame depth min/max reduction would make this exact for any rotation;
// baked constants suffice for the unit-normalized models we raster.)
const RELIEF_LO: f32 = 0.55;
const RELIEF_HI: f32 = 0.75;

fn relief(clip_z: f32) -> f32 {
    let d = 1.0 - clip_z;
    return clamp((d - RELIEF_LO) / (RELIEF_HI - RELIEF_LO), 0.0, 1.0);
}

@fragment
fn fs_mesh(in: MeshOut) -> @location(0) vec4<f32> {
    let d = relief(in.clip.z);
    return vec4<f32>(d, d, d, d);
}

@fragment
fn fs_splat(in: SplatOut) -> @location(0) vec4<f32> {
    // Round splats: drop the quad corners so overlapping discs approximate the
    // surface rather than a grid of squares.
    if (dot(in.uv, in.uv) > 1.0) {
        discard;
    }
    let d = relief(in.clip.z);
    return vec4<f32>(d, d, d, d);
}
