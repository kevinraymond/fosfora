/*! trama
{
  "name": "Mirror",
  "id": "mirror",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Bool",  "name": "horizontal", "default": true },
    { "type": "Bool",  "name": "vertical",   "default": false },
    { "type": "Float", "name": "center_x",   "default": 0.5, "min": 0.0, "max": 1.0 },
    { "type": "Float", "name": "center_y",   "default": 0.5, "min": 0.0, "max": 1.0 }
  ]
}
*/
// Mirror — folds the picture about a line, so one half is reflected over the
// other. Both axes at once gives quadrant symmetry; sliding the center off 0.5
// picks which slice gets repeated, which is the knob worth modulating.
//
// The fold reflects the side the center leans away from: everything is sampled
// from `center - |uv - center|`, so the whole output comes from one half of
// the input and the seam lands exactly on the center line.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    var p = frag_coord.xy / u.resolution;

    if param(0u) > 0.5 {
        let cx = param(2u);
        p.x = cx - abs(p.x - cx);
    }
    if param(1u) > 0.5 {
        let cy = param(3u);
        p.y = cy - abs(p.y - cy);
    }

    return input0(clamp(p, vec2f(0.0), vec2f(1.0)));
}
