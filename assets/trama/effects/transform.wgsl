/*! trama
{
  "name": "Transform",
  "id": "transform",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "scale",       "default": 1.0, "min": 0.25, "max": 4.0 },
    { "type": "Float", "name": "rotate",      "default": 0.0, "min": -0.5, "max": 0.5 },
    { "type": "Float", "name": "translate_x", "default": 0.0, "min": -0.5, "max": 0.5 },
    { "type": "Float", "name": "translate_y", "default": 0.0, "min": -0.5, "max": 0.5 }
  ]
}
*/
// Transform — scale / rotate / translate about the center, the feedback-loop
// workhorse: a whisker off identity inside an echo loop turns into spiral
// trails. `rotate` is in turns, translation in UV units; all four params are
// Floats so every one of them is modulatable (M1 modulates Floats only).
// Pixels pulled from outside the input read transparent, so trails fade at
// the frame edge instead of smearing the border pixels.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let aspect = res.x / res.y;

    // Inverse mapping (output pixel -> input position): untranslate, then
    // unrotate and unscale in centered aspect-corrected space so rotation
    // stays circular on non-square targets. Forward order: scale, rotate,
    // translate.
    let t = vec2f(param(2u), param(3u));
    var p = (uv - 0.5 - t) * vec2f(aspect, 1.0);
    let ang = -param(1u) * 6.2831853;
    let cs = cos(ang);
    let sn = sin(ang);
    p = vec2f(p.x * cs - p.y * sn, p.x * sn + p.y * cs);
    p /= max(param(0u), 1e-4);
    let suv = p / vec2f(aspect, 1.0) + 0.5;

    let sample = input0(clamp(suv, vec2f(0.0), vec2f(1.0)));
    let inside = all(suv == clamp(suv, vec2f(0.0), vec2f(1.0)));
    return select(vec4f(0.0), sample, inside);
}
