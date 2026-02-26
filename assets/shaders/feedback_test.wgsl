// Feedback test effect — spinning dot that leaves fading trails.
// Demonstrates prev_frame / feedback() function.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let aspect = u.resolution.x / u.resolution.y;
    let centered = (uv - 0.5) * vec2f(aspect, 1.0);

    let t = u.time;
    // param(0) = trail_length slider (0.0 to 0.99)
    let decay = param(0u);

    // Read previous frame and fade it
    var prev = feedback(uv);
    prev *= decay;

    // Spinning dot
    let speed = 1.5 + u.bass * 2.0;
    let base_radius = param(1u) * 0.55 + 0.05;  // maps 0-1 → 0.05-0.6
    let radius = base_radius + u.mid * 0.15;
    let dot_pos = vec2f(cos(t * speed), sin(t * speed)) * radius;
    let dist = length(centered - dot_pos);

    // Dot glow with audio-reactive size and color
    let dot_size = 0.03 + u.rms * 0.02;
    let glow = exp(-dist * dist / (dot_size * dot_size));

    // Color cycle based on time and audio
    let hue = t * 0.3 + u.centroid;
    let col = vec3f(
        0.5 + 0.5 * cos(hue * 6.28),
        0.5 + 0.5 * cos((hue + 0.33) * 6.28),
        0.5 + 0.5 * cos((hue + 0.67) * 6.28),
    );

    // Combine: faded previous frame + new dot
    let new_color = col * glow * (1.5 + u.onset * 3.0);
    let result = max(prev.rgb, new_color);
    let new_alpha = clamp(max(new_color.r, max(new_color.g, new_color.b)) * 2.0, 0.0, 1.0);
    let result_alpha = max(prev.a * decay, new_alpha);

    return vec4f(result, result_alpha);
}
