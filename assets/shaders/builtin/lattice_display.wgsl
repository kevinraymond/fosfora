// Lattice display pass: once per frame, derive a smoothed density field from the
// final CA state. This decouples the shown volume from raw CA generations — an
// exponential moving average (rate `smooth_rate`, `dt` from the frame) means fast
// rules fade in and out instead of strobing, and reseeds / onset injections
// crossfade instead of popping. Reads the freshest state buffer; writes (read-
// modify-write) the r32float density texture the ray marcher samples.
//
// The uniform block byte-matches lattice_seed.wgsl / lattice_step.wgsl (and the
// Rust `LatticeUniforms`). Builtin shaders get no lib preamble.

struct LatticeUniforms {
    grid_res: u32,
    birth_mask: u32,
    survival_mask: u32,
    num_states: u32,
    neighborhood: u32,
    boundary: u32,
    frame: u32,
    init_mode: u32,
    init_density: f32,
    seed_size: u32,
    seed_hash: u32,
    inject_active: u32,
    perturb_prob: f32,
    smooth_rate: f32,
    color_mode: u32,
    time: f32,
    dt: f32,
    domain_mode: u32,
    domain_radius: f32,
    _pad0: u32,
}

@group(0) @binding(0) var<uniform> u: LatticeUniforms;
@group(0) @binding(1) var<storage, read> state_in: array<u32>;
@group(0) @binding(2) var density: texture_storage_3d<r32float, read_write>;
@group(0) @binding(3) var age_tex: texture_storage_3d<r32float, read_write>;
// Live-cell population for CPU stagnation detection (auto-reseed). One global
// atomicAdd per workgroup via a shared counter, not per cell.
@group(0) @binding(4) var<storage, read_write> population: array<atomic<u32>>;

var<workgroup> wg_alive: atomic<u32>;

@compute @workgroup_size(4, 4, 4)
fn cs_display(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) lidx: u32,
) {
    // Zero the workgroup counter (all threads reach the barriers uniformly — no
    // early return before the barrier, which would be UB).
    if (lidx == 0u) {
        atomicStore(&wg_alive, 0u);
    }
    workgroupBarrier();

    let g = u.grid_res;
    if (all(gid < vec3<u32>(g))) {
        let idx = (gid.z * g + gid.y) * g + gid.x;
        let packed = state_in[idx];
        let s = packed & 0xFFu;              // life-state (low 8 bits)
        let age = f32(packed >> 8u) / 255.0; // normalised age (high bits)

        // Target density: alive = 1, dying cells fade toward 0 by their state
        // index, dead = 0. (The beat pulse lives in the ray marcher, not here.)
        var target_density = 0.0;
        if (s == 1u) {
            target_density = 1.0;
        } else if (s > 1u) {
            target_density = 1.0 - f32(s - 1u) / f32(max(u.num_states - 1u, 1u));
        }

        // EMA toward the target: frame-rate-independent via 1 - exp(-dt * rate).
        let c = vec3i(gid);
        let a = 1.0 - exp(-u.dt * u.smooth_rate);
        let dens = mix(textureLoad(density, c).r, target_density, a);
        textureStore(density, c, vec4f(dens, 0.0, 0.0, 0.0));

        // Age follows density's fade so the tint tracks the visible structure.
        let age_now = mix(textureLoad(age_tex, c).r, age, a);
        textureStore(age_tex, c, vec4f(age_now, 0.0, 0.0, 0.0));

        // Count every density-contributing cell (alive OR dying), not just alive —
        // a multi-state rule can fill the domain with mostly dying cells, which
        // still render, so an alive-only count under-reads the visible saturation.
        if (s != 0u) {
            atomicAdd(&wg_alive, 1u);
        }
    }

    workgroupBarrier();
    if (lidx == 0u) {
        atomicAdd(&population[0], atomicLoad(&wg_alive));
    }
}
