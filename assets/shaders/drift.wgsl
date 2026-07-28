// Drift — Fluid smoke via triple domain-warped FBM noise + feedback
// Creates organic flowing forms with advected feedback creating real fluid motion.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let aspect = res.x / res.y;
    let p = (uv - 0.5) * vec2f(aspect, 1.0);
    let t = u.time;

    // param(0) = warp_intensity, param(1) = flow_speed, param(2) = color_mode, param(3) = density
    let warp_intensity = param(0u) * 2.0 + 0.5;
    let flow_speed = param(1u) * 0.8 + 0.1;
    let color_mode = param(2u);
    let density = param(3u) * 1.5 + 0.5;

    // Audio modulation
    let bass_warp = (u.sub_bass + u.bass) * 0.5 * warp_intensity;
    let flow_t = t * flow_speed;

    // Triple domain warping for fluid look
    let q = vec2f(
        phosphor_fbm2(p * 2.0 + vec2f(0.0, flow_t * 0.3), 5, 0.5),
        phosphor_fbm2(p * 2.0 + vec2f(5.2, flow_t * 0.4 + 1.3), 5, 0.5)
    );

    let r = vec2f(
        phosphor_fbm2((p + q * (1.0 + bass_warp)) * 2.0 + vec2f(1.7, flow_t * 0.2 + 9.2), 5, 0.5),
        phosphor_fbm2((p + q * (1.0 + bass_warp)) * 2.0 + vec2f(8.3, flow_t * 0.25 + 2.8), 5, 0.5)
    );

    let s = vec2f(
        phosphor_fbm2((p + r * 1.5) * 1.5 + vec2f(flow_t * 0.15, 3.1), 4, 0.5),
        phosphor_fbm2((p + r * 1.5) * 1.5 + vec2f(2.7, flow_t * 0.18), 4, 0.5)
    );

    let f = phosphor_fbm2((p + s * (1.2 + bass_warp * 0.5)) * density, 6, 0.5);

    // Color from centroid-driven palette — use f² to keep darks dark
    let pal_t = f * 2.0 + t * 0.05;
    var col = phosphor_audio_palette(pal_t, u.centroid, color_mode * 0.5);
    col *= f * f * 1.8;

    // Read feedback with advection
    let advect_offset = (s - 0.5) * 0.003 * (1.0 + u.bass);
    let prev_advected = feedback(uv + advect_offset);

    // Blend: mix (not max) so dark areas can reclaim space
    let decay = 0.78;
    let result = mix(col, prev_advected.rgb * decay, 0.5);

    // Clamp to prevent blowout
    return vec4f(min(result, vec3f(1.2)), 1.0);
}
