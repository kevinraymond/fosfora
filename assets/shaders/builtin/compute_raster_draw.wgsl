// Compute raster draw: each alive particle writes color to atomic framebuffer.
// Three paths: single-pixel (≤1px), bilinear 2×2 (1–1.5px), Gaussian area splat (>1.5px).
// Dispatch: ceil(max_particles / 256) workgroups, 1D.

struct DrawUniforms {
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
}

// Particle SoA (read-only)
@group(0) @binding(0) var<storage, read> pos_life: array<vec4f>;
@group(0) @binding(1) var<storage, read> vel_size: array<vec4f>;
@group(0) @binding(2) var<storage, read> color: array<vec4f>;
// Alive indices + counter
@group(0) @binding(3) var<storage, read> alive_indices: array<u32>;
@group(0) @binding(4) var<storage, read> counters: array<atomic<u32>>;
// Render uniforms
@group(0) @binding(5) var<uniform> u: DrawUniforms;
// Atomic framebuffer (read_write)
@group(0) @binding(6) var<storage, read_write> fb_r: array<atomic<i32>>;
@group(0) @binding(7) var<storage, read_write> fb_g: array<atomic<i32>>;
@group(0) @binding(8) var<storage, read_write> fb_b: array<atomic<i32>>;
@group(0) @binding(9) var<storage, read_write> fb_a: array<atomic<i32>>;

const PRECISION: f32 = 4096.0;

fn write_pixel(ix: i32, iy: i32, col: vec3f, weight: f32) {
    if ix < 0 || ix >= i32(u.width) || iy < 0 || iy >= i32(u.height) {
        return;
    }
    let idx = u32(iy) * u.width + u32(ix);
    let s = PRECISION * weight;
    atomicAdd(&fb_r[idx], i32(col.r * s));
    atomicAdd(&fb_g[idx], i32(col.g * s));
    atomicAdd(&fb_b[idx], i32(col.b * s));
    atomicAdd(&fb_a[idx], i32(s));
}

@compute @workgroup_size(256)
fn cs_draw(@builtin(global_invocation_id) gid: vec3u) {
    let thread_idx = gid.x;
    let alive_count = atomicLoad(&counters[0]);
    if thread_idx >= alive_count {
        return;
    }

    let particle_idx = alive_indices[thread_idx];
    let pl = pos_life[particle_idx];
    let vs = vel_size[particle_idx];
    let col = color[particle_idx];

    // Dead particle check
    if pl.w <= 0.0 {
        return;
    }

    let w = f32(u.width);
    let h = f32(u.height);

    // NDC (-1..1) to pixel coordinates
    let px = (pl.x * 0.5 + 0.5) * w;
    let py = (1.0 - (pl.y * 0.5 + 0.5)) * h; // flip Y

    // Particle radius in pixels (size is in NDC units, height-relative)
    let radius_px = vs.w * h * 0.5;

    if radius_px <= 1.0 {
        // Single-pixel fast path (4 atomicAdds)
        let ix = i32(floor(px));
        let iy = i32(floor(py));
        write_pixel(ix, iy, col.rgb, col.a);
    } else if radius_px <= 1.5 {
        // Bilinear 2×2 splat: distribute energy to 4 nearest pixels
        // based on sub-pixel position (16 atomicAdds)
        let fx = fract(px - 0.5);
        let fy = fract(py - 0.5);
        let ix = i32(floor(px - 0.5));
        let iy = i32(floor(py - 0.5));

        let w00 = (1.0 - fx) * (1.0 - fy);
        let w10 = fx * (1.0 - fy);
        let w01 = (1.0 - fx) * fy;
        let w11 = fx * fy;

        write_pixel(ix,     iy,     col.rgb, col.a * w00);
        write_pixel(ix + 1, iy,     col.rgb, col.a * w10);
        write_pixel(ix,     iy + 1, col.rgb, col.a * w01);
        write_pixel(ix + 1, iy + 1, col.rgb, col.a * w11);
    } else {
        // Gaussian area splat: soft circle matching billboard renderer output
        let r = min(radius_px, 8.0);
        let r_ceil = i32(ceil(r));
        let cx = i32(floor(px));
        let cy = i32(floor(py));
        let inv_r2 = 1.0 / (r * r);

        for (var dy = -r_ceil; dy <= r_ceil; dy++) {
            for (var dx = -r_ceil; dx <= r_ceil; dx++) {
                let dist_sq = f32(dx * dx + dy * dy) * inv_r2;
                if dist_sq > 1.5 {
                    continue;
                }
                let glow = exp(-dist_sq * 2.0);
                write_pixel(cx + dx, cy + dy, col.rgb, col.a * glow * glow);
            }
        }
    }
}
