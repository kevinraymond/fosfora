// Pegboard background — the moulded board the pegs sit in.
// Deliberately has no feedback pass: the pegs are the picture, and a trail would smear
// the lattice that makes the toy legible.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let backlight = param(5u);

    // Dark grey plastic with a slight vertical mould gradient.
    let plastic = mix(vec3f(0.055, 0.055, 0.065), vec3f(0.025, 0.025, 0.032), uv.y);

    // The lamp behind the board bleeds through as a soft centre glow that breathes
    // with the low end. Kept well under the peg brightness so it never flattens them.
    let centred = (uv - 0.5) * vec2f(u.resolution.x / u.resolution.y, 1.0);
    let falloff = exp(-dot(centred, centred) * 2.4);
    let lamp = falloff * backlight * (0.05 + u.bass * 0.09);

    let rgb = plastic + vec3f(1.0, 0.93, 0.85) * lamp;
    return vec4f(rgb, 1.0);
}
