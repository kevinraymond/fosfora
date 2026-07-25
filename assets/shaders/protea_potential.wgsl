// Protea — sense pass: ring-kernel convolution + growth, per species (Flow Lenia).
//   input0 = mass field, PREVIOUS frame (a prev_input; rgb = species A/B/C mass)
//   output = (G_a, G_b, G_c, U_mean): per-species growth in [-1, 1] + mean potential
//
// Each species senses the world through a two-ring kernel (16 directions x 2 radii)
// with its own radius, so the three species live at three different body scales. The
// sensed value is a signed affinity mix of all three mass channels — cyclic
// predation (A hunts C hunts B hunts A) so blobs chase instead of settling. The
// growth curve G is applied HERE so the mass pass's gradients read grad(G) directly.
//
// Non-feedback pass at scale 0.5: size comes from input0_tex, never u.resolution.

const TAU: f32 = 6.28318530718;

// Signed affinity: what each species smells in (A, B, C) mass. +next species = prey
// attraction, -previous = predator avoidance; self-sensing 1.0 forms the body. The
// asymmetry (prey pull != predator push) biases the growth gradient off-centre, so
// bodies translate and chase instead of locking into a static Turing lattice.
const SENSE_A = vec3f(1.0, -0.45, 0.60);
const SENSE_B = vec3f(0.60, 1.0, -0.45);
const SENSE_C = vec3f(-0.45, 0.60, 1.0);

// Per-species kernel radii in sim texels — three well-separated creature scales so the
// colony carries small, medium, and large bodies at once rather than one wavelength.
const RADII = vec3f(6.0, 12.0, 20.0);

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(input0_tex));
    let texel = 1.0 / dims;
    let uv = frag_coord.xy / dims;
    let aspect = dims.x / dims.y;

    let radius_p = 0.6 + 0.8 * param(1u);  // p1 kernel radius
    let mu_p = 0.6 + 0.8 * param(2u);      // p2 growth center
    let sigma_p = 0.6 + 0.8 * param(3u);   // p3 growth width
    let mutation = param(5u);              // p5 chroma sector mutation depth

    // Bass swells every kernel — creatures physically inflate on low end.
    let radii = RADII * radius_p * (1.0 + 0.30 * u.bass);

    // Two-ring shell approximating the Lenia ring kernel: taps at 0.45R and 0.9R.
    var pot = vec3f(0.0);
    var wsum = 0.0;
    for (var i = 0u; i < 16u; i = i + 1u) {
        let ang = TAU * (f32(i) + 0.5) / 16.0;
        let dir = vec2f(cos(ang), sin(ang));
        // ring 0: r=0.45R w=0.55 · ring 1: r=0.9R w=1.0 (peak off-center = Lenia)
        let r0 = 0.45;
        let w0 = 0.55;
        let r1 = 0.9;
        let w1 = 1.0;
        let m0a = input0(uv + dir * (r0 * radii.x) * texel).rgb;
        let m0b = input0(uv + dir * (r0 * radii.y) * texel).rgb;
        let m0c = input0(uv + dir * (r0 * radii.z) * texel).rgb;
        let m1a = input0(uv + dir * (r1 * radii.x) * texel).rgb;
        let m1b = input0(uv + dir * (r1 * radii.y) * texel).rgb;
        let m1c = input0(uv + dir * (r1 * radii.z) * texel).rgb;
        pot += w0 * vec3f(dot(SENSE_A, m0a), dot(SENSE_B, m0b), dot(SENSE_C, m0c));
        pot += w1 * vec3f(dot(SENSE_A, m1a), dot(SENSE_B, m1b), dot(SENSE_C, m1c));
        wsum += w0 + w1;
    }
    pot /= wsum;

    // Chroma 12-sector spatial parameter map: the growth optimum shifts per angular
    // sector with that pitch class's energy, so key changes mutate regional species.
    // Sectors are lerped at the seams to avoid visible pie-slice edges.
    let ctr = (uv - vec2f(0.5)) * vec2f(aspect, 1.0);
    let sector = (atan2(ctr.y, ctr.x) / TAU + 0.5) * 12.0;
    let s0 = u32(sector) % 12u;
    let ch = mix(chroma_val(s0), chroma_val((s0 + 1u) % 12u), fract(sector));
    let mu = vec3f(0.22, 0.26, 0.30) * mu_p * (1.0 + mutation * 0.8 * (ch - 0.35));
    let sigma = vec3f(0.10, 0.11, 0.12) * sigma_p;

    // Lenia growth: gaussian bump around the per-species optimum, in [-1, 1].
    let d = (pot - mu) / sigma;
    let growth = 2.0 * exp(-0.5 * d * d) - 1.0;

    let u_mean = clamp(dot(pot, vec3f(1.0 / 3.0)), 0.0, 1.0);
    return vec4f(growth, u_mean);
}
