/*! trama
{
  "name": "Color Grade",
  "id": "color_grade",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "lift",        "default": 0.0, "min": -0.5, "max": 0.5 },
    { "type": "Float", "name": "gamma",       "default": 1.0, "min": 0.2,  "max": 4.0 },
    { "type": "Float", "name": "gain",        "default": 1.0, "min": 0.0,  "max": 4.0 },
    { "type": "Float", "name": "temperature", "default": 0.0, "min": -1.0, "max": 1.0 },
    { "type": "Float", "name": "tint",        "default": 0.0, "min": -1.0, "max": 1.0 },
    { "type": "Float", "name": "hue",         "default": 0.0, "min": -0.5, "max": 0.5 },
    { "type": "Float", "name": "saturation",  "default": 1.0, "min": 0.0,  "max": 3.0 },
    { "type": "Color", "name": "shadows",     "default": [1.0, 1.0, 1.0, 1.0] },
    { "type": "Color", "name": "highlights",  "default": [1.0, 1.0, 1.0, 1.0] }
  ]
}
*/
// Color Grade — a colorist's controls: lift, gamma and gain, white balance,
// hue and saturation. Where Levels makes a picture brighter or punchier, this
// changes what color it is: warm the highlights and cool the shadows for a
// film look, or pull one effect's palette toward another's so two layers sit
// together.
//
// - `lift` raises (or crushes) the blacks and leaves white where it is;
//   `gain` scales the whole range; `gamma` bends the middle between them.
// - `temperature` runs blue (−) to amber (+), `tint` green (−) to magenta
//   (+). Both keep brightness, so sweeping them only moves the color.
// - `hue` rotates every color by up to half a turn either way.
// - `shadows` and `highlights` push the dark and bright ends toward a color,
//   the lift and gain wheels of a grading suite. White, or any gray, is
//   neutral; the more saturated the color, the harder the push.
//
// Params are packed in order: the seven floats are param(0u)..param(6u), so
// `shadows` is param(7u)..param(10u) and `highlights` param(11u)..param(14u).
// The colors' alpha is unused.
//
// Grading happens on the color divided back out by coverage, as in Levels
// (INV-A, docs/alpha.md), so a half-covered edge grades like its interior.
// `lift` fades out toward white, so light brighter than white (bloom, HDR
// glow) is not lifted further; `gain` and `gamma` apply to it as to the rest.

const GRADE_LUMA = vec3f(0.2126, 0.7152, 0.0722);

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let c = input0(uv);

    let lit = c.a <= 1e-4;
    var rgb = max(select(c.rgb / max(c.a, 1e-4), c.rgb, lit), vec3f(0.0));

    // White balance as per-channel gains, renormalized to unit luminance so
    // temperature and tint shift the color without dimming or brightening it.
    let temperature = param(3u) * 0.3;
    let tint = param(4u) * 0.3;
    var balance = max(vec3f(1.0 + temperature, 1.0 - tint, 1.0 - temperature), vec3f(0.0));
    balance /= max(dot(balance, GRADE_LUMA), 1e-4);
    rgb *= balance;

    // The wheels: a color's offset from its own gray, so white and every gray
    // are neutral and saturation sets how hard each channel is pushed.
    let shadows = vec3f(param(7u), param(8u), param(9u));
    let highlights = vec3f(param(11u), param(12u), param(13u));
    let lift = vec3f(param(0u)) + (shadows - dot(shadows, GRADE_LUMA)) * 0.25;
    let gain = vec3f(param(2u)) * max(vec3f(1.0) + highlights - dot(highlights, GRADE_LUMA), vec3f(0.0));

    rgb = gain * (rgb + lift * (vec3f(1.0) - min(rgb, vec3f(1.0))));
    rgb = pow(max(rgb, vec3f(0.0)), vec3f(1.0 / max(param(1u), 1e-3)));

    rgb = fosfora_hue_shift(rgb, param(5u));
    rgb = mix(vec3f(dot(max(rgb, vec3f(0.0)), GRADE_LUMA)), rgb, param(6u));
    rgb = max(rgb, vec3f(0.0));

    return vec4f(rgb * select(c.a, 1.0, lit), c.a);
}
