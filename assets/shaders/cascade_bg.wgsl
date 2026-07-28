// Cascade background shader — feedback trails with directional warp and audio-reactive edge glow.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let p = uv * 2.0 - 1.0;

    let decay = param(0u);
    let edge_glow_param = param(5u);
    let beat_sync = param(7u);

    // --- Directional UV warp: perpendicular to nearest edge ---
    // Instead of warping toward center, warp perpendicular to each edge
    // This smears trails inward from each edge, reinforcing the wall flow
    let to_center = vec2f(0.5, 0.5) - uv;
    let warp_str = 0.002 + u.rms * 0.001;
    let warped_uv = clamp(uv + to_center * warp_str, vec2f(0.001), vec2f(0.999));
    let prev = feedback(warped_uv);

    let trail = prev.rgb * decay;

    // --- Audio-reactive edge glow ---
    // Glow width SCALES with audio energy (like Aurora's ribbon width)
    let bass_energy = max(u.bass + u.sub_bass * 0.5, 0.0);
    let mid_energy = max(u.mid, 0.0);
    let high_energy = max(u.centroid, 0.0);

    let d_bottom = 1.0 - uv.y;
    let d_top = uv.y;
    let d_left = uv.x;
    let d_right = 1.0 - uv.x;

    // Glow width grows with audio — small base, expands when band is active
    let base_width = 0.02 + edge_glow_param * 0.03;
    let bottom_width = base_width * (1.0 + bass_energy * 3.0);
    let top_width = base_width * (1.0 + high_energy * 3.0);
    let left_width = base_width * (1.0 + mid_energy * 3.0);
    let right_width = base_width * (1.0 + mid_energy * 3.0);

    let bottom_glow = exp(-d_bottom * d_bottom / (bottom_width * bottom_width)) * bass_energy;
    let top_glow = exp(-d_top * d_top / (top_width * top_width)) * high_energy;
    let left_glow = exp(-d_left * d_left / (left_width * left_width)) * mid_energy;
    let right_glow = exp(-d_right * d_right / (right_width * right_width)) * mid_energy;

    let bottom_color = vec3f(1.0, 0.4, 0.1);
    let left_color = vec3f(0.1, 0.9, 0.6);
    let right_color = vec3f(0.4, 0.3, 1.0);
    let top_color = vec3f(0.7, 0.9, 1.0);

    let edge_color = bottom_color * bottom_glow
                   + left_color * left_glow
                   + right_color * right_glow
                   + top_color * top_glow;

    let glow = edge_color * edge_glow_param * 0.12;

    // --- Beat pulse (disabled) ---
    let flash_color = vec3f(0.0);

    // --- Composite ---
    let result = min(trail + glow + flash_color, vec3f(1.5));
    let alpha = clamp(max(result.r, max(result.g, result.b)) * 2.0, 0.0, 1.0);
    return vec4f(result, alpha);
}
