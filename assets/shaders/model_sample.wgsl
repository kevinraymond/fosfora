// Model-to-particle sampling pass (#1993 — BYO models as a particle source).
//
// Renders a posed 3D model (triangle mesh or splat point cloud) into an offscreen
// RGBA frame which `image_source::sample_rgba_buffer` then decomposes into particle
// home positions and colours — the same call video and webcam frames already make.
// So this shader's whole job is to produce something that looks like a PNG of the
// model: shaded colour where the model is, and ALPHA 0 everywhere else.
//
// That transparent background is load-bearing, not cosmetic. The grid sampler
// rejects pixels with alpha < 10 (image_source.rs), twice, so clearing to
// transparent means the silhouette comes out for free and not one particle is
// spent on background.
//
// Distinct from `obstacle_model.wgsl`, which rasters the same geometry into a
// near-bright DEPTH ramp for collision. That one answers "how far away is this",
// this one answers "what colour is this" — hence a real depth buffer here, so
// nearer surfaces win, rather than depth-as-output.

struct SampleUniforms {
    // Split model-view and projection so splat billboards can offset in view
    // space (screen-facing quads of a world-space radius).
    mv: mat4x4<f32>,
    proj: mat4x4<f32>,
    radius_scale: f32,
    // Floor on the lambert term. Cavities that fall to pure black still carry
    // alpha 1, so they are a legitimate dark TONE rather than background — but a
    // mesh whose interior reads solid black gives Pegboard and Etch nothing to
    // quantize, so lift the shadows off the floor.
    ambient: f32,
    _pad0: f32,
    _pad1: f32,
    base_color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: SampleUniforms;

// Key light in VIEW space, so the model stays lit from the viewer's upper left at
// any yaw. A world-space light would swing the lit face away as the model turns
// and hand the sampler a silhouette that is mostly shadow.
const KEY_DIR = vec3<f32>(-0.4, 0.6, 0.7);

// Perceptual tone -> linear, so a shading value written here comes back out of
// the sRGB target as that same value in the BYTE the sampler reads.
//
// Without this the lambert term is treated as linear light and the sRGB encode
// lifts it: a cube's three visible faces landed at bytes 0.78/0.87/0.97, a fifth
// of the available range, and every tone-quantizing effect downstream (Pegboard's
// eight-peg tray, Etch's scan bands) saw them as nearly the same shade. Encoding
// the intent instead spreads the same three faces across 0.56/0.72/0.92.
fn srgb_to_linear(c: f32) -> f32 {
    if (c <= 0.04045) {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

fn shade(normal: vec3<f32>) -> vec4<f32> {
    let n = normalize(normal);
    let l = normalize(KEY_DIR);
    // abs(), not max(..., 0): mesh winding is unreliable (obstacle_model.rs keeps
    // both faces for the same reason), so a back-facing normal is a lighting
    // artefact rather than a surface that should go black.
    let lambert = abs(dot(n, l));
    let lit = u.ambient + (1.0 - u.ambient) * lambert;
    let rgb = u.base_color.rgb * lit;
    return vec4<f32>(
        srgb_to_linear(rgb.r),
        srgb_to_linear(rgb.g),
        srgb_to_linear(rgb.b),
        1.0,
    );
}

// ---- Mesh path: indexed triangles ----------------------------------------

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) view_normal: vec3<f32>,
};

@vertex
fn vs_mesh(
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
) -> MeshOut {
    var out: MeshOut;
    out.clip = u.proj * (u.mv * vec4<f32>(pos, 1.0));
    // The model transform is a rotation of a uniformly-normalized model, so the
    // upper 3x3 of mv carries normals correctly with no inverse-transpose.
    out.view_normal = (u.mv * vec4<f32>(normal, 0.0)).xyz;
    return out;
}

@fragment
fn fs_mesh(in: MeshOut) -> @location(0) vec4<f32> {
    return shade(in.view_normal);
}

// ---- Splat path: instanced screen-facing quads ---------------------------

// Unit-quad corners (two triangles), generated from vertex_index so no
// per-vertex buffer is needed — draw 6 verts x instance_count.
const QUAD = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0), vec2<f32>( 1.0, -1.0), vec2<f32>( 1.0,  1.0),
    vec2<f32>(-1.0, -1.0), vec2<f32>( 1.0,  1.0), vec2<f32>(-1.0,  1.0),
);

struct SplatOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec3<f32>,
};

@vertex
fn vs_splat(
    @builtin(vertex_index) vi: u32,
    // instance a: xyz = center, w = radius (world units)
    @location(0) inst_a: vec4<f32>,
    // instance b: rgb = splat colour
    @location(1) inst_b: vec4<f32>,
) -> SplatOut {
    var out: SplatOut;
    let corner = QUAD[vi];
    var view_pos = u.mv * vec4<f32>(inst_a.xyz, 1.0);
    view_pos = vec4<f32>(view_pos.xy + corner * inst_a.w * u.radius_scale, view_pos.zw);
    out.clip = u.proj * view_pos;
    out.uv = corner;
    out.color = inst_b.rgb;
    return out;
}

@fragment
fn fs_splat(in: SplatOut) -> @location(0) vec4<f32> {
    // Round splats: drop the quad corners so overlapping discs approximate the
    // surface rather than a grid of squares.
    if (dot(in.uv, in.uv) > 1.0) {
        discard;
    }
    // A capture already carries its own baked lighting, so shading it again would
    // double up. Tint by base_color so the same control works on both paths.
    return vec4<f32>(in.color * u.base_color.rgb, 1.0);
}
