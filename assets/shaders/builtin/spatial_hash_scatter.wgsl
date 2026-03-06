// Spatial hash pass 3: Scatter particles into sorted order.
// Each alive particle writes its index to sorted_indices[cell_offsets[cell] + local_offset].
// cell_offsets is used as an atomic counter (incremented per particle in each cell).

struct Uniforms {
    delta_time: f32,
    time: f32,
    max_particles: u32,
    emit_count: u32,
}

const GRID_W: u32 = 40u;
const GRID_H: u32 = 40u;

@group(0) @binding(0) var<storage, read> pos_life: array<vec4f>;
@group(0) @binding(1) var<storage, read_write> cell_offsets: array<atomic<u32>>;
@group(0) @binding(2) var<storage, read_write> sorted_indices: array<u32>;
@group(0) @binding(3) var<uniform> u: Uniforms;

fn pos_to_cell(pos: vec2f) -> u32 {
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
    let slot = atomicAdd(&cell_offsets[cell], 1u);
    sorted_indices[slot] = idx;
}
