/*! trama
{
  "name": "Blur",
  "id": "blur",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "radius", "default": 8.0, "min": 0.0, "max": 48.0 },
    { "type": "Float", "name": "amount", "default": 1.0, "min": 0.0, "max": 1.0 }
  ]
}
*/
// Blur — softens the picture, `radius` in pixels. `amount` crossfades back to
// the original, so partway is a bloom-ish haze over a picture that still has
// its detail.
//
// One pass, sixteen taps on a golden-angle spiral over the disc rather than a
// separable Gaussian: a chain node is not the place to spend two passes, and
// the spiral has no axis for the eye to catch, so the failure mode at a big
// radius is softness rather than the cross-shaped banding a small box kernel
// gives. Sampling is in premultiplied space, where a weighted sum is exactly
// the right operation, so a blur across an edge does not drag color out of
// transparent pixels.

const BLUR_TAPS = 16;
const BLUR_GOLDEN_ANGLE = 2.3999632;

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let c = input0(uv);

    let step = param(0u) / res;
    var acc = vec4f(0.0);
    for (var i = 0; i < BLUR_TAPS; i++) {
        let fi = f32(i);
        let a = fi * BLUR_GOLDEN_ANGLE;
        // sqrt spreads the taps evenly over the disc's AREA, not its radius —
        // otherwise they crowd the center and the edge of the disc is noise.
        let r = sqrt((fi + 0.5) / f32(BLUR_TAPS));
        let o = vec2f(cos(a), sin(a)) * r;
        acc += input0(clamp(uv + o * step, vec2f(0.0), vec2f(1.0)));
    }

    return mix(c, acc / f32(BLUR_TAPS), param(1u));
}
