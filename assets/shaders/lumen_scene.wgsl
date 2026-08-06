// Lumen — scene pass: build the light field + occlusion the radiance cascades light.
//   feedback() = own previous frame (used ONLY for correct size at scale 0.5; not blended)
//   output     = (r,g,b = emission HDR, a = occlusion optical density)
//
// Two things live here, both fed by the music:
//   * Fireflies — many small coloured emitters. Firefly i takes the hue of pitch class
//     i%12 and lights up with that class's chroma energy, so loud notes light their own
//     lights; kick/onset flash them all. They drift and orbit at `motion` speed.
//   * A soft "dancer" silhouette — one or two large metaballs that bass inflates and that
//     turn slowly. Written into alpha as optical density; the cascade march treats it as an
//     occluder, so every firefly casts a soft coloured shadow behind it.
//
// Feedback pass at scale 0.5: size comes from prev_frame, never u.resolution.

const TAU: f32 = 6.28318530718;
const MAX_FIRE: i32 = 48;

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(prev_frame));
    let uv = frag_coord.xy / dims;
    let aspect = dims.x / dims.y;
    // Isotropic screen space (y is the reference axis) so blobs stay round.
    let p = vec2f((uv.x - 0.5) * aspect, uv.y - 0.5);

    let motion = 0.15 + 1.1 * param(0u);      // p0 drift/orbit speed
    let count_p = param(1u);                  // p1 firefly count
    let occ_size = param(4u);                 // p4 silhouette size
    let warmth = param(7u);                   // p7 palette warmth
    let t = u.time * motion;

    let n = i32(6.0 + f32(MAX_FIRE - 6) * count_p);
    // A global flash: every light pulses on kick, sparks on onset.
    let flash = 1.0 + 2.4 * u.kick + 1.8 * u.onset;

    var emission = vec3f(0.0);
    for (var i = 0; i < MAX_FIRE; i = i + 1) {
        if (i >= n) { break; }
        let fi = f32(i);
        let h1 = fosfora_hash2(vec2f(fi + 1.0, 1.3));
        let h2 = fosfora_hash2(vec2f(fi + 3.0, 7.1));
        let h3 = fosfora_hash2(vec2f(fi + 5.0, 3.7));
        let h4 = fosfora_hash2(vec2f(fi + 9.0, 5.9));

        // Scattered orbit centre + a slow elliptical orbit; half the swarm counter-rotates.
        let center = vec2f((h1 - 0.5) * 0.85 * aspect, (h2 - 0.5) * 0.85);
        let orbit_r = 0.05 + 0.30 * h3;
        let spin = select(-1.0, 1.0, h4 > 0.5);
        let ph = h4 * TAU + t * (0.4 + 0.9 * h2) * spin;
        let pos = center + orbit_r * vec2f(cos(ph), 0.72 * sin(ph));

        // Colour + brightness from the firefly's pitch class.
        let pc = u32(i) % 12u;
        let hue = f32(pc) / 12.0;
        let col = fosfora_hue_shift(vec3f(1.0, 0.32, 0.12), hue);
        let energy = 0.30 + 1.5 * chroma_val(pc);

        let d = p - pos;
        let fr = 0.014 + 0.020 * h1;           // firefly radius (isotropic units)
        let g = exp(-dot(d, d) / (fr * fr));
        emission += col * (energy * flash) * g;
    }

    // "Dancer" silhouette — two merged metaballs, bass-inflated, slowly turning. Stored as
    // occlusion density in alpha; a tall x-squash makes it read as a figure, not a disc.
    var occ = 0.0;
    for (var j = 0u; j < 2u; j = j + 1u) {
        let js = f32(j);
        let oc = vec2f(
            0.16 * cos(t * 0.5 + js * 2.3) + (js - 0.5) * 0.20 * aspect,
            0.12 * sin(t * 0.4 + js * 1.7),
        );
        let od = (p - oc) * vec2f(1.7, 0.85);   // squash x → taller than wide
        let rr = (0.09 + 0.11 * occ_size) * (1.0 + 0.45 * u.bass);
        occ += smoothstep(rr, rr * 0.35, length(od));
    }
    let density = clamp(occ, 0.0, 1.0);

    // Warmth tilt on the whole light field (cool → warm).
    emission *= mix(vec3f(0.82, 0.94, 1.18), vec3f(1.20, 1.0, 0.78), warmth);

    return vec4f(emission, density);
}
