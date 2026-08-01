// Bezel — border chrome (overlay family).
//
// Phase-locked: the scanline drift loops exactly once per bars_per_cycle bars;
// the beat only pulses thickness/brightness. Premultiplied RGBA over a
// transparent background.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let thickness = param(0u);
    let corner = param(1u);
    let ticks = param(2u);
    let scanline_alpha = param(3u);
    let pulse = param(4u);
    let seed = param(5u);
    let bars = max(param(6u), 1.0);
    let tint = vec3f(param(7u), param(8u), param(9u));

    let cycle = fract((u.bar_index + u.bar_phase) / bars);

    let asp = u.resolution.x / max(u.resolution.y, 1.0);
    let ws = vec2f(asp, 1.0);
    let p = uv * ws;
    let centre = ws * 0.5;

    // Subtle decaying pulse across each beat.
    let beat_env = 1.0 + pulse * 0.5 * (1.0 - u.beat_phase);
    let th = thickness * beat_env;

    // Outer corner brackets + a finer inset rule.
    let outer_half = centre - vec2f(0.035);
    let inner_half = centre - vec2f(0.06);
    var chrome = ovl_bracket(p, centre, outer_half, corner, th);
    chrome = max(chrome, ovl_rect_stroke(p, centre, inner_half, th * 0.35) * 0.55);

    // Tick marks along the top and bottom inset rules, one seeded gap pattern.
    if ticks >= 1.0 {
        let tx = fract(uv.x * ticks);
        let tick_col = floor(uv.x * ticks);
        let keep = step(0.15, ovl_cell_hash(u32(tick_col), seed)); // a few gaps
        let tick_band = step(0.86, tx) * keep;
        let near_rule = step(abs(p.y - (centre.y - inner_half.y)), 0.014)
            + step(abs(p.y - (centre.y + inner_half.y)), 0.014);
        chrome = max(chrome, tick_band * clamp(near_rule, 0.0, 1.0) * 0.8);
    }

    // Scanlines drifting on the bar cycle — integer line-shift per cycle, so the
    // loop point is seamless.
    let sl = scanline_alpha * step(0.65, fract(uv.y * 90.0 + cycle * 3.0));

    let colour = phosphor_audio_palette(
        phosphor_key_hue(u.key_class, u.key_is_minor),
        u.centroid,
        u.bar_phase,
    ) * tint * beat_env;

    let a = clamp(max(chrome, sl), 0.0, 1.0);
    let rgb = colour * max(chrome, sl * 0.6);
    return vec4f(rgb * a, a);
}
