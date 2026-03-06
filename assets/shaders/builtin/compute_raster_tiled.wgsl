// Compute raster pass 4: Tiled accumulation — one workgroup per 16×16 tile.
// Accumulates particle contributions in shared memory (fast workgroup atomics),
// then flushes to global framebuffer with plain stores (no global atomics).
// Dispatch: num_tiles_x × num_tiles_y workgroups, 2D.

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

@group(0) @binding(0)  var<storage, read> pos_life: array<vec4f>;
@group(0) @binding(1)  var<storage, read> vel_size: array<vec4f>;
@group(0) @binding(2)  var<storage, read> color: array<vec4f>;
@group(0) @binding(3)  var<storage, read> tile_offsets: array<u32>;
@group(0) @binding(4)  var<storage, read> tile_counts_buf: array<u32>;
@group(0) @binding(5)  var<uniform> u: TileUniforms;
@group(0) @binding(6)  var<storage, read> sorted_particles: array<u32>;
@group(0) @binding(7)  var<storage, read_write> fb_r: array<i32>;
@group(0) @binding(8)  var<storage, read_write> fb_g: array<i32>;
@group(0) @binding(9)  var<storage, read_write> fb_b: array<i32>;
@group(0) @binding(10) var<storage, read_write> fb_a: array<i32>;

const TILE_SIZE: u32 = 16u;
const TILE_PIXELS: u32 = 256u; // 16 * 16
const PRECISION: f32 = 4096.0;

// Shared memory: 4 channels × 256 pixels = 4 KB
var<workgroup> sm_r: array<atomic<i32>, 256>;
var<workgroup> sm_g: array<atomic<i32>, 256>;
var<workgroup> sm_b: array<atomic<i32>, 256>;
var<workgroup> sm_a: array<atomic<i32>, 256>;

fn write_pixel_shared(lx: u32, ly: u32, col: vec3f, weight: f32) {
    let local_idx = ly * TILE_SIZE + lx;
    let s = PRECISION * weight;
    atomicAdd(&sm_r[local_idx], i32(col.r * s));
    atomicAdd(&sm_g[local_idx], i32(col.g * s));
    atomicAdd(&sm_b[local_idx], i32(col.b * s));
    atomicAdd(&sm_a[local_idx], i32(s));
}

@compute @workgroup_size(256)
fn cs_main(
    @builtin(workgroup_id) wg_id: vec3u,
    @builtin(local_invocation_index) lid: u32,
) {
    let tile_x = wg_id.x;
    let tile_y = wg_id.y;

    if tile_x >= u.num_tiles_x || tile_y >= u.num_tiles_y {
        return;
    }

    let tile_id = tile_y * u.num_tiles_x + tile_x;

    // Zero shared memory
    atomicStore(&sm_r[lid], 0);
    atomicStore(&sm_g[lid], 0);
    atomicStore(&sm_b[lid], 0);
    atomicStore(&sm_a[lid], 0);
    workgroupBarrier();

    // Tile pixel origin in global coords
    let tile_px_x = tile_x * TILE_SIZE;
    let tile_px_y = tile_y * TILE_SIZE;

    let w = f32(u.width);
    let h = f32(u.height);

    // Cooperatively iterate over this tile's particle list
    let offset = tile_offsets[tile_id];
    let count = tile_counts_buf[tile_id];

    for (var i = lid; i < count; i += TILE_PIXELS) {
        let particle_idx = sorted_particles[offset + i];
        let pl = pos_life[particle_idx];
        let vs = vel_size[particle_idx];
        let col = color[particle_idx];

        if pl.w <= 0.0 {
            continue;
        }

        // NDC to pixel
        let px = (pl.x * 0.5 + 0.5) * w;
        let py = (1.0 - (pl.y * 0.5 + 0.5)) * h;

        let radius_px = vs.w * h * 0.5;

        if radius_px <= 1.0 {
            // Single-pixel path
            let ix = i32(floor(px));
            let iy = i32(floor(py));
            let lx = ix - i32(tile_px_x);
            let ly = iy - i32(tile_px_y);
            if lx >= 0 && lx < i32(TILE_SIZE) && ly >= 0 && ly < i32(TILE_SIZE) {
                write_pixel_shared(u32(lx), u32(ly), col.rgb, col.a);
            }
        } else if radius_px <= 1.5 {
            // Bilinear 2×2 splat
            let fx = fract(px - 0.5);
            let fy = fract(py - 0.5);
            let ix = i32(floor(px - 0.5));
            let iy = i32(floor(py - 0.5));

            let w00 = (1.0 - fx) * (1.0 - fy);
            let w10 = fx * (1.0 - fy);
            let w01 = (1.0 - fx) * fy;
            let w11 = fx * fy;

            // Write each of the 4 pixels if within this tile
            let lx0 = ix - i32(tile_px_x);
            let ly0 = iy - i32(tile_px_y);
            let lx1 = lx0 + 1;
            let ly1 = ly0 + 1;

            if lx0 >= 0 && lx0 < i32(TILE_SIZE) && ly0 >= 0 && ly0 < i32(TILE_SIZE) {
                write_pixel_shared(u32(lx0), u32(ly0), col.rgb, col.a * w00);
            }
            if lx1 >= 0 && lx1 < i32(TILE_SIZE) && ly0 >= 0 && ly0 < i32(TILE_SIZE) {
                write_pixel_shared(u32(lx1), u32(ly0), col.rgb, col.a * w10);
            }
            if lx0 >= 0 && lx0 < i32(TILE_SIZE) && ly1 >= 0 && ly1 < i32(TILE_SIZE) {
                write_pixel_shared(u32(lx0), u32(ly1), col.rgb, col.a * w01);
            }
            if lx1 >= 0 && lx1 < i32(TILE_SIZE) && ly1 >= 0 && ly1 < i32(TILE_SIZE) {
                write_pixel_shared(u32(lx1), u32(ly1), col.rgb, col.a * w11);
            }
        } else {
            // Gaussian area splat: soft circle matching billboard renderer
            let r = min(radius_px, 8.0);
            let r_ceil = i32(ceil(r));
            let cx = i32(floor(px));
            let cy = i32(floor(py));
            let inv_r2 = 1.0 / (r * r);

            for (var dy = -r_ceil; dy <= r_ceil; dy++) {
                let gy = cy + dy;
                let ly = gy - i32(tile_px_y);
                if ly < 0 || ly >= i32(TILE_SIZE) {
                    continue;
                }
                for (var dx = -r_ceil; dx <= r_ceil; dx++) {
                    let gx = cx + dx;
                    let lx = gx - i32(tile_px_x);
                    if lx < 0 || lx >= i32(TILE_SIZE) {
                        continue;
                    }
                    let dist_sq = f32(dx * dx + dy * dy) * inv_r2;
                    if dist_sq > 1.5 {
                        continue;
                    }
                    let glow = exp(-dist_sq * 2.0);
                    write_pixel_shared(u32(lx), u32(ly), col.rgb, col.a * glow * glow);
                }
            }
        }
    }

    workgroupBarrier();

    // Flush shared memory to global framebuffer (plain stores — tiles own exclusive pixels)
    let gx = tile_px_x + (lid % TILE_SIZE);
    let gy = tile_px_y + (lid / TILE_SIZE);

    if gx < u.width && gy < u.height {
        let global_idx = gy * u.width + gx;
        let local_idx = lid;
        // Plain stores — no atomicAdd needed since each tile owns its pixel region
        fb_r[global_idx] = atomicLoad(&sm_r[local_idx]);
        fb_g[global_idx] = atomicLoad(&sm_g[local_idx]);
        fb_b[global_idx] = atomicLoad(&sm_b[local_idx]);
        fb_a[global_idx] = atomicLoad(&sm_a[local_idx]);
    }
}
