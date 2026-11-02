// Layer compositor — blends a foreground layer onto a background accumulator.
// Operates in HDR space (before tonemapping).

struct CompositeUniforms {
    blend_mode: u32,
    opacity: f32,
    displace_amount: f32,
    _pad1: f32,
}

@group(0) @binding(0) var bg_texture: texture_2d<f32>;
@group(0) @binding(1) var bg_sampler: sampler;
@group(0) @binding(2) var fg_texture: texture_2d<f32>;
@group(0) @binding(3) var fg_sampler: sampler;
@group(0) @binding(4) var<uniform> comp: CompositeUniforms;

// --- Blend mode functions (operate per-channel in HDR) ---
//
// Screen, Overlay, Hard Light and Exclusion are 0..1 formulas: fed HDR values
// they invert (Screen of 2 over 2 is 0, so bloom turned black) or go negative
// (Exclusion of 2 over 2 is -4). They are evaluated on the 0..1 part of each
// input, and the part above 1 passes through additively, as in Add. For inputs
// in 0..1 that is exactly the classic formula; above it the result never drops
// below the bright input's excess and never goes negative (#93).

fn ldr(c: vec3f) -> vec3f {
    return clamp(c, vec3f(0.0), vec3f(1.0));
}

fn hdr_excess(bg: vec3f, fg: vec3f) -> vec3f {
    return max(bg - vec3f(1.0), vec3f(0.0)) + max(fg - vec3f(1.0), vec3f(0.0));
}

fn blend_add(bg: vec3f, fg: vec3f) -> vec3f {
    return bg + fg;
}

fn blend_screen(bg: vec3f, fg: vec3f) -> vec3f {
    let b = ldr(bg);
    let f = ldr(fg);
    return b + f - b * f + hdr_excess(bg, fg);
}

fn blend_color_dodge(bg: vec3f, fg: vec3f) -> vec3f {
    let HDR_MAX = 4.0;
    return min(bg / max(vec3f(1.0) - fg, vec3f(0.001)), vec3f(HDR_MAX));
}

fn blend_multiply(bg: vec3f, fg: vec3f) -> vec3f {
    return bg * fg;
}

fn blend_overlay_ch(bg: f32, fg: f32) -> f32 {
    if bg < 0.5 {
        return 2.0 * bg * fg;
    } else {
        return 1.0 - 2.0 * (1.0 - bg) * (1.0 - fg);
    }
}

fn blend_overlay(bg: vec3f, fg: vec3f) -> vec3f {
    let b = ldr(bg);
    let f = ldr(fg);
    return vec3f(
        blend_overlay_ch(b.x, f.x),
        blend_overlay_ch(b.y, f.y),
        blend_overlay_ch(b.z, f.z),
    ) + hdr_excess(bg, fg);
}

fn blend_hard_light_ch(bg: f32, fg: f32) -> f32 {
    if fg < 0.5 {
        return 2.0 * bg * fg;
    } else {
        return 1.0 - 2.0 * (1.0 - bg) * (1.0 - fg);
    }
}

fn blend_hard_light(bg: vec3f, fg: vec3f) -> vec3f {
    let b = ldr(bg);
    let f = ldr(fg);
    return vec3f(
        blend_hard_light_ch(b.x, f.x),
        blend_hard_light_ch(b.y, f.y),
        blend_hard_light_ch(b.z, f.z),
    ) + hdr_excess(bg, fg);
}

fn blend_difference(bg: vec3f, fg: vec3f) -> vec3f {
    return abs(bg - fg);
}

fn blend_exclusion(bg: vec3f, fg: vec3f) -> vec3f {
    let b = ldr(bg);
    let f = ldr(fg);
    return b + f - 2.0 * b * f + hdr_excess(bg, fg);
}

fn blend_subtract(bg: vec3f, fg: vec3f) -> vec3f {
    return max(bg - fg, vec3f(0.0));
}

// Lighten and Darken are per-channel max/min, which hold for any input range,
// so they need no HDR split.
fn blend_lighten(bg: vec3f, fg: vec3f) -> vec3f {
    return max(bg, fg);
}

fn blend_darken(bg: vec3f, fg: vec3f) -> vec3f {
    return min(bg, fg);
}

// Soft Light, the W3C compositing formula: Overlay's shape with the foreground
// steering it, gentler at the extremes. Like Overlay it is a 0..1 formula, so
// it takes the same HDR split.
fn blend_soft_light_ch(bg: f32, fg: f32) -> f32 {
    if fg <= 0.5 {
        return bg - (1.0 - 2.0 * fg) * bg * (1.0 - bg);
    }
    var d: f32;
    if bg <= 0.25 {
        d = ((16.0 * bg - 12.0) * bg + 4.0) * bg;
    } else {
        d = sqrt(bg);
    }
    return bg + (2.0 * fg - 1.0) * (d - bg);
}

fn blend_soft_light(bg: vec3f, fg: vec3f) -> vec3f {
    let b = ldr(bg);
    let f = ldr(fg);
    return vec3f(
        blend_soft_light_ch(b.x, f.x),
        blend_soft_light_ch(b.y, f.y),
        blend_soft_light_ch(b.z, f.z),
    ) + hdr_excess(bg, fg);
}

// --- Hue, Saturation, Color, Luminosity ---
//
// The W3C non-separable blends: each takes some of hue, saturation and
// luminance from one input and the rest from the other. Luminance is the
// Rec. 709 `luma` this file already uses (the composite is linear light), not
// the spec's sRGB-era 0.3/0.59/0.11.
//
// They are 0..1 formulas (clip_color pulls a result back into range), so they
// run on the 0..1 part of each input. What happens to the part above 1 follows
// which input the mode keeps the brightness of: Hue, Saturation and Color keep
// the background's, so its excess passes through; Luminosity keeps the
// foreground's, so its does. A bright bloom under a Hue layer stays bright.

fn hsl_max(c: vec3f) -> f32 {
    return max(c.x, max(c.y, c.z));
}

fn hsl_min(c: vec3f) -> f32 {
    return min(c.x, min(c.y, c.z));
}

fn clip_color(c: vec3f) -> vec3f {
    let l = luma(c);
    let n = hsl_min(c);
    let x = hsl_max(c);
    var r = c;
    if n < 0.0 {
        r = vec3f(l) + (r - vec3f(l)) * l / max(l - n, 1e-6);
    }
    if x > 1.0 {
        r = vec3f(l) + (r - vec3f(l)) * (1.0 - l) / max(x - l, 1e-6);
    }
    return r;
}

fn set_lum(c: vec3f, l: f32) -> vec3f {
    return clip_color(c + vec3f(l - luma(c)));
}

fn sat(c: vec3f) -> f32 {
    return hsl_max(c) - hsl_min(c);
}

// The spec's SetSat sorts the channels and rescales the middle one; for each
// channel that is the same as stretching (c - min) over the new range.
fn set_sat(c: vec3f, s: f32) -> vec3f {
    let lo = hsl_min(c);
    let range = hsl_max(c) - lo;
    if range <= 0.0 {
        return vec3f(0.0);
    }
    return (c - vec3f(lo)) * (s / range);
}

fn excess(c: vec3f) -> vec3f {
    return max(c - vec3f(1.0), vec3f(0.0));
}

fn blend_hue(bg: vec3f, fg: vec3f) -> vec3f {
    let b = ldr(bg);
    return set_lum(set_sat(ldr(fg), sat(b)), luma(b)) + excess(bg);
}

fn blend_saturation(bg: vec3f, fg: vec3f) -> vec3f {
    let b = ldr(bg);
    return set_lum(set_sat(b, sat(ldr(fg))), luma(b)) + excess(bg);
}

fn blend_color(bg: vec3f, fg: vec3f) -> vec3f {
    return set_lum(ldr(fg), luma(ldr(bg))) + excess(bg);
}

fn blend_luminosity(bg: vec3f, fg: vec3f) -> vec3f {
    return set_lum(ldr(bg), luma(ldr(fg))) + excess(fg);
}

// --- Displacement modes (#1478) ---
//
// These read the foreground as a warp *field* rather than an image: its
// luminance offsets the UV used to sample the accumulated composite beneath.
// The foreground contributes no color of its own. Unlike the blends above they
// need `uv`, so they can't be expressed as blend_*(bg, fg) and are handled in a
// separate branch of fs_main.
//
// The trama Displace node (assets/trama/effects/displace.wgsl) runs the same
// three warps with copies of the constants below; change them together.

// Largest UV offset a warp may produce, so `displace_amount` can stay a 0..1
// control like opacity. Also the clamp that keeps HDR foregrounds honest —
// luminance is unbounded above 1.0, so an un-clamped gradient could sample
// halfway across the frame from a single bright particle.
const MAX_WARP_UV: f32 = 0.12;

// How far apart the gradient taps sit, in UV. Deliberately NOT one texel.
//
// Two reasons. A one-texel stencil measures per-texel slope, and this engine's
// output is mostly soft glow — a bloomed ring spanning 50 texels has a slope of
// ~0.02, which at full strength warped by about two pixels, i.e. the effect was
// invisible on exactly the content it exists to warp. And a texel-sized stencil
// makes the result resolution-dependent: the same scene would warp differently
// at 1080p and 4K, which is not something a VJ can work with. A fixed UV step
// reads structure at a scale comparable to the warp it drives, at any output size.
const GRAD_STEP: f32 = 0.004;

// Maps a typical soft-glow gradient onto a usable share of MAX_WARP_UV. Hard
// edges overshoot and are caught by the clamp, which is the intended behaviour:
// sharp foregrounds warp to the limit, soft ones scale in below it.
const GRAD_GAIN: f32 = 6.0;

// Refract drives its magnitude from luminance (0..1) rather than from a slope,
// so it needs its own gain to reach a comparable share of MAX_WARP_UV.
const REFRACT_BODY_GAIN: f32 = 1.4;

// Per-channel spread, applied symmetrically about the offset. Large on purpose:
// dispersion is Refract's visual signature, and the first cut's 6% split was
// measured at 8% of the warp — invisible next to Displace.
const REFRACT_DISPERSION: f32 = 0.35;

fn luma(c: vec3f) -> f32 {
    return dot(c, vec3f(0.2126, 0.7152, 0.0722));
}

fn fg_luma(uv: vec2f) -> f32 {
    return luma(textureSample(fg_texture, fg_sampler, uv).rgb);
}

// Central-difference gradient of foreground luminance across GRAD_STEP.
fn fg_luma_gradient(uv: vec2f) -> vec2f {
    let dx = fg_luma(uv + vec2f(GRAD_STEP, 0.0)) - fg_luma(uv - vec2f(GRAD_STEP, 0.0));
    let dy = fg_luma(uv + vec2f(0.0, GRAD_STEP)) - fg_luma(uv - vec2f(0.0, GRAD_STEP));
    return vec2f(dx, dy) * 0.5 * GRAD_GAIN;
}

fn clamp_offset(offset: vec2f) -> vec2f {
    let len = length(offset);
    if len > MAX_WARP_UV {
        return offset * (MAX_WARP_UV / len);
    }
    return offset;
}

fn sample_bg(uv: vec2f) -> vec3f {
    return textureSample(bg_texture, bg_sampler, uv).rgb;
}

@fragment
fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
    let bg = textureSample(bg_texture, bg_sampler, uv);
    let fg = textureSample(fg_texture, fg_sampler, uv);

    // Displacement family: warp the background, draw none of the foreground.
    // Handled before the color switch because these need `uv`.
    if comp.blend_mode >= 10u && comp.blend_mode <= 12u {
        let amount = comp.displace_amount * MAX_WARP_UV;
        var warped: vec3f;
        switch comp.blend_mode {
            // Refract: the foreground as a slab of textured glass.
            //
            // Deliberately body-driven where Displace is edge-driven. The bend
            // DIRECTION still comes from the slope, but its MAGNITUDE comes from
            // luminance — thickness — so the whole interior of a bright shape
            // carries the image sideways, not just its rim. That is what makes
            // this a different mode rather than a tinted Displace: a different
            // part of the frame moves.
            //
            // Then real dispersion on top. Split symmetrically about the offset
            // (red long, blue short) so the fringe is twice the width a one-sided
            // split of the same coefficient would give.
            case 11u: {
                let slope = fg_luma_gradient(uv);
                let dir = normalize(slope + vec2f(1e-6, 0.0));
                let thickness = clamp(fg_luma(uv), 0.0, 1.0);
                // Slope still gates it: a perfectly flat region of glass, however
                // bright, bends nothing. Softly, so interiors survive.
                let bend = thickness * clamp(length(slope) * 2.0, 0.0, 1.0);
                let offset = clamp_offset(dir * bend * amount * REFRACT_BODY_GAIN);
                warped = vec3f(
                    sample_bg(uv + offset * (1.0 + REFRACT_DISPERSION)).r,
                    sample_bg(uv + offset).g,
                    sample_bg(uv + offset * (1.0 - REFRACT_DISPERSION)).b,
                );
            }
            // Lens: foreground luminance magnifies radially about frame centre,
            // so a bright blob reads as a bulge rather than a directional shove.
            case 12u: {
                let offset = clamp_offset((uv - 0.5) * fg_luma(uv) * amount);
                warped = sample_bg(uv + offset);
            }
            // Displace (10u): straight push along the luminance gradient.
            default: {
                let offset = clamp_offset(fg_luma_gradient(uv) * amount);
                warped = sample_bg(uv + offset);
            }
        }
        // Warp strength follows opacity and foreground coverage, exactly like a
        // color blend: sparse effects warp only where they have particles,
        // full-frame effects warp by gradient (zero across flat regions).
        // Alpha is the background's alone — a pure field adds no coverage.
        return vec4f(mix(bg.rgb, warped, comp.opacity * fg.a), bg.a);
    }

    // Normal (0u): the premultiplied over operator (INV-A, docs/alpha.md). Layer
    // RGB is already scaled by its coverage, so mixing it by fg.a again darkened
    // every soft edge (#94). Coverage is clamped: additive particle passes can
    // accumulate alpha past 1, which would push bg's weight negative.
    if comp.blend_mode == 0u {
        let a = clamp(fg.a, 0.0, 1.0) * comp.opacity;
        return vec4f(fg.rgb * comp.opacity + bg.rgb * (1.0 - a), a + bg.a * (1.0 - a));
    }

    var blended: vec3f;
    switch comp.blend_mode {
        case 1u: { blended = blend_add(bg.rgb, fg.rgb); }
        case 2u: { blended = blend_screen(bg.rgb, fg.rgb); }
        case 3u: { blended = blend_color_dodge(bg.rgb, fg.rgb); }
        case 4u: { blended = blend_multiply(bg.rgb, fg.rgb); }
        case 5u: { blended = blend_overlay(bg.rgb, fg.rgb); }
        case 6u: { blended = blend_hard_light(bg.rgb, fg.rgb); }
        case 7u: { blended = blend_difference(bg.rgb, fg.rgb); }
        case 8u: { blended = blend_exclusion(bg.rgb, fg.rgb); }
        case 9u: { blended = blend_subtract(bg.rgb, fg.rgb); }
        case 13u: { blended = blend_lighten(bg.rgb, fg.rgb); }
        case 14u: { blended = blend_darken(bg.rgb, fg.rgb); }
        case 15u: { blended = blend_soft_light(bg.rgb, fg.rgb); }
        case 16u: { blended = blend_hue(bg.rgb, fg.rgb); }
        case 17u: { blended = blend_saturation(bg.rgb, fg.rgb); }
        case 18u: { blended = blend_color(bg.rgb, fg.rgb); }
        case 19u: { blended = blend_luminosity(bg.rgb, fg.rgb); }
        default: { blended = fg.rgb; }
    }

    // Mix with opacity: lerp between background and blended result
    let result = mix(bg.rgb, blended, comp.opacity * fg.a);
    return vec4f(result, max(bg.a, fg.a * comp.opacity));
}
