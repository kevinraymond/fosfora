// Bitonic merge-sort for depth-sorted particle rendering.
// Each dispatch performs one comparison-and-swap step.
// Uniforms provide (block_size, sub_block_size, count) per step.

struct SortUniforms {
    block_size: u32,
    sub_block_size: u32,
    count: u32,
    _pad: u32,
}

struct Particle {
    pos_life: vec4f,
    vel_size: vec4f,
    color: vec4f,
    flags: vec4f,
}

@group(0) @binding(0) var<uniform> sort: SortUniforms;
@group(0) @binding(1) var<storage, read_write> keys: array<f32>;
@group(0) @binding(2) var<storage, read_write> indices: array<u32>;

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let i = gid.x;
    if i >= sort.count {
        return;
    }

    // Bitonic sort: determine partner index
    let block = sort.block_size;
    let sub = sort.sub_block_size;

    // Which sub-block are we in?
    let group_id = i / sub;
    let local_id = i % sub;

    // Partner is mirrored within the sub-block
    let partner = group_id * sub * 2u + sub - 1u - local_id;

    // Only the "left" side performs the swap
    if i >= partner || partner >= sort.count {
        return;
    }

    // Sort direction: ascending within even blocks, descending within odd blocks
    let block_id = i / block;
    let ascending = (block_id & 1u) == 0u;

    let key_i = keys[i];
    let key_p = keys[partner];

    let should_swap = select(key_i < key_p, key_i > key_p, ascending);

    if should_swap {
        keys[i] = key_p;
        keys[partner] = key_i;
        let idx_i = indices[i];
        let idx_p = indices[partner];
        indices[i] = idx_p;
        indices[partner] = idx_i;
    }
}
