// Shared uniform block for the volumetric passes. Prepended as a preamble to
// volumetric_scatter.wgsl / volumetric_resolve.wgsl / volumetric_raymarch.wgsl
// (and any further density producer, e.g. helix_sweep.wgsl) at pipeline creation
// — see `vol_shader()` in gpu/volumetric.rs.
//
// Must byte-match `VolumetricUniforms` in gpu/volumetric.rs; the size assertion
// there is what catches drift. This file exists so that adding a field is one
// edit here plus one in Rust, instead of a byte-for-byte paste into every
// consumer.
//
// All scalars are 4-byte aligned; the trailing pads keep the total a multiple of
// 16 for the uniform address space.

struct VolUniforms {
    grid_res: u32,
    march_steps: u32,
    res_x: f32,
    res_y: f32,
    time: f32,
    absorption: f32,
    detail_scale: f32,
    detail_strength: f32,
    density_threshold: f32,
    volume_depth: f32,
    density_scale: f32,
    cam_yaw: f32,
    cam_pitch: f32,
    cam_distance: f32,
    cam_orbit_speed: f32,
    fov: f32,
    palette_hue: f32,
    emission_gain: f32,
    beat: f32,
    kick: f32,
    rms: f32,
    beat_phase: f32,
    dominant_chroma: f32,
    density_gain: f32,
    env_shape: u32,
    jitter_amp: f32,
    age_influence: f32,
    // Camera model: 0 = orbit at the origin (R3 / Lattice), 1 = flythrough with
    // the camera INSIDE the volume (Helix). In flythrough, cam_yaw/cam_pitch
    // become a look direction relative to -Z rather than an orbit position, and
    // cam_distance becomes the camera's Z coordinate.
    cam_mode: u32,
    cam_roll: f32, // bank around the view axis (flythrough only)
    cam_x: f32,    // camera lateral position (flythrough only)
    cam_y: f32,
    _pad0: f32,
}

// ---- Deprecated aliases (pre-rename API, kept so user custom effects keep
// compiling). Do not use in new code; may be removed in a future major release. ----

