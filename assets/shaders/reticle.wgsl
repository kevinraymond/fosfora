// Reticle — crosshair targeting (overlay family).
//
// Phase-locked: targets re-roll from hash(k, u.bar_index, seed) — this is the
// effect the monotonic bar counter exists for. Premultiplied RGBA over a
// transparent background.

fn reticle_target(k: u32, bar: u32, seed: f32, ws: vec2f) -> vec2f {
    let key = k * 977u + bar * 131u;
    let hx = ovl_cell_hash(key, seed);
    let hy = ovl_cell_hash(key + 41u, seed);
    return (vec2f(0.12) + vec2f(hx, hy) * 0.76) * ws;
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let count = clamp(param(0u), 1.0, 8.0);
    let size = param(1u);
    let snap_move = param(2u);
    let lock_flash = param(3u);
    let seed = param(4u);
    let bars = max(param(5u), 1.0);
    let tint = vec3f(param(6u), param(7u), param(8u));

    let asp = u.resolution.x / max(u.resolution.y, 1.0);
    let ws = vec2f(asp, 1.0);
    let p = uv * ws;

    // bars_per_cycle re-seeds the whole constellation each cycle; within a cycle
    // every reticle takes one target per bar.
    let bar = u32(u.bar_index) + u32(floor((u.bar_index + u.bar_phase) / bars)) * 8191u;

    var rgb = vec3f(0.0);
    var a = 0.0;
    for (var k = 0u; k < 8u; k++) {
        if f32(k) >= count {
            break;
        }
        let cur = reticle_target(k, bar, seed, ws);
        let nxt = reticle_target(k, bar + 1u, seed, ws);
        // Glide covers the last quarter of the bar; snap teleports on the "one".
        let glide = smoothstep(0.75, 1.0, u.bar_phase);
        let pos = select(mix(cur, nxt, glide), cur, snap_move > 0.5);

        // Lock-on: the bracket contracts onto the crosshair through the beat.
        let lock = ovl_trigger(u.beat_phase, 0.0, 0.3, 0.4, 0.3);
        let br_half = size * (1.6 - 0.6 * lock);
        let chrome = max(
            ovl_cross(p, pos, size * 0.9, size * 0.16, 0.01),
            ovl_bracket(p, pos, vec2f(br_half), br_half * 0.55, 0.007),
        );

        let hue = ovl_cell_hash(k * 577u, seed);
        let colour = phosphor_audio_palette(hue, u.centroid, u.bar_phase) * tint
            * (1.0 + lock_flash * u.beat + 0.4 * lock);

        rgb = max(rgb, colour * chrome);
        a = max(a, chrome);
    }
    a = clamp(a, 0.0, 1.0);
    return vec4f(rgb * a, a);
}
