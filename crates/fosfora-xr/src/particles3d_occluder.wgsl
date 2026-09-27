// S7 depth-only occluders: every obstacle box (scene anchors, floor) and
// hand-joint sphere is drawn as a cube into the depth buffer with color
// writes off, before the sprites. Passthrough carries no depth, so without
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
    var center = vec3<f32>(0.0);
    var q = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    var half = vec3<f32>(0.0);
    if ii < obstacles.box_count {
        center = obstacles.box_center[ii].xyz;
        q = obstacles.box_rot[ii];
        half = obstacles.box_half[ii].xyz;
    } else {
        let s = obstacles.spheres[ii - obstacles.box_count];
        center = s.xyz;
        half = vec3<f32>(max(s.w - obstacles.sphere_shrink, 0.01));
    }
    let world = center + quat_rotate(q, cube_corner(vi % 36u) * half);
    return eye.view_proj * vec4<f32>(world, 1.0);
}

@fragment
fn fs_occluder() -> @location(0) vec4<f32> {
    // Color writes are masked off; only depth lands.
    return vec4<f32>(0.0);
}
