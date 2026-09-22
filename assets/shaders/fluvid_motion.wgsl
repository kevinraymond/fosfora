// Fluvid — where the camera moved, and which way.
//   input0 = cam, this frame: r = fast luma, g = slow luma
// Output: xy = push direction × gate, z = gate (0..1, how much counts as motion).
//
// The direction is the normal flow of brightness constancy, v = −It·∇I / |∇I|²:
// the component of motion across the image's edges, which is the only part a
// single brightness field can see. A hand sweeping right brightens pixels ahead
// of a light-on-dark edge and darkens those behind it, and both give the same
// rightward push. Only the direction is kept; the gate carries the strength,
// so a faint but real movement pushes as firmly as a high-contrast one.
// Non-feedback: the working size comes from input0, not prev_frame (1x1 here).

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(input0_tex));
    let texel = 1.0 / dims;
    let uv = frag_coord.xy / dims;

    let here = input0(uv);
    let it = here.r - here.g;

    // Gradient over ±2 texels (±8 display pixels at 4x): wide enough to span the
    // soft edge a moving limb draws after the box filter.
    let dx = vec2f(2.0 * texel.x, 0.0);
    let dy = vec2f(0.0, 2.0 * texel.y);
    let grad = 0.25 * vec2f(input0(uv + dx).r - input0(uv - dx).r, input0(uv + dy).r - input0(uv - dy).r);

    // The gate reads the change AVERAGED over a patch about 4% of the frame
    // high (taps 2% apart), not the change at this texel. A head swaying while you talk moves
    // every edge a few pixels, which changes thin lines along the hairline,
    // eyes and nostrils; an opening mouth or a moving hand changes a solid
    // patch. Averaging thins the lines out below the threshold and leaves the
    // patch above it. The spacing is a fraction of the frame, so the size that
    // counts is the same at 1080p and 4K.
    let s = max(1.0, dims.y * 0.02) * texel;
    var area = 0.0;
    for (var j = -1; j <= 1; j++) {
        for (var k = -1; k <= 1; k++) {
            let c = input0(uv + vec2f(f32(k), f32(j)) * s);
            area += abs(c.r - c.g);
        }
    }
    area /= 9.0;

    // Motion size (p8) blends the two: 0 is the per-texel change (the first
    // Fluvid: every edge counts, so a swaying head inks its outline), 1 the
    // patch average.
    let change = mix(abs(it), area, clamp(param(8u), 0.0, 1.0));

    // Motion threshold (p2) on a squared curve: the thresholds that matter
    // are small, and a linear slider packed them all into its first fifth.
    let t = param(2u);
    let thr = 0.004 + 0.12 * t * t;
    let gate = smoothstep(thr, thr * 2.0 + 0.004, change);

    let flow = -it * grad / (dot(grad, grad) + 1e-4);
    let fl = length(flow);
    var dir = vec2f(0.0);
    if (fl > 1e-5) {
        dir = flow / fl;
    }
    return vec4f(dir * gate, gate, 1.0);
}
