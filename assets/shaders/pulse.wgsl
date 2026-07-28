// Pulse — Concentric rings expanding outward from center, synced to beat_phase.
// Dark background with sharp bright rings that emanate from the origin.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let aspect = res.x / res.y;
    let p = (uv - 0.5) * vec2f(aspect, 1.0);
    let t = u.time;

    // param(0) = ring_count, param(1) = expansion_speed, param(2) = ring_width
    let ring_count = floor(param(0u) * 6.0 + 2.0); // 2-8 rings
    let max_radius = param(1u) * 1.5 + 0.5;        // how far rings travel
    let ring_width = param(2u) * 0.012 + 0.004;     // thin rings

    // Audio
    let phase = u.beat_phase; // 0-1 sawtooth at detected BPM
    let bass_thick = 1.0 + u.bass * 1.5;

    // Read feedback and decay
    let prev = feedback(uv);
    let decay = 0.82;

    // Radial distance from center
    let r = length(p);

    // Rings expand from center outward, evenly spaced in phase
    var ring_sum = 0.0;
    for (var i = 0; i < 8; i++) {
        if (f32(i) >= ring_count) { break; }
        // Each ring offset in phase so they're evenly distributed
        let ring_phase = fract(phase + f32(i) / ring_count);
        let ring_r = ring_phase * max_radius;
        let thickness = ring_width * bass_thick;

        // Sharp ring: narrow gaussian
        let d = abs(r - ring_r);
        let ring = exp(-(d * d) / (thickness * thickness));

        // Fade out as ring expands (born bright at center, fades at edge)
        let fade = (1.0 - ring_phase) * (1.0 - ring_phase);
        ring_sum += ring * fade;
    }

    // Kick: extra burst ring from center
    let kick_ring_r = u.kick * 0.6;
    let kick_d = abs(r - kick_ring_r);
    ring_sum += exp(-(kick_d * kick_d) / 0.001) * u.kick * 1.5;

    // Onset: thin fast micro-ring
    let onset_r = u.onset * 0.9;
    let onset_d = abs(r - onset_r);
    ring_sum += exp(-(onset_d * onset_d) / 0.0003) * u.onset * 1.0;

    // Clamp ring intensity to prevent blowout
    ring_sum = min(ring_sum, 2.0);

    // Color by radius + time (no angle — avoids atan2 seam)
    let pal_t = r * 2.0 + t * 0.08 + phase * 0.5;
    var col = phosphor_audio_palette(pal_t, u.centroid, 0.0) * ring_sum * 0.7;

    // Beat flash — small bright dot at center
    let center_flash = exp(-r * r * 80.0) * u.beat * 2.0;
    col += phosphor_audio_palette(t * 0.1, u.centroid, 0.2) * center_flash;

    // Feedback blend: new frame on top, old trails fade
    let result = max(col, prev.rgb * decay);
    let new_alpha = clamp(max(col.r, max(col.g, col.b)) * 2.0, 0.0, 1.0);
    let result_alpha = max(new_alpha, prev.a * decay);

    return vec4f(result, result_alpha);
}
