// Virtual-pipes shallow-water sim over the obstacle height field (#1851 water
// accumulation). The terrain is the obstacle alpha (near-bright depth); water
// height W accumulates on top, flows from high (terrain+W) to low via per-cell
// outflow "pipes" to the 4 neighbours, fills enclosed basins (eye sockets),
// and overflows the rims. Two compute steps per sub-step, ping-ponging the
// water + flux textures (mirrors the reaction-diffusion sim's read-load /
// storage-store pattern).
//
// Textures are all texel-aligned at the grid resolution; neighbours are read
// with textureLoad (integer coords), so no sampler is involved.

struct WaterUniforms {
    grid: u32,
    _pad0: u32,
    dt: f32,
    flux_gain: f32,     // lumped g·A/l — pressure→flux gain (CFL: dt·flux_gain·4 < 1)
    source_rate: f32,   // inflow per second where terrain > floor ("rain on the model")
    drain: f32,         // evaporation per second (keeps it bounded)
    terrain_floor: f32, // alpha above which a cell counts as "on the model"
    edge_drain: f32,    // per-step retention off-model (overflow sheets away)
};

@group(0) @binding(0) var<uniform> u: WaterUniforms;
@group(0) @binding(1) var terrain_tex: texture_2d<f32>;
@group(0) @binding(2) var water_in: texture_2d<f32>;
// flux pass: previous frame's flux. height pass: this step's new flux.
@group(0) @binding(3) var flux_in: texture_2d<f32>;
@group(0) @binding(4) var out_tex: texture_storage_2d<rgba16float, write>;

fn clampc(c: vec2i) -> vec2i {
    let g = i32(u.grid) - 1;
    return clamp(c, vec2i(0), vec2i(g, g));
}

// Total hydraulic head (terrain + water) at a clamped cell.
fn head(c: vec2i) -> f32 {
    let cc = clampc(c);
    return textureLoad(terrain_tex, cc, 0).a + textureLoad(water_in, cc, 0).r;
}

// --- Pass 1: update outflow flux from head differences ---------------------
// flux channels = outflow toward (Left, Right, Bottom, Top).
@compute @workgroup_size(8, 8)
fn flux_step(@builtin(global_invocation_id) gid: vec3u) {
    let g = i32(u.grid);
    let c = vec2i(gid.xy);
    if (c.x >= g || c.y >= g) { return; }

    let w = textureLoad(water_in, c, 0).r;
    let hc = textureLoad(terrain_tex, c, 0).a + w;
    let prev = textureLoad(flux_in, c, 0);

    let dl = hc - head(c + vec2i(-1, 0));
    let dr = hc - head(c + vec2i(1, 0));
    let db = hc - head(c + vec2i(0, -1));
    let dt2 = hc - head(c + vec2i(0, 1));

    var f = vec4f(
        max(0.0, prev.x + u.dt * u.flux_gain * dl),
        max(0.0, prev.y + u.dt * u.flux_gain * dr),
        max(0.0, prev.z + u.dt * u.flux_gain * db),
        max(0.0, prev.w + u.dt * u.flux_gain * dt2),
    );

    // Never drain more water than the cell holds this step.
    let sum = (f.x + f.y + f.z + f.w) * u.dt;
    if (sum > w && sum > 1e-8) {
        f *= w / sum;
    }
    textureStore(out_tex, c, f);
}

// --- Pass 2: update water height from flux divergence + source/drain -------
@compute @workgroup_size(8, 8)
fn height_step(@builtin(global_invocation_id) gid: vec3u) {
    let g = i32(u.grid);
    let c = vec2i(gid.xy);
    if (c.x >= g || c.y >= g) { return; }

    let w = textureLoad(water_in, c, 0).r;
    let fc = textureLoad(flux_in, c, 0); // this cell's outflow (L,R,B,T)

    // Inflow = each neighbour's outflow directed at this cell.
    let in_l = textureLoad(flux_in, clampc(c + vec2i(-1, 0)), 0).y; // left's Right
    let in_r = textureLoad(flux_in, clampc(c + vec2i(1, 0)), 0).x;  // right's Left
    let in_b = textureLoad(flux_in, clampc(c + vec2i(0, -1)), 0).w; // below's Top
    let in_t = textureLoad(flux_in, clampc(c + vec2i(0, 1)), 0).z;  // above's Bottom
    let inflow = in_l + in_r + in_b + in_t;
    let outflow = fc.x + fc.y + fc.z + fc.w;

    var wn = w + u.dt * (inflow - outflow);

    let terr = textureLoad(terrain_tex, c, 0).a;
    if (terr > u.terrain_floor) {
        wn += u.source_rate * u.dt; // rain on the model
    } else {
        wn *= u.edge_drain; // off-model: shed fast so overflow sheets away
    }
    wn = max(0.0, wn * (1.0 - u.drain * u.dt));

    // .r = water height (sim); .a = same (nonzero alpha for readback / debug).
    let vel = vec2f(fc.y - fc.x, fc.w - fc.z); // net flow (Right-Left, Top-Bottom)
    textureStore(out_tex, c, vec4f(wn, vel.x, vel.y, wn));
}
