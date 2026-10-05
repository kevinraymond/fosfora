// Strand — helices drawn with no depth, so the eye cannot tell which way they turn.
//
// Each strand is a helix seen side-on: on every row its points sit at
// x = center + radius * cos(angle), drawn as outlined boxes snapped to a column
// grid. Nothing about a box says whether it is on the near or the far side of
// its helix (same size, same brightness), so the spin direction is ambiguous
// and flips as you watch it, the kinetic depth illusion.
//
// `depth_cue` puts the depth back: far boxes shrink and dim, which settles the
// direction. `resolve` adds cue as the drums come in, so a breakdown is
// ambiguous and the picture locks into one real spin when the beat returns.
// `flip_bars` really reverses the spin every N bars (in the state pass); with no
// cue that reads as one more perceptual flip.
//
// The spin phase and the kick envelope come from strand_state.wgsl.

const TAU: f32 = 6.28318531;
const MAX_INNER: u32 = 6u;
const MAX_POINTS: u32 = 6u;

// One helix point's box: the coverage of its outline at `p`, times its depth-cue brightness.
fn draw_box(
    p: vec2f, x: f32, yc: f32, z: f32, half_h: f32, snap: f32, cue: f32, px: f32, row_h: f32,
) -> f32 {
    let front = z * 0.5 + 0.5;
    let size = mix(1.0, 0.6 + 0.55 * front, cue);
    let bright = mix(1.0, 0.2 + 0.8 * front, cue);

    // The column grid is one undepthed box wide, so neighboring boxes share an edge.
    let cw = half_h * 2.5;
    let xs = mix(x, (floor(x / cw) + 0.5) * cw, snap);

    let hh = min(half_h * size, row_h * 0.5 - px);
    let hw = hh * 1.25;
    let line = max(px * 1.2, hh * 0.22);
    let d = abs(p - vec2f(xs, yc)) - vec2f(hw, hh);
    let e = max(d.x, d.y);
    let a = 1.0 - smoothstep(0.0, px, abs(e + 0.5 * line) - 0.5 * line);
    return a * bright;
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let aspect = u.resolution.x / u.resolution.y;
    let px = 1.0 / u.resolution.y;
    // Height units, origin at the center.
    let p = (frag_coord.xy - 0.5 * u.resolution) * px;

    let n_inner = u32(clamp(floor(param(1u) + 0.5), 0.0, f32(MAX_INNER)));
    let n_points = u32(clamp(floor(param(2u) + 0.5), 1.0, f32(MAX_POINTS)));
    let twist = param(3u) * TAU;
    let rows = clamp(floor(param(4u) + 0.5), 4.0, 200.0);
    let box_size = clamp(param(5u), 0.05, 1.0);
    let snap = clamp(param(6u), 0.0, 1.0);
    let reactivity = param(10u);
    let decay = frame_decay(param(11u));

    let phase = phase_unpack(textureLoad(input0_tex, vec2i(0, 0), 0));
    let drums = textureLoad(input0_tex, vec2i(1, 0), 0).g;
    let cue = clamp(param(7u) + param(8u) * drums, 0.0, 1.0);

    let row_h = 1.0 / rows;
    let j = floor((p.y + 0.5) / row_h);
    let yc = (j + 0.5) * row_h - 0.5;
    let half_h = 0.5 * row_h * box_size;

    let outer_r = min(0.4, 0.46 * aspect) * (1.0 + reactivity * 0.08 * u.bass);
    let inner_span = outer_r * 0.62;

    var v = 0.0;
    for (var i = 0u; i <= n_inner; i++) {
        var center = 0.0;
        var radius = outer_r;
        var turn = 1.0;
        var k = twist;
        if i > 0u {
            let fi = f32(i - 1u);
            let count = f32(n_inner);
            center = inner_span * (2.0 * (fi + 0.5) / count - 1.0);
            radius = inner_span / count * 0.95;
            // Neighbors counter-rotate and twist at slightly different pitches.
            turn = select(-1.0, 1.0, (i % 2u) == 0u);
            k = twist * (1.3 + 0.35 * fract(fi * 0.618));
        }
        let phi = f32(i) * 2.39996;
        let base = TAU * phase * turn + k * yc + phi;
        for (var q = 0u; q < n_points; q++) {
            let a = base + TAU * f32(q) / f32(n_points);
            let x = center + radius * cos(a);
            v = max(v, draw_box(p, x, yc, sin(a), half_h, snap, cue, px, row_h));
        }
    }

    let ink = vec3f(0.93, 0.95, 1.0) * v;
    let prev = feedback(uv) * decay;
    return vec4f(max(prev.rgb, ink), max(prev.a, v));
}
