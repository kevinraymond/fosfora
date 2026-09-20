/*! trama
{
  "name": "Hue Drift",
  "id": "hue_drift",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "shift", "default": 0.0, "min": 0.0, "max": 1.0 },
    { "type": "Float", "name": "speed", "default": 0.2, "min": 0.0, "max": 4.0 }
  ],
  "rates": ["speed"]
}
*/
// Hue Drift — rotates the input's hue by shift + drift (in turns), via the
// palette library's Rodrigues rotation.
//
// `speed` is a RATE (see "rates" above): param(1u) arrives as its running
// integral, the drift so far — NOT as the slider value. Never write
// `u.time * speed`: every change in speed is then multiplied by the app's
// uptime, and a modulated speed strobes.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let c = input0(uv);
    return vec4f(fosfora_hue_shift(c.rgb, param(0u) + param(1u)), c.a);
}
