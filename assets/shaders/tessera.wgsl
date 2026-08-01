// Tessera — render-bucket tile reveal (overlay family).
//
// Phase-locked: all motion derives from the bar clock (u.bar_index + u.bar_phase);
// audio may only accent brightness. Output is premultiplied RGBA over a
// transparent background.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let cols = u32(clamp(param(0u), 4.0, 64.0));
    let rows = u32(clamp(param(1u), 4.0, 48.0));
    let softness = param(2u);
    let scan_mode = param(3u);
    let punch = param(4u);
    let stroke = param(5u);
    let scrim = param(6u);
    let seed = param(7u);
    let bars = max(param(8u), 1.0);
    let tint = vec3f(param(9u), param(10u), param(11u));

    // Master clock: one reveal per bars_per_cycle bars, exact at any tempo.
    let cycle = fract((u.bar_index + u.bar_phase) / bars);

    let cell = ovl_cell_id(uv, cols, rows);
    let cuv = ovl_cell_uv(uv, cols, rows);
    let ch = ovl_cell_hash(cell, seed);

    // Reveal order: hash scatter (0), row sweep (1), center-out (2). The hash
    // dither on the sweeping modes keeps the front from reading as a hard line.
    let grid = vec2f(f32(cols), f32(rows));
    let centre = (floor(uv * grid) + 0.5) / grid;
    var order = ch;
    if scan_mode >= 1.5 {
        order = clamp(distance(centre, vec2f(0.5)) * 1.9 + (ch - 0.5) * 0.08, 0.0, 0.999);
    } else if scan_mode >= 0.5 {
        order = clamp(centre.y * 0.94 + (ch - 0.5) * 0.06, 0.0, 0.999);
    }
    let on = ovl_reveal(order, cycle, softness);

    let line = ovl_rect_stroke(cuv, vec2f(0.5), vec2f(0.42), stroke);
    let colour = phosphor_audio_palette(ch, u.centroid, u.bar_phase) * tint * (1.0 + 0.5 * u.beat);

    var rgb: vec3f;
    var a: f32;
    if punch > 0.5 {
        // Scrim covers the frame; revealed tiles knock through it. Strokes trace
        // the still-covered tiles so the grid reads before it opens.
        let cover = 1.0 - on;
        let chrome = line * cover;
        a = clamp(max(scrim * cover, chrome), 0.0, 1.0);
        rgb = colour * chrome;
    } else {
        // Transparent field; revealed tiles draw their chrome.
        let chrome = line * on;
        a = clamp(chrome, 0.0, 1.0);
        rgb = colour * chrome;
    }
    return vec4f(rgb * a, a);
}
