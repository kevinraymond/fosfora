// Bezel — border chrome (overlay family).
//
// Phase-locked: the scanline drift loops exactly once per bars_per_cycle bars;
// beats pulse thickness/brightness, band energies drive the corner meters, and
// a drop throws the whole frame into alert. Premultiplied RGBA over a
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

    // Per-beat decay pulse, harder on the "one"; a drop surges the chrome.
    let beat_env = 1.0 + pulse * (0.6 * exp(-u.beat_phase * 5.0) + 0.5 * exp(-u.bar_phase * 8.0));
    let th = thickness * beat_env * (1.0 + 0.8 * u.drop);

    // Outer corner brackets + a finer inset rule.
    let outer_half = centre - vec2f(0.035);
    let inner_half = centre - vec2f(0.06);
    var chrome = ovl_bracket(p, centre, outer_half, corner, th);
    chrome = max(chrome, ovl_rect_stroke(p, centre, inner_half, th * 0.35) * 0.55);
    // Side rules: short vertical strokes at the left/right mid-edges.
    let side_len = inner_half.y * 0.42;
    let lx = centre.x - inner_half.x;
    let rx = centre.x + inner_half.x;
    chrome = max(
        chrome,
        max(
            ovl_segment(p, vec2f(lx, centre.y - side_len), vec2f(lx, centre.y + side_len), th * 0.9),
            ovl_segment(p, vec2f(rx, centre.y - side_len), vec2f(rx, centre.y + side_len), th * 0.9),
        ) * 0.7,
    );

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

    // Corner data-blocks: four mini bar-meters per corner riding the band
    // energies — instrumentation that visibly listens.
    let bands = array<f32, 4>(u.sub_bass, u.low_mid, u.upper_mid, u.brilliance);
    var meters = 0.0;
    let mcorner = vec2f(0.075, 0.075);
    for (var c = 0u; c < 4u; c++) {
        let cpos = vec2f(
            select(mcorner.x, ws.x - mcorner.x, (c & 1u) == 1u),
            select(mcorner.y, ws.y - mcorner.y, (c & 2u) == 2u),
        );
        let q = p - cpos;
        for (var b = 0u; b < 4u; b++) {
            let bx = (f32(b) - 1.5) * 0.02;
            let height = 0.01 + bands[b] * 0.05;
            let bar = step(abs(q.x - bx), 0.0075) * step(abs(q.y), height);
            meters = max(meters, bar * (0.5 + 0.5 * bands[b]));
        }
    }
    chrome = max(chrome, meters);

    // Top-center arc: sweeps exactly once per cycle.
    chrome = max(chrome, ovl_arc(p, vec2f(centre.x, 0.055), 0.04, cycle, 0.35, th * 0.6) * 0.7);

    // Scanlines drifting on the bar cycle — integer line-shift per cycle, so the
    // loop point is seamless. Builds thicken the field with a second, faster set.
    let sl_base = scanline_alpha * step(0.65, fract(uv.y * 90.0 + cycle * 3.0));
    let sl_build = scanline_alpha * u.buildup * step(0.75, fract(uv.y * 150.0 + cycle * 6.0));
    let sl = clamp(sl_base + sl_build, 0.0, 0.6);

    // Alert tint on the drop: the chrome flips toward hot white-red.
    let base_colour = phosphor_audio_palette(
        phosphor_key_hue(u.key_class, u.key_is_minor),
        u.centroid,
        u.bar_phase,
    ) * tint;
    let colour = mix(base_colour, vec3f(1.0, 0.35, 0.28), clamp(u.drop, 0.0, 1.0) * 0.8) * beat_env;

    let a = clamp(max(chrome, sl), 0.0, 1.0);
    let rgb = colour * max(chrome, sl * 0.6);
    return vec4f(rgb * a, a);
}
