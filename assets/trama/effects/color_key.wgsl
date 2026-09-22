/*! trama
{
  "name": "Color Key",
  "id": "color_key",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "hue",            "default": 0.0,  "min": 0.0, "max": 1.0 },
    { "type": "Float", "name": "hue_width",      "default": 0.08, "min": 0.0, "max": 0.5 },
    { "type": "Float", "name": "min_saturation", "default": 0.25, "min": 0.0, "max": 1.0 },
    { "type": "Float", "name": "softness",       "default": 0.05, "min": 0.0, "max": 0.5 },
    { "type": "Bool",  "name": "invert",         "default": false }
  ]
}
*/
// Color Key — keeps one family of colors and makes everything else
// transparent. On a webcam layer under an effect that reads the layers
// beneath it (Fluvid), it decides what that effect can see: a colored glove,
// a prop, lips.
//
// `hue` is the color to keep (0 and 1 are both red, 1/3 green, 2/3 blue) and
// `hue_width` how far either side of it still counts. `min_saturation` drops
// grays, whites and near-blacks, whose hue is noise. `softness` ramps both
// edges. `invert` keeps everything EXCEPT that color, e.g. drop a green
// screen. Set it by what survives: the kept region stays picture, the rest
// goes transparent, so the result reads by shape as well as color.
//
// Internal color is premultiplied (INV-A, docs/alpha.md): hue and saturation
// are read from the un-premultiplied color, and the key scales RGB and alpha
// together, which keeps the result premultiplied.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let c = input0(uv);
    let rgb = c.rgb / max(c.a, 1e-4);

    // HSV hue and saturation.
    let mx = max(rgb.r, max(rgb.g, rgb.b));
    let mn = min(rgb.r, min(rgb.g, rgb.b));
    let chroma = mx - mn;
    let sat = chroma / max(mx, 1e-4);
    var h = 0.0;
    if chroma > 1e-4 {
        if mx == rgb.r {
            h = (rgb.g - rgb.b) / chroma;
        } else if mx == rgb.g {
            h = 2.0 + (rgb.b - rgb.r) / chroma;
        } else {
            h = 4.0 + (rgb.r - rgb.g) / chroma;
        }
        h = fract(h / 6.0);
    }

    let soft = max(param(3u), 1e-4);
    // Distance around the hue wheel, 0..0.5.
    let d = abs(fract(h - param(0u) + 0.5) - 0.5);
    let in_hue = 1.0 - smoothstep(param(1u), param(1u) + soft, d);
    let min_sat = param(2u);
    let saturated = smoothstep(min_sat - soft, min_sat + soft, sat);
    // Near-black pixels have no reliable hue either.
    let lit = smoothstep(0.03, 0.08, mx);
    var k = in_hue * saturated * lit;
    if param(4u) > 0.5 {
        k = 1.0 - k;
    }
    return c * k;
}
