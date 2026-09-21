/*! trama
{
  "name": "Levels",
  "id": "levels",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "brightness", "default": 0.0, "min": -1.0, "max": 1.0 },
    { "type": "Float", "name": "contrast",   "default": 1.0, "min": 0.0,  "max": 4.0 },
    { "type": "Float", "name": "gamma",      "default": 1.0, "min": 0.2,  "max": 4.0 },
    { "type": "Float", "name": "saturation", "default": 1.0, "min": 0.0,  "max": 3.0 }
  ]
}
*/
// Levels — brightness, contrast, gamma and saturation, the grading workhorse.
// Put it in front of a Key to decide what counts as "dark", or behind one to
// bring back the punch the key took out.
//
// Tone curves are not linear, so they do not commute with premultiplied alpha
// (INV-A, docs/alpha.md): the color has to be divided back out by coverage,
// graded, and multiplied in again, or a half-covered edge grades as if it were
// half as bright. A pixel with no coverage but light in it (RGB > A: glow,
// bloom spill) is the exception — there is no coverage to divide by, so its
// RGB is graded as light and left unpremultiplied.

const LEVELS_LUMA = vec3f(0.2126, 0.7152, 0.0722);

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let c = input0(uv);

    let lit = c.a <= 1e-4;
    var rgb = select(c.rgb / max(c.a, 1e-4), c.rgb, lit);

    rgb = pow(max(rgb, vec3f(0.0)), vec3f(1.0 / max(param(2u), 1e-3)));
    rgb = (rgb - 0.5) * param(1u) + 0.5 + param(0u);
    rgb = mix(vec3f(dot(max(rgb, vec3f(0.0)), LEVELS_LUMA)), rgb, param(3u));
    rgb = max(rgb, vec3f(0.0));

    return vec4f(rgb * select(c.a, 1.0, lit), c.a);
}
