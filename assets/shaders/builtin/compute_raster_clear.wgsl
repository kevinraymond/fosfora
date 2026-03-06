// Compute raster clear: zero all 4 atomic framebuffer channels.
// Dispatch: ceil(W*H / 256) workgroups, 1D.

struct ClearUniforms {
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<storage, read_write> fb_r: array<atomic<i32>>;
@group(0) @binding(1) var<storage, read_write> fb_g: array<atomic<i32>>;
@group(0) @binding(2) var<storage, read_write> fb_b: array<atomic<i32>>;
@group(0) @binding(3) var<storage, read_write> fb_a: array<atomic<i32>>;
@group(0) @binding(4) var<uniform> u: ClearUniforms;

@compute @workgroup_size(256)
fn cs_clear(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    let total = u.width * u.height;
    if idx >= total {
        return;
    }
    atomicStore(&fb_r[idx], 0);
    atomicStore(&fb_g[idx], 0);
    atomicStore(&fb_b[idx], 0);
    atomicStore(&fb_a[idx], 0);
}
