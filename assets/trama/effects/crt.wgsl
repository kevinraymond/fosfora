/*! trama
{
  "name": "CRT",
  "id": "crt",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "curvature", "default": 0.15, "min": 0.0, "max": 0.5 },
    { "type": "Float", "name": "scanlines", "default": 0.5,  "min": 0.0, "max": 1.0 },
    { "type": "Float", "name": "mask",      "default": 0.4,  "min": 0.0, "max": 1.0 },
    { "type": "Float", "name": "mask_size", "default": 3.0,  "min": 1.0, "max": 8.0 },
    { "type": "Float", "name": "glow",      "default": 0.3,  "min": 0.0, "max": 1.0 },
    { "type": "Float", "name": "vignette",  "default": 0.3,  "min": 0.0, "max": 1.0 }
  ],
  "integers": ["mask_size"]
}
*/
// CRT — an old tube monitor: a bulging screen, dark gaps between scanlines,
// the red/green/blue stripes of an aperture-grille shadow mask, a soft glow
// bleeding off bright parts, and corners that fall off. Each part has its own
// slider and every one at 0 turns that part off. `mask_size` is the stripe
// width in pixels and the scanline pitch with it, so the mask and lines stay
// in proportion.
//
// Outside the bulged screen is transparent, like Transform's edges, so the
// layers beneath show around it. Scanlines follow the curve because they are
// counted in the bent coordinates.
//
// The mask darkens two of every three stripes, which would dim the picture
// to a third at full strength; it is divided by its own average so the
// stripes cost no brightness overall (the bright stripe goes above 1, which
// the HDR pipeline carries). Everything here scales or adds light in
// premultiplied space, so alpha is untouched except at the screen edge.

const CRT_GLOW_TAPS = 8;

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;

    // Barrel distortion: each axis bends by the square of the other, the
    // classic tube profile. 2.0 maps the slider's 0.5 to a strong bulge.
    var cc = uv * 2.0 - 1.0;
    cc = cc + cc * (cc.yx * cc.yx) * (param(0u) * 0.5);
    let suv = cc * 0.5 + 0.5;

    // One pixel of feather at the screen edge, in bent coordinates.
    let px = 1.0 / res;
    let edge = min(suv, vec2f(1.0) - suv);
    let inside = smoothstep(vec2f(0.0), px, edge);
    let screen = inside.x * inside.y;
    if screen <= 0.0 {
        return vec4f(0.0);
    }

    var c = input0(clamp(suv, vec2f(0.0), vec2f(1.0)));

    // Glow: a ring of taps a few pixels out, added back as light.
    let g = param(4u);
    if g > 0.0 {
        var acc = vec4f(0.0);
        let r = 4.0 * px * (res.y / 1080.0 + 0.5);
        for (var i = 0; i < CRT_GLOW_TAPS; i++) {
            let a = f32(i) * (6.2831853 / f32(CRT_GLOW_TAPS));
            acc += input0(clamp(suv + vec2f(cos(a), sin(a)) * r, vec2f(0.0), vec2f(1.0)));
        }
        c = vec4f(c.rgb + acc.rgb / f32(CRT_GLOW_TAPS) * g, c.a);
    }

    let pitch = max(round(param(3u)), 1.0);

    // Scanlines: bright at each line's center, dark between.
    let line = suv.y * res.y / pitch;
    let scan = 1.0 - param(1u) * (0.5 - 0.5 * cos(6.2831853 * line));

    // Aperture grille: one bright channel per stripe, in R, G, B order.
    let m = param(2u);
    let stripe = u32(floor(frag_coord.x / pitch)) % 3u;
    let one_hot = vec3f(f32(stripe == 0u), f32(stripe == 1u), f32(stripe == 2u));
    let mask = (vec3f(1.0 - m) + one_hot * m) / (1.0 - m * (2.0 / 3.0));

    // Vignette: fall off toward the corners of the bent screen.
    let vig = 1.0 - param(5u) * smoothstep(0.4, 1.4, dot(cc, cc) * 0.5);

    let rgb = c.rgb * scan * mask * vig;
    return vec4f(rgb, c.a) * screen;
}
