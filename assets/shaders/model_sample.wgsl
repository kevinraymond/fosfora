// Model-to-particle sampling pass (#1993 — BYO models as a particle source).
//
// Renders a posed 3D model (triangle mesh or splat point cloud) into an offscreen
// RGBA frame which `image_source::sample_rgba_buffer` then decomposes into particle
// home positions and colours — the same call video and webcam frames already make.
// So this shader's whole job is to produce something that looks like a PNG of the
// model: shaded colour where the model is, and ALPHA 0 everywhere else.
//
// That transparent background is load-bearing, not cosmetic. The grid sampler
// rejects pixels with alpha < 10 (image_source.rs), twice, so clearing to
// transparent means the silhouette comes out for free and not one particle is
// spent on background.
//
// Distinct from `obstacle_model.wgsl`, which rasters the same geometry into a
// near-bright DEPTH ramp for collision. That one answers "how far away is this",
// this one answers "what colour is this" — hence a real depth buffer here, so
// nearer surfaces win, rather than depth-as-output.

// Every field is a SCALAR f32 by design, including the light position. A `vec3f`
// here would align to 16 and silently desync this struct from its `#[repr(C)]`
// Rust mirror — a mismatch wgpu rejects at pipeline creation and no compile-only
// probe can see.
struct SampleUniforms {
    // Split model-view and projection so splat billboards can offset in view
    // space (screen-facing quads of a world-space radius).
    mv: mat4x4<f32>,
    proj: mat4x4<f32>,
    radius_scale: f32,
    // Floor on the lambert term. Cavities that fall to pure black still carry
    // alpha 1, so they are a legitimate dark TONE rather than background — but a
    // mesh whose interior reads solid black gives Pegboard and Etch nothing to
    // quantize, so lift the shadows off the floor.
    ambient: f32,
    // 0 = the shipped directional key light, 1 = only the point light (#1996).
    light_mix: f32,
    ray_strength: f32,
    base_color: vec4<f32>,
    // Point light in VIEW space — transformed CPU-side by the same `view * model`
    // that builds `mv`, so the light rides WITH the model and a light parked
    // inside a skull stays inside it at every yaw.
    light_x: f32,
    light_y: f32,
    light_z: f32,
    // ...and its projected screen position, for the radial march in fs_godray.
    light_u: f32,
    light_v: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var<uniform> u: SampleUniforms;

// Key light in VIEW space, so the model stays lit from the viewer's upper left at
// any yaw. A world-space light would swing the lit face away as the model turns
// and hand the sampler a silhouette that is mostly shadow.
const KEY_DIR = vec3<f32>(-0.4, 0.6, 0.7);

// Inverse-square-ish falloff for the point light. The model is normalized to a
// bounding radius of 1 (see the ortho fit in model_source.rs), so interior
// distances run 0..2 and a coefficient of 1.0 puts the half-light shell right at
// the surface — bright cavity walls, dim far side.
const LIGHT_FALLOFF = 1.0;

// Perceptual tone -> linear, so a shading value written here comes back out of
// the sRGB target as that same value in the BYTE the sampler reads.
//
// Without this the lambert term is treated as linear light and the sRGB encode
// lifts it: a cube's three visible faces landed at bytes 0.78/0.87/0.97, a fifth
// of the available range, and every tone-quantizing effect downstream (Pegboard's
// eight-peg tray, Etch's scan bands) saw them as nearly the same shade. Encoding
// the intent instead spreads the same three faces across 0.56/0.72/0.92.
fn srgb_to_linear(c: f32) -> f32 {
    if (c <= 0.04045) {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

// The other direction, for fs_godray: sampling the sRGB target hands back LINEAR
// light, but every threshold in this file is written against perceptual intent.
fn linear_to_srgb(c: f32) -> f32 {
    if (c <= 0.0031308) {
        return c * 12.92;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

fn shade(normal: vec3<f32>, view_pos: vec3<f32>) -> vec4<f32> {
    var n = normalize(normal);

    // Orient the normal to the side actually being LOOKED at, before any lighting.
    // In view space the camera sits at the origin, so `view_pos` is itself the
    // direction from eye to fragment: a normal that agrees with it points away
    // from the viewer and belongs to the far side of the surface.
    //
    // This is what makes a light inside a closed form work at all. A skull mesh
    // has outward normals everywhere, so the interior of the far wall — the
    // surface you see THROUGH an eye socket — arrives with its normal pointing
    // away from both the camera and the cavity, and would shade black no matter
    // where the light sat. Flipping it recovers the inward-facing surface it
    // visually is. Unlike a winding or front_facing test this needs no assumption
    // about the mesh: it reads only the normal and the position.
    if (dot(n, view_pos) > 0.0) {
        n = -n;
    }

    // Key light: directional and TWO-sided. abs(), not max(..., 0), because mesh
    // winding is unreliable (obstacle_model.rs keeps both faces for the same
    // reason), so a back-facing normal is a lighting artefact rather than a
    // surface that should go black. (The flip above makes this abs() redundant
    // rather than wrong; it stays so the shipped key light is bit-for-bit what
    // v1.28.0 rendered.)
    let key = abs(dot(n, normalize(KEY_DIR)));

    // Point light: positional and deliberately ONE-sided, which is the entire
    // mechanism behind #1996. A light parked inside a skull must leave the outer
    // cranium dark and light only the surfaces that FACE the cavity — the socket
    // walls, the nasal interior, the underside of the brow. Reusing the key
    // light's abs() here would light the cranium exactly as strongly as the
    // cavity and collapse "lit from inside" into a flat matte.
    let to_light = vec3<f32>(u.light_x, u.light_y, u.light_z) - view_pos;
    let point = max(dot(n, normalize(to_light)), 0.0)
        / (1.0 + LIGHT_FALLOFF * dot(to_light, to_light));

    let lambert = mix(key, point, u.light_mix);
    let lit = u.ambient + (1.0 - u.ambient) * lambert;
    let rgb = u.base_color.rgb * lit;
    return vec4<f32>(
        srgb_to_linear(rgb.r),
        srgb_to_linear(rgb.g),
        srgb_to_linear(rgb.b),
        1.0,
    );
}

// ---- Mesh path: indexed triangles ----------------------------------------

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) view_normal: vec3<f32>,
    @location(1) view_pos: vec3<f32>,
};

@vertex
fn vs_mesh(
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
) -> MeshOut {
    var out: MeshOut;
    let view_pos = u.mv * vec4<f32>(pos, 1.0);
    out.clip = u.proj * view_pos;
    out.view_pos = view_pos.xyz;
    // The model transform is a rotation of a uniformly-normalized model, so the
    // upper 3x3 of mv carries normals correctly with no inverse-transpose.
    out.view_normal = (u.mv * vec4<f32>(normal, 0.0)).xyz;
    return out;
}

@fragment
fn fs_mesh(in: MeshOut) -> @location(0) vec4<f32> {
    return shade(in.view_normal, in.view_pos);
}

// ---- Splat path: instanced screen-facing quads ---------------------------

// Unit-quad corners (two triangles), generated from vertex_index so no
// per-vertex buffer is needed — draw 6 verts x instance_count.
const QUAD = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0), vec2<f32>( 1.0, -1.0), vec2<f32>( 1.0,  1.0),
    vec2<f32>(-1.0, -1.0), vec2<f32>( 1.0,  1.0), vec2<f32>(-1.0,  1.0),
);

struct SplatOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec3<f32>,
};

@vertex
fn vs_splat(
    @builtin(vertex_index) vi: u32,
    // instance a: xyz = center, w = radius (world units)
    @location(0) inst_a: vec4<f32>,
    // instance b: rgb = splat colour
    @location(1) inst_b: vec4<f32>,
) -> SplatOut {
    var out: SplatOut;
    let corner = QUAD[vi];
    var view_pos = u.mv * vec4<f32>(inst_a.xyz, 1.0);
    view_pos = vec4<f32>(view_pos.xy + corner * inst_a.w * u.radius_scale, view_pos.zw);
    out.clip = u.proj * view_pos;
    out.uv = corner;
    out.color = inst_b.rgb;
    return out;
}

@fragment
fn fs_splat(in: SplatOut) -> @location(0) vec4<f32> {
    // Round splats: drop the quad corners so overlapping discs approximate the
    // surface rather than a grid of squares.
    if (dot(in.uv, in.uv) > 1.0) {
        discard;
    }
    // A capture already carries its own baked lighting, so shading it again would
    // double up — which is also why the point light does not reach this path.
    // God rays still do: fs_godray works off the rendered frame, not the geometry.
    return vec4<f32>(in.color * u.base_color.rgb, 1.0);
}

// ---- God rays: radial light scattering over the rendered frame -------------
//
// Second pass (#1996). The first pass leaves exactly the two channels this needs:
// bright pixels where the point light strikes a cavity wall, and alpha 0 in the
// gaps — sockets, nasal cavity, the gap under the jaw. Marching each pixel back
// toward the light's screen position accumulates whatever brightness lies along
// that line, so a ray that crosses a lit socket picks light up and a ray that
// would have to cross solid cranium picks up nothing. Shafts leave the holes and
// only the holes, which is the picture, and it falls out of the geometry rather
// than being authored.

@group(1) @binding(0) var src_tex: texture_2d<f32>;
@group(1) @binding(1) var src_samp: sampler;

const RAY_SAMPLES = 64;
// How far back along the ray the march reaches, as a fraction of the distance to
// the light. Below 1 the shafts stop short of the source and read as detached
// streaks; at 1 they run all the way in.
const RAY_DENSITY = 1.0;
// Attenuation across the WHOLE march, so a shaft fades with distance from its
// socket instead of terminating in a hard edge at the sample budget.
//
// Expressed end-to-end rather than per step on purpose. A per-step constant
// couples the LOOK to RAY_SAMPLES: raising the sample count to smooth the march
// also compounds the decay over the same distance, so a quality knob silently
// becomes a brightness knob — the same shape of bug as a per-frame decay standing
// in for a per-second one (#1986). Here RAY_SAMPLES buys only smoothness.
const RAY_FALLOFF = 0.07;
// Only genuinely bright pixels emit. The band is deliberately high and narrow:
// ordinary key-lit surfaces sit well below it, so turning rays up on a model with
// no interior light does nothing rather than fogging the whole silhouette.
const EMIT_LO = 0.55;
const EMIT_HI = 0.95;
// Legibility gain on the normalized march. Without it "Rays 1.00" means "the
// mean emission along this line", and an opening that occupies a tenth of a ray's
// length — an eye socket, which is the whole use case — scatters to alpha ~18 of
// 255: sampler-visible, but a shaft of nearly black particles. The shafts are
// brightened deliberately; the clamp below still bounds them.
const RAY_EXPOSURE = 4.0;
// Scatter below this is haze, not a shaft. It matters far more here than in a
// normal god-ray pass, because this frame is not shown — it is SAMPLED, and the
// sampler takes any texel with alpha >= 10 (0.04). Ungated, the faint tail of the
// march crossed that everywhere and the raster came back at 100% coverage: every
// particle in the budget spent on a frame-filling fog instead of on the model and
// its shafts. The gate is soft, so it removes the fog without drawing a ring.
const RAY_FLOOR = 0.10;

// Interleaved-gradient noise — deterministic per pixel, and free of the
// fract(sin(...)) degeneracy that bit #1987.
fn dither(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

struct FullscreenOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) vi: u32) -> FullscreenOut {
    // Oversized triangle rather than a quad: no vertex buffer, no seam.
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: FullscreenOut;
    out.uv = uv;
    // The y term is NEGATED, and that is the whole point. NDC y runs up (+1 is
    // the top of the frame) while texture v runs down (0 is the top row), so
    // `uv * 2 - 1` on both axes silently mirrors the frame vertically. It only
    // showed when rays were on, because rays-off skips this pass and reads back
    // the first target untouched.
    out.clip = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    return out;
}

// Perceptual luminance of a sample, gated to the emissive band. Sampling an sRGB
// texture returns linear light, so it has to be re-encoded before thresholding —
// the same lesson as srgb_to_linear above, in reverse. Thresholding the linear
// value instead would put EMIT_LO at a completely different apparent brightness.
fn emission(uv: vec2<f32>) -> f32 {
    let s = textureSampleLevel(src_tex, src_samp, uv, 0.0);
    let lum = dot(s.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    // Weight by alpha so the transparent clear cannot emit.
    return smoothstep(EMIT_LO, EMIT_HI, linear_to_srgb(lum)) * s.a;
}

@fragment
fn fs_godray(in: FullscreenOut) -> @location(0) vec4<f32> {
    let base = textureSampleLevel(src_tex, src_samp, in.uv, 0.0);

    let light_uv = vec2<f32>(u.light_u, u.light_v);
    let delta = (in.uv - light_uv) * (RAY_DENSITY / f32(RAY_SAMPLES));

    // Start each pixel's march at a random fraction of one step. Marching from a
    // fixed offset makes neighbouring pixels cross the aperture at the same
    // quantized step count, which the 64-tap budget renders as concentric rings
    // around the light; scattering the phase turns those rings into film grain.
    var ray_uv = in.uv - delta * dither(in.clip.xy);
    var illum = 1.0;
    var accum = 0.0;
    // Total weight the march CAN deposit, accumulated alongside it. Dividing by
    // the sample count instead would fold the decay series into the result: with
    // RAY_DECAY 0.96 over 64 steps the weights sum to 23.2, so a fully emissive
    // path would top out near a third of full strength and "Rays 1.00" would
    // deliver a faint wash. Summing the weights makes it mean what it says, and
    // survives anyone retuning the two constants above.
    var norm = 0.0;
    let step_decay = pow(RAY_FALLOFF, 1.0 / f32(RAY_SAMPLES));
    for (var i = 0; i < RAY_SAMPLES; i = i + 1) {
        ray_uv = ray_uv - delta;
        accum = accum + emission(ray_uv) * illum;
        norm = norm + illum;
        illum = illum * step_decay;
    }
    accum = clamp(
        accum / max(norm, 1e-5) * u.ray_strength * RAY_EXPOSURE,
        0.0,
        1.0,
    );

    // Rays sit BEHIND the model — a shaft must not wash over the bone it escaped
    // from — so weight by how transparent this pixel already is.
    let scattered = accum * (1.0 - base.a);
    // Soft gate: full strength well above the floor, nothing well below it.
    let behind = scattered * smoothstep(RAY_FLOOR, RAY_FLOOR * 2.0, scattered);

    // The alpha is the load-bearing half of this line, not a detail. The sampler
    // rejects alpha < 10 twice (see the header), so a shaft written with colour
    // and no alpha is INVISIBLE to it: the readback would look correct in a
    // screenshot and yield not one extra particle. Alpha here is what puts
    // particles on the beams, which is the only reason to draw them in the raster
    // rather than as a post-effect.
    return vec4<f32>(
        base.rgb + vec3<f32>(srgb_to_linear(behind)),
        max(base.a, behind),
    );
}
