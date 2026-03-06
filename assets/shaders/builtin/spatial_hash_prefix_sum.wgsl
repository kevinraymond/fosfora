// Spatial hash pass 2: Exclusive prefix sum over cell_counts → cell_offsets.
// Parallel block scan: 256 threads each scan a chunk sequentially,
// then combine partial sums via shared-memory Blelloch scan.
// Handles up to 256*256 = 65,536 cells in a single dispatch.

const NUM_CELLS: u32 = 1600u; // patched at pipeline creation
const WG_SIZE: u32 = 256u;

@group(0) @binding(0) var<storage, read> cell_counts: array<u32>;
@group(0) @binding(1) var<storage, read_write> cell_offsets: array<u32>;

var<workgroup> partial_sums: array<u32, 256>;
var<workgroup> block_offsets: array<u32, 256>;

@compute @workgroup_size(256)
fn cs_main(@builtin(local_invocation_id) lid: vec3u) {
    let tid = lid.x;
    let chunk_size = (NUM_CELLS + WG_SIZE - 1u) / WG_SIZE;
    let start = tid * chunk_size;
    let end = min(start + chunk_size, NUM_CELLS);

    // Phase 1: Each thread scans its chunk sequentially, writing local prefix sums
    var local_sum = 0u;
    for (var i = start; i < end; i++) {
        cell_offsets[i] = local_sum;
        local_sum += cell_counts[i];
    }
    partial_sums[tid] = local_sum;
    workgroupBarrier();

    // Phase 2: Blelloch exclusive scan on partial_sums (256 elements in shared memory)
    // Up-sweep (reduce)
    var offset = 1u;
    var n = WG_SIZE;
    for (var d = n >> 1u; d > 0u; d >>= 1u) {
        if tid < d {
            let ai = offset * (2u * tid + 1u) - 1u;
            let bi = offset * (2u * tid + 2u) - 1u;
            partial_sums[bi] += partial_sums[ai];
        }
        offset <<= 1u;
        workgroupBarrier();
    }

    // Set root to zero
    if tid == 0u {
        partial_sums[n - 1u] = 0u;
    }
    workgroupBarrier();

    // Down-sweep
    for (var d = 1u; d < n; d <<= 1u) {
        offset >>= 1u;
        if tid < d {
            let ai = offset * (2u * tid + 1u) - 1u;
            let bi = offset * (2u * tid + 2u) - 1u;
            let t = partial_sums[ai];
            partial_sums[ai] = partial_sums[bi];
            partial_sums[bi] += t;
        }
        workgroupBarrier();
    }

    // Phase 3: Each thread adds its block offset to its chunk
    let block_offset = partial_sums[tid];
    for (var i = start; i < end; i++) {
        cell_offsets[i] += block_offset;
    }
}
