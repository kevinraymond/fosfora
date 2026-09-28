// S7 depth-only occluders: every obstacle box (scene anchors, floor) is
// drawn as a cube and every hand-joint sphere as a sphere impostor into the
// depth buffer with color writes off, before the sprites. Passthrough carries no depth, so without
// this a particle behind a real desk or hand renders on top of it and the
// obstacle void carved by the sim is invisible. Same `Obstacles` block as
// the sim (particles3d_sim.wgsl), bound to group 0 binding 0 here.

struct Obstacles {
    sphere_count: u32,
    box_count: u32,
    restitution: f32,
    margin: f32,
    // Hand spheres are padded for the collision; the occluder cube uses
    // the radius minus this (a joint-sized block), never below 1 cm.
    sphere_shrink: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
    spheres: array<vec4<f32>, 64>,
    box_center: array<vec4<f32>, 32>,
    box_rot: array<vec4<f32>, 32>,
    box_half: array<vec4<f32>, 32>,
}

@group(0) @binding(0) var<uniform> obstacles: Obstacles;

// Skinning matrices for the runtime's hand meshes (XR_FB_hand_tracking_mesh):
// per hand, one matrix per joint = this frame's joint pose x the inverse of
// the mesh's bind pose for that joint. HAND_JOINTS matches
// XR_HAND_JOINT_COUNT_EXT and `HAND_JOINTS` in particles3d.rs.
const HAND_JOINTS: u32 = 26u;
struct HandSkins {
    mats: array<mat4x4<f32>, 52>,
}

@group(0) @binding(1) var<uniform> hand_skins: HandSkins;

struct Eye {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
}

@group(1) @binding(0) var<uniform> eye: Eye;

fn quat_rotate(q: vec4<f32>, v: vec3<f32>) -> vec3<f32> {
    return v + 2.0 * cross(q.xyz, cross(q.xyz, v) + q.w * v);
}

// 36 vertices of a unit cube (both windings appear; culling is off).
fn cube_corner(vi: u32) -> vec3<f32> {
    var corners = array<vec3<f32>, 8>(
        vec3<f32>(-1.0, -1.0, -1.0), vec3<f32>(1.0, -1.0, -1.0),
        vec3<f32>(1.0, 1.0, -1.0), vec3<f32>(-1.0, 1.0, -1.0),
        vec3<f32>(-1.0, -1.0, 1.0), vec3<f32>(1.0, -1.0, 1.0),
        vec3<f32>(1.0, 1.0, 1.0), vec3<f32>(-1.0, 1.0, 1.0),
    );
    var idx = array<u32, 36>(
        0u, 1u, 2u, 0u, 2u, 3u,  // -z
        4u, 6u, 5u, 4u, 7u, 6u,  // +z
        0u, 4u, 5u, 0u, 5u, 1u,  // -y
        3u, 2u, 6u, 3u, 6u, 7u,  // +y
        0u, 3u, 7u, 0u, 7u, 4u,  // -x
        1u, 5u, 6u, 1u, 6u, 2u,  // +x
    );
    return corners[idx[vi]];
}

@vertex
fn vs_occluder(
    @builtin(vertex_index) vi: u32,
    @builtin(instance_index) ii: u32,
) -> @builtin(position) vec4<f32> {
    // Boxes only; spheres go through vs_sphere / fs_sphere below.
    let center = obstacles.box_center[ii].xyz;
    let q = obstacles.box_rot[ii];
    let half = obstacles.box_half[ii].xyz;
    let world = center + quat_rotate(q, cube_corner(vi % 36u) * half);
    return eye.view_proj * vec4<f32>(world, 1.0);
}

@fragment
fn fs_occluder() -> @location(0) vec4<f32> {
    // Color writes are masked off; only depth lands.
    return vec4<f32>(0.0);
}

// ---- hand meshes -------------------------------------------------------
// The runtime's skinned hand mesh (bind pose, static vertex buffer) skinned
// on the GPU by linear blending of up to four joints, drawn depth-only like
// the boxes. One draw per tracked hand; the instance index selects the
// hand's block of skinning matrices. This is what makes a hand read as a
// hand instead of a row of spheres.

struct HandVertex {
    @location(0) pos: vec3<f32>,
    @location(1) weights: vec4<f32>,
    @location(2) joints: vec4<u32>,
}

@vertex
fn vs_hand_mesh(
    v: HandVertex,
    @builtin(instance_index) hand: u32,
) -> @builtin(position) vec4<f32> {
    let base = hand * HAND_JOINTS;
    let p = vec4<f32>(v.pos, 1.0);
    var world = vec3<f32>(0.0);
    for (var i = 0u; i < 4u; i++) {
        let joint = min(v.joints[i], HAND_JOINTS - 1u);
        world += v.weights[i] * (hand_skins.mats[base + joint] * p).xyz;
    }
    return eye.view_proj * vec4<f32>(world, 1.0);
}

// ---- hand joints as sphere impostors -----------------------------------
// A camera-facing disc per sphere whose fragments write the depth of the
// sphere's surface, so the union of joints reads as a smooth hand rather
// than a pile of cubes.

struct SphereOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) view_center: vec3<f32>,
    @location(2) radius: f32,
}

@vertex
fn vs_sphere(
    @builtin(vertex_index) vi: u32,
    @builtin(instance_index) ii: u32,
) -> SphereOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let s = obstacles.spheres[ii];
    let radius = max(s.w - obstacles.sphere_shrink, 0.005);
    let view_center = (eye.view * vec4<f32>(s.xyz, 1.0)).xyz;
    let corner = corners[vi % 6u];
    // The disc sits at the sphere's near point so a sphere that straddles
    // the near plane is not clipped away whole.
    let view_pos = view_center + vec3<f32>(corner * radius, radius);
    var out: SphereOut;
    out.pos = eye.proj * vec4<f32>(view_pos, 1.0);
    out.uv = corner;
    out.view_center = view_center;
    out.radius = radius;
    return out;
}

struct SphereFrag {
    @builtin(frag_depth) depth: f32,
    @location(0) color: vec4<f32>,
}

@fragment
fn fs_sphere(in: SphereOut) -> SphereFrag {
    let d2 = dot(in.uv, in.uv);
    if d2 > 1.0 {
        discard;
    }
    // Surface point toward the camera (view space looks down -z).
    let z = in.view_center.z + in.radius * sqrt(1.0 - d2);
    let p = vec3<f32>(in.view_center.xy + in.uv * in.radius, z);
    let clip = eye.proj * vec4<f32>(p, 1.0);
    var out: SphereFrag;
    out.depth = clamp(clip.z / clip.w, 0.0, 1.0);
    out.color = vec4<f32>(0.0);
    return out;
}
