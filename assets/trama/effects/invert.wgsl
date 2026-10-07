/*! trama
{
  "name": "Invert",
  "id": "invert",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "amount",    "default": 1.0, "min": 0.0, "max": 1.0 },
    { "type": "Bool",  "name": "luma_only", "default": false }
  ]
}
*/
// Invert — the photographic negative. `amount` crossfades from the original,
// so a beat-synced square wave on it is the classic invert-on-the-kick.
// `luma_only` flips brightness but keeps the hue: a red stays red, only a dark
// red turns into a light one.
//
// HDR is clamped to 0–1 first. 1 - x of a 4.0 highlight is -3, which would
// punch a black hole in the picture where it was brightest.
//
// Inverting is not linear in coverage, so like Levels it works on the
// unpremultiplied color (INV-A, docs/alpha.md) and leaves alpha alone:
// transparent stays transparent instead of inverting to opaque white. A pixel
// with light but no coverage (glow spill) is left as it is, since there is no
// coverage to put an inverted picture in.

const INVERT_LUMA = vec3f(0.2126, 0.7152, 0.0722);

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let c = input0(uv);
    if c.a <= 1e-4 {
        return c;
    }

    let rgb = clamp(c.rgb / c.a, vec3f(0.0), vec3f(1.0));
    var inv = vec3f(1.0) - rgb;
    if param(1u) > 0.5 {
        // Shift every channel by the same amount so the luma lands on 1 - Y.
        let y = dot(rgb, INVERT_LUMA);
        inv = clamp(rgb + (1.0 - 2.0 * y), vec3f(0.0), vec3f(1.0));
    }
    let out_rgb = mix(c.rgb / c.a, inv, clamp(param(0u), 0.0, 1.0));
    return vec4f(out_rgb * c.a, c.a);
}
