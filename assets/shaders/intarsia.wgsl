// Intarsia — inlay tiles over the content beneath (overlay family,
// backdrop-reactive #2061).
//
// Reads the analysis pass's patch colors; a grid of tiles fills in wherever
// the backdrop actually has content, each tile taking its color from the
// patch it covers. Tiles trade in and out on the bar clock — every tile
// fires once per cycle at its own hash-staggered moment, so the mosaic
// holds a steady density instead of filling and resetting (#2068: cycles
// must not snap). Content-dependent: loop "free". Premultiplied RGBA over
// transparency.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let cols = u32(clamp(param(0u), 6.0, 64.0));
    let rows = u32(clamp(param(1u), 4.0, 48.0));
    let coverage = param(2u);
    let inset = param(3u);
    let boost = param(4u);
    let seed = param(5u);
    let bars = max(param(6u), 1.0);
    let density = clamp(param(7u), 0.05, 0.95);
    let tint = vec3f(param(8u), param(9u), param(10u));

    let cycle = fract((u.bar_index + u.bar_phase) / bars);

    let cell = ovl_cell_id(uv, cols, rows);
    let cuv = ovl_cell_uv(uv, cols, rows);
    let ch = ovl_cell_hash(cell, seed);

    // The patch under this tile, from the analysis pass (premultiplied).
    let grid = vec2f(f32(cols), f32(rows));
    let centre = (floor(uv * grid) + 0.5) / grid;
    let swatch = input0(centre);

    // Tiles exist only where the backdrop has content; `coverage` sets how
    // much presence a patch needs before its tile is eligible.
    let presence = smoothstep(coverage * 0.5, coverage, swatch.a);

    // Steady turnover: each tile fires once per cycle at its hash-staggered
    // moment and stays lit for `density` of the cycle. The wrap carries no
    // snap — a tile's envelope is periodic in the cycle phase by construction.
    let ramp = min(0.12, density * 0.3);
    let on = ovl_trigger(cycle, ch, ramp, max(density - 2.0 * ramp, 0.0), ramp);
    // Fresh tiles land bright and cool off; a drop flashes every live tile.
    let t_on = fract(cycle - ch);
    let arrive = exp(-t_on * 10.0);
    let beat_hit = exp(-u.beat_phase * 6.0);

    // Tile body: an inset filled square with a thin stroke.
    let half = vec2f(0.5 - inset);
    let sd = ovl_rect_sdf(cuv, vec2f(0.5), half);
    let fill = 1.0 - smoothstep(-0.02, 0.02, sd);
    let stroke = ovl_rect_stroke(cuv, vec2f(0.5), half, 0.06);

    let patch_rgb = swatch.rgb / max(swatch.a, 1e-3);
    let colour = clamp(patch_rgb * boost, vec3f(0.0), vec3f(1.6)) * tint
        * (1.0 + 0.4 * beat_hit + 0.5 * arrive + 1.3 * u.drop);

    let body = max(fill * 0.9, stroke);
    let a = clamp(body * on * presence, 0.0, 1.0);
    return vec4f(colour * a, a);
}
