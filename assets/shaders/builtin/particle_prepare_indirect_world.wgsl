// Prepare the indirect draw arguments for the world-space (XR) particle draw.
// Sibling of particle_prepare_indirect.wgsl, which keeps writing the 2D
// renderer's [6, alive, 0, 0]. The world draw is one non-instanced draw whose
// vertex index picks the particle (vertex pulling, three vertices per sprite),
// so it needs [3 * alive, 1, 0, 0]. Written on the GPU so the draw needs no
// CPU readback of the alive count. Dispatched only once the world path exists.

// counters: [0]=alive_count, [1]=dead_count, [2]=emit_used, [3]=aux emit
@group(0) @binding(0) var<storage, read> counters: array<u32, 4>;
// indirect_args: DrawIndirectArgs = [vertex_count, instance_count, first_vertex, first_instance]
@group(0) @binding(1) var<storage, read_write> indirect_args: array<u32, 4>;

@compute @workgroup_size(1)
fn cs_main() {
    indirect_args[0] = 3u * counters[0]; // vertex_count (3 vertices per sprite)
    indirect_args[1] = 1u;               // instance_count (one instance)
    indirect_args[2] = 0u;               // first_vertex
    indirect_args[3] = 0u;               // first_instance
}
