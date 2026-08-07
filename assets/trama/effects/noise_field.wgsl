/*! trama
{
  "name": "Noise Field",
  "id": "noise_field",
  "kind": "source",
  "inputs": 0,
  "params": [
    { "type": "Float", "name": "scale",    "default": 3.0,  "min": 0.5,  "max": 12.0 },
    { "type": "Float", "name": "speed",    "default": 0.25, "min": 0.0,  "max": 2.0 },
    { "type": "Float", "name": "octaves",  "default": 4.0,  "min": 1.0,  "max": 6.0 },
    { "type": "Float", "name": "contrast", "default": 1.0,  "min": 0.25, "max": 4.0 }
  ]
}
*/
// Noise Field — drifting monochrome fBM, the trama hello-world source.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let p = (uv - 0.5) * vec2f(res.x / res.y, 1.0);
    let n = fosfora_fbm3(vec3f(p * param(0u), u.time * param(1u)), i32(param(2u)), 0.5);
    let v = pow(clamp(n * 0.5 + 0.5, 0.0, 1.0), param(3u));
    return vec4f(v, v, v, 1.0);
}
