// Astrolabe — the overlay family's set-piece: a full-frame targeting instrument
// that assembles ring by ring across the cycle.
//
// Phase-locked: assembly and every rotation derive from the bar clock (all
// sweep angles are integer multiples of the cycle phase, so the loop point is
// seamless); buildup accelerates assembly and widens the sweep arms, the beat
// slams the center lock, a drop white-flashes the whole instrument.
// Premultiplied RGBA over a transparent background.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let rings = clamp(param(0u), 2.0, 6.0);
    let size = param(1u);
    let detail = param(2u);
    let lock_strength = param(3u);
    let seed = param(4u);
    let bars = max(param(5u), 1.0);
    let tint = vec3f(param(6u), param(7u), param(8u));

    let asp = u.resolution.x / max(u.resolution.y, 1.0);
    let ws = vec2f(asp, 1.0);
    let p = uv * ws;
    let centre = ws * 0.5;

    let cycle = fract((u.bar_index + u.bar_phase) / bars);
    // Breathing assembly: rings build through the first half of the cycle and
    // retract through the second — a triangle, not a sawtooth, so the cycle
    // boundary (and any loop wrap) carries no reset snap.
    let acc = clamp(cycle * (1.0 + u.buildup * 0.5), 0.0, 1.0);
    let assemble = 1.0 - abs(1.0 - 2.0 * acc);

    // Beat-slammed center lock: hits on every beat, hardest on the "one".
    let beat_hit = exp(-u.beat_phase * 7.0);
    let bar_hit = exp(-u.bar_phase * 9.0);
    let lock = lock_strength * max(beat_hit * 0.7, bar_hit);

    var chrome = 0.0;
    var hot = 0.0; // parts that flash hardest (arms, lock, diamonds)

    let n = u32(rings);
    for (var i = 0u; i < 6u; i++) {
        if i >= n {
            break;
        }
        let fi = f32(i);
        let radius = size * (0.28 + 0.72 * (fi + 1.0) / rings);
        // Ring i assembles in hash-staggered order across the first 70% of the
        // cycle, outermost last.
        let at = (fi / rings) * 0.55 + ovl_stagger(i, seed, 0.12);
        let born = ovl_reveal(at, assemble, 0.06);
        if born < 0.003 {
            continue;
        }

        // The ring, its degree ticks, and one or two counter-rotating sweep arms.
        var ring = ovl_ring(p, centre, radius, 0.004) * 0.5;
        ring = max(ring, ovl_ticks_ring(p, centre, radius, 12.0 + fi * 12.0, size * 0.045, 0.006) * (0.35 + 0.65 * detail));
        // Integer turns per cycle, alternating direction: loop-exact rotation.
        let turns = f32(i + 1u) * select(1.0, -1.0, (i & 1u) == 1u);
        let sweep_w = 0.06 + 0.1 * u.buildup + 0.04 * detail;
        let arm = ovl_arc(p, centre, radius, fract(cycle * turns), sweep_w, 0.012);
        ring = max(ring, arm);
        hot = max(hot, arm * born);
        // A second, dimmer arm opposite the first on the detailed rings.
        if detail > 0.4 {
            ring = max(ring, ovl_arc(p, centre, radius, fract(cycle * turns + 0.5), sweep_w * 0.6, 0.008) * 0.6);
        }

        // Orbiting target diamond: one per ring, position = the arm's leading
        // edge; drawn as a rotated square via the 45°-rotated coordinate trick.
        let ang = fract(cycle * turns) * 6.2831853;
        let dpos = centre + vec2f(cos(ang), sin(ang)) * radius;
        let dq = abs(vec2f(
            (p.x - dpos.x) + (p.y - dpos.y),
            (p.x - dpos.x) - (p.y - dpos.y),
        )) * 0.7071;
        let dia = 1.0 - smoothstep(0.008, 0.012, max(dq.x, dq.y));
        ring = max(ring, dia);
        hot = max(hot, dia * born);

        chrome = max(chrome, ring * born);
    }

    // Radial spokes, born with the instrument's core.
    let core_born = ovl_reveal(0.02, assemble, 0.05);
    let dvec = p - centre;
    let ang8 = fract(atan2(dvec.y, dvec.x) * 0.15915494 * 8.0);
    // Spokes grow outward with the assembly, never past the born rings.
    let spoke_reach = size * (0.3 + 0.7 * assemble);
    let spoke = (1.0 - smoothstep(0.012, 0.03, min(ang8, 1.0 - ang8)))
        * smoothstep(size * 0.2, size * 0.26, length(dvec))
        * (1.0 - smoothstep(spoke_reach * 0.92, spoke_reach, length(dvec)));
    chrome = max(chrome, spoke * 0.3 * core_born * (0.5 + 0.5 * detail));

    // Center lock: crosshair + bracket that slams shut with every beat hit.
    let br = size * 0.2 * (1.5 - 0.6 * lock) * (1.0 - 0.2 * u.drop);
    let cross = ovl_cross(p, centre, size * 0.16, size * 0.03, 0.01);
    let bracket = ovl_bracket(p, centre, vec2f(br), br * 0.6, 0.009);
    chrome = max(chrome, max(cross, bracket) * core_born);
    hot = max(hot, max(cross, bracket) * core_born * lock);

    // Colour: key-locked palette; arms/lock/diamonds run hotter; a drop flashes
    // the whole instrument toward white.
    let base = phosphor_audio_palette(
        phosphor_key_hue(u.key_class, u.key_is_minor),
        u.centroid,
        u.bar_phase,
    ) * tint;
    let energy = 1.0 + 0.6 * beat_hit + 0.5 * lock + 1.8 * u.drop;
    var colour = base * energy + vec3f(1.0) * hot * (0.4 + 0.6 * lock);
    colour = mix(colour, vec3f(1.0), clamp(u.drop * 0.6, 0.0, 0.6));

    let a = clamp(chrome, 0.0, 1.0);
    return vec4f(colour * a, a);
}
