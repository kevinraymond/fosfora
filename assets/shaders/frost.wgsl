// Frost — spectral-flatness material dissolution
// Tonal sound freezes the field into faceted Voronoi crystal; noisy sound
// erodes it into drifting sand. flatness = master morph, zcr = agitation,
// bandwidth = grain size. Feedback smears the sand state only.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let aspect = res.x / res.y;
    let p = (uv - 0.5) * vec2f(aspect, 1.0);
    let t = u.time;

    // param(0) = cell_scale, param(1) = morph_bias, param(2) = grain_size,
    // param(3) = drift_speed, param(4) = edge_glow, param(5) = ice_hue,
    // param(6) = feedback_amount, param(7) = audio_reactivity, param(8-9) = drift
    let cell_scale = 3.0 + param(0u) * 9.0;
    let morph_bias = param(1u);
    let grain_size = param(2u);
    let drift_speed = param(3u);
    let edge_glow = 0.5 + param(4u) * 14.5;
    let ice_hue = param(5u);
    let feedback_amount = param(6u);
    let reactivity = param(7u);
    let drift_dir = vec2f(param(8u), param(9u));

    // zcr physically tops out near 0.5 for broadband noise
    let zcr_x = min(u.zcr * 2.5, 1.0);

    // Master morph: 0 = crystal, 1 = sand
    let m = clamp(mix(0.5, u.flatness, reactivity) + morph_bias, 0.0, 1.0);

    // === VORONOI CRYSTAL (3x3, F1/F2) ===
    let sp = p * cell_scale * (1.0 + m * 1.2);
    let ip = floor(sp);
    let fp = fract(sp);

    var d1 = 8.0;
    var d2 = 8.0;
    var closest_id = vec2f(0.0);
    var cvec = vec2f(0.0);

    let agitation = 0.15 + m * (0.6 + zcr_x * 2.0); // wander rate: calm ice, fizzing sand
    let amp = 0.25 + m * 0.45;                      // wander radius grows as it melts
    let shatter = u.onset * 0.35;

    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let neighbor = vec2f(f32(x), f32(y));
            let cell_id = ip + neighbor;

            let h = phosphor_hash2(cell_id);
            let h2 = phosphor_hash2(cell_id + vec2f(7.13, 3.71));
            let ang = h * 6.2831 + t * agitation;
            var center = neighbor + 0.5 + amp * vec2f(sin(ang), cos(ang * 1.31 + h2 * 6.2831));

            // Onset shatter kick
            center += vec2f(sin(h2 * 10.0 + t * 3.0), cos(h2 * 13.0 + t * 2.5)) * shatter;

            let diff = center - fp;
            let dist = length(diff);
            if (dist < d1) {
                d2 = d1;
                d1 = dist;
                closest_id = cell_id;
                cvec = diff;
            } else if (dist < d2) {
                d2 = dist;
            }
        }
    }

    // Facet layer: sharp fracture lines + specular glint from a fake facet normal
    let edge = d2 - d1;
    let edge_line = exp(-edge * edge * mix(60.0, 6.0, m) * edge_glow)
        * (1.0 + u.bass * 1.5 + u.kick * 2.0);
    let fn_ = normalize(vec3f(cvec * 1.5, 1.0));
    let gloss = mix(22.0, 4.0, m);
    let glint = pow(max(dot(fn_, normalize(vec3f(0.4, -0.5, 0.75))), 0.0), gloss);

    let facet_hash = phosphor_hash2(closest_id);
    // Beat-locked glint sweep walking across facets in hash order (crystal only)
    let sweep_d = fract(facet_hash + u.beat_phase) - 0.5;
    let sweep = exp(-sweep_d * sweep_d * 40.0) * 0.3 * (1.0 - m);

    // Cold steel-blue range only — small b amplitudes keep every facet icy
    let ice = phosphor_palette(
        facet_hash * 0.35 + ice_hue,
        vec3f(0.30, 0.42, 0.58), vec3f(0.12, 0.16, 0.28),
        vec3f(1.0, 1.0, 1.0), vec3f(0.62, 0.55, 0.48)
    );
    let edge_col = mix(vec3f(1.0, 0.95, 0.9), vec3f(0.6, 0.85, 1.0), u.centroid);
    let facet_body = ice * (0.30 + 0.65 * glint + sweep);
    let facet_col = facet_body + edge_col * edge_line;
    let facet_lum = dot(facet_body, vec3f(0.2126, 0.7152, 0.0722));

    // Erosion field, shared by the dissolve mask and the sand dunes
    let er = phosphor_fbm2(p * cell_scale * 0.9 + vec2f(t * 0.05, -t * 0.03), 3, 0.5);

    // === GRAIN LAYER (sand) ===
    // bandwidth widens the grain: broad spectrum = coarser, more scattered
    let grain_scale = mix(220.0, 60.0, grain_size) * mix(1.35, 0.65, u.bandwidth);
    let gp = floor(p * grain_scale);
    let g_static = phosphor_hash2(gp);
    // Per-frame shimmer: integer hash is safe for frame-varying identity
    let gu = u32(i32(gp.x) + 4096) * 8192u + u32(i32(gp.y) + 4096);
    let sparkle = f32(phosphor_ihash(gu + phosphor_ihash(u32(u.frame_index)))) / 4294967295.0;
    let gval = mix(g_static, sparkle, 0.15 + 0.55 * zcr_x);
    let dust = vec3f(0.55, 0.47, 0.36) * (0.25 + 0.75 * gval);
    // Sand inherits the crystal's light; a low-frequency field scrolling along
    // the wind direction shades dunes into the grain carpet
    let dune = phosphor_fbm2(p * 2.2 + drift_dir * (t * 0.15), 3, 0.5);
    let dune_shade = mix(0.30, 1.25, smoothstep(0.22, 0.68, dune));
    let dust_col = dust * (0.4 + 0.6 * facet_lum) * dune_shade;

    // === EROSION MASK: patches crumble before centers ===
    let dissolve = smoothstep(er - 0.18, er + 0.18, m * 1.15 - 0.05);
    var col = mix(facet_col, dust_col, dissolve);
    col = phosphor_hue_shift(col, (u.centroid - 0.5) * 0.15);

    // === FEEDBACK: crystal redraws crisp, sand smears along the wind ===
    let turb = vec2f(
        phosphor_noise2(p * 3.0 + vec2f(t * 0.2, 0.0)),
        phosphor_noise2(p * 3.0 + vec2f(4.7, -t * 0.17))
    ) - 0.5;
    let d_off = (drift_dir * (0.0006 + 0.0035 * m) + turb * 0.0015 * m)
        * (drift_speed * 2.0) * (1.0 + u.bass * 0.6);
    let prev = feedback(uv + d_off);
    let decay = mix(0.60, 0.85, m);
    let fb_w = feedback_amount * mix(0.15, 0.55, m);
    var result = mix(col, prev.rgb * decay, fb_w);

    // Onset glint on fracture edges — after the blend so it never accumulates
    result += edge_col * edge_line * u.onset * 0.5 * (1.0 - m);

    return vec4f(min(result, vec3f(1.2)), 1.0);
}
