/*! trama
{
  "name": "Posterize",
  "id": "posterize",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "levels", "default": 4.0, "min": 2.0, "max": 32.0 },
    { "type": "Float", "name": "amount", "default": 1.0, "min": 0.0, "max": 1.0 }
  ],
  "integers": ["levels"]
}
*/
// Posterize — cuts each channel down to a few flat steps, the screen-print
// look. `levels` is the number of steps per channel, so 2 leaves eight
// colors in all and 32 is barely visible.
//
// The steps are spaced in display space (sqrt of the linear value), where the
// eye sees brightness evenly. Spaced in linear light, four levels put three of
// the four steps in the highlights and the shadows go to one flat black.
//
// Rounding is not linear in coverage, so the color is unpremultiplied first
// (INV-A, docs/alpha.md) and alpha is left alone. HDR is clamped to 1.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let c = input0(uv);
    if c.a <= 1e-4 {
        return c;
    }

    let n = max(round(param(0u)), 2.0) - 1.0;
    let rgb = c.rgb / c.a;
    let disp = sqrt(clamp(rgb, vec3f(0.0), vec3f(1.0)));
    let q = round(disp * n) / n;
    let out_rgb = mix(rgb, q * q, clamp(param(1u), 0.0, 1.0));
    return vec4f(out_rgb * c.a, c.a);
}
