/*! trama
{
  "name": "Threshold",
  "id": "threshold",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "threshold", "default": 0.5,  "min": 0.0, "max": 1.0 },
    { "type": "Float", "name": "softness",  "default": 0.02, "min": 0.0, "max": 0.5 },
    { "type": "Float", "name": "amount",    "default": 1.0,  "min": 0.0, "max": 1.0 },
    { "type": "Bool",  "name": "invert",    "default": false }
  ]
}
*/
// Threshold — two tones: white where the picture is brighter than
// `threshold`, black where it is darker. `softness` widens the cut into a
// ramp, `invert` swaps the tones, and `amount` crossfades from the original.
// Put Palette Map after it to make the two tones any pair of colors.
//
// Unlike Key, which turns the dark side transparent, this keeps alpha as it
// is: the shape of the layer is untouched, only its tones change.
//
// Brightness is measured in display space (sqrt of linear luma) so the
// threshold slider's middle is the picture's visual middle grey.

const THRESHOLD_LUMA = vec3f(0.2126, 0.7152, 0.0722);

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let c = input0(uv);
    if c.a <= 1e-4 {
        return c;
    }

    let rgb = c.rgb / c.a;
    let y = sqrt(clamp(dot(rgb, THRESHOLD_LUMA), 0.0, 1.0));
    let t = param(0u);
    let s = max(param(1u), 1e-4);
    var v = smoothstep(t - s, t + s, y);
    if param(3u) > 0.5 {
        v = 1.0 - v;
    }
    let out_rgb = mix(rgb, vec3f(v), clamp(param(2u), 0.0, 1.0));
    return vec4f(out_rgb * c.a, c.a);
}
