/*! trama
{
  "name": "Dither",
  "id": "dither",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "levels", "default": 2.0, "min": 2.0, "max": 16.0 },
    { "type": "Float", "name": "scale",  "default": 2.0, "min": 1.0, "max": 8.0 },
    { "type": "Float", "name": "amount", "default": 1.0, "min": 0.0, "max": 1.0 },
    { "type": "Bool",  "name": "mono",   "default": false }
  ],
  "integers": ["levels", "scale"]
}
*/
// Dither — ordered (Bayer) dithering: the picture cut down to a few levels
// per channel, with the in-between tones drawn as a fixed crosshatch of the
// two nearest levels. At `levels` 2 that is eight colors, the early-computer
// look; `mono` makes it one-bit black and white. `scale` is the size of a
// dither dot in pixels, so 1 is fine grain and 4 or more reads as a pattern.
//
// The 8x8 Bayer matrix is computed from the pixel position by interleaving
// bits rather than stored as a table, so it costs a few integer ops. Levels
// are spaced in display space (sqrt of linear), where tones step evenly to
// the eye. Like Posterize this works on unpremultiplied color and leaves
// alpha alone (INV-A, docs/alpha.md).

const DITHER_LUMA = vec3f(0.2126, 0.7152, 0.0722);

// Threshold in [0, 1) for one cell of the 8x8 Bayer matrix.
fn dither_bayer8(p: vec2u) -> f32 {
    let x = p.x & 7u;
    let y = p.y & 7u;
    let xy = x ^ y;
    let v = ((xy & 1u) << 5u) | ((y & 1u) << 4u) | ((xy & 2u) << 2u)
          | ((y & 2u) << 1u) | ((xy & 4u) >> 1u) | ((y & 4u) >> 2u);
    return (f32(v) + 0.5) / 64.0;
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let dot_px = max(round(param(1u)), 1.0);
    let cell = floor(frag_coord.xy / dot_px);
    // Every pixel of a dot reads the dot's center, so a dot is one flat color.
    let c = input0(clamp((cell + 0.5) * dot_px / res, vec2f(0.0), vec2f(1.0)));
    let own = input0(frag_coord.xy / res);
    if c.a <= 1e-4 {
        return own;
    }

    var rgb = c.rgb / c.a;
    if param(3u) > 0.5 {
        rgb = vec3f(dot(rgb, DITHER_LUMA));
    }
    let n = max(round(param(0u)), 2.0) - 1.0;
    let disp = sqrt(clamp(rgb, vec3f(0.0), vec3f(1.0)));
    let t = dither_bayer8(vec2u(cell));
    let q = min(floor(disp * n + t), vec3f(n)) / n;

    let base = own.rgb / max(own.a, 1e-4);
    let out_rgb = mix(base, q * q, clamp(param(2u), 0.0, 1.0));
    return vec4f(out_rgb * own.a, own.a);
}
