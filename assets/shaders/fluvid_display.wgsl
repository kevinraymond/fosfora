// Fluvid — the rendered frame, at full resolution.
//   input0 = dye (half res)
//   input1 = rd (quarter res): g = B
//   input2 = @backdrop (the camera)
//   input3 = wave (half res): r = surface height
//
// Medium (p15) morphs continuously between three liquids:
//   0 ink:     the dye, with the reaction-diffusion maze cut into it
//   1 mercury: the dye's thickness read as a liquid-metal surface that
//              reflects the camera
//   2 ripple:  a water surface over the dye, bending it and the camera, with
//              caustics where the waves focus light
// The wave field shapes the mercury surface and is the ripple medium; ink never
// sees it, so at medium 0 a faint wisp of ink (breath, smoke) stays a wisp.
//
// The chemistry runs at quarter resolution; its walls are drawn here as an
// anti-aliased iso-line of B, which keeps the maze crisp at 4K without running
// the reaction on 8 million pixels.

// Non-finite guard: see fluvid_cam.wgsl.
fn fluvid_finite(x: f32) -> bool {
    return (bitcast<u32>(x) & 0x7f800000u) != 0x7f800000u;
}

fn fluvid_clean(v: vec4f, rest: vec4f) -> vec4f {
    return select(rest, v, vec4<bool>(fluvid_finite(v.x), fluvid_finite(v.y), fluvid_finite(v.z), fluvid_finite(v.w)));
}

fn fluvid_luma(c: vec3f) -> f32 {
    return dot(c, vec3f(0.299, 0.587, 0.114));
}

// The layers below, finite and bounded.
fn fluvid_cam(uv: vec2f) -> vec3f {
    return clamp(fluvid_clean(input2(uv), vec4f(0.0)).rgb, vec3f(0.0), vec3f(4.0));
}

fn fluvid_ink_l(uv: vec2f) -> f32 {
    return fluvid_luma(input0(uv).rgb);
}

// Light from the upper left, viewer straight on (screen y points down).
const FLUVID_LIGHT: vec3f = vec3f(-0.4, -0.6, 0.7);

fn fluvid_spec(n: vec3f, power: f32) -> f32 {
    let l = normalize(FLUVID_LIGHT);
    return pow(max(dot(reflect(-l, n), vec3f(0.0, 0.0, 1.0)), 0.0), power);
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    // Full-scale pass: the target is the window, so u.resolution is its size.
    let uv = frag_coord.xy / u.resolution;

    let camera = param(7u);                        // p7
    let gloss = param(12u);                        // p12
    let pattern = param(14u);                      // p14
    let medium = clamp(param(15u), 0.0, 2.0);      // p15
    let w_ink = clamp(1.0 - medium, 0.0, 1.0);
    let w_merc = clamp(1.0 - abs(medium - 1.0), 0.0, 1.0);
    let w_rip = clamp(medium - 1.0, 0.0, 1.0);

    // ---- the water surface: slope and curvature of the wave height
    let wt = 1.0 / vec2f(textureDimensions(input3_tex));
    let hc = input3(uv).r;
    let hl = input3(uv - vec2f(wt.x, 0.0)).r;
    let hr = input3(uv + vec2f(wt.x, 0.0)).r;
    let hb = input3(uv - vec2f(0.0, wt.y)).r;
    let ht = input3(uv + vec2f(0.0, wt.y)).r;
    let slope = 0.5 * vec2f(hr - hl, ht - hb);
    let curv = hl + hr + hb + ht - 4.0 * hc;
    // Refraction offset in uv, for the ripple medium only.
    let refr = slope * 3.0;

    // ---- ink: the dye with the maze cut into it
    let iuv = uv;
    let dye = input0(iuv).rgb;
    let dye_l = fluvid_luma(dye);
    let rd_texel = 1.0 / vec2f(textureDimensions(input1_tex));
    let b = input1(iuv).g;
    let aa = max(fwidth(b), 1e-4);
    let wall = smoothstep(0.2 - aa, 0.2 + aa, b);
    let bx = input1(iuv + vec2f(rd_texel.x, 0.0)).g - input1(iuv - vec2f(rd_texel.x, 0.0)).g;
    let by = input1(iuv + vec2f(0.0, rd_texel.y)).g - input1(iuv - vec2f(0.0, rd_texel.y)).g;
    let n_ink = normalize(vec3f(-bx * 6.0, -by * 6.0, 1.0));
    let diffuse = 0.55 + 0.45 * max(dot(n_ink, normalize(FLUVID_LIGHT)), 0.0);
    let shade = mix(1.0, mix(0.15, 1.0, wall) * diffuse, pattern);
    var col_ink = dye * shade
        + vec3f(fluvid_spec(n_ink, 24.0) * gloss * 0.8 * min(dye_l, 1.0) * wall * pattern);
    col_ink += fluvid_cam(iuv) * camera * (1.0 - clamp(dye_l * 1.5, 0.0, 1.0));

    // ---- mercury: the dye's thickness as a metal surface
    let dt = 2.0 / vec2f(textureDimensions(input0_tex));
    let thick = fluvid_ink_l(uv);
    let gx = fluvid_ink_l(uv + vec2f(dt.x, 0.0)) - fluvid_ink_l(uv - vec2f(dt.x, 0.0));
    let gy = fluvid_ink_l(uv + vec2f(0.0, dt.y)) - fluvid_ink_l(uv - vec2f(0.0, dt.y));
    // A metal blob has a hard edge: surface tension, drawn rather than simulated.
    let surf = smoothstep(0.05, 0.14, thick);
    let relief = vec2f(gx, gy) * 2.5 + slope * 8.0 + vec2f(bx, by) * pattern * 1.5;
    let n_m = normalize(vec3f(-relief, 1.0));
    // It mirrors the camera, bent by the surface; with nothing to mirror it
    // still reads as chrome under a studio gradient, lit from above.
    let mirror = fluvid_cam(uv + n_m.xy * 0.12);
    let studio = mix(vec3f(0.04), vec3f(0.8, 0.82, 0.86), smoothstep(-0.7, 0.5, -n_m.y));
    let env = max(mirror * 1.1, studio * 0.75);
    let fresnel = pow(1.0 - n_m.z, 2.0);
    // A hint of the ink's hue, so the metal still answers the hue slider.
    let tint = mix(vec3f(1.0), dye / max(dye_l, 1e-3), 0.12);
    let chrome = (env * 0.9 + vec3f(fresnel * 0.5)) * tint
        + vec3f(fluvid_spec(n_m, 64.0) * (0.5 + 2.0 * gloss));
    let col_merc = chrome * surf + fluvid_cam(uv) * camera * (1.0 - surf);

    // ---- ripple: water over the dye and the camera
    let ruv = uv + refr;
    let base = input0(ruv).rgb + fluvid_cam(ruv) * camera;
    let n_w = normalize(vec3f(-slope * 40.0, 1.0));
    let caustic = max(-curv, 0.0) * 60.0;
    let glint = fluvid_spec(n_w, 48.0) * (0.3 + 1.5 * gloss);
    let col_rip = base + vec3f(min(caustic, 1.5) * 0.5 + glint) * (0.2 + clamp(fluvid_luma(base), 0.0, 1.0));

    let col = col_ink * w_ink + col_merc * w_merc + col_rip * w_rip;
    return vec4f(col, 1.0);
}
