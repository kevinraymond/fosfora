// Flux history (#1482 Chronoflow) — advected smoke trails. The old flux_bg
// feedback smear is replaced by real motion-field advection, with the same
// noise warp folded into the advection so the smoky character survives.
//   feedback() = last frame's final image (bg + trails + particles)
//   input0     = chronoflow velocity field (uv-space, per second)
//   input1     = background pass (ambient glow, this frame)
// Params: 0 trail_decay (exposure), 1 flow_intensity (warp), 4 beat_snap,
// 5 flow_stretch.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(prev_frame));
    let uv = frag_coord.xy / dims;
    let p = uv * 2.0 - 1.0;
    let dt = clamp(u.delta_time, 1e-4, 0.05);

    // Flux's signature smoke warp, now applied to the advection lookup.
    let warp_str = 0.002 + param(1u) * 0.003 + u.bass * 0.002;
    let warp_x = phosphor_noise2(p * 3.0 + vec2f(u.time * 0.1, 0.0)) - 0.5;
    let warp_y = phosphor_noise2(p * 3.0 + vec2f(0.0, u.time * 0.08)) - 0.5;
    let warp = vec2f(warp_x, warp_y) * warp_str;

    let stretch = 0.5 + param(5u) * 2.5;
    let v = input0(uv).xy * stretch;
    let hist = feedback(uv - v * dt + warp).rgb;

    // Signed trail over the ambient background: re-adding bg each frame would
    // integrate to blowout, so only the deviation from bg is retained.
    // trail_decay is a RAW per-frame decay here (its 0.5..0.88 range predates
    // Chronoflow): 2M screen-filling additive particles saturate under the
    // remapped exposure curve.
    let bg = input1(uv).rgb;
    let keep = chrono_keep_direct(param(0u) + u.rms * 0.02, param(4u));
    let col = clamp(bg + (hist - bg) * keep, vec3f(0.0), vec3f(1.5));

    let alpha = clamp(max(col.r, max(col.g, col.b)) * 2.0, 0.0, 1.0);
    return vec4f(col, alpha);
}
