// Lattice CA step: one 3D-cellular-automata generation. Reads state_in, counts
// live neighbours in a 3x3x3 Moore (26) or Von Neumann (6) neighbourhood with
// toroidal-wrap or dead-border boundaries, applies the birth / survival bitmasks
// carried in the uniform block, advances generations (dying) states, and writes
// the next state buffer. Density is NOT written here — a separate once-per-frame
// display pass EMA-blends state into the density texture so fast rules fade
// instead of strobing. Audio extras (onset seed injection, flux perturbation)
// fold in here; all rule/mask logic is resolved CPU-side.
//
// Only fully-alive cells (state == 1) count as live neighbours; dying cells
// (state 2..N-1) do NOT — this is the standard "generations" rule.
//
// Builtin shaders get no lib preamble, so the uniform block + hash helpers are
// duplicated here and in lattice_seed.wgsl / lattice_display.wgsl (must byte-match
// the Rust struct).

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
    _pad0: u32,
}

@group(0) @binding(0) var<uniform> u: LatticeUniforms;
@group(0) @binding(1) var<storage, read> state_in: array<u32>;
@group(0) @binding(2) var<storage, read_write> state_out: array<u32>;

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

// Linear index of a neighbour with the active boundary policy.
fn widx(p: vec3<i32>, g: i32) -> u32 {
    if (u.boundary == 0u) {
        let w = ((p % g) + g) % g; // toroidal wrap
        return (u32(w.z) * u32(g) + u32(w.y)) * u32(g) + u32(w.x);
    }
    let c = clamp(p, vec3<i32>(0), vec3<i32>(g - 1));
    return (u32(c.z) * u32(g) + u32(c.y)) * u32(g) + u32(c.x);
}

// Cell life-state (low 8 bits): 0 dead, 1 alive, 2..N-1 dying. The high bits carry
// the cell's age (generations since birth) for the age→hue channel, so every rule
// test must mask them off first.
fn cell_of(s: u32) -> u32 {
    return s & 0xFFu;
}

// 1 if the neighbour is fully alive (state == 1), else 0. Dead-border cells
// outside the grid count as 0.
fn live_at(p: vec3<i32>, g: i32) -> u32 {
    if (u.boundary == 1u && (any(p < vec3<i32>(0)) || any(p >= vec3<i32>(g)))) {
        return 0u;
    }
    return select(0u, 1u, cell_of(state_in[widx(p, g)]) == 1u);
}

// Frame-hashed injection cluster center (world voxel coords). Kept inside the
// spherical domain (when active) so onset seeds don't land in the dead shell.
fn inject_center(gf: f32) -> vec3f {
    let base = u.frame ^ (u.seed_hash * 2654435761u);
    let h = vec3f(
        hash01(pcg(base + 11u)),
        hash01(pcg(base + 23u)),
        hash01(pcg(base + 37u)),
    );
    if (u.domain_mode == 1u) {
        let np = (h * 2.0 - vec3f(1.0)) * (u.domain_radius * 0.7);
        return (np * 0.5 + vec3f(0.5)) * gf;
    }
    return h * gf;
}

// True when a cell lies outside the active domain (spherical) and must stay dead.
fn outside_domain(gid: vec3<u32>, gf: f32) -> bool {
    if (u.domain_mode != 1u) {
        return false;
    }
    let np = (vec3f(gid) + vec3f(0.5)) / gf * 2.0 - vec3f(1.0);
    return length(np) > u.domain_radius;
}

@compute @workgroup_size(4, 4, 4)
fn cs_step(@builtin(global_invocation_id) gid: vec3<u32>) {
    let g = u.grid_res;
    if (any(gid >= vec3<u32>(g))) {
        return;
    }
    let gi = i32(g);
    let x = i32(gid.x);
    let y = i32(gid.y);
    let z = i32(gid.z);
    let idx = (gid.z * g + gid.y) * g + gid.x;
    let cur = state_in[idx];
    let cur_cell = cell_of(cur);
    let cur_age = cur >> 8u;

    var count = 0u;
    if (u.neighborhood == 0u) {
        // Moore (26).
        for (var dz = -1; dz <= 1; dz++) {
            for (var dy = -1; dy <= 1; dy++) {
                for (var dx = -1; dx <= 1; dx++) {
                    if (dx == 0 && dy == 0 && dz == 0) {
                        continue;
                    }
                    count += live_at(vec3<i32>(x + dx, y + dy, z + dz), gi);
                }
            }
        }
    } else {
        // Von Neumann (6).
        count = live_at(vec3<i32>(x - 1, y, z), gi) + live_at(vec3<i32>(x + 1, y, z), gi)
            + live_at(vec3<i32>(x, y - 1, z), gi) + live_at(vec3<i32>(x, y + 1, z), gi)
            + live_at(vec3<i32>(x, y, z - 1), gi) + live_at(vec3<i32>(x, y, z + 1), gi);
    }

    // `next` is the life-state only (0/1/2..); age is packed on at the end.
    var next = 0u;
    if (cur_cell == 0u) {
        // Dead cell — birth rule.
        if ((u.birth_mask & (1u << count)) != 0u) {
            next = 1u;
        }
    } else if (cur_cell == 1u) {
        // Alive cell — survival rule.
        if ((u.survival_mask & (1u << count)) != 0u) {
            next = 1u;
        } else {
            // Die, or begin dying (refractory) if this is a generations rule.
            next = select(0u, 2u, u.num_states > 2u);
        }
    } else {
        // Dying cell — decay toward death, ignoring the rules.
        next = cur_cell + 1u;
        if (next >= u.num_states) {
            next = 0u;
        }
    }

    // Onset seed injection: sprinkle a hashed cluster alive on the beat. Sparse
    // (≈40% fill in a smaller radius) rather than a solid blob, so repeated beats
    // perturb and re-seed structure instead of accumulating into a packed ball.
    if (u.inject_active == 1u) {
        let ctr = inject_center(f32(g));
        if (distance(vec3f(gid) + vec3f(0.5), ctr) < f32(u.seed_size) * 0.7
            && hash01(cell_hash(gid, u.frame ^ 0x5bd1e995u)) < 0.4) {
            next = 1u;
        }
    }

    // Spectral-flux perturbation: sparse random alive<->dead toggle to break stasis.
    if (u.perturb_prob > 0.0) {
        if (hash01(cell_hash(gid, u.frame ^ u.seed_hash)) < u.perturb_prob) {
            next = select(1u, 0u, next != 0u);
        }
    }

    // Cells outside the spherical domain are forced dead (kills the cube silhouette
    // and confines growth), applied last so it overrides injection / perturbation.
    if (outside_domain(gid, f32(g))) {
        next = 0u;
    }

    // Age (generations since birth, saturating at 255): reset on a fresh birth,
    // otherwise advance while the cell is alive or dying. Packed into the high bits.
    var next_age = 0u;
    if (next != 0u) {
        let was_live = cur_cell != 0u;
        next_age = select(0u, min(cur_age + 1u, 255u), was_live);
    }
    state_out[idx] = (next_age << 8u) | next;
}
