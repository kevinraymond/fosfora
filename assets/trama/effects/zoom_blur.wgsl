/*! trama
{
  "name": "Zoom Blur",
  "id": "zoom_blur",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "strength",    "default": 0.2, "min": 0.0,  "max": 1.0 },
    { "type": "Float", "name": "center_x",    "default": 0.5, "min": 0.0,  "max": 1.0 },
    { "type": "Float", "name": "center_y",    "default": 0.5, "min": 0.0,  "max": 1.0 },
    { "type": "Float", "name": "angle",       "default": 0.0, "min": -0.5, "max": 0.5 },
    { "type": "Bool",  "name": "directional", "default": false }
  ]
}
*/
// Zoom Blur — streaks the picture toward and away from a point, the rush of
// a camera zooming in on the kick. Put `onset` or `bass` on `strength`'s mod
// row for the classic punch. `strength` is how far the streak reaches as a
// fraction of each pixel's distance to the center, so the center stays sharp
// and the corners smear most.
//
// `directional` turns it into a motion blur instead: every pixel streaks the
// same distance along `angle` (0 is horizontal, 0.25 vertical), and
// `strength` is that distance as a fraction of the frame's height. The center
// is ignored.
//
// Both kernels are centered on the pixel, so the picture smears without
// shrinking or sliding — a pumping `strength` does not make it breathe.
// Each pixel starts its taps at a slightly different point along the streak,
// which turns the stepped ghost copies a long streak would otherwise show
// into fine grain. Sampling is in premultiplied space, so a streak across an
// edge does not drag color out of transparent pixels.
//
// Param slots: strength 0, center_x 1, center_y 2, angle 3, directional 4.

const ZOOM_TAPS = 32;

// Interleaved gradient noise (Jimenez 2014): a per-pixel offset in [0, 1)
// that is stable frame to frame, so the grain does not crawl.
fn zoom_blur_jitter(p: vec2f) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2f(0.06711056, 0.00583715))));
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let strength = max(param(0u), 0.0);
    if strength <= 0.0 {
        return input0(uv);
    }

    let center = vec2f(param(1u), param(2u));
    let directional = param(4u) > 0.5;

    // Directional: one fixed step for every pixel, in aspect-corrected space
    // so `angle` 0.125 is a true 45 degrees on a wide frame.
    let th = param(3u) * 6.2831853;
    let streak = vec2f(cos(th) * res.y / res.x, sin(th)) * strength;

    let j = zoom_blur_jitter(frag_coord.xy);
    var acc = vec4f(0.0);
    for (var i = 0; i < ZOOM_TAPS; i++) {
        // t spans [-0.5, 0.5): centered on the pixel.
        let t = (f32(i) + j) / f32(ZOOM_TAPS) - 0.5;
        var p: vec2f;
        if directional {
            p = uv + streak * t;
        } else {
            p = center + (uv - center) * (1.0 + strength * t);
        }
        acc += input0(clamp(p, vec2f(0.0), vec2f(1.0)));
    }
    return acc / f32(ZOOM_TAPS);
}
