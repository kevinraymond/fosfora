// Strata — spectral canyon. A heightfield raymarcher flown forward over the A17
// scrolling mel-spectrogram: the last ~8 seconds of the song ARE the terrain.
// Forward axis (world z) = time, lateral axis (world x) = frequency (mel band),
// height = spectral magnitude. Loud moments are ridges, quiet ones are chasms;
// because the spectrogram texture scrolls, terrain flows toward the camera.
//
// First consumer of spectrogram() and of u.rolloff. Audio: rolloff -> draw
// distance / fog, flatness -> surface roughness (tonal = glassy, noise = scree),
// beat_phase -> camera bob, kick -> camera lift, centroid -> palette temperature,
// beat -> wet specular. Reference for the flythrough feel: tunnel.wgsl.

const S_TAU: f32 = 6.28318530;

// World extents mapped onto the spectrogram.
const Z_NEAR: f32 = 0.6;    // world z at the near edge (oldest visible time)
const Z_FAR: f32 = 14.0;    // world z at the far edge (newest time)
const X_HALF: f32 = 7.0;    // lateral half-width mapped to the full mel range

const MAX_STEPS: i32 = 140;
const T_MIN: f32 = 0.05;
const T_MAX: f32 = 24.0;

// Sample the terrain height at world (x = xz.x lateral, z = xz.y forward).
fn strata_h(xz: vec2f, hs: f32, detail: f32) -> f32 {
    // z -> time (oldest 0 .. newest 1); x -> mel (0..1).
    let uvx = clamp((xz.y - Z_NEAR) / (Z_FAR - Z_NEAR), 0.0, 1.0);
    let uvy = clamp(xz.x / (2.0 * X_HALF) + 0.5, 0.0, 1.0);
    // Smooth across the coarse mel axis (only 64 bands) to kill the faceted /
    // stair-stepped look; the 512-frame time axis is fine enough to leave alone.
    let ms = 1.0 / 64.0;
    var m = spectrogram(vec2f(uvx, uvy)) * 0.5
        + spectrogram(vec2f(uvx, uvy + ms)) * 0.25
        + spectrogram(vec2f(uvx, uvy - ms)) * 0.25;
    // Lift quiet bands (a perceptual curve) so high-frequency strata aren't flat,
    // then boost the naturally-weaker high mel bands so ridges span the full width.
    m = pow(clamp(m, 0.0, 1.0), 0.55) * mix(0.85, 1.7, uvy);
    let rock = phosphor_fbm2(xz * 1.0, 3, 0.5);
    return m * hs + rock * detail * 0.035;
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    // Aspect-correct screen coords, centred, y up.
    let sc = (frag_coord.xy - 0.5 * res) / res.y;
    let scr = vec2f(sc.x, -sc.y);
    let t = u.time;

    // --- Params ---
    let height_scale = mix(0.5, 3.0, param(0u));
    let fog_amt = param(1u);
    let pitch = mix(0.14, 0.40, param(2u));   // look-down amount
    let detail = param(3u);                    // rock micro-noise amount

    // --- Camera (stationary in world; terrain scrolls toward it) ---
    // Very gentle beat bob only — the kick drives brightness, not camera height
    // (a large kick lift read as distracting bounce).
    let bob = sin(u.beat_phase * S_TAU) * 0.02;
    let cam_h = 1.6 + bob;
    let ro = vec3f(0.0, cam_h, 0.0);

    // Camera basis: forward +z, tilted down by `pitch`.
    let fwd = normalize(vec3f(0.0, -pitch, 1.0));
    let right = vec3f(1.0, 0.0, 0.0);
    let up = cross(fwd, right);
    let fov = 1.35;
    let rd = normalize(scr.x * right + scr.y * up + fov * fwd);

    // --- Raymarch the heightfield (linear march, growing steps, refine on cross) ---
    var tt = T_MIN;
    var hit = false;
    var prev_t = tt;
    for (var i = 0; i < MAX_STEPS; i++) {
        let pos = ro + rd * tt;
        let diff = pos.y - strata_h(pos.xz, height_scale, detail);
        if (diff < 0.0) {
            hit = true;
            break;
        }
        prev_t = tt;
        // Conservative step, capped so we can't tunnel through thin ridges (speckle).
        tt += clamp(diff * 0.35, 0.015, 0.4) + tt * 0.004;
        if (tt > T_MAX) { break; }
    }

    // Atmosphere colours: a cool, fairly dark fog/sky so terrain reads against it.
    let sky_col = phosphor_audio_palette(0.6, u.centroid, 0.0) * 0.35;
    let fog_col = phosphor_audio_palette(0.5, u.centroid, 0.15) * 0.45;
    var col: vec3f;

    if (hit) {
        // Refine the crossing with bisection.
        var a = prev_t;
        var b = tt;
        for (var k = 0; k < 6; k++) {
            let mid = 0.5 * (a + b);
            let pm = ro + rd * mid;
            if (pm.y - strata_h(pm.xz, height_scale, detail) < 0.0) { b = mid; } else { a = mid; }
        }
        let th = b;
        let pos = ro + rd * th;

        // Terrain normal from finite differences (wide epsilon = softer, less speckly).
        let e = 0.12;
        let hL = strata_h(pos.xz - vec2f(e, 0.0), height_scale, detail);
        let hR = strata_h(pos.xz + vec2f(e, 0.0), height_scale, detail);
        let hB = strata_h(pos.xz - vec2f(0.0, e), height_scale, detail);
        let hF = strata_h(pos.xz + vec2f(0.0, e), height_scale, detail);
        let n = normalize(vec3f(hL - hR, 2.0 * e, hB - hF));

        // Band colour: lateral position = frequency -> palette temperature.
        let band = clamp(pos.x / (2.0 * X_HALF) + 0.5, 0.0, 1.0);
        let uvx = clamp((pos.z - Z_NEAR) / (Z_FAR - Z_NEAR), 0.0, 1.0);
        let mag = spectrogram(vec2f(uvx, band));
        let base_col = phosphor_audio_palette(band * 0.95 + 0.05, u.centroid, 0.12);

        // Lighting: low ambient, strong key light for contrast.
        let light = normalize(vec3f(0.5, 0.7, -0.35));
        let dif = clamp(dot(n, light), 0.0, 1.0);
        let amb = 0.10 + 0.16 * n.y;
        var lit = base_col * (amb + dif * 1.15);

        // Ridge tops glow with their magnitude (loud strata catch the light).
        lit += base_col * smoothstep(0.35, 1.0, mag) * 0.6;

        // Wet specular on beat (glassy when tonal via flatness).
        let gloss = mix(8.0, 40.0, 1.0 - u.flatness);
        let h_vec = normalize(light - rd);
        let spec = pow(clamp(dot(n, h_vec), 0.0, 1.0), gloss);
        lit += vec3f(spec) * (0.08 + u.beat * 0.5);

        // Distance fog; rolloff opens up the draw distance (brighter spectrum = clearer).
        let fog_density = mix(0.02, 0.10, fog_amt) * (1.2 - u.rolloff * 0.6);
        let fog = 1.0 - exp(-th * fog_density);
        col = mix(lit, fog_col, fog);
    } else {
        // Sky: darker toward the top, brighter atmospheric band at the horizon.
        let grad = clamp(rd.y * 2.0 + 0.2, 0.0, 1.0);
        col = mix(fog_col, sky_col, grad);
    }

    // Kick flash lifts the whole frame briefly.
    col *= 1.0 + u.kick * 0.2;

    let a = clamp(max(col.r, max(col.g, col.b)) * 1.5, 0.0, 1.0);
    return vec4f(col, a);
}
