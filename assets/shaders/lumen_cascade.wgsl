// Lumen — cascade pass: one radiance-cascade level (Radiance Cascades, Osborne/Sannikov).
// ONE file drives EVERY cascade pass; a pass learns its own level from its texture size.
//   feedback() = own previous frame (used ONLY for correct size; cascades are recomputed)
//   input0     = scene: rgb emission, a occlusion density (always at scale 0.5)
//   input1     = the cascade ONE LEVEL UP (already merged this frame). The TOP pass has no
//                level above it and is wired inputs:["scene","scene"]; the merge is skipped
//                there (level == MAX_LEVEL), so input1 is just a declared placeholder.
//
// Layout (decreasing-resolution, fixed-16-direction variant, so a single shader serves all
// levels with no per-pass constant): 16 directions per probe packed in a 4x4 texel block.
// Cascade c renders at scale 0.5/2^c, so its probe grid halves per axis each level while the
// ray interval doubles — cascade 0 is dense probes / short rays, the top is sparse probes /
// long rays. level = round(log2(scene_dims / my_dims)); cascade 0 (scale 0.5) → level 0.
//
// Per texel: raymarch the scene along this probe's direction over the level's interval,
// accumulating emission attenuated by occluders + fog (rgb) and the interval's end
// visibility (a). Then merge the level above: radiance = near + near_visibility * far, with
// `far` bilinearly interpolated across the 4 nearest upper-cascade probes (kills blocky GI).

const TAU: f32 = 6.28318530718;
const DIRS: i32 = 16;
const BLOCK: i32 = 4;       // 4x4 = 16 directions per probe
const MAX_LEVEL: i32 = 5;   // top cascade index — MUST equal (cascade-pass count - 1) in lumen.pfx
const STEPS: i32 = 12;      // march steps per interval
const OCC_K: f32 = 42.0;    // occluder extinction per unit density·distance
const EMIT: f32 = 11.0;     // emission → radiance gain
const FALL: f32 = 13.0;     // inverse-square range falloff — light pools near sources, so the
                            // field falls to darkness between lights instead of a uniform wash

// Read one exact direction texel of a cascade texture (textureLoad, NOT the linear accessor:
// linear sampling would bleed neighbouring directions inside a probe block).
fn cascade_texel(tex: texture_2d<f32>, probe: vec2i, dir_k: i32, grid: vec2i) -> vec3f {
    let pc = clamp(probe, vec2i(0), grid - vec2i(1));
    let dl = vec2i(dir_k % BLOCK, dir_k / BLOCK);
    return textureLoad(tex, pc * BLOCK + dl, 0).rgb;
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let my_dims = vec2f(textureDimensions(prev_frame));      // this cascade's target
    let scene_dims = vec2f(textureDimensions(input0_tex));   // scene, always scale 0.5
    let level = i32(round(log2(scene_dims.x / my_dims.x)));  // 0 = finest
    let aspect = scene_dims.x / scene_dims.y;

    let texel = vec2i(frag_coord.xy);
    let probe = texel / BLOCK;
    let dloc = texel - probe * BLOCK;              // 0..3 each axis
    let k = dloc.y * BLOCK + dloc.x;               // direction index 0..15
    let ang = TAU * (f32(k) + 0.5) / f32(DIRS);
    let dir = vec2f(cos(ang), sin(ang));           // isotropic (y-reference) direction

    // Probe centre in scene-uv (0..1).
    let pgrid = vec2i(my_dims) / BLOCK;
    let probe_uv = (vec2f(probe) + 0.5) / vec2f(pgrid);

    // Contiguous doubling intervals: L0 [0,R0], L1 [R0,3R0], ... reach ≈ R0*(2^(L+1)-1).
    let r0 = 0.020 + 0.030 * param(6u);            // p6 reach
    let t0 = r0 * (pow(2.0, f32(level)) - 1.0);
    let t1 = r0 * (pow(2.0, f32(level + 1)) - 1.0);

    // Fog: tonal passages (low flatness) thicken the medium → god-ray shafts. Extinction only
    // here (glow is added in the display); param(5) is the base amount.
    let fog = param(5u) * mix(0.15, 2.2, 1.0 - u.flatness);

    // --- raymarch the scene along `dir` over [t0, t1] ---
    var radiance = vec3f(0.0);
    var trans = 1.0;
    let seg = (t1 - t0) / f32(STEPS);
    for (var s = 0; s < STEPS; s = s + 1) {
        let dist = t0 + (f32(s) + 0.5) * seg;
        // uv step: isotropic distance → x compressed by aspect.
        let sp = probe_uv + vec2f(dir.x / aspect, dir.y) * dist;
        if (any(sp < vec2f(0.0)) || any(sp > vec2f(1.0))) { break; }
        let sc = input0(sp);
        let atten = 1.0 / (1.0 + FALL * dist * dist);
        radiance += trans * sc.rgb * EMIT * seg * atten;
        let ext = sc.a * OCC_K + fog;
        trans *= exp(-ext * seg);
    }

    // --- merge the level above (near + visibility * far), bilinear across its probes ---
    if (level < MAX_LEVEL) {
        let up_dims = vec2i(textureDimensions(input1_tex));
        let up_grid = up_dims / BLOCK;
        let fp = probe_uv * vec2f(up_grid) - vec2f(0.5);
        let ip = vec2i(floor(fp));
        let fr = fract(fp);
        let c00 = cascade_texel(input1_tex, ip + vec2i(0, 0), k, up_grid);
        let c10 = cascade_texel(input1_tex, ip + vec2i(1, 0), k, up_grid);
        let c01 = cascade_texel(input1_tex, ip + vec2i(0, 1), k, up_grid);
        let c11 = cascade_texel(input1_tex, ip + vec2i(1, 1), k, up_grid);
        let far = mix(mix(c00, c10, fr.x), mix(c01, c11, fr.x), fr.y);
        radiance += trans * far;
    }

    return vec4f(radiance, trans);
}
