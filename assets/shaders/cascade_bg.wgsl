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
    // The warp is CONVERGENT — uv' = mix(uv, centre, w) — so n frames compose to
    // 1-(1-w)^n, which is exactly what frame_diffuse returns (#2378). Without it
    // the trail converged twice as far per wall-clock second at 120 fps as at 60.
    // frame_steps() measures identically here (w <= 0.003, where 1-(1-w)^n ~ nw),
    // but this is the form the shape actually has. Exactly 1.0 at 60 fps.
    let warped_uv = clamp(uv + to_center * frame_diffuse(warp_str), vec2f(0.001), vec2f(0.999));
    let prev = feedback(warped_uv);

    let trail = prev.rgb * frame_decay(decay);

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

    // The edge glow is a CONTINUOUS source added every frame, so its gain is a
    // rate and has to track the retention above (#2376).
    //
    // THIS AND THE WARP CORRECTION ABOVE ARE ONE FIX, NOT TWO. Cascade's
    // as-shipped 5.8% brightness spread was a CANCELLATION of two opposing
    // errors: correcting the source gain alone reads 19.2% (and inverts the
    // sign), correcting the advection alone reads 21.9%, and correcting both
    // reads 2.4%. Two earlier attempts landed one half each, measured a
    // regression, and reverted. Do not remove either one on its own.
    let glow = edge_color * edge_glow_param * 0.12 * frame_gain(1.0, decay);

    // --- Beat pulse (disabled) ---
    let flash_color = vec3f(0.0);

    // --- Composite ---
    let result = min(trail + glow + flash_color, vec3f(1.5));
    let alpha = clamp(max(result.r, max(result.g, result.b)) * 2.0, 0.0, 1.0);
    return vec4f(result, alpha);
}
