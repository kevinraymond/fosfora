/*! trama
{
  "name": "Palette Map",
  "id": "palette_map",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "offset", "default": 0.0, "min": 0.0,  "max": 1.0 },
    { "type": "Float", "name": "drift",  "default": 0.0, "min": -2.0, "max": 2.0 },
    { "type": "Float", "name": "spread", "default": 1.0, "min": 0.25, "max": 4.0 },
    { "type": "Float", "name": "blend",  "default": 1.0, "min": 0.0,  "max": 1.0 }
  ],
  "rates": ["drift"]
}
*/
// Palette Map — throws away the input's color and keeps only its brightness,
// then recolors that through a cosine palette. A white-on-black effect comes
// back as a full-color one, and the whole look is one slider.
//
// `spread` is a gamma on the brightness before the lookup: under 1 pushes the
// picture into the bright end of the palette, over 1 into the dark end.
// `blend` fades back to the original color.
//
// `drift` is a RATE (see "rates" above): param(1u) arrives as its running
// integral, how far the palette has rotated, so colors cycle instead of
// jumping when the slider moves. Never write `u.time * drift`; see
// hue_drift.wgsl for what that does.

const PALETTE_MAP_LUMA = vec3f(0.2126, 0.7152, 0.0722);

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let c = input0(uv);

    // Brightness is a property of the color, not of the coverage, so divide
    // coverage back out first (INV-A, docs/alpha.md). RGB > A means light with
    // no coverage — nothing to divide by, so it is read as light directly.
    let lit = c.a <= 1e-4;
    let rgb = select(c.rgb / max(c.a, 1e-4), c.rgb, lit);

    let t = pow(clamp(dot(rgb, PALETTE_MAP_LUMA), 0.0, 1.0), param(2u));
    let mapped = fosfora_palette(
        t + param(0u) + param(1u),
        vec3f(0.5),
        vec3f(0.5),
        vec3f(1.0),
        vec3f(0.0, 0.33, 0.67),
    );

    let out = mix(rgb, mapped, param(3u));
    return vec4f(out * select(c.a, 1.0, lit), c.a);
}
