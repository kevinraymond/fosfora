// WBOIT (Weighted Blended Order-Independent Transparency) particle renderer.
// Same vertex shader as particle_render.wgsl, but fragment outputs to 2 targets:
//   location(0) = accumulation (Rgba16Float): premultiplied color * weight, alpha * weight
//   location(1) = revealage (R8Unorm): alpha (multiplied via Zero+OneMinusSrc blend)

struct RenderUniforms {
    resolution: vec2f,
    time: f32,
    render_mode: u32,   // 0=circle, 1=sprite, 2=animated sprite
    sprite_cols: u32,
    sprite_rows: u32,
    sprite_frames: u32,
    frame_index: u32,
    trail_length: u32,
    trail_width: f32,
    _pad: vec2f,
}

@group(0) @binding(0) var<storage, read> pos_life: array<vec4f>;
@group(0) @binding(1) var<storage, read> vel_size: array<vec4f>;
@group(0) @binding(2) var<storage, read> color: array<vec4f>;
@group(0) @binding(3) var<storage, read> flags: array<vec4f>;
@group(0) @binding(4) var<uniform> ru: RenderUniforms;
@group(0) @binding(5) var<storage, read> alive_indices: array<u32>;

@group(1) @binding(0) var sprite_tex: texture_2d<f32>;
@group(1) @binding(1) var sprite_samp: sampler;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) color: vec4f,
    @location(1) quad_uv: vec2f,
    @location(2) @interpolate(flat) sprite_frame: u32,
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let particle_idx = alive_indices[instance_index];
    let pl = pos_life[particle_idx];
    let vs = vel_size[particle_idx];
    let col = color[particle_idx];
    let fl = flags[particle_idx];
    var out: VertexOutput;

    var corner: vec2f;
    let vi = vertex_index % 6u;
    switch vi {
        case 0u: { corner = vec2f(-1.0, -1.0); }
        case 1u: { corner = vec2f( 1.0, -1.0); }
        case 2u: { corner = vec2f( 1.0,  1.0); }
        case 3u: { corner = vec2f( 1.0,  1.0); }
        case 4u: { corner = vec2f(-1.0,  1.0); }
        case 5u: { corner = vec2f(-1.0, -1.0); }
        default: { corner = vec2f(0.0); }
    }

    let size = vs.w;
    let aspect = ru.resolution.x / ru.resolution.y;

    var rotated_corner = corner;
    let spin_angle = pl.z;
    if spin_angle != 0.0 {
        let ca = cos(spin_angle);
        let sa = sin(spin_angle);
        rotated_corner = vec2f(
            corner.x * ca - corner.y * sa,
            corner.x * sa + corner.y * ca
        );
    }

    let offset = rotated_corner * size * vec2f(1.0 / aspect, 1.0);
    let pos = pl.xy + offset;
    out.position = vec4f(pos, 0.0, 1.0);
    out.color = col;
    out.quad_uv = corner;

    if ru.render_mode == 2u && ru.sprite_frames > 0u {
        let age = fl.x;
        let lifetime = fl.y;
        let life_frac = clamp(age / max(lifetime, 0.001), 0.0, 0.999);
        out.sprite_frame = u32(life_frac * f32(ru.sprite_frames));
    } else {
        out.sprite_frame = 0u;
    }

    return out;
}

struct WboitOutput {
    @location(0) accum: vec4f,
    @location(1) reveal: f32,
}

@fragment
fn fs_wboit(in: VertexOutput) -> WboitOutput {
    var base_color: vec4f;

    if ru.render_mode == 0u {
        // Soft circle glow
        let dist = length(in.quad_uv);
        let glow = exp(-dist * dist * 2.0);
        if glow < 0.01 {
            discard;
        }
        base_color = vec4f(in.color.rgb * glow, in.color.a * glow);
    } else {
        // Sprite texture sampling
        let uv01 = in.quad_uv * 0.5 + 0.5;
        let col = in.sprite_frame % ru.sprite_cols;
        let row = in.sprite_frame / ru.sprite_cols;
        let tile_w = 1.0 / f32(ru.sprite_cols);
        let tile_h = 1.0 / f32(ru.sprite_rows);
        let atlas_uv = vec2f(
            (f32(col) + uv01.x) * tile_w,
            (f32(row) + (1.0 - uv01.y)) * tile_h
        );
        let tex_color = textureSample(sprite_tex, sprite_samp, atlas_uv);
        base_color = tex_color * in.color;
        if base_color.a < 0.01 {
            discard;
        }
    }

    // WBOIT weight function (McGuire & Bavoil 2013)
    // Particles are 2D (z=0), so weight degenerates to alpha-only — correct for 2D:
    // higher-alpha particles contribute proportionally more.
    let z = in.position.z;
    let weight = clamp(
        base_color.a * max(1e-2, min(3e3,
            10.0 / (1e-5 + pow(z / 5.0, 2.0) + pow(z / 200.0, 6.0))
        )), 1e-2, 3e3
    );

    var out: WboitOutput;
    out.accum = vec4f(base_color.rgb * base_color.a * weight, base_color.a * weight);
    out.reveal = base_color.a;
    return out;
}
