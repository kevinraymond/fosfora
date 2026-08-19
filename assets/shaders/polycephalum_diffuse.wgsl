// Polycephalum — trail-field diffuse + decay compute pass.
//
// The behavioral trail is CHANNELS scalar maps (one per species) packed into a flat
// storage buffer: index = (y * grid_w + x) * CHANNELS + channel. Agents accumulate deposits
// into a parallel atomic-i32 buffer (they cannot atomic-add into a storage texture); this pass
// folds those deposits into the trail, applies a 4-neighbour box blur (diffusion) and a decay,
// then clears the deposit buffer for the next frame. Toroidal (wrapping) boundaries.
//
// Runs once per frame BEFORE the particle sim (like the reaction-diffusion pass), ping-ponging
// trail_src -> trail_dst. Standalone pipeline — no particle_lib, no ParticleUniforms.

const CHANNELS: u32 = 12u;

struct TrailUniforms {
    grid_w: u32,
    grid_h: u32,
    channels: u32,
    deposit_scale: f32,
    decay: f32,
    diffuse: f32,
    time: f32,
    delta_time: f32,
}

@group(0) @binding(0) var<uniform> tu: TrailUniforms;
@group(0) @binding(1) var<storage, read> trail_src: array<f32>;
@group(0) @binding(2) var<storage, read_write> trail_dst: array<f32>;
@group(0) @binding(3) var<storage, read_write> deposit: array<atomic<i32>>;

// Flat index into a per-channel trail/deposit buffer with toroidal wrapping.
fn tf_index(x: i32, y: i32, c: u32, w: i32, h: i32) -> u32 {
    let xx = ((x % w) + w) % w;
    let yy = ((y % h) + h) % h;
    return (u32(yy) * u32(w) + u32(xx)) * CHANNELS + c;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    let w = i32(tu.grid_w);
    let h = i32(tu.grid_h);
    if i32(gid.x) >= w || i32(gid.y) >= h { return; }

    let x = i32(gid.x);
    let y = i32(gid.y);

    for (var c = 0u; c < CHANNELS; c++) {
        let center = trail_src[tf_index(x, y, c, w, h)];
        let l = trail_src[tf_index(x - 1, y, c, w, h)];
        let r = trail_src[tf_index(x + 1, y, c, w, h)];
        let up = trail_src[tf_index(x, y - 1, c, w, h)];
        let dn = trail_src[tf_index(x, y + 1, c, w, h)];

        // Diffusion: blend center toward the 4-neighbour mean.
        let mean4 = (l + r + up + dn) * 0.25;
        let blurred = mix(center, mean4, clamp(tu.diffuse, 0.0, 1.0));

        // Fold in this texel's accumulated deposits, then clear for next frame.
        let di = tf_index(x, y, c, w, h);
        let dep = f32(atomicLoad(&deposit[di])) / max(tu.deposit_scale, 1.0);
        atomicStore(&deposit[di], 0);

        // Decay + deposit; clamp to keep the field bounded.
        //
        // tu.decay is authored per 1/60 s frame, so it is re-exponentiated for the real
        // frame time (#1986). The helper is duplicated from lib/chronoflow.wgsl rather
        // than called: this is a standalone pipeline with no lib prepended (see header),
        // the same reason etch_sim/etch_bg duplicate their clear-cycle helper. Keep the
        // two in sync. delta_time == 0 means "behave exactly as authored", so an older
        // binary still writing the old padding slot degrades to today's behavior instead
        // of leaving the field undecayed.
        var keep = tu.decay;
        if (tu.delta_time > 0.0) {
            let n = clamp(tu.delta_time * 60.0, 1e-4, 2.0);
            if (n != 1.0) {
                keep = pow(clamp(tu.decay, 0.0, 1.0), n);
            }
        }
        let v = min(blurred * keep + dep, 16.0);
        trail_dst[di] = v;
    }
}
