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
    let warp_x = fosfora_noise2(p * 3.0 + vec2f(u.time * 0.1, 0.0)) - 0.5;
    let warp_y = fosfora_noise2(p * 3.0 + vec2f(0.0, u.time * 0.08)) - 0.5;
    let warp = vec2f(warp_x, warp_y) * warp_str;

    // flow_stretch is a dry/wet for the whole Chronoflow transport: 0 = the
    // classic anchored ghosting river (no image-flow at all), 1 = 1.75× raw
    // particle velocity. Squared response: the lower third of the slider stays
    // essentially anchored (default 0.3 → 0.16×), so the default look matches
    // the classic river while leaving fine control right where it matters.
    let s5 = param(5u);
    let stretch = s5 * s5 * 1.75;
    let v = input0(uv).xy * stretch;
    // Musical advection envelope: quiet passages hold the river nearly
    // anchored (classic in-place ghosting), loudness surges the whole body
    // into flow — the trail motion follows the music instead of churning
    // constantly.
    let surge = 0.2 + 0.8 * clamp(u.rms * 1.6, 0.0, 1.0);
    let speed = length(v) * surge;
    let hist = feedback(uv - v * dt * surge + warp).rgb;

    // Signed trail over the ambient background: re-adding bg each frame would
    // integrate to blowout, so only the deviation from bg is retained.
    // trail_decay is a RAW per-frame decay here (its 0.5..0.88 range predates
    // Chronoflow): 2M screen-filling additive particles saturate under the
    // remapped exposure curve.
    let bg = input1(uv).rgb;
    // Snap is squared + scaled: on a dense field the trail body carries most
    // of the luminance, so a hard per-beat collapse strobes like a failing
    // fluorescent tube. The curve keeps the lower half of beat_snap subtle;
    // the top end still hits a hard shutter for those who want it.
    let snap_p = param(4u) * param(4u) * 0.75;
    var keep = chrono_keep_direct(param(0u), snap_p);
    // Mild speed trim only — heavy damping (÷(1+2·speed)) starved the trail
    // body entirely under music and left a loose dot cloud; the saturation
    // knee below is what actually prevents the pegged-wash failure.
    keep = keep / (1.0 + speed * 0.3);
    var trail = hist - bg;
    // Soft knee well below the HDR clamp: accumulation self-limits as a region
    // approaches saturation (steady state stays ≈1.0 even at 8M particles), so
    // the field keeps gradient contrast instead of pegging into a uniform wash.
    let tl = max(max(trail.r, trail.g), trail.b);
    trail *= 1.0 / (1.0 + 0.35 * max(tl, 0.0));
    let col = clamp(bg + trail * keep, vec3f(0.0), vec3f(1.5));

    let alpha = clamp(max(col.r, max(col.g, col.b)) * 2.0, 0.0, 1.0);
    return vec4f(col, alpha);
}
