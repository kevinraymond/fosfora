// Compute raster pass 3: Scatter — write particle indices into sorted_particles
// using exclusive prefix-sum offsets. Each particle atomicAdds its tile's scatter offset
// and writes its index at that position.
// Dispatch: ceil(max_particles / 256) workgroups, 1D.

struct TileUniforms {
    width: u32,
    height: u32,
    num_tiles_x: u32,
    num_tiles_y: u32,
    max_particles: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(0) var<storage, read> pos_life: array<vec4f>;
@group(0) @binding(1) var<storage, read> vel_size: array<vec4f>;
@group(0) @binding(2) var<storage, read> alive_indices: array<u32>;
@group(0) @binding(3) var<storage, read> counters: array<atomic<u32>>;
@group(0) @binding(4) var<uniform> u: TileUniforms;
@group(0) @binding(5) var<storage, read_write> tile_scatter_offsets: array<atomic<u32>>;
@group(0) @binding(6) var<storage, read_write> sorted_particles: array<u32>;

const TILE_SIZE: u32 = 16u;

fn scatter_to_tile(tx: i32, ty: i32, particle_idx: u32) {
    if tx < 0 || tx >= i32(u.num_tiles_x) || ty < 0 || ty >= i32(u.num_tiles_y) {
        return;
    }
    let tile_id = u32(ty) * u.num_tiles_x + u32(tx);
    let slot = atomicAdd(&tile_scatter_offsets[tile_id], 1u);
    sorted_particles[slot] = particle_idx;
}

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let thread_idx = gid.x;
    let alive_count = atomicLoad(&counters[0]);
    if thread_idx >= alive_count {
        return;
    }

    let particle_idx = alive_indices[thread_idx];
    let pl = pos_life[particle_idx];
    let vs = vel_size[particle_idx];

    if pl.w <= 0.0 {
        return;
    }

    let w = f32(u.width);
    let h = f32(u.height);

    let px = (pl.x * 0.5 + 0.5) * w;
    let py = (1.0 - (pl.y * 0.5 + 0.5)) * h;

    let radius_px = vs.w * h * 0.5;

    if radius_px <= 1.0 {
        let tx = i32(floor(px)) / i32(TILE_SIZE);
        let ty = i32(floor(py)) / i32(TILE_SIZE);
        scatter_to_tile(tx, ty, particle_idx);
    } else if radius_px <= 1.5 {
        let ix = i32(floor(px - 0.5));
        let iy = i32(floor(py - 0.5));
        let tx0 = ix / i32(TILE_SIZE);
        let ty0 = iy / i32(TILE_SIZE);
        let tx1 = (ix + 1) / i32(TILE_SIZE);
        let ty1 = (iy + 1) / i32(TILE_SIZE);

        scatter_to_tile(tx0, ty0, particle_idx);
        if tx1 != tx0 {
            scatter_to_tile(tx1, ty0, particle_idx);
        }
        if ty1 != ty0 {
            scatter_to_tile(tx0, ty1, particle_idx);
            if tx1 != tx0 {
                scatter_to_tile(tx1, ty1, particle_idx);
            }
        }
    } else {
        // Gaussian area splat: scatter to all tiles the bounding box overlaps
        let r = min(radius_px, 8.0);
        let r_ceil = i32(ceil(r));
        let cx = i32(floor(px));
        let cy = i32(floor(py));
        let tx_min = (cx - r_ceil) / i32(TILE_SIZE);
        let tx_max = (cx + r_ceil) / i32(TILE_SIZE);
        let ty_min = (cy - r_ceil) / i32(TILE_SIZE);
        let ty_max = (cy + r_ceil) / i32(TILE_SIZE);

        for (var ty = ty_min; ty <= ty_max; ty++) {
            for (var tx = tx_min; tx <= tx_max; tx++) {
                scatter_to_tile(tx, ty, particle_idx);
            }
        }
    }
}
