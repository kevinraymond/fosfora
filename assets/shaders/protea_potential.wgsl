// Protea — scent pass: the smooth field the mass flow condenses toward (Flow Lenia).
//   input0 = mass field, PREVIOUS frame (a prev_input; rgb = species A/B/C mass)
//   output = (S_a, S_b, S_c, S_total): per-species affinity-weighted ring density + total
//
// This is NOT a fixed-density Lenia growth curve (that tiles the plane with regular
// spots). It is a smooth "scent": each species senses its own body plus its prey
// (attract) minus its predator (avoid), ring-averaged at its own body scale. The mass
// pass flows UP this gradient, so mass condenses toward density maxima into distinct
// clumps that chase prey and flee predators — leaving empty water between them instead of
// filling the frame. The chroma-driven kernel radius still varies the body scale by region.
//
// Non-feedback pass at scale 0.5: size comes from input0_tex, never u.resolution.

const TAU: f32 = 6.28318530718;

// Signed affinity: what each species smells in (A, B, C) mass. Self 1.0 forms the body;
// +next species = prey (flow toward it), -previous = predator (flow away). The asymmetry
// biases the scent gradient so bodies translate and chase rather than sitting still.
const SENSE_A = vec3f(1.0, -0.45, 0.60);
const SENSE_B = vec3f(0.60, 1.0, -0.45);
const SENSE_C = vec3f(-0.45, 0.60, 1.0);

// Per-species kernel radii in sim texels — three well-separated body scales.
const RADII = vec3f(9.0, 15.0, 24.0);

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(input0_tex));
    let texel = 1.0 / dims;
    let uv = frag_coord.xy / dims;
    let aspect = dims.x / dims.y;

    let radius_p = 0.6 + 0.8 * param(1u);  // p1 kernel radius
    let mutation = param(5u);              // p5 chroma sector mutation depth

    // Bass swells every kernel — creatures physically inflate on low end.
    let radii = RADII * radius_p * (1.0 + 0.30 * u.bass);

    // Chroma 12-sector spatial map: each pitch class scales the body radius of its angular
    // sector, so key changes remap where small vs large creatures live. Lerped at the seams.
    let ctr = (uv - vec2f(0.5)) * vec2f(aspect, 1.0);
    let sector = (atan2(ctr.y, ctr.x) / TAU + 0.5) * 12.0;
    let s0 = u32(sector) % 12u;
    let ch = mix(chroma_val(s0), chroma_val((s0 + 1u) % 12u), fract(sector));
    let r_mod = 1.0 + mutation * 0.6 * (ch - 0.35);
    let rr = radii * r_mod;

    // Two-ring shell (16 dirs x 2 radii) — the ring-averaged species density and total.
    var scent = vec3f(0.0);
    var total = 0.0;
    var wsum = 0.0;
    for (var i = 0u; i < 16u; i = i + 1u) {
        let ang = TAU * (f32(i) + 0.5) / 16.0;
        let dir = vec2f(cos(ang), sin(ang));
        let w0 = 0.55;
        let w1 = 1.0;
        let m0a = input0(uv + dir * (0.45 * rr.x) * texel).rgb;
        let m0b = input0(uv + dir * (0.45 * rr.y) * texel).rgb;
        let m0c = input0(uv + dir * (0.45 * rr.z) * texel).rgb;
        let m1a = input0(uv + dir * (0.9 * rr.x) * texel).rgb;
        let m1b = input0(uv + dir * (0.9 * rr.y) * texel).rgb;
        let m1c = input0(uv + dir * (0.9 * rr.z) * texel).rgb;
        scent += w0 * vec3f(dot(SENSE_A, m0a), dot(SENSE_B, m0b), dot(SENSE_C, m0c));
        scent += w1 * vec3f(dot(SENSE_A, m1a), dot(SENSE_B, m1b), dot(SENSE_C, m1c));
        // Total (unsigned) density, sensed at the mid species scale — the pressure field.
        total += w0 * (m0b.r + m0b.g + m0b.b) + w1 * (m1b.r + m1b.g + m1b.b);
        wsum += w0 + w1;
    }
    scent /= wsum;
    total /= wsum;

    return vec4f(scent, total);
}
