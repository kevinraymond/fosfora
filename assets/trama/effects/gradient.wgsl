/*! trama
{
  "name": "Gradient",
  "id": "gradient",
  "kind": "source",
  "inputs": 0,
  "params": [
    { "type": "Color", "name": "color_a",  "default": [0.0, 0.0, 0.0, 1.0] },
    { "type": "Color", "name": "color_b",  "default": [1.0, 0.4, 0.1, 1.0] },
    { "type": "Float", "name": "angle",    "default": 0.0, "min": -0.5, "max": 0.5 },
    { "type": "Float", "name": "midpoint", "default": 0.5, "min": 0.05, "max": 0.95 }
  ]
}
*/
// Gradient — a linear ramp between two colors, a picture from nothing.
// `angle` is in turns, `midpoint` slides where the two meet without moving
// either end. Both colors carry alpha, so a ramp from opaque to transparent
// is a fade-out mask for a Mix.
//
// Two Color params take four scalar slots each, so the floats start at
// param(8u): color_a is param(0u)..param(3u), color_b param(4u)..param(7u),
// then `angle` and `midpoint`.
//
// The ramp is measured against the frame's extent along `angle`, so 0 and 1
// land on the two corners the direction points at whatever the aspect ratio —
// rotating the gradient does not change how much of it you can see.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let aspect = res.x / res.y;

    let th = param(8u) * 6.2831853;
    let dir = vec2f(cos(th), sin(th));
    let p = (uv - 0.5) * vec2f(aspect, 1.0);
    let extent = 0.5 * (abs(dir.x) * aspect + abs(dir.y));
    let d = clamp(dot(p, dir) / (2.0 * extent) + 0.5, 0.0, 1.0);

    // Two straight segments that meet at `midpoint`, so sliding it biases the
    // ramp without clipping either end to a flat band.
    let m = param(9u);
    let t = select(0.5 * d / m, 0.5 + 0.5 * (d - m) / (1.0 - m), d > m);

    let ca = vec4f(param(0u), param(1u), param(2u), param(3u));
    let cb = vec4f(param(4u), param(5u), param(6u), param(7u));
    let c = mix(ca, cb, clamp(t, 0.0, 1.0));
    return vec4f(c.rgb * c.a, c.a);
}
