// Cleave background — duet afterglow whose motion crossfades with the music (#1798)
//
// The feedback advection itself switches voice: percussive content pulls
// echoes RADIALLY outward from center (each hit leaves an expanding
// shockwave ghost), harmonic content weaves them slowly sideways. The blend
// follows harmonic_ratio, so the afterglow teaches the same split as the
// particles. Cold chromatic decay: aged light turns ice-blue. Echo center
// is screen center, not the draggable fracture origin — the fragment
// uniform block carries no emitter position; the ghost reads as ambience.
// (NB: naming the uniform struct in a comment here would suppress the
// loader's uniform-block injection — it string-matches the source.)
//
// param(5) = hue           (palette anchor, shared with the sim)
// param(6) = shatter_glint (hit glint + radial echo strength)
// param(7) = trail_decay   (feedback decay)

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let decay = param(7u);

    let d = uv - 0.5;
    // Percussive pole: sample toward center -> echoes expand radially.
    let radial = -d * (0.0006 + 0.004 * u.percussive_energy * param(6u));
    // Harmonic pole: slow lateral weave (x-only, Tide shimmer idiom).
    let weave = vec2f(phosphor_noise2(vec2f(uv.y * 6.0, u.time * 0.2)) - 0.5, 0.0)
        * 0.002 * u.harmonic_energy;
    let offset = mix(radial, weave, smoothstep(0.2, 0.8, u.harmonic_ratio));
    let prev = feedback(clamp(uv + offset, vec2f(0.001), vec2f(0.999)));

    // Cold chromatic decay: red dies fastest -> aged light goes ice-blue.
    var trail = prev.rgb * decay * vec3f(0.94, 0.985, 1.0);

    // Faint key-locked aurora floor swelling with sustained content — barely
    // visible alone, it keeps a pad bridge from going pitch black.
    let aurora_n = phosphor_noise2(vec2f(uv.x * 3.0 + u.time * 0.05, uv.y * 5.0));
    let hue = fract(u.dominant_chroma * 0.15 + param(5u) * 0.3 + 0.05);
    let r = clamp(abs(hue * 6.0 - 3.0) - 1.0, 0.0, 1.0);
    let g = clamp(2.0 - abs(hue * 6.0 - 2.0), 0.0, 1.0);
    let b = clamp(2.0 - abs(hue * 6.0 - 4.0), 0.0, 1.0);
    let aurora = mix(vec3f(r, g, b), vec3f(0.8, 0.5, 0.3), 0.4)
        * aurora_n * aurora_n * 0.008 * u.harmonic_energy;

    // Hit glint: a brief cold flash injected into the feedback so every drum
    // hit leaves an expanding ghost ring. Gated on HPSS, never the kick band.
    let hit = smoothstep(0.5, 1.0, u.percussive_energy) * param(6u);
    let glint = vec3f(0.85, 0.93, 1.0) * hit * 0.02 / (1.0 + dot(d, d) * 40.0);

    // HDR clamp — mandatory: TWO additive voices feed this loop.
    let result = min(trail + aurora + glint, vec3f(1.5));
    let alpha = clamp(max(result.r, max(result.g, result.b)) * 2.0, 0.0, 1.0);
    return vec4f(result, alpha);
}
