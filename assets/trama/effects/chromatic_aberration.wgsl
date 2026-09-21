/*! trama
{
  "name": "Chromatic Aberration",
  "id": "chromatic_aberration",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "amount", "default": 0.006, "min": 0.0,  "max": 0.05 },
    { "type": "Float", "name": "angle",  "default": 0.0,   "min": -0.5, "max": 0.5 },
    { "type": "Bool",  "name": "radial", "default": true }
  ]
}
*/
// Chromatic Aberration — samples red and blue a little either side of green,
// the cheap lens-fringe look. `radial` splits outward from the center, which
// grows with distance and leaves the middle sharp; turn it off and the split
// is uniform in the direction `angle` points.
//
// Each channel comes from a different place, so coverage does too. The output
// takes the widest of the three alphas: keeping green's would multiply the
// fringe back out of existence wherever green happened to land on a
// transparent pixel.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;

    let th = param(1u) * 6.2831853;
    let linear = vec2f(cos(th), sin(th)) * param(0u);
    let radial = (uv - 0.5) * param(0u) * 2.0;
    let d = select(linear, radial, param(2u) > 0.5);

    let r = input0(clamp(uv + d, vec2f(0.0), vec2f(1.0)));
    let g = input0(uv);
    let b = input0(clamp(uv - d, vec2f(0.0), vec2f(1.0)));

    return vec4f(r.r, g.g, b.b, max(g.a, max(r.a, b.a)));
}
