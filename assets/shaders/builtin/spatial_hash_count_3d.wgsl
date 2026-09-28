// Spatial hash pass 1, 3D mode: count particles per cell of a GRID_D^3 grid.
// Used by effects that set "interaction_3d": true; the 2D pair is untouched.
// Cells cover the cube within +-emitter_radius meters of the anchor on all three
// axes (the world layout's volume, see particle_lib.wgsl). Same bindings as
// spatial_hash_count.wgsl; particle_lib's sh_pos_to_cell_3d mirrors pos_to_cell.

struct Uniforms {
    delta_time: f32,
    time: f32,
    max_particles: u32,
    emit_count: u32,
    emitter_pos: vec2f,
    emitter_radius: f32, // byte 24 of ParticleUniforms, pinned in spatial_hash.rs
    emitter_shape: u32,
    // ... rest of ParticleUniforms (we only need max_particles and emitter_radius)
}

const GRID_D: u32 = 8u;

@group(0) @binding(0) var<storage, read> pos_life: array<vec4f>;
@group(0) @binding(1) var<storage, read_write> cell_counts: array<atomic<u32>>;
@group(0) @binding(2) var<uniform> u: Uniforms;

fn pos_to_cell(pos: vec3f) -> u32 {
    // Map [-extent, extent]^3 to [0, GRID_D-1]^3, clamping what lies outside.
    let extent = max(u.emitter_radius, 1e-3);
    let g = clamp(
        vec3u((pos / extent * 0.5 + 0.5) * f32(GRID_D)),
        vec3u(0u),
        vec3u(GRID_D - 1u),
    );
    return (g.z * GRID_D + g.y) * GRID_D + g.x;
}

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles {
        return;
    }

    let pl = pos_life[idx];
    if pl.w <= 0.0 {
        return; // Dead particle
    }

    let cell = pos_to_cell(pl.xyz);
    atomicAdd(&cell_counts[cell], 1u);
}
