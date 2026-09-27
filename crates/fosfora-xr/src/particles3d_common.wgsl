// S5 world-space particles, shared declarations ("World-space particles
// (S5)" in docs/xr/XR_DESIGN.md). particles3d.rs prepends this to
// particles3d_sim.wgsl and particles3d_draw.wgsl; the draw module gets the
// particle buffer declared `read` instead of `read_write`.
//
// The sim keeps every slot alive: a particle that dies or leaves the cube
// respawns in place, so the draw's instance count is the particle count and
// no alive list is needed. Position is world xyz in the reference space
// (meters, -Z forward), never screen space.

struct Sim {
    cube_center: vec3<f32>,
    cube_half: f32,
    time: f32,
    dt: f32,
    count: u32,
    // Audio-driven: flow speed multiplier and sprite size multiplier.
    speed: f32,
    size: f32,
    // Base sprite radius in meters, and the flow field's spatial scale (1/m).
    base_size: f32,
    flow_scale: f32,
    lifetime: f32,
    // Vertices per sprite (6 quad, 3 triangle) and whether the draw is one
    // non-instanced call (the vertex index alone picks particle and corner).
    verts_per_sprite: u32,
    pull: u32,
    // Downward acceleration (m/s^2); 0 for the pure flow sim (S5), small in
    // mixed reality (S7) so particles settle on real surfaces.
    gravity: f32,
    _pad1: u32,
}

struct Particle {
    pos: vec3<f32>,
    life: f32,
    vel: vec3<f32>,
    seed: f32,
}

@group(0) @binding(0) var<uniform> sim: Sim;
@group(0) @binding(1) var<storage, read_write> particles: array<Particle>;
