// Lattice seed / init pass: fills the CURRENT CA state buffer from a PCG hash.
// Runs on enable, reseed, init-mode change, or grid resize. Modes: 0 random-fill
// (init_density), 1 center sphere, 2 multi-seed clusters, 3 clear. Density is no
// longer written here — the display pass derives it from state each frame with an
// EMA so first-frame reseeds fade in instead of popping. Builtin shaders get no
// lib preamble, so the hash helpers + uniform block are duplicated across the
// lattice_*.wgsl passes (must byte-match the Rust struct).

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

@group(0) @binding(0) var<uniform> u: LatticeUniforms;
@group(0) @binding(1) var<storage, read_write> state_out: array<u32>;

// True when a cell lies outside the active domain (spherical) and must stay dead.
fn outside_domain(gid: vec3<u32>, gf: f32) -> bool {
    if (u.domain_mode != 1u) {
        return false;
    }
    let np = (vec3f(gid) + vec3f(0.5)) / gf * 2.0 - vec3f(1.0);
    return length(np) > u.domain_radius;
}

fn pcg(v_in: u32) -> u32 {
    let state = v_in * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn hash01(v: u32) -> f32 {
    return f32(pcg(v)) / 4294967295.0;
}

fn cell_hash(p: vec3<u32>, salt: u32) -> u32 {
    return pcg(p.x + pcg(p.y + pcg(p.z + salt)));
}

@compute @workgroup_size(4, 4, 4)
fn cs_seed(@builtin(global_invocation_id) gid: vec3<u32>) {
    let g = u.grid_res;
    if (any(gid >= vec3<u32>(g))) {
        return;
    }
    let idx = (gid.z * g + gid.y) * g + gid.x;
    let pf = vec3f(gid) + vec3f(0.5);
    let gf = f32(g);

    var s = 0u;
    switch u.init_mode {
        // Random fill.
        case 0u: {
            if (hash01(cell_hash(gid, u.seed_hash)) < u.init_density) {
                s = 1u;
            }
        }
        // Center sphere.
        case 1u: {
            if (distance(pf, vec3f(gf * 0.5)) < f32(u.seed_size)) {
                s = 1u;
            }
        }
        // Multi-seed: a handful of hashed spherical clusters.
        case 2u: {
            for (var k = 0u; k < 8u; k++) {
                let cx = hash01(pcg(u.seed_hash + k * 3u + 1u)) * gf;
                let cy = hash01(pcg(u.seed_hash + k * 3u + 2u)) * gf;
                let cz = hash01(pcg(u.seed_hash + k * 3u + 3u)) * gf;
                if (distance(pf, vec3f(cx, cy, cz)) < f32(u.seed_size)) {
                    s = 1u;
                }
            }
        }
        // Seed noise: a central sphere filled with random cells (init_density).
        // A solid seed collapses under low-survival rules — the varied neighbour
        // counts of a noisy blob are what let birth/survival rules propagate.
        case 4u: {
            if (distance(pf, vec3f(gf * 0.5)) < f32(u.seed_size)
                && hash01(cell_hash(gid, u.seed_hash)) < u.init_density) {
                s = 1u;
            }
        }
        // Clear (all dead).
        default: {
            s = 0u;
        }
    }

    // Cells outside the spherical domain never come alive.
    if (outside_domain(gid, gf)) {
        s = 0u;
    }

    // Randomise each live seed cell's starting age across [0, max_age) so a
    // lifetime cap doesn't age the whole seed out on the same generation — a
    // synchronised die-off reads as a global flash. Packed into the high bits.
    if (s == 1u && u.max_age > 1u) {
        let a0 = cell_hash(gid, u.seed_hash ^ 0x5f356495u) % u.max_age;
        s = s | (a0 << 8u);
    }

    state_out[idx] = s;
}
