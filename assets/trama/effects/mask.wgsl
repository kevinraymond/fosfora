/*! trama
{
  "name": "Mask",
  "id": "mask",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "center_x",  "default": 0.5,  "min": 0.0, "max": 1.0 },
    { "type": "Float", "name": "center_y",  "default": 0.65, "min": 0.0, "max": 1.0 },
    { "type": "Float", "name": "width",     "default": 0.3,  "min": 0.0, "max": 1.0 },
    { "type": "Float", "name": "height",    "default": 0.25, "min": 0.0, "max": 1.0 },
    { "type": "Float", "name": "softness",  "default": 0.05, "min": 0.0, "max": 0.5 },
    { "type": "Bool",  "name": "rectangle", "default": false },
    { "type": "Bool",  "name": "invert",    "default": false }
  ]
}
*/
// Mask — keeps an ellipse (or rectangle) of the picture and makes the rest
// transparent. On a webcam layer under an effect that reads the layers
// beneath it (Fluvid), it limits where that effect sees motion: put it over
// your mouth and only your mouth makes smoke.
//
// Position and size are fractions of the frame (0,0 top left); `softness`
// feathers the edge, in the same units. `invert` cuts the shape OUT instead,
// e.g. to ignore a ceiling fan in one corner.
//
// Premultiplied in, premultiplied out: the mask scales RGB and alpha together.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let c = input0(uv);

    let half_size = max(vec2f(param(2u), param(3u)) * 0.5, vec2f(1e-4));
    let p = (uv - vec2f(param(0u), param(1u))) / half_size;
    var r = length(p);
    if param(5u) > 0.5 {
        r = max(abs(p.x), abs(p.y));
    }
    // Softness in frame units, converted to this shape's normalized radius.
    let soft = max(param(4u), 1e-4) / min(half_size.x, half_size.y);
    var k = 1.0 - smoothstep(1.0 - soft, 1.0 + soft, r);
    if param(6u) > 0.5 {
        k = 1.0 - k;
    }
    return c * k;
}
