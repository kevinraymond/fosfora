// Strata — spectral canyon. A heightfield raymarcher flown forward over the A17
// scrolling mel-spectrogram: the last ~8 seconds of the song ARE the terrain.
// Forward axis (world z) = time, lateral axis (world x) = frequency (mel band),
// height = spectral magnitude. Loud moments are ridges, quiet ones are chasms;
// because the spectrogram texture scrolls, terrain flows toward the camera.
//
// Rebuilt on proper terrain-raymarch technique (#1508, refining #1479):
//   - iq adaptive march with a distance-scaled hit tolerance (t*eps) + linear
//     crossing refine (kills overstep speckle);
//   - analytic pixel-footprint LOD: fade the rock micro-detail and grow the
//     normal epsilon with distance (kills far "TV static" + sparkly normals);
//   - Catmull-Rom interpolation across the coarse 64-band mel axis for
//     C1-continuous height (kills faceting) — kept SEPARATE from the procedural
//     rock detail so the mel lookup (the audio meaning) is never warped;
//   - iq soft raymarched shadows + height/slope materials (slope rock, strata
//     banding, snow on flat peaks) + aerial-perspective sun-tinted height fog.
// March uses cheap bilinear height; the expensive cubic + detail is only paid at
// the hit (normal/material) and shadow rays run on the cheap height.
//
// Audio: rolloff -> draw distance / fog, flatness -> surface gloss, beat_phase ->
// camera bob, kick -> brightness, beat -> wet specular, centroid -> palette temp.
// Temporal ridge-snap is smoothed upstream by the mel-EMA at ingest (audio_textures.rs).

const S_TAU: f32 = 6.28318530;

// World extents mapped onto the spectrogram.
const Z_NEAR: f32 = 0.6;    // world z at the near edge (oldest visible time)
const Z_FAR: f32 = 14.0;    // world z at the far edge (newest time)
const X_HALF: f32 = 7.0;    // lateral half-width mapped to the full mel range
const MELS: f32 = 64.0;     // mel rows in the spectrogram texture

const MAX_STEPS: i32 = 200;
const T_MIN: f32 = 0.05;
const T_MAX: f32 = 24.0;

const SCROLL_W: f32 = 512.0;   // spectrogram time-axis width (HISTORY_FRAMES)

// world (x,z) -> spectrogram uv (x=time, y=mel low..high).
// Direction: the NEWEST audio is mapped to the NEAR edge (z=Z_NEAR) so a fresh beat
// erupts closest to the camera (largest, most prominent, "a chasm opening beneath")
// and ages/recedes into the distance — nothing can occlude the newest data. Hence
// the `1.0 -` inversion. `u.scroll_phase` (0..1) is the fractional column offset
// since the last commit — added as a sub-texel time offset so the terrain scrolls
// continuously instead of snapping one whole column per audio frame (#1508 Phase 1b).
fn strata_uv(xz: vec2f) -> vec2f {
    let uvx = clamp(1.0 - (xz.y - Z_NEAR) / (Z_FAR - Z_NEAR) + u.scroll_phase / SCROLL_W, 0.0, 1.0);
    let uvy = clamp(xz.x / (2.0 * X_HALF) + 0.5, 0.0, 1.0);
    return vec2f(uvx, uvy);
}

// Two-sided height envelope:
//  - NEAR: ramp height up from 0 over the strip right in front of the camera, so a
//    loud newest ridge can't spike up *at* the camera and swallow it (near-black
//    "inside the geometry"). The newest emerges just ahead and rises as it recedes.
//  - FAR: fade to 0 at Z_FAR so rays past the oldest edge don't sample the
//    ClampToEdge-extruded column as an infinite terraced wall (the "monolith").
fn strata_edge_fade(z: f32) -> f32 {
    let near = smoothstep(Z_NEAR, Z_NEAR + 1.8, z);
    let far = 1.0 - smoothstep(Z_FAR - 2.0, Z_FAR, z);
    return near * far;
}

// Perceptual lift: raise quiet bands so high-frequency strata aren't flat, then
// boost the naturally-weaker high mel bands so ridges span the full width.
fn strata_lift(m: f32, uvy: f32) -> f32 {
    return pow(clamp(m, 0.0, 1.0), 0.55) * mix(0.85, 1.7, uvy);
}

// Sample one mel row `r` (clamped) at time `uvx`. Row centre = (r+0.5)/MELS so the
// linear sampler returns that row exactly (no vertical blend) — we do the vertical
// interpolation ourselves (Catmull-Rom) below.
fn strata_row(uvx: f32, r: f32) -> f32 {
    return spectrogram(vec2f(uvx, clamp((r + 0.5) / MELS, 0.0, 1.0)));
}

// Catmull-Rom across the coarse mel (y) axis, linear across the fine time (x) axis.
// C1-continuous height with no texel-edge slope discontinuities. Anti-ring clamped
// to the 4-tap range so sharp audio transients don't produce spurious spikes/pits.
fn strata_mel_cubic(uvx: f32, uvy: f32) -> f32 {
    let ty = uvy * MELS - 0.5;
    let r1 = floor(ty);
    let fy = ty - r1;
    let v0 = strata_row(uvx, r1 - 1.0);
    let v1 = strata_row(uvx, r1);
    let v2 = strata_row(uvx, r1 + 1.0);
    let v3 = strata_row(uvx, r1 + 2.0);
    let fy2 = fy * fy;
    let fy3 = fy2 * fy;
    let w0 = -0.5 * fy3 + fy2 - 0.5 * fy;
    let w1 = 1.5 * fy3 - 2.5 * fy2 + 1.0;
    let w2 = -1.5 * fy3 + 2.0 * fy2 + 0.5 * fy;
    let w3 = 0.5 * fy3 - 0.5 * fy2;
    let m = w0 * v0 + w1 * v1 + w2 * v2 + w3 * v3;
    let lo = min(min(v0, v1), min(v2, v3));
    let hi = max(max(v0, v1), max(v2, v3));
    return clamp(m, lo, hi);
}

// Smooth terrain base (Catmull-Rom cubic across mel) — used for the march AND as
// the base of the shading height, so ridge silhouettes are C1-smooth (no coarse
// 64-band faceting on the outline).
fn strata_base_h(xz: vec2f, hs: f32) -> f32 {
    let uv = strata_uv(xz);
    return strata_lift(strata_mel_cubic(uv.x, uv.y), uv.y) * hs * strata_edge_fade(xz.y);
}

// Shading terrain height = smooth base + separate domain-warped rock micro-detail.
// The detail is subtle, masked by mel magnitude (rock only rides loud ridges) and
// faded early by the pixel footprint `fp` (fine relief vanishes with distance ->
// no far speckle). Kept deliberately gentle: heavy detail aliases badly at grazing.
fn strata_h_shade(xz: vec2f, hs: f32, detail: f32, fp: f32) -> f32 {
    let uv = strata_uv(xz);
    let m = strata_mel_cubic(uv.x, uv.y);
    var h = strata_lift(m, uv.y) * hs;
    // Rock micro-relief — applied across the whole surface (not just loud ridges),
    // faded with the pixel footprint so it doesn't alias far off. Amplitude raised
    // + gating dropped so the `rock_detail` slider is actually visible; the gentle
    // domain warp gives organic flow without the earlier oily folding.
    let fp_fade = 1.0 - smoothstep(0.05, 0.30, fp);
    let warp = vec2f(
        fosfora_fbm2(xz * 0.6, 2, 0.5),
        fosfora_fbm2(xz * 0.6 + vec2f(3.7, 1.3), 2, 0.5)
    );
    let rock = fosfora_fbm2(xz * 1.4 + (warp - 0.5) * 0.5, 4, 0.5);
    h += (rock - 0.5) * detail * 0.16 * fp_fade;
    return h * strata_edge_fade(xz.y);
}

// Cheap bilinear height for shadow rays only (shadows tolerate coarseness, and the
// low frequency keeps them fast and un-shimmery).
fn strata_h_shadow(xz: vec2f, hs: f32) -> f32 {
    let uv = strata_uv(xz);
    return strata_lift(spectrogram(uv), uv.y) * hs * strata_edge_fade(xz.y);
}

// iq soft raymarched shadow: march toward the sun tracking the smallest k*gap/t as
// the penumbra estimate.
fn strata_shadow(p0: vec3f, ldir: vec3f, hs: f32) -> f32 {
    var res = 1.0;
    var t = 0.06;
    for (var i = 0; i < 24; i++) {
        let p = p0 + ldir * t;
        let gap = p.y - strata_h_shadow(p.xz, hs);
        if (gap < 0.001) { return 0.0; }
        res = min(res, 8.0 * gap / t);
        t += clamp(gap * 0.5, 0.05, 0.6);
        if (t > 9.0) { break; }
    }
    return clamp(res, 0.0, 1.0);
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
    let fog_amt = param(1u);                    // draw distance (higher = clearer)
    let pitch = mix(0.14, 0.40, param(2u));   // look-down amount
    let detail = param(3u);                    // rock micro-relief amount
    let snow_line = param(4u);                 // loudness threshold for snow caps
    let zoom = param(5u);                       // pull the camera up + back for an overview

    // --- Camera (stationary in world; terrain scrolls toward it) ---
    // Gentle time-based bob + slow yaw sway for life. Time-based (not raw beat_phase)
    // so a beat-tracker phase re-sync can't pop the camera. `zoom` lifts the camera
    // and tilts it down for a wider overview of the whole canyon.
    let bob = sin(t * 2.0) * 0.012;
    let sway = sin(t * 0.13) * 0.06;
    let ro = vec3f(0.0, 1.6 + zoom * 3.5 + bob, 0.0);
    let look = pitch + zoom * 0.5;   // look down more as we rise so terrain stays framed
    let fwd = normalize(vec3f(sway, -look, 1.0));
    let right = normalize(cross(vec3f(0.0, 1.0, 0.0), fwd));
    let up = cross(fwd, right);
    let fov = 1.35 - zoom * 0.3;      // slightly wider field of view when zoomed out
    let rd = normalize(scr.x * right + scr.y * up + fov * fwd);

    // Atmosphere: dramatic dark canyon — near-black moody haze/sky (only subtly
    // shifted by centroid, so it doesn't rainbow-cycle), lit by a low warm sun for
    // long shadows. Colour comes from the sun and the glowing loud strata, not fog.
    let warmth = u.centroid;   // 0 = bass-heavy/warm, 1 = bright/cool
    let fog_col = mix(vec3f(0.030, 0.045, 0.075), vec3f(0.050, 0.060, 0.080), warmth);
    let sky_col = mix(vec3f(0.008, 0.015, 0.035), vec3f(0.015, 0.022, 0.038), warmth);
    let sun = normalize(vec3f(0.55, 0.34, -0.42));   // low sun -> long dramatic shadows
    let sun_col = vec3f(1.0, 0.86, 0.66);

    // --- Raymarch: step by a fraction of the vertical gap, but CAP the step
    // relative to distance so grazing rays can't overstep thin near ridges (which
    // tunnels through terrain and smears the near field into oily "onion shells").
    // Small steps near the camera, growing gently with distance; then bisect. ---
    var tt = T_MIN;
    var hit = false;
    var reached_far = false;
    var prev_t = tt;
    for (var i = 0; i < MAX_STEPS; i++) {
        let pos = ro + rd * tt;
        let gap = pos.y - strata_base_h(pos.xz, height_scale);
        if (gap < 0.001 * tt) { hit = true; break; }
        prev_t = tt;
        tt += clamp(gap * 0.35, 0.015, 0.04 + tt * 0.05);
        if (tt > T_MAX) { reached_far = true; break; }
    }
    // Budget-exhausted rays (ran out of steps without a hit and without reaching the
    // far plane) sit in the grazing tangent band where the marcher can't converge —
    // shade them as fully-fogged terrain rather than sky, so they don't punch black
    // slivers through the mid-far terrain.
    let exhausted = !hit && !reached_far;

    var col: vec3f;

    if (hit) {
        // Bisect between the last above-surface sample and the hit — robust to any
        // residual overstep (unlike a single linear interp across a big overshoot).
        var a = prev_t;
        var b = tt;
        for (var k = 0; k < 6; k++) {
            let mid = 0.5 * (a + b);
            let gm = (ro + rd * mid).y - strata_base_h((ro + rd * mid).xz, height_scale);
            if (gm < 0.0) { b = mid; } else { a = mid; }
        }
        let th = b;
        let pos = ro + rd * th;

        // Analytic pixel footprint (world width of a pixel at this hit distance);
        // grazing views widen the xz footprint. Drives detail fade + normal eps.
        let pix = 1.0 / (res.y * fov);
        let fp = th * pix / max(abs(rd.y), 0.05);

        // Footprint-scaled finite-difference normal (never below ~1 mel texel).
        let eps = max(0.12, 0.6 * fp);
        let hL = strata_h_shade(pos.xz - vec2f(eps, 0.0), height_scale, detail, fp);
        let hR = strata_h_shade(pos.xz + vec2f(eps, 0.0), height_scale, detail, fp);
        let hB = strata_h_shade(pos.xz - vec2f(0.0, eps), height_scale, detail, fp);
        let hF = strata_h_shade(pos.xz + vec2f(0.0, eps), height_scale, detail, fp);
        let n = normalize(vec3f(hL - hR, 2.0 * eps, hB - hF));

        let uv = strata_uv(pos.xz);
        let band_t = uv.y;
        let m = strata_mel_cubic(uv.x, uv.y);   // magnitude for catch-light

        let altitude = clamp(pos.y / max(height_scale * 1.3, 0.5), 0.0, 1.0);

        // --- Materials: near-black slope rock, strata banding, snow on flat peaks ---
        // Dark base so chasms read near-black; audio only nudges warmth.
        let rock_dark = vec3f(0.014, 0.013, 0.017);              // near-black chasm rock
        let rock_mid = mix(vec3f(0.17, 0.115, 0.075), vec3f(0.10, 0.115, 0.145), warmth);
        let slope = clamp(1.0 - n.y, 0.0, 1.0);
        var mat = mix(rock_mid, rock_dark, smoothstep(0.20, 0.85, slope));
        // Strong sedimentary strata banding by height.
        mat *= 0.70 + 0.30 * sin(pos.y * 8.0 + band_t * 3.0);
        // Snow caps the LOUDEST ridges (per-ridge, driven by magnitude m — NOT a
        // global world-height line), on their flatter crests. snow_line = loudness
        // threshold: lower caps more ridges, higher only the loudest.
        let snow_amt = smoothstep(snow_line, snow_line + 0.15, m)
            * smoothstep(0.55, 0.15, slope);
        mat = mix(mat, vec3f(0.90, 0.93, 1.0), snow_amt);

        // --- Lighting: low fill (dark chasms) + hard shadowed key + rim + glow ---
        let dif = clamp(dot(n, sun), 0.0, 1.0);
        let sh = strata_shadow(pos + n * 0.03, sun, height_scale);
        let amb = 0.05 + 0.09 * clamp(n.y, 0.0, 1.0);           // low hemispheric fill
        var lit = mat * (fog_col * amb * 3.0 + sun_col * dif * sh * 1.5);
        // Valley darkening (cheap AO proxy): deep chasms fall to near-black.
        lit *= mix(0.30, 1.0, clamp(altitude * 1.6, 0.0, 1.0));
        // Warm rim-light on ridge silhouettes facing the sun.
        let rim = pow(clamp(1.0 - max(dot(n, -rd), 0.0), 0.0, 1.0), 3.0)
            * clamp(dot(n, sun) * 0.5 + 0.5, 0.0, 1.0);
        lit += sun_col * rim * 0.35 * sh;
        // Up-facing weight: gate the sparkly terms (glow + specular) to ground that
        // faces up, so aliased grazing normals can't sparkle into coloured speckle.
        let face = clamp(n.y * 1.3, 0.0, 1.0);
        // Smooth beat envelope (decays over the first half of each beat) instead of
        // the raw on/off u.beat — avoids the 7x specular strobe on every beat.
        let beat_env = pow(max(1.0 - u.beat_phase * 2.0, 0.0), 2.0) * u.beat_strength;
        // Loud strata GLOW with their frequency colour (the "spectral" accent —
        // audio colour appears as emissive on ridges, not as a whole-scene cycle).
        // Kick brightens the glow locally (not the whole frame -> no bloom pop).
        let glow_col = fosfora_audio_palette(band_t * 0.9 + 0.05, u.centroid, 0.1);
        lit += glow_col * smoothstep(0.55, 0.95, m) * (0.35 + beat_env * 0.3)
            * (1.0 + u.kick * 0.5) * face;
        // Wet specular on beat — modest gloss, smooth envelope, gated to up-facing ground.
        let gloss = mix(6.0, 22.0, 1.0 - u.flatness);
        let h_vec = normalize(sun - rd);
        let spec = pow(clamp(dot(n, h_vec), 0.0, 1.0), gloss) * sh * face;
        lit += sun_col * spec * (0.06 + beat_env * 0.3);

        // --- Aerial-perspective fog: distance + height falloff + sun tint ---
        // draw_distance (fog_amt): higher = clearer / see farther (less fog).
        let fog_density = mix(0.09, 0.004, fog_amt) * (1.3 - u.rolloff * 0.5);
        var fog = 1.0 - exp(-th * fog_density);
        fog *= exp(-max(pos.y - 0.6, 0.0) * 0.35);   // peaks poke above the haze
        let sun_amt = pow(max(dot(rd, sun), 0.0), 8.0);
        let fog_tint = mix(fog_col, sun_col * 0.55, sun_amt);
        col = mix(lit, fog_tint, clamp(fog, 0.0, 1.0));
    } else if (exhausted) {
        // Grazing tangent band the marcher couldn't resolve — paint fog, not sky.
        col = fog_col;
    } else {
        // Sky: near-black zenith down to the dark fog band at the horizon, with a
        // warm sun glow and a faint horizon haze for depth.
        let grad = clamp(rd.y * 1.8 + 0.15, 0.0, 1.0);
        col = mix(fog_col, sky_col, grad);
        col += sun_col * pow(max(dot(rd, sun), 0.0), 5.0) * 0.7;          // sun bloom
        col += fog_col * pow(clamp(1.0 - abs(rd.y), 0.0, 1.0), 10.0) * 0.5; // horizon band
    }

    let a = clamp(max(col.r, max(col.g, col.b)) * 1.5, 0.0, 1.0);
    return vec4f(col, a);
}
