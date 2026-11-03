// Fosfora integer hash library — PCG (O'Neill's RXS-M-XS output on an LCG step),
// for builtin compute passes that need reproducible per-cell randomness: the same
// input gives the same bits on every GPU, unlike a sin()-based hash. Prepended by
// the passes that use it (lattice_shader! in gpu/lattice.rs); not part of the
// effect-shader library.

fn fosfora_pcg(v_in: u32) -> u32 {
    let state = v_in * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// 0..1 inclusive.
fn fosfora_hash01(v: u32) -> f32 {
    return f32(fosfora_pcg(v)) / 4294967295.0;
}

// A 3D cell's hash, salted: chained so each axis decorrelates the next.
fn fosfora_cell_hash(p: vec3<u32>, salt: u32) -> u32 {
    return fosfora_pcg(p.x + fosfora_pcg(p.y + fosfora_pcg(p.z + salt)));
}
