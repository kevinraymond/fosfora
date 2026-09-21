/*! trama
{
  "name": "Scanlines",
  "id": "scanlines",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "count",     "default": 240.0, "min": 10.0, "max": 1080.0 },
    { "type": "Float", "name": "depth",     "default": 0.5,   "min": 0.0,  "max": 1.0 },
    { "type": "Float", "name": "scroll",    "default": 0.0,   "min": -4.0, "max": 4.0 },
    { "type": "Float", "name": "sharpness", "default": 0.25,  "min": 0.0,  "max": 1.0 }
  ],
  "rates": ["scroll"]
}
*/
// Scanlines — darkens the picture in horizontal bands, the CRT look.
// `sharpness` trades wide soft bands for thin hard ones, and `depth` is how
// far into black the dark band goes.
//
// `scroll` is a RATE (see "rates" above): param(2u) arrives as its running
// integral, how many lines the pattern has crept, so the bands roll the way a
// mistuned monitor rolls. Never write `u.time * scroll`; see hue_drift.wgsl.
//
// Only RGB is scaled, never alpha: this darkens the light, it does not make
// holes. Key is the node that turns dark into transparent, and putting one
// after this is how you get the bands to cut through.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let c = input0(uv);

    let phase = uv.y * max(param(0u), 1.0) + param(2u);
    let band = 0.5 + 0.5 * cos(6.2831853 * phase);
    let k = mix(1.0, pow(band, param(3u) * 8.0 + 0.25), param(1u));

    return vec4f(c.rgb * k, c.a);
}
