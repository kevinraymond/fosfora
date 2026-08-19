// Protea — display pass: shade the ecosystem (full res).
//   feedback() = own previous frame (small blend to smooth the half-res sim)
//   input0     = mass field, this frame (rgb species, a total)
//   input1     = potential/growth, this frame (a = mean potential, for organelles)
//
// Each species gets its own hue; the body is a soft saturating fill, the membrane is
// an edge-lit rim from the mass gradient, and the interior carries organelle banding
// from the sensed potential. Peak output stays ~1.2 so bloom picks up rims without
// the frame washing (mix-based history blend is self-limiting, never additive).

const COL_A = vec3f(0.15, 0.85, 0.90); // teal
const COL_B = vec3f(1.00, 0.55, 0.12); // amber
const COL_C = vec3f(0.62, 0.30, 1.00); // violet

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(prev_frame));
    let uv = frag_coord.xy / dims;

    let saturation = param(8u);  // p8
    let brightness = param(9u);  // p9

    let mass = input0(uv);
    let total = mass.a;
    let pot = input1(uv).a;

    // Membrane: gradient of total mass, stepped at one SIM texel (the field is half res).
    let stex = 1.0 / vec2f(textureDimensions(input0_tex));
    let gx = input0(uv + vec2f(stex.x, 0.0)).a - input0(uv - vec2f(stex.x, 0.0)).a;
    let gy = input0(uv + vec2f(0.0, stex.y)).a - input0(uv - vec2f(0.0, stex.y)).a;
    let edge = length(vec2f(gx, gy)) * 0.5;

    // Soft saturating bodies, one hue per species.
    let body = vec3f(1.0) - exp(-mass.rgb * 2.0);
    var col = COL_A * body.x + COL_B * body.y + COL_C * body.z;

    // Organelles: concentric banding of the sensed potential, only inside bodies.
    let bands = 0.5 + 0.5 * cos(pot * 26.0 - u.time * 0.8);
    let interior = smoothstep(0.12, 0.50, total);
    col *= 0.60 + 0.40 * mix(1.0, bands * bands, interior);

    // Membrane rim light, tinted by the body underneath.
    let rim = smoothstep(0.05, 0.55, edge * 8.0);
    col += rim * (col * 0.6 + vec3f(0.14, 0.18, 0.22)) * 0.7;

    // Detected key tilts the whole palette, gently and only when confident.
    let key_shift = (fosfora_key_hue(u.key_class, u.key_is_minor) - 0.5) * 0.25 * u.key_confidence;
    col = fosfora_hue_shift(col, key_shift);

    // Deep-water background where nothing lives.
    col += vec3f(0.010, 0.014, 0.026) * (1.0 - smoothstep(0.0, 0.4, total));

    let luma = dot(col, vec3f(0.299, 0.587, 0.114));
    col = mix(vec3f(luma), col, 0.4 + 1.2 * saturation);
    col *= 0.18 + 0.6 * brightness;

    // Temporal smoothing over own history (mix, not add — self-limiting).
    col = mix(col, feedback(uv).rgb, frame_decay(0.30));
    col = min(col, vec3f(1.5));
    return vec4f(col, 1.0);
}
