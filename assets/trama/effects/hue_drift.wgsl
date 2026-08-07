/*! trama
{
  "name": "Hue Drift",
  "id": "hue_drift",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "shift", "default": 0.0, "min": 0.0, "max": 1.0 },
    { "type": "Float", "name": "speed", "default": 0.2, "min": 0.0, "max": 4.0 }
  ]
}
*/
// Hue Drift — rotates the input's hue by shift + time * speed (in turns).

// YIQ-axis rotation: cheap, artifact-free for VJ use.
fn hue_rotate(c: vec3f, a: f32) -> vec3f {
    let k = vec3f(0.57735);
    return c * cos(a) + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - cos(a));
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let c = input0(uv);
    let a = (param(0u) + u.time * param(1u)) * 6.2831853;
    return vec4f(hue_rotate(c.rgb, a), c.a);
}
