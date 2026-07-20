// Splat — background plate (#1800): a near-black radial gradient with a
// faint audio-breathing floor glow, so empty space around the captured scene
// reads as depth instead of void. Deliberately subdued — the splats are the
// subject; the plate only answers "what is behind reality".
// NOTE: this file must never contain the injected uniform struct's name,
// even in a comment — the effect loader suppresses the block if it appears.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let aspect = res.x / res.y;
    let p = (uv - 0.5) * vec2f(aspect, 1.0);
    let r = length(p);

    // Deep cool vignette base.
    var col = mix(
        vec3f(0.030, 0.034, 0.050),
        vec3f(0.004, 0.005, 0.009),
        smoothstep(0.1, 0.95, r)
    );

    // Audio floor: rms warms the center; the buildup riser lifts the whole
    // plate so the drop's blackout-and-shatter lands harder by contrast.
    let lift = u.rms * 0.6 + u.buildup * 0.5;
    col += vec3f(0.020, 0.023, 0.032) * lift * (1.0 - smoothstep(0.0, 0.85, r));

    // Drop: a one-frame pale shock swallowed by the vignette.
    col += vec3f(0.05, 0.055, 0.07) * u.drop * (1.0 - r);

    return vec4f(col, 1.0);
}
