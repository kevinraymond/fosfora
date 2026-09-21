/*! trama
{
  "name": "Key",
  "id": "key",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "threshold", "default": 0.08, "min": 0.0, "max": 1.0 },
    { "type": "Float", "name": "softness",  "default": 0.12, "min": 0.0, "max": 1.0 },
    { "type": "Bool",  "name": "invert",    "default": false }
  ]
}
*/
// Key — turns dark into transparent, so a layer can actually sit on top of
// another one.
//
// The problem it solves: a particle layer does not render to pure black. Its
// background is a faint haze, opaque and barely above zero, which every blend
// mode faithfully composites as a visible rectangle. Key ramps that haze down
// to real transparency and the box goes away.
//
// `threshold` is the brightness that reads as fully transparent; everything
// at `threshold + softness` and above survives untouched, with a smooth ramp
// between. `invert` keys out the BRIGHT end instead, which turns the node into
// a matte: wire it before a Mix and the highlights become the cutout.
//
// Internal color is premultiplied (INV-A, docs/alpha.md), so brightness is
// read straight off RGB — no divide by alpha — and the key scales RGB and
// alpha together, which keeps the result premultiplied.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let c = input0(uv);

    let lo = param(0u);
    // A zero-width ramp is a hard step; smoothstep needs edge1 > edge0.
    let hi = lo + max(param(1u), 1e-4);
    var k = smoothstep(lo, hi, dot(c.rgb, vec3f(0.2126, 0.7152, 0.0722)));
    if param(2u) > 0.5 {
        k = 1.0 - k;
    }

    return c * k;
}
