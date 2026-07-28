// Beam — vector-CRT oscilloscope. Draws the actual audio signal as a glowing,
// over-focused beam with phosphor persistence. First consumer of the A17
// waveform() texture and the first real consumer of u.zcr (focus/defocus).
//
// Modes (param(0u)): 0 = scope (horizontal sweep), 1 = radial (waveform wrapped
// on a circle, radius scales with rms). Lissajous needs stereo capture (A13,
// #1464) and is deferred.
//
// Technique (board #1476): integrate Gaussian beam energy along the waveform
// polyline using the 2D segment SDF (phosphor_sd_segment2), weighting each
// segment by dwell time (inverse of its screen length) for the signature
// bright-slow / dim-fast CRT look, rendered into a feedback pass whose slow
// decay is the phosphor persistence.

const PI: f32 = 3.14159265;
const TAU: f32 = 6.28318530;

// Number of neighbour samples of the trace taken per fragment (odd; centred).
const SAMPLES: i32 = 5;

// Gaussian beam falloff around a distance d with width sigma.
fn beam_glow(d: f32, sigma: f32) -> f32 {
    return exp(-(d * d) / (sigma * sigma));
}

// Soft fill of the [lo, hi] band around scalar v: 0 outside (Gaussian skirt),
// bright thin line when the band is narrow (slow signal = beam dwells), dimmer
// as the band widens (fast signal sweeps a tall column).
fn beam_band(v: f32, lo: f32, hi: f32, sigma: f32) -> f32 {
    let d = max(max(lo - v, v - hi), 0.0);
    let w = hi - lo;
    let line_w = sigma / (w + sigma);
    return beam_glow(d, sigma) * (0.35 + 0.65 * line_w);
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let aspect = res.x / res.y;
    // Centred, aspect-corrected coords. p.y increases downward, so we negate the
    // signal below to put positive amplitude up on screen.
    let p = (uv - 0.5) * vec2f(aspect, 1.0);

    // --- Params ---
    let mode = param(0u);                       // 0 scope, 1 radial
    let base_decay = param(1u);                 // 0.80..0.975
    let base_focus = mix(0.004, 0.020, param(2u)); // beam core half-width
    let intensity = param(3u);                  // 0.2..2.0 overall gain

    // --- Audio ---
    // zcr defocuses the beam (noisier signal = softer/wider trace).
    let sigma = base_focus * (1.0 + u.zcr * 3.0);
    // rms drives overall beam brightness; onset adds a transient flash.
    let gain = intensity * (0.55 + u.rms * 1.4) * (1.0 + u.onset * 1.5);
    // beat kicks the persistence (trails linger a touch longer on the beat).
    let decay = min(base_decay + u.beat * 0.04, 0.99);

    // Phosphor persistence: decayed previous frame.
    let prev = feedback(uv);
    let acc = prev.rgb * decay;

    var energy = 0.0;

    if (mode < 0.5) {
        // ---------------- SCOPE MODE ----------------
        let amp = 0.40;                         // vertical gain (signal -> screen)
        let du = 1.0 / res.x;                   // one screen pixel in uv.x
        let spacing = du * aspect;              // its screen-space width

        // Sample SAMPLES columns centred on this fragment; build the min/max
        // envelope points in screen space.
        var top: array<vec2f, 5>;               // max-envelope (upper edge)
        var bot: array<vec2f, 5>;               // min-envelope (lower edge)
        for (var i = 0; i < SAMPLES; i++) {
            let sx = uv.x + f32(i - SAMPLES / 2) * du;
            let mm = waveform(clamp(sx, 0.0, 1.0));
            let scr_x = (sx - 0.5) * aspect;
            top[i] = vec2f(scr_x, -clamp(mm.y, -1.2, 1.2) * amp);
            bot[i] = vec2f(scr_x, -clamp(mm.x, -1.2, 1.2) * amp);
        }

        // Trace both envelopes as glowing polylines, dwell-weighted by length.
        for (var i = 0; i < SAMPLES - 1; i++) {
            let dt = phosphor_sd_segment2(p, top[i], top[i + 1]);
            let db = phosphor_sd_segment2(p, bot[i], bot[i + 1]);
            let lt = length(top[i + 1] - top[i]) + 1e-4;
            let lb = length(bot[i + 1] - bot[i]) + 1e-4;
            let dwell_t = 0.25 + 0.75 * spacing / lt;
            let dwell_b = 0.25 + 0.75 * spacing / lb;
            energy += beam_glow(dt, sigma) * dwell_t;
            energy += beam_glow(db, sigma) * dwell_b;
        }

        // Soft fill between the envelopes at this column so loud sections read
        // as a filled trace rather than a hollow outline.
        let cy_top = top[SAMPLES / 2].y;
        let cy_bot = bot[SAMPLES / 2].y;
        energy += beam_band(p.y, cy_top, cy_bot, sigma * 1.6) * 0.5;
    } else {
        // ---------------- RADIAL MODE ----------------
        let ramp = 0.28;                        // signal -> radial excursion
        let ang = atan2(p.y, p.x);
        let tx = fract((ang + PI) / TAU);       // 0..1 around the circle
        let rad = length(p);
        let r0 = 0.16 + u.rms * 0.18;           // base ring radius grows with rms
        let mm = waveform(tx);
        let r_in = r0 + clamp(mm.x, -1.2, 1.2) * ramp;   // inner (min)
        let r_out = r0 + clamp(mm.y, -1.2, 1.2) * ramp;  // outer (max)
        // Band in radius, dwell-weighted by ring thickness.
        energy += beam_band(rad, r_in, r_out, sigma);
        // Sharpen the outer edge into a crisp beam line.
        energy += beam_glow(abs(rad - r_out), sigma) * 0.6;
    }

    energy = min(energy, 2.5);

    // Color: house warm->cool audio palette (centroid = colour temperature),
    // with a slow drift so persistence trails shade coherently.
    let col_t = 0.5 + 0.12 * sin(u.time * 0.15) + p.x * 0.05;
    let beam_col = phosphor_audio_palette(col_t, u.centroid, 0.0);
    let col = beam_col * energy * gain;

    // Feedback blend: fresh beam over decaying trails, clamped to avoid blowout.
    let result = max(min(col, vec3f(1.6)), acc);
    let new_alpha = clamp(max(col.r, max(col.g, col.b)) * 2.0, 0.0, 1.0);
    let result_alpha = max(new_alpha, prev.a * decay);
    return vec4f(result, result_alpha);
}
