// Fenestra — staggered GUI panels snapping into place (overlay family).
//
// Phase-locked: panel placement re-rolls per cycle from (index, cycle_index,
// seed) hashes; entrances stagger across the cycle via ovl_trigger. Buildup
// sharpens the snaps, a drop flashes the whole array white. Premultiplied RGBA
// over a transparent background.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let count = clamp(param(0u), 1.0, 24.0);
    let margin = param(1u);
    let scrim = param(2u);
    let flash = param(3u);
    let snap = param(4u);
    let seed = param(5u);
    let bars = max(param(6u), 1.0);
    let tint = vec3f(param(7u), param(8u), param(9u));

    // Loop contract (#2063): counters are consumed ONLY through cycle
    // arithmetic — the constellation repeats every bars_per_cycle bars, so a
    // loop of that length closes exactly. Variety comes from `seed`, not from
    // a monotonic re-roll (which made the visual period infinite).
    let cycle = fract((u.bar_index + u.bar_phase) / bars);

    // Aspect-corrected space so panels are true rectangles.
    let asp = u.resolution.x / max(u.resolution.y, 1.0);
    let ws = vec2f(asp, 1.0);
    let p = uv * ws;

    // Builds tighten the attack; the drop whites out every live panel.
    let snap_eff = clamp(snap + u.buildup * 0.3, 0.0, 1.0);
    let beat_hit = exp(-u.beat_phase * 6.0);

    var rgb = vec3f(0.0);
    var a = 0.0;
    var prev_centre = vec2f(-1.0);
    var prev_env = 0.0;
    for (var i = 0u; i < 24u; i++) {
        if f32(i) >= count {
            break;
        }
        // Per-panel hashes: position, size, colour, entrance offset.
        let key = i * 613u;
        let hx = ovl_cell_hash(key, seed);
        let hy = ovl_cell_hash(key + 97u, seed);
        let hw = ovl_cell_hash(key + 193u, seed);
        let hh = ovl_cell_hash(key + 389u, seed);
        let centre = (vec2f(margin) + vec2f(hx, hy) * (1.0 - 2.0 * margin)) * ws;
        let half = vec2f(0.055 + hw * 0.16, 0.035 + hh * 0.1);

        // Snap in staggered across the first 60% of the cycle; release before the
        // wrap so the loop point is clean.
        let at = ovl_stagger(i, seed, 0.6);
        let attack = mix(0.1, 0.006, snap_eff);
        let env = ovl_trigger(cycle, at, attack, 0.28, 0.1);

        let arm = min(half.x, half.y) * 0.7;
        var chrome = max(
            ovl_bracket(p, centre, half, arm, 0.009),
            ovl_rect_stroke(p, centre, half, 0.0045) * 0.65,
        );
        // Header bar: a solid strip along the panel's top edge — GUI anatomy.
        let local = (uv * ws - (centre - half)) / (2.0 * half);
        let in_header = step(0.0, local.x) * step(local.x, 1.0)
            * step(0.0, local.y) * step(local.y, 0.14);
        let inside = 1.0 - smoothstep(-0.002, 0.002, ovl_rect_sdf(p, centre, half));

        // Connector to the previous panel: the array reads as one system.
        var link = 0.0;
        if prev_centre.x >= 0.0 {
            link = ovl_segment(p, prev_centre, centre, 0.0025) * min(env, prev_env) * 0.5;
        }
        prev_centre = centre;
        prev_env = env;

        // The tint-switch trick: white on arrival, decaying into palette colour;
        // a drop re-whites everything at once.
        let age = fract(cycle - at);
        let flash_w = clamp(flash * exp(-age * 24.0) + u.drop, 0.0, 1.0);
        let colour = mix(
            phosphor_audio_palette(ovl_cell_hash(key + 577u, seed), u.centroid, u.bar_phase) * tint,
            vec3f(1.0),
            flash_w,
        );

        let shape = clamp(chrome + in_header * inside * 0.85 + inside * scrim, 0.0, 1.0);
        let pa = max(env * shape, link);
        rgb = max(rgb, colour * max(env * clamp(chrome + in_header * inside * 0.7 + inside * scrim * 0.5, 0.0, 1.0), link));
        a = max(a, pa);
    }
    a = clamp(a, 0.0, 1.0);
    // Beat accents brightness only — coverage stays phase-derived.
    rgb *= 1.0 + 0.45 * beat_hit + 1.2 * u.drop;
    return vec4f(rgb * a, a);
}
