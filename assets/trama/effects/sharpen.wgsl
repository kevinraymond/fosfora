/*! trama
{
  "name": "Sharpen",
  "id": "sharpen",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "amount", "default": 1.0, "min": 0.0, "max": 4.0 },
    { "type": "Float", "name": "radius", "default": 1.5, "min": 0.5, "max": 8.0 }
  ]
}
*/
// Sharpen — an unsharp mask: the picture minus a blurred copy of itself is
// its fine detail, and adding that back makes edges crisper. `radius` (in
// pixels) picks how fine "detail" is: small sharpens texture, large lifts
// local contrast. `amount` is how much detail goes back in; past about 2 the
// edges grow halos, which is a look of its own.
//
// The blur is eight taps on a ring at `radius`, weighted like a 3x3 Gaussian.
// It runs in premultiplied space, where a weighted sum is exact, so detail
// next to a transparent edge does not leak color. The result is floored at 0:
// the dark side of a halo can overshoot below black.

fn sharpen_tap(uv: vec2f) -> vec4f {
    return input0(clamp(uv, vec2f(0.0), vec2f(1.0)));
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let d = param(1u) / res;
    let c = input0(uv);

    let edges = sharpen_tap(uv + vec2f(d.x, 0.0)) + sharpen_tap(uv - vec2f(d.x, 0.0))
              + sharpen_tap(uv + vec2f(0.0, d.y)) + sharpen_tap(uv - vec2f(0.0, d.y));
    let corners = sharpen_tap(uv + d) + sharpen_tap(uv - d)
                + sharpen_tap(uv + vec2f(d.x, -d.y)) + sharpen_tap(uv + vec2f(-d.x, d.y));
    let blur = (c * 4.0 + edges * 2.0 + corners) / 16.0;

    let out = c + (c - blur) * param(0u);
    let a = clamp(out.a, 0.0, 1.0);
    return vec4f(max(out.rgb, vec3f(0.0)), a);
}
