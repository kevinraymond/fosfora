/*! trama
{
  "name": "Kaleidoscope",
  "id": "kaleido",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "segments", "default": 6.0, "min": 2.0,  "max": 24.0 },
    { "type": "Float", "name": "spin",     "default": 0.0, "min": -1.0, "max": 1.0 },
    { "type": "Float", "name": "rotate",   "default": 0.0, "min": -0.5, "max": 0.5 },
    { "type": "Float", "name": "zoom",     "default": 1.0, "min": 0.25, "max": 4.0 }
  ],
  "rates": ["spin"]
}
*/
// Kaleidoscope — wraps the picture into mirrored wedges around the center.
// Feed it something with a strong off-center feature and the symmetry has
// material to work with; a flat field gives a flat kaleidoscope.
//
// `spin` is a RATE (see "rates" above): param(1u) arrives as its running
// integral, the angle turned so far, and `rotate` offsets it by a fixed
// amount. Never write `u.time * spin`; see hue_drift.wgsl for what that does.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let aspect = res.x / res.y;

    // Work in centered, aspect-corrected space so the wedges stay wedges on a
    // non-square target.
    let p = (uv - 0.5) * vec2f(aspect, 1.0);
    let r = length(p);

    let seg = 6.2831853 / max(param(0u), 2.0);
    var a = atan2(p.y, p.x) + (param(1u) + param(2u)) * 6.2831853;
    a = a - seg * floor(a / seg); // wrap into one segment
    a = abs(a - seg * 0.5);       // fold it: the wedge mirrors about its middle

    let q = vec2f(cos(a), sin(a)) * r / max(param(3u), 1e-3);
    let suv = q / vec2f(aspect, 1.0) + 0.5;
    return input0(clamp(suv, vec2f(0.0), vec2f(1.0)));
}
