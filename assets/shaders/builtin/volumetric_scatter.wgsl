// Volumetric scatter: each alive particle deposits fixed-point density into a
// 3D voxel grid (atomic u32 storage buffer). Particles are 2D (pos_life.xy in
// NDC); a stable per-particle Z is synthesized from a PCG hash of the particle
// index so the flat cloud gains temporally-coherent depth.
//
// Dispatch: ceil(max_particles / 256) workgroups, 1D. The resolve pass turns
// this buffer into the samplable r32float 3D density texture.

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

@group(0) @binding(0) var<storage, read> pos_life: array<vec4f>;
@group(0) @binding(1) var<storage, read> alive_indices: array<u32>;
@group(0) @binding(2) var<storage, read> counters: array<atomic<u32>>;
@group(0) @binding(3) var<uniform> u: VolUniforms;
@group(0) @binding(4) var<storage, read_write> voxel: array<atomic<u32>>;

fn pcg_hash(v: u32) -> u32 {
    var h = v * 747796405u + 2891336453u;
    h = ((h >> 16u) ^ h) * 2654435769u;
    h = ((h >> 16u) ^ h);
    return h;
}

@compute @workgroup_size(256)
fn cs_scatter(@builtin(global_invocation_id) gid: vec3u) {
    let thread_idx = gid.x;
    let alive = atomicLoad(&counters[0]);
    if thread_idx >= alive {
        return;
    }

    let particle_idx = alive_indices[thread_idx];
    let pl = pos_life[particle_idx];
    if pl.w <= 0.0 {
        return; // dead particle
    }

    // Synthesize a stable Z in [-1, 1] from the particle index (not the thread
    // index, so depth does not flicker as particles are recycled/reordered).
    let h = pcg_hash(particle_idx);
    let z_base = f32(h & 0xFFFFu) / 65535.0 * 2.0 - 1.0;
    let z = clamp(z_base * u.volume_depth, -1.0, 1.0);

    // Map the unit cube [-1, 1]^3 to voxel coordinates [0, grid_res).
    let g = f32(u.grid_res);
    let v = (vec3f(pl.x, pl.y, z) * 0.5 + 0.5) * g;
    if any(v < vec3f(0.0)) || any(v >= vec3f(g)) {
        return;
    }
    let vi = vec3u(v);
    let idx = (vi.z * u.grid_res + vi.y) * u.grid_res + vi.x;

    // Fixed-point additive deposit (density_scale keeps u32 accumulation stable).
    atomicAdd(&voxel[idx], u32(u.density_scale));
}
