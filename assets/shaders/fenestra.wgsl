// Fenestra — staggered GUI panels snapping into place (overlay family).
//
// Phase-locked: panel placement re-rolls per cycle from (index, cycle_index,
// seed) hashes; entrances stagger across the cycle via ovl_trigger. Premultiplied
// RGBA over a transparent background.

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

    let bar_clock = (u.bar_index + u.bar_phase) / bars;
    let cycle = fract(bar_clock);
    let cyc_idx = u32(floor(bar_clock));

    // Aspect-corrected space so panels are true rectangles.
    let asp = u.resolution.x / max(u.resolution.y, 1.0);
    let ws = vec2f(asp, 1.0);
    let p = uv * ws;

    var rgb = vec3f(0.0);
    var a = 0.0;
    for (var i = 0u; i < 24u; i++) {
        if f32(i) >= count {
            break;
        }
        // Per-panel, per-cycle hashes: position, size, colour, entrance offset.
        let key = i * 613u + cyc_idx * 2749u;
        let hx = ovl_cell_hash(key, seed);
        let hy = ovl_cell_hash(key + 97u, seed);
        let hw = ovl_cell_hash(key + 193u, seed);
        let hh = ovl_cell_hash(key + 389u, seed);
        let centre = (vec2f(margin) + vec2f(hx, hy) * (1.0 - 2.0 * margin)) * ws;
        let half = vec2f(0.05 + hw * 0.14, 0.03 + hh * 0.09);

        // Snap in staggered across the first 60% of the cycle; release before the
        // wrap so the loop point is clean.
        let at = ovl_stagger(i + cyc_idx * 31u, seed, 0.6);
        let attack = mix(0.1, 0.008, snap);
        let env = ovl_trigger(cycle, at, attack, 0.28, 0.1);

        let arm = min(half.x, half.y) * 0.7;
        let chrome = max(
            ovl_bracket(p, centre, half, arm, 0.008),
            ovl_rect_stroke(p, centre, half, 0.004) * 0.6,
        );
        let inside = 1.0 - smoothstep(-0.002, 0.002, ovl_rect_sdf(p, centre, half));

        // The tint-switch trick: white on arrival, decaying into palette colour.
        let age = fract(cycle - at);
        let flash_w = flash * exp(-age * 24.0);
        let colour = mix(
            phosphor_audio_palette(ovl_cell_hash(key + 577u, seed), u.centroid, u.bar_phase) * tint,
            vec3f(1.0),
            clamp(flash_w, 0.0, 1.0),
        );

        let pa = env * clamp(chrome + inside * scrim, 0.0, 1.0);
        rgb = max(rgb, colour * env * clamp(chrome + inside * scrim * 0.5, 0.0, 1.0));
        a = max(a, pa);
    }
    a = clamp(a, 0.0, 1.0);
    // Beat accents brightness only — coverage stays phase-derived.
    rgb *= 1.0 + 0.3 * u.beat;
    return vec4f(rgb * a, a);
}
