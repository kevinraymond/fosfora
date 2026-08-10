/*! trama
{
  "name": "Mix",
  "id": "mix",
  "kind": "effect",
  "inputs": 2,
  "params": [
    { "type": "Float", "name": "amount", "default": 0.5, "min": 0.0, "max": 1.0 }
  ]
}
*/
// Mix — crossfades input 0 toward input 1. The 2-input workhorse: feed it a
// source and a feedback loop and `amount` becomes the echo strength. An
// unwired input reads as transparent black (the executor binds a 1x1
// placeholder), so a half-patched mix fades to black rather than erroring.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    return mix(input0(uv), input1(uv), param(0u));
}
