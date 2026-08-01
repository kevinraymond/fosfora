// Limn — living edge-tracer (overlay family, backdrop-reactive #2061).
//
// Samples `@backdrop` (the composite of every layer beneath this one,
// premultiplied) and outlines it with a Sobel edge field; dashes march along
// the contours on the bar clock. Content-dependent by design: loop "free",
// outside the exact-export gate. Premultiplied RGBA over transparency.

fn limn_lum(c: vec4f) -> f32 {
    return dot(c.rgb, vec3f(0.299, 0.587, 0.114));
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let edge_gain = param(0u);
    let thickness = param(1u);
    let dash_density = param(2u);
    let inherit = param(3u);
    let glow = param(4u);
    let bars = max(param(5u), 1.0);
    let tint = vec3f(param(6u), param(7u), param(8u));

    // Beats fatten the stroke for a breath; the bar clock marches the dashes.
    let beat_hit = exp(-u.beat_phase * 5.0);
    let ts = (thickness * (1.0 + 0.4 * beat_hit)) / u.resolution;
    let march = (u.bar_index + u.bar_phase) / bars;

    // 3x3 Sobel on backdrop luminance.
    let tl = limn_lum(input0(uv + vec2f(-ts.x, -ts.y)));
    let tc = limn_lum(input0(uv + vec2f(0.0, -ts.y)));
    let tr = limn_lum(input0(uv + vec2f(ts.x, -ts.y)));
    let ml = limn_lum(input0(uv + vec2f(-ts.x, 0.0)));
    let mr = limn_lum(input0(uv + vec2f(ts.x, 0.0)));
    let bl = limn_lum(input0(uv + vec2f(-ts.x, ts.y)));
    let bc = limn_lum(input0(uv + vec2f(0.0, ts.y)));
    let br = limn_lum(input0(uv + vec2f(ts.x, ts.y)));
    let gx = (tr + 2.0 * mr + br) - (tl + 2.0 * ml + bl);
    let gy = (bl + 2.0 * bc + br) - (tl + 2.0 * tc + tr);
    let grad = vec2f(gx, gy);
    let mag = length(grad) * edge_gain;
    let edge = smoothstep(0.15, 0.55, mag);

    // Dashes travel PERPENDICULAR to the gradient — i.e. along the contour —
    // so the outline reads as circulating, not flickering.
    let along = dot(uv, normalize(vec2f(-grad.y, grad.x) + vec2f(1e-5)));
    var dash = 1.0;
    if dash_density > 1.0 {
        dash = 0.45 + 0.55 * step(0.4, fract(along * dash_density + march * 2.0));
    }

    // Colour: the backdrop's own (un-premultiplied) local colour, or the palette.
    let centre = input0(uv);
    let local_rgb = centre.rgb / max(centre.a, 1e-3);
    let pal = phosphor_audio_palette(
        phosphor_key_hue(u.key_class, u.key_is_minor),
        u.centroid,
        u.bar_phase,
    );
    let colour = mix(pal, clamp(local_rgb * 1.4, vec3f(0.0), vec3f(1.5)), step(0.5, inherit))
        * tint
        * (1.0 + 0.6 * beat_hit + 1.4 * u.drop);

    let a = clamp(edge * dash + mag * glow * 0.15, 0.0, 1.0);
    return vec4f(colour * a, a);
}
