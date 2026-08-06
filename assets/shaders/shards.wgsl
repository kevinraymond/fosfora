// Shards — Animated Voronoi cells with glowing fracture edges
// Geometric, crisp cells with audio-reactive edges and shattering.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let aspect = res.x / res.y;
    let p = (uv - 0.5) * vec2f(aspect, 1.0);
    let t = u.time;

    // param(0) = cell_scale, param(1) = edge_glow, param(2) = fill_amount, param(3) = saturation
    let cell_scale = param(0u) * 8.0 + 3.0;
    let edge_glow = param(1u) * 15.0 + 0.5;
    let fill_amount = param(2u);
    let saturation = param(3u) * 2.0; // 0 = grayscale, 1 = normal, 2 = vivid

    // Onset shatters: offset cell centers
    let shatter = u.onset * 0.3;

    // Voronoi
    let sp = p * cell_scale;
    let ip = floor(sp);
    let fp = fract(sp);

    var d1 = 8.0;  // closest distance
    var d2 = 8.0;  // second closest
    var closest_id = vec2f(0.0);

    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let neighbor = vec2f(f32(x), f32(y));
            let cell_id = ip + neighbor;

            // Animated cell center
            let h = fosfora_hash2(cell_id) * 6.283 + t * 0.3;
            let h2 = fosfora_hash2(cell_id + vec2f(7.13, 3.71));
            var center = neighbor + 0.5 + 0.4 * vec2f(sin(h), cos(h * 1.3 + h2));

            // Shatter displacement
            center += vec2f(
                sin(h2 * 10.0 + t * 3.0),
                cos(h2 * 13.0 + t * 2.5)
            ) * shatter;

            let diff = center - fp;
            let dist = length(diff);

            if (dist < d1) {
                d2 = d1;
                d1 = dist;
                closest_id = cell_id;
            } else if (dist < d2) {
                d2 = dist;
            }
        }
    }

    // Edge detection: difference between closest and second closest
    let edge = d2 - d1;
    let edge_line = exp(-edge * edge * 40.0 * edge_glow) * (1.0 + u.bass * 2.0 + u.kick * 3.0);

    // Cell fill with audio color
    let cell_hash = fosfora_hash2(closest_id);
    let pal_t = cell_hash + t * 0.1 + u.beat_phase * 0.5;
    let cell_col = fosfora_audio_palette(pal_t, u.centroid, 0.0);

    // Fill: stained-glass style — flat color across entire cell, modulated by fill_amount
    let fill_bright = fill_amount * (0.6 + u.rms * 0.3 + u.kick * 0.3);
    let fill = cell_col * fill_bright;

    // Edge color: bright white/blue shifted by centroid
    let edge_col = mix(vec3f(1.0, 0.9, 0.8), vec3f(0.5, 0.8, 1.0), u.centroid) * edge_line;

    // Beat flash on edges
    let beat_flash = u.beat * 1.5;
    var result = fill + edge_col * (1.0 + beat_flash);

    // Saturation: mix with luminance
    let lum = dot(result, vec3f(0.2126, 0.7152, 0.0722));
    result = mix(vec3f(lum), result, saturation);

    return vec4f(result, 1.0);
}
