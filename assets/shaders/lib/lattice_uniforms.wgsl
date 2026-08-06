// Shared uniform block for the Lattice CA passes. Prepended as a preamble to
// lattice_seed.wgsl / lattice_step.wgsl / lattice_display.wgsl at pipeline
// creation — see `lattice_shader()` in gpu/lattice.rs.
//
// Must byte-match `LatticeUniforms` in gpu/lattice.rs; the size assertion there
// is what catches drift.
//
// NOTE: the pcg / hash01 / cell_hash / outside_domain helpers are still
// duplicated between lattice_seed.wgsl and lattice_step.wgsl. They are not
// hoisted here because lattice_display.wgsl uses none of them and would inherit
// four dead functions.

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
    max_age: u32,
}

// ---- Deprecated aliases (pre-rename API, kept so user custom effects keep
// compiling). Do not use in new code; may be removed in a future major release. ----

