// Generate sort keys from alive particle data for bitonic depth sort.
// Writes key = -particle_size (negate for back-to-front: larger particles first).
// Dead slots get key = 1e30 (pushed to end after sort).

@group(0) @binding(0) var<storage, read> counters: array<u32, 4>;
@group(0) @binding(1) var<storage, read> vel_size: array<vec4f>;
@group(0) @binding(2) var<storage, read> alive_indices: array<u32>;
@group(0) @binding(3) var<storage, read_write> sort_keys: array<f32>;

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let i = gid.x;
    let total = arrayLength(&sort_keys);
    if i >= total {
        return;
    }

    let alive_count = counters[0];
    if i < alive_count {
        let particle_idx = alive_indices[i];
        // Negate size for ascending sort = largest first (back-to-front rendering)
        sort_keys[i] = -vel_size[particle_idx].w;
    } else {
        sort_keys[i] = 1e30;
    }
}
