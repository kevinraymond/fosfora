// Eulerian velocity-grid fluid over the obstacle (shared flow field, #1939).
//
// A single-phase incompressible (Navier–Stokes, semi-Lagrangian "Stable
// Fluids") solve on a square grid in CLIP space [-1,1]². The obstacle is a
// SOLID boundary (no-slip, velocity 0 inside), so a throughflow prescribed at
// the top edge is routed AROUND the silhouette by the pressure projection —
// producing a genuine bow wave in front, a wake and recirculation behind, and
// vortices shed off edges. Particles read the resulting field with
// `fluid_velocity(pos)` (particle_lib) and are advected by it.
//
// Why an inflow boundary and not just gravity: a uniform body force on an
// incompressible fluid in a closed box does nothing — the pressure gradient
// cancels it (hydrostatic). The motion comes from prescribing velocity at the
// top (Dirichlet inflow) with the bottom/sides open (zero-gradient outflow), so
// there is a throughflow for the obstacle to divert. Gravity is a small tunable
// on top, not the driver.
//
// Passes (all grid², @workgroup_size(8,8), ping-ponging velocity + pressure):
//   advect_forces → divergence → pressure_jacobi ×N → project
// Structure mirrors water_sim.wgsl: Rgba16Float textures, textureLoad for
// integer-coord neighbours, a linear sampler only for the advection backtrace,
// a WriteOnly storage output.
//
// Storage conventions:
//   velocity texture .rg = CLIP-space velocity (y-up), so particles read it raw.
//   Texel row 0 = TOP of screen (clip.y = +1); row grid-1 = bottom (clip.y=-1).
//   pressure texture .r = pressure; divergence texture .r = ∇·v.

struct FluidUniforms {
    grid: u32,
    _pad0: u32,
    dt: f32,
    flow_speed: f32,      // inflow magnitude at the top edge (clip units/sec)
    flow_dx: f32,         // inflow direction x (added to the downward inflow)
    gravity: f32,         // small downward body force (clip units/sec²)
    viscosity: f32,       // velocity damping per second (0 = inviscid)
    vorticity: f32,       // vorticity-confinement strength (0 = off)
    jacobi_iters: u32,    // pressure iterations (informational; host drives count)
    _pad1: u32,
    threshold: f32,       // solid where effective height >= threshold
    water_scale: f32,     // effective height = terrain.a + water_scale * water.r
    fit: u32,             // obstacle fit: 0=stretch, 1=contain, 2=cover
    res_x: f32,           // render resolution (for the fit mapping)
    res_y: f32,
    obst_w: f32,          // obstacle texture dimensions (for the fit mapping)
    obst_h: f32,
    _pad2: f32,
    _pad3: f32,
    _pad4: f32,
};

@group(0) @binding(0) var<uniform> fu: FluidUniforms;
@group(0) @binding(1) var obstacle_tex: texture_2d<f32>;
@group(0) @binding(2) var water_tex: texture_2d<f32>;
@group(0) @binding(3) var lin_sampler: sampler;
// Role varies per pass: velocity (advect/divergence/project) or pressure (jacobi).
@group(0) @binding(4) var tex_a: texture_2d<f32>;
// Role varies per pass: velocity (project) or divergence (jacobi). Bound to a
// harmless texture when unused.
@group(0) @binding(5) var tex_b: texture_2d<f32>;
@group(0) @binding(6) var out_tex: texture_storage_2d<rgba16float, write>;

fn clampc(c: vec2i) -> vec2i {
    let g = i32(fu.grid) - 1;
    return clamp(c, vec2i(0), vec2i(g, g));
}

// Cell centre → clip space. Row 0 is the top of the screen (clip.y = +1).
fn cell_to_clip(c: vec2i) -> vec2f {
    let g = f32(fu.grid);
    let x = (f32(c.x) + 0.5) / g * 2.0 - 1.0;
    let y = 1.0 - (f32(c.y) + 0.5) / g * 2.0;
    return vec2f(x, y);
}

// --- Obstacle solid mask (replicates particle_lib's obstacle_uv/height so the
// solid shape matches exactly what the particles collide with). ------------
fn fit_size() -> vec2f {
    if fu.fit == 0u { return vec2f(1.0); }
    let res = vec2f(fu.res_x, fu.res_y);
    let dims = max(vec2f(fu.obst_w, fu.obst_h), vec2f(1.0));
    let fit = res / dims;
    let s = select(min(fit.x, fit.y), max(fit.x, fit.y), fu.fit == 2u);
    return dims * s / res;
}

fn obst_uv(p: vec2f) -> vec2f {
    let s = vec2f(p.x * 0.5 + 0.5, -p.y * 0.5 + 0.5);
    return (s - 0.5) / fit_size() + 0.5;
}

fn height_at(p: vec2f) -> f32 {
    let uv = obst_uv(p);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 { return 0.0; }
    let terr = textureSampleLevel(obstacle_tex, lin_sampler, uv, 0.0).a;
    let w = textureSampleLevel(water_tex, lin_sampler, uv, 0.0).r;
    return terr + fu.water_scale * w;
}

fn is_solid_clip(p: vec2f) -> bool {
    return height_at(p) >= fu.threshold;
}

fn is_solid_cell(c: vec2i) -> bool {
    return is_solid_clip(cell_to_clip(clampc(c)));
}

// Velocity at an arbitrary clip position via the linear sampler (advection).
fn sample_vel(p: vec2f) -> vec2f {
    let uv = vec2f((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5);
    return textureSampleLevel(tex_a, lin_sampler, uv, 0.0).rg;
}

fn load_vel(c: vec2i) -> vec2f {
    return textureLoad(tex_a, clampc(c), 0).rg;
}

// Velocity of a neighbour for the divergence/pressure stencil: a solid neighbour
// contributes zero velocity (no-slip wall) so fluid neither enters nor leaves it.
fn nb_vel(c: vec2i) -> vec2f {
    if is_solid_cell(c) { return vec2f(0.0); }
    return load_vel(c);
}

// The prescribed top-edge inflow (Dirichlet). A smooth band so the sheet has no
// hard birth line; mostly downward, with an optional lateral tilt.
fn inflow_at(p: vec2f, band: f32) -> vec2f {
    return vec2f(fu.flow_dx * fu.flow_speed, -fu.flow_speed) * band;
}

fn top_band(p: vec2f) -> f32 {
    return smoothstep(0.86, 1.0, p.y);
}

// --- Pass 1: semi-Lagrangian advection + body forces + inflow BC + vorticity -
@compute @workgroup_size(8, 8)
fn advect_forces(@builtin(global_invocation_id) gid: vec3u) {
    let g = i32(fu.grid);
    let c = vec2i(gid.xy);
    if c.x >= g || c.y >= g { return; }
    let p = cell_to_clip(c);

    // Solid interior holds zero velocity (no-slip boundary).
    if is_solid_clip(p) {
        textureStore(out_tex, c, vec4f(0.0));
        return;
    }

    // Backtrace: sample the incoming field at p - v·dt.
    let v_here = load_vel(c);
    var v = sample_vel(p - v_here * fu.dt);

    // Body force (small; the throughflow is the real driver).
    v.y -= fu.gravity * fu.dt;

    // Vorticity confinement: push velocity toward regions of high |curl| to
    // counter numerical diffusion and keep eddies crisp.
    if fu.vorticity > 0.0 {
        let wl = curl_at(c + vec2i(-1, 0));
        let wr = curl_at(c + vec2i(1, 0));
        let wt = curl_at(c + vec2i(0, -1)); // row up = clip.y up
        let wb = curl_at(c + vec2i(0, 1));  // row down = clip.y down
        var grad = vec2f(abs(wr) - abs(wl), abs(wt) - abs(wb));
        let l = length(grad);
        if l > 1e-5 {
            grad = grad / l;
            let w = curl_at(c);
            // N × ω gives the confinement force (2D: (grad.y, -grad.x) * ω).
            v += fu.vorticity * vec2f(grad.y, -grad.x) * w * fu.dt;
        }
    }

    // Viscosity (explicit damping toward rest).
    v *= max(0.0, 1.0 - fu.viscosity * fu.dt);

    // Prescribed inflow at the top edge (soft Dirichlet, re-imposed each frame).
    let band = top_band(p);
    v = mix(v, inflow_at(p, 1.0), clamp(band, 0.0, 1.0));

    textureStore(out_tex, c, vec4f(v, 0.0, 0.0));
}

// Curl ω = ∂vy/∂x − ∂vx/∂y in clip space (row up = +clip.y).
fn curl_at(c: vec2i) -> f32 {
    let vr = nb_vel(c + vec2i(1, 0));
    let vl = nb_vel(c + vec2i(-1, 0));
    let vt = nb_vel(c + vec2i(0, -1)); // clip.y up
    let vb = nb_vel(c + vec2i(0, 1));  // clip.y down
    return 0.5 * ((vr.y - vl.y) - (vt.x - vb.x));
}

// --- Pass 2: divergence of the advected velocity ---------------------------
@compute @workgroup_size(8, 8)
fn divergence(@builtin(global_invocation_id) gid: vec3u) {
    let g = i32(fu.grid);
    let c = vec2i(gid.xy);
    if c.x >= g || c.y >= g { return; }
    if is_solid_cell(c) {
        textureStore(out_tex, c, vec4f(0.0));
        return;
    }
    let vr = nb_vel(c + vec2i(1, 0));
    let vl = nb_vel(c + vec2i(-1, 0));
    let vt = nb_vel(c + vec2i(0, -1)); // clip.y up
    let vb = nb_vel(c + vec2i(0, 1));  // clip.y down
    // ∇·v in grid units (y-up: +y neighbour is the row above).
    let div = 0.5 * ((vr.x - vl.x) + (vt.y - vb.y));
    textureStore(out_tex, c, vec4f(div, 0.0, 0.0, 0.0));
}

// Neighbour pressure for the Poisson stencil: Neumann (∂p/∂n=0) at solids and
// at the domain edge (clampc already gives the edge value) → use this cell's p.
fn nb_pressure(c: vec2i, self_p: f32) -> f32 {
    if is_solid_cell(c) { return self_p; }
    return textureLoad(tex_a, clampc(c), 0).r;
}

// --- Pass 3: one Jacobi iteration of ∇²p = ∇·v (tex_a=pressure, tex_b=div) ---
@compute @workgroup_size(8, 8)
fn pressure_jacobi(@builtin(global_invocation_id) gid: vec3u) {
    let g = i32(fu.grid);
    let c = vec2i(gid.xy);
    if c.x >= g || c.y >= g { return; }
    if is_solid_cell(c) {
        textureStore(out_tex, c, vec4f(0.0));
        return;
    }
    let self_p = textureLoad(tex_a, c, 0).r;
    let pl = nb_pressure(c + vec2i(-1, 0), self_p);
    let pr = nb_pressure(c + vec2i(1, 0), self_p);
    let pt = nb_pressure(c + vec2i(0, -1), self_p);
    let pb = nb_pressure(c + vec2i(0, 1), self_p);
    let div = textureLoad(tex_b, c, 0).r;
    // ∇²p = div  →  p = (Σ neighbours − div) / 4  (grid-unit Laplacian).
    let p = (pl + pr + pt + pb - div) * 0.25;
    textureStore(out_tex, c, vec4f(p, 0.0, 0.0, 0.0));
}

// --- Pass 4: project velocity divergence-free (tex_a=velocity, tex_b=pressure)
@compute @workgroup_size(8, 8)
fn project(@builtin(global_invocation_id) gid: vec3u) {
    let g = i32(fu.grid);
    let c = vec2i(gid.xy);
    if c.x >= g || c.y >= g { return; }
    let p = cell_to_clip(c);
    if is_solid_clip(p) {
        textureStore(out_tex, c, vec4f(0.0));
        return;
    }
    var v = textureLoad(tex_a, c, 0).rg;
    let self_p = textureLoad(tex_b, c, 0).r;
    let pl = nb_pressure_b(c + vec2i(-1, 0), self_p);
    let pr = nb_pressure_b(c + vec2i(1, 0), self_p);
    let pt = nb_pressure_b(c + vec2i(0, -1), self_p);
    let pb = nb_pressure_b(c + vec2i(0, 1), self_p);
    // v -= ∇p (central difference, matching the 0.5 in divergence).
    v.x -= 0.5 * (pr - pl);
    v.y -= 0.5 * (pt - pb);
    // Re-impose the inflow so projection can't erase the driver at the top.
    let band = top_band(p);
    v = mix(v, inflow_at(p, 1.0), clamp(band, 0.0, 1.0));
    textureStore(out_tex, c, vec4f(v, 0.0, 0.0));
}

// Pressure neighbour for the project pass (pressure lives in tex_b here).
fn nb_pressure_b(c: vec2i, self_p: f32) -> f32 {
    if is_solid_cell(c) { return self_p; }
    return textureLoad(tex_b, clampc(c), 0).r;
}
