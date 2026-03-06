// Spatial hash pass 1: Count particles per grid cell.
// Each alive particle hashes its position to a grid cell and atomically increments the count.

struct Uniforms {
    delta_time: f32,
    time: f32,
    max_particles: u32,
    emit_count: u32,
    // ... rest of ParticleUniforms (we only need max_particles)
}

const GRID_W: u32 = 40u;
const GRID_H: u32 = 40u;

@group(0) @binding(0) var<storage, read> pos_life: array<vec4f>;
@group(0) @binding(1) var<storage, read_write> cell_counts: array<atomic<u32>>;
@group(0) @binding(2) var<uniform> u: Uniforms;

fn pos_to_cell(pos: vec2f) -> u32 {
    // Map clip space [-1,1] to grid [0, GRID_W-1] x [0, GRID_H-1]
    let gx = clamp(u32((pos.x * 0.5 + 0.5) * f32(GRID_W)), 0u, GRID_W - 1u);
    let gy = clamp(u32((pos.y * 0.5 + 0.5) * f32(GRID_H)), 0u, GRID_H - 1u);
    return gy * GRID_W + gx;
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

    let cell = pos_to_cell(pl.xy);
    atomicAdd(&cell_counts[cell], 1u);
}
