// Intarsia — inlay tiles over the content beneath (overlay family,
// backdrop-reactive #2061).
//
// Reads the analysis pass's patch colors; a grid of tiles fills in wherever
// the backdrop actually has content, revealed in hash order on the bar clock,
// each tile taking its color from the patch it covers. Content-dependent:
// loop "free". Premultiplied RGBA over transparency.

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
    let tint = vec3f(param(7u), param(8u), param(9u));

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

    // Beat-locked reveal in hash order; a drop flashes every live tile.
    let on = ovl_reveal(ch, cycle, 0.12);
    let beat_hit = exp(-u.beat_phase * 6.0);

    // Tile body: an inset filled square with a thin stroke.
    let half = vec2f(0.5 - inset);
    let sd = ovl_rect_sdf(cuv, vec2f(0.5), half);
    let fill = 1.0 - smoothstep(-0.02, 0.02, sd);
    let stroke = ovl_rect_stroke(cuv, vec2f(0.5), half, 0.06);

    let patch_rgb = swatch.rgb / max(swatch.a, 1e-3);
    let colour = clamp(patch_rgb * boost, vec3f(0.0), vec3f(1.6)) * tint
        * (1.0 + 0.4 * beat_hit + 1.3 * u.drop);

    let body = max(fill * 0.9, stroke);
    let a = clamp(body * on * presence, 0.0, 1.0);
    return vec4f(colour * a, a);
}
