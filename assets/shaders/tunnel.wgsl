// Tunnel — Infinite cylindrical flythrough using log-polar mapping
// Log-polar gives perceptually uniform ring spacing for real motion feel.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let aspect = res.x / res.y;
    let p = (uv - 0.5) * vec2f(aspect, 1.0);
    let t = u.time;

    // param(0) = twist_amount, param(1) = speed, param(2) = tunnel_radius, param(3) = segments
    let twist = (param(0u) - 0.5) * 10.0;
    let speed = param(1u) * 0.8 + 0.08;
    let tun_scale = param(2u) * 2.0 + 1.0;
    let segments = floor(param(3u) * 8.0 + 4.0); // 4-12 wall panels

    // Constant speed — no audio multiplier on t (causes back-and-forth jitter)
    let fly_speed = speed;

    // Polar coordinates — rotate the whole tunnel over time with twist
    let r = length(p);
    let angle = atan2(p.y, p.x) + twist * t * 0.15;

    // Log-polar depth: uniform ring spacing on screen → real motion perception
    let z = -log(r + 0.001) * tun_scale + t * fly_speed;

    // Additional depth-dependent twist on ribs
    let ta = angle + twist * z * 0.05;

    // === Wall texture: asymmetric noise the eye can track ===
    // Seamless angular coord via cos/sin embedding
    let ax = cos(ta);
    let ay = sin(ta);

    // Large-scale wall texture (trackable detail)
    let wall_tex = phosphor_fbm2(vec2f(z * 0.8, ax * 1.5 + ay * 1.5), 4, 0.5);

    // Wall panel edges (angular segments) — wrap angle to 0-1 first to avoid atan2 seam
    let angle_01 = fract(ta / 6.28318); // 0-1, seamless
    let seg_pos = angle_01 * segments;
    let seg_edge = abs(fract(seg_pos) - 0.5) * 2.0;
    let panel_line = exp(-seg_edge * seg_edge * 60.0);

    // Depth ring edges
    let ring_pos = z * 1.5;
    let ring_edge = abs(fract(ring_pos) - 0.5) * 2.0;
    let ring_line = exp(-ring_edge * ring_edge * 60.0);

    // Alternating panel shading (checkerboard)
    let panel_id = floor(seg_pos);
    let ring_id = floor(ring_pos);
    let checker = fract((panel_id + ring_id) * 0.5) * 2.0; // 0 or 1

    // Panel base color — varies per panel for trackable asymmetry
    let panel_hash = phosphor_hash2(vec2f(panel_id, ring_id));
    let pal_t = panel_hash * 0.5 + z * 0.04;
    let panel_col = phosphor_audio_palette(pal_t, u.centroid, 0.0);

    // Depth shading: closer (edge) = brighter, farther (center) = darker
    let depth_shade = smoothstep(0.0, 0.6, r);

    // Compose wall
    let base = (0.08 + checker * 0.05 + wall_tex * 0.12) * depth_shade;
    var col = panel_col * base;

    // Grid lines (panel + ring edges)
    let grid = max(panel_line, ring_line);
    let line_col = mix(vec3f(0.5, 0.7, 1.0), vec3f(1.0, 0.5, 0.3), u.centroid);
    col += line_col * grid * 0.2 * depth_shade * (0.4 + u.mid * 0.6);

    // Audio-reactive glow on ring lines
    col += line_col * ring_line * 0.08 * u.upper_mid * depth_shade;

    // Vanishing point: subtle glow
    let vp_glow = exp(-r * r * 25.0) * 0.08;
    col += phosphor_audio_palette(t * 0.05, u.centroid, 0.1) * vp_glow;

    // Kick flash
    col += vec3f(exp(-r * r * 35.0) * u.kick * 0.4);

    // Tunnel opening: dark at very center
    col *= smoothstep(0.0, 0.06, r);

    // Bass breathe: subtle radius pulse
    col *= 1.0 + u.bass * 0.15;

    // Almond/eye vignette mask — param(4) = pinch_x, param(5) = pinch_y
    let pinch_x = param(4u);
    let pinch_y = param(5u);
    let mx = abs(p.x) / (aspect * 0.5);  // [0, 1] at screen edges
    let my = abs(p.y) * 2.0;              // [0, 1] at screen edges
    // Superellipse exponent: 2.0 = circle, lower = pointier
    let ex = max(2.0 - pinch_x * 1.5, 0.5);
    let ey = max(2.0 - pinch_y * 1.5, 0.5);
    let d = pow(mx, ex) + pow(my, ey);
    // Adaptive radius: covers full screen at pinch=0, tight almond at pinch=1
    let pinch_max = max(pinch_x, pinch_y);
    let mask_r = mix(3.0, 0.85, pinch_max);
    let mask = 1.0 - smoothstep(mask_r - 0.2, mask_r, d);
    col *= mask;

    return vec4f(col, mask);
}
