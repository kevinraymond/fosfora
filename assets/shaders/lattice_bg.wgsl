// Lattice — background pass. A flat dark backdrop for the CA density volume,
// which the R3 ray marcher composites over (premultiplied, LoadOp::Load). Every
// particle effect needs at least one pass; this is Lattice's.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    return vec4f(0.015, 0.015, 0.03, 1.0);
}
