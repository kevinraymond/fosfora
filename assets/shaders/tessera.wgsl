// Tessera — render-bucket tile reveal (overlay family).
//
// Phase-locked: all structure derives from the bar clock (u.bar_index +
// u.bar_phase); buildup/drop/beat are uniform-block accents (exact-loop-safe).
// Output is premultiplied RGBA over a transparent background.

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

    // Master clock: one breath per bars_per_cycle bars. The reveal OPENS
    // through the first half of the cycle and CLOSES through the second —
    // a triangle, not a sawtooth, so there is no reset snap at the cycle
    // boundary (live, every N bars) or at a loop wrap (the jolt Kevin caught:
    // bit-exact closure is not visual smoothness). A rising build still
    // accelerates the front.
    let cycle = fract((u.bar_index + u.bar_phase) / bars);
    let cyc_acc = clamp(cycle * (1.0 + u.buildup * 0.6), 0.0, 1.0);
    let cyc_eff = 1.0 - abs(1.0 - 2.0 * cyc_acc);

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
    let on = ovl_reveal(order, cyc_eff, softness);

    // Freshly-revealed cells burn bright and cool off as the front moves past —
    // the reveal reads as a wave of fire, not a fade.
    let age = clamp((cyc_eff - order) * 5.0, 0.0, 1.0);
    let ember = (1.0 - age) * on;

    let line = ovl_rect_stroke(cuv, vec2f(0.5), vec2f(0.42), stroke);
    // Coarse super-grid over the fine one: structure at two scales. Its stroke
    // is in SUPER-cell-local units — 8x the screen distance of the fine grid —
    // so the numeric thickness must be much smaller to read as a hairline.
    let super_cols = max(cols / 8u, 2u);
    let super_rows = max(rows / 8u, 2u);
    let super_line = ovl_rect_stroke(
        ovl_cell_uv(uv, super_cols, super_rows),
        vec2f(0.5),
        vec2f(0.485),
        stroke * 0.12,
    );

    // Per-beat hit decays across the beat; downbeat lands harder; a detected
    // drop strobes the whole field for its frames.
    let beat_hit = exp(-u.beat_phase * 6.0);
    let bar_hit = exp(-u.bar_phase * 8.0);
    let energy = 1.0 + 0.5 * beat_hit + 0.4 * bar_hit + 1.6 * u.drop;
    // Narrow per-cell hue spread around the song's key: variation without the
    // full-spectrum confetti the raw hash produced.
    let hue = phosphor_key_hue(u.key_class, u.key_is_minor) + (ch - 0.5) * 0.22;
    let colour = phosphor_audio_palette(hue, u.centroid, u.bar_phase) * tint * energy;

    var rgb: vec3f;
    var a: f32;
    if punch > 0.5 {
        // Scrim covers the frame; revealed tiles knock through it. Strokes trace
        // the still-covered tiles; embers glow in the freshly-opened ones. On a
        // drop the scrim itself blinks thinner — the frame gasps open.
        let cover = 1.0 - on;
        let chrome = max(line, super_line * 0.6) * cover;
        let scrim_eff = scrim * (1.0 - 0.5 * u.drop);
        a = clamp(max(scrim_eff * cover, max(chrome, ember * 0.55)), 0.0, 1.0);
        rgb = colour * max(chrome, ember * 0.8);
    } else {
        // Transparent field; revealed tiles draw chrome + ember fill.
        let chrome = max(line, super_line * 0.6) * on;
        a = clamp(max(chrome, ember * 0.5), 0.0, 1.0);
        rgb = colour * max(chrome, ember * 0.7);
    }
    return vec4f(rgb * a, a);
}
