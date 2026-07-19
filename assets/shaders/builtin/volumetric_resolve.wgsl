// Volumetric resolve: read the atomic u32 voxel grid, normalise to float, apply
// a light 3x3x3 box blur (turns discrete particle deposits into continuous fog),
// and write into the samplable r32float 3D density texture that the ray marcher
// reads. Lattice (a later effect) will write this same texture directly.
//
// Dispatch: ceil(grid_res / 4) in x/y/z, @workgroup_size(4,4,4).

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
}

@group(0) @binding(0) var<storage, read> voxel: array<u32>;
@group(0) @binding(1) var<uniform> u: VolUniforms;
@group(0) @binding(2) var density_out: texture_storage_3d<r32float, write>;

fn load_vox(x: i32, y: i32, z: i32, g: i32) -> f32 {
    if x < 0 || y < 0 || z < 0 || x >= g || y >= g || z >= g {
        return 0.0;
    }
    let idx = (u32(z) * u32(g) + u32(y)) * u32(g) + u32(x);
    return f32(voxel[idx]);
}

@compute @workgroup_size(4, 4, 4)
fn cs_resolve(@builtin(global_invocation_id) gid: vec3u) {
    let g = u.grid_res;
    if gid.x >= g || gid.y >= g || gid.z >= g {
        return;
    }

    let gi = i32(g);
    let cx = i32(gid.x);
    let cy = i32(gid.y);
    let cz = i32(gid.z);

    // 3x3x3 box blur over the raw fixed-point deposits.
    var sum = 0.0;
    for (var dz = -1; dz <= 1; dz++) {
        for (var dy = -1; dy <= 1; dy++) {
            for (var dx = -1; dx <= 1; dx++) {
                sum += load_vox(cx + dx, cy + dy, cz + dz, gi);
            }
        }
    }
    // Mean particles per voxel: /27 for the blur kernel, /density_scale to undo
    // fixed-point. This is unbounded (scales with particle count), so squash it
    // through a saturating curve into a bounded, count-robust density in [0,1) that
    // the ray marcher's absorption/hue math expects. `density_gain` controls how fast
    // occupancy saturates (lower = more tonal range before white-out).
    let occupancy = sum / (27.0 * max(u.density_scale, 1.0));
    let density = 1.0 - exp(-max(u.density_gain, 0.0) * occupancy);

    textureStore(density_out, vec3i(gid.xyz), vec4f(density, 0.0, 0.0, 0.0));
}
