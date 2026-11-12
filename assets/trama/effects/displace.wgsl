/*! trama
{
  "name": "Displace",
  "id": "displace",
  "kind": "effect",
  "inputs": 2,
  "params": [
    { "type": "Float", "name": "amount", "default": 0.5, "min": -1.0, "max": 1.0 },
    { "type": "Float", "name": "mode",   "default": 0.0, "min": 0.0,  "max": 2.0 }
  ],
  "integers": ["mode"]
}
*/
// Displace — warps input 0 by the brightness of input 1, the compositor's
// displacement blends as a chain node. Wire the picture to warp into the
// first input and any pattern (Noise Field, Gradient, a feedback loop, another
// layer's chain) into the second; the second input draws none of its own
// color.
//
// `mode` picks the same three warps as the layer blend modes, with the same
// math and the same 0.12-of-the-frame reach at full `amount`:
//   0 Displace — pushes along the map's brightness slope, so edges shove the
//     picture and flat regions leave it alone: shockwaves, heat haze.
//   1 Refract  — the map as thick glass: its brightness sets how far, its
//     slope which way, and red and blue split apart either side of green.
//   2 Lens     — bright areas magnify about the frame's center, a bulge.
// A negative `amount` warps the other way: Displace pulls instead of
// pushing, Lens pinches instead of bulging.
//
// The warp follows the map's coverage, as a displacement blend follows its
// layer's: where the map is transparent nothing moves. An unwired map reads
// as transparent black, so the node passes the picture through untouched.

// Shared with assets/shaders/builtin/composite.wgsl — see the reasoning there.
const DISPLACE_MAX_WARP_UV: f32 = 0.12;
const DISPLACE_GRAD_STEP: f32 = 0.004;
const DISPLACE_GRAD_GAIN: f32 = 6.0;
const DISPLACE_REFRACT_BODY_GAIN: f32 = 1.4;
const DISPLACE_REFRACT_DISPERSION: f32 = 0.35;
const DISPLACE_LUMA = vec3f(0.2126, 0.7152, 0.0722);

fn displace_map_luma(uv: vec2f) -> f32 {
    return dot(input1(uv).rgb, DISPLACE_LUMA);
}

// Central-difference slope of the map's brightness across DISPLACE_GRAD_STEP.
fn displace_map_gradient(uv: vec2f) -> vec2f {
    let s = DISPLACE_GRAD_STEP;
    let dx = displace_map_luma(uv + vec2f(s, 0.0)) - displace_map_luma(uv - vec2f(s, 0.0));
    let dy = displace_map_luma(uv + vec2f(0.0, s)) - displace_map_luma(uv - vec2f(0.0, s));
    return vec2f(dx, dy) * 0.5 * DISPLACE_GRAD_GAIN;
}

fn displace_clamp(offset: vec2f) -> vec2f {
    let len = length(offset);
    if len > DISPLACE_MAX_WARP_UV {
        return offset * (DISPLACE_MAX_WARP_UV / len);
    }
    return offset;
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let src = input0(uv);
    let coverage = clamp(input1(uv).a, 0.0, 1.0);
    let amount = param(0u) * DISPLACE_MAX_WARP_UV;

    var warped: vec4f;
    switch u32(round(clamp(param(1u), 0.0, 2.0))) {
        case 1u: {
            let slope = displace_map_gradient(uv);
            let dir = normalize(slope + vec2f(1e-6, 0.0));
            let thickness = clamp(displace_map_luma(uv), 0.0, 1.0);
            let bend = thickness * clamp(length(slope) * 2.0, 0.0, 1.0);
            let offset = displace_clamp(dir * bend * amount * DISPLACE_REFRACT_BODY_GAIN);
            let g = input0(uv + offset);
            warped = vec4f(
                input0(uv + offset * (1.0 + DISPLACE_REFRACT_DISPERSION)).r,
                g.g,
                input0(uv + offset * (1.0 - DISPLACE_REFRACT_DISPERSION)).b,
                g.a,
            );
        }
        case 2u: {
            warped = input0(uv + displace_clamp((uv - 0.5) * displace_map_luma(uv) * amount));
        }
        default: {
            warped = input0(uv + displace_clamp(displace_map_gradient(uv) * amount));
        }
    }

    return mix(src, warped, coverage);
}
