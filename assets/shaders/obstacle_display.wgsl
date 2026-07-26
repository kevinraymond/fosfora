// Faint underlay of the obstacle model (#1851) so the form the water flows over
// is visible — a tuning aid and a legitimate look (show the sculpture under the
// water). Fullscreen pass sampling the obstacle depth field, alpha-over into the
// HDR target beneath the particle render. Fit math matches particle_lib's
// obstacle_uv so it lines up exactly with the collision field.

struct DisplayU {
    resolution: vec2f,
    tex_dims: vec2f,
    fit: u32,
    opacity: f32,
    _p0: f32,
    _p1: f32,
};

@group(0) @binding(0) var<uniform> u: DisplayU;
@group(0) @binding(1) var obstacle_tex: texture_2d<f32>;
@group(0) @binding(2) var obstacle_samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VsOut {
    // Fullscreen triangle.
    var corners = array<vec2f, 3>(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
    let xy = corners[vi];
    var out: VsOut;
    out.pos = vec4f(xy, 0.0, 1.0);
    out.uv = xy * 0.5 + 0.5; // 0..1, y up
    return out;
}

fn fit_size() -> vec2f {
    if u.fit == 0u { return vec2f(1.0); }
    let fit = u.resolution / max(u.tex_dims, vec2f(1.0));
    let s = select(min(fit.x, fit.y), max(fit.x, fit.y), u.fit == 2u);
    return u.tex_dims * s / u.resolution;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
    // clip→obstacle uv (y flip + fit), matching particle_lib::obstacle_uv.
    let s = vec2f(in.uv.x, 1.0 - in.uv.y);
    let uv = (s - 0.5) / fit_size() + 0.5;
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
        discard;
    }
    let h = textureSampleLevel(obstacle_tex, obstacle_samp, uv, 0.0).a;
    let cover = smoothstep(0.04, 0.12, h);
    if cover < 0.001 {
        discard;
    }
    // Depth-shaded cool grey so relief (sockets darker, brow brighter) reads.
    let shade = 0.12 + 0.6 * h;
    let col = vec3f(0.30, 0.45, 0.65) * shade;
    return vec4f(col, cover * u.opacity);
}
