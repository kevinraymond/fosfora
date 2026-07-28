// Chromatica — chord mandala + key-locked palette (#1477)
// Twelve concentric rings, one per pitch class, spaced around the circle of fifths so
// consonant notes sit radially adjacent. Each ring blooms with its chroma[pc] energy, so
// the mandala visibly swells with the chord being played. The song's detected key rotates
// the whole palette (via phosphor_key_hue) and a minor key droops + cools the geometry.
// An interval_edges toggle overlays a consonance graph: lines between simultaneously
// sounding pitch classes, warm-gold for consonant intervals, tense-violet for dissonant.
// A max-decay feedback trail gives cathedral-glass persistence.

const TAU: f32 = 6.28318530718;

// Full-wheel colour from a hue in 0..1 (iq cosine palette, rainbow variant).
fn chroma_wheel(h: f32) -> vec3f {
    return phosphor_palette(fract(h), vec3f(0.5), vec3f(0.5), vec3f(1.0), vec3f(0.0, 0.33, 0.67));
}

// Consonance weight for an interval in semitones, folded to interval class 0..6.
// 1.0 = most consonant (unison/octave, perfect fifth), ~0.1 = most dissonant (tritone, m2).
fn interval_consonance(semis: i32) -> f32 {
    let ic = ((semis % 12) + 12) % 12;
    let folded = min(ic, 12 - ic);
    var table = array<f32, 7>(1.0, 0.15, 0.32, 0.68, 0.74, 0.95, 0.08);
    return table[folded];
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let aspect = res.x / res.y;
    var p = (uv - 0.5) * vec2f(aspect, 1.0);
    let t = u.time;

    // param(0) ring_spacing, (1) bloom_gain, (2) arc_thickness, (3) rotation_speed,
    // (4) palette_shift, (5) minor_droop, (6) feedback_amount, (7) interval_edges,
    // (8) consonance_gain, (9) glow
    let ring_spacing = 0.03 + param(0u) * 0.04;
    let bloom_gain = 0.4 + param(1u) * 1.1;
    let arc_thick = 0.5 + param(2u) * 1.5;
    let rot_speed = param(3u);
    let pal_shift = param(4u);
    let minor_droop = param(5u);
    let feedback_amount = param(6u);
    let edges_master = param(7u); // continuous overlay opacity, not a toggle
    let consonance_gain = 0.3 + param(8u) * 1.2;
    let glow = 0.2 + param(9u) * 1.0;
    let edge_spin = (param(10u) - 0.5) * 2.0; // -1..1, independent graph rotation
    let edge_breath = param(11u);             // expand/contract amount

    // Key-locked global hue + major/minor geometry warp.
    let key_hue = phosphor_key_hue(u.key_class, u.key_is_minor);
    let minor = clamp(u.key_is_minor, 0.0, 1.0);
    let droop = minor * minor_droop;
    p.y = p.y - droop * 0.05 * (1.0 - p.y * p.y); // sag the lower half
    p.x = p.x * (1.0 + droop * 0.06);             // gentle elliptical squash

    let r = length(p);
    let ang = atan2(p.y, p.x);
    let base_rot = u.beat_phase * TAU * rot_speed * 0.25 + t * 0.03 * rot_speed;

    var col = vec3f(0.0);

    // Node positions on a shared circle-of-fifths ring, for the consonance overlay.
    var node_pos: array<vec2f, 12>;
    var node_amp: array<f32, 12>;
    // The consonance graph moves on its own: an independent spin, and an audio-driven breath
    // (idle sway + swell with loudness + punch out on kicks) that expands/contracts the ring.
    let node_rot = t * edge_spin * 0.6;
    let breath = 0.12 * sin(t * 0.6) + u.rms * 0.15 + u.kick * 0.18;
    let node_ring_r = 0.34 * (1.0 + edge_breath * breath);
    var total = 0.0;

    // --- 12 concentric mandala rings, fifths-ordered (s*7 mod 12 is a bijection over 0..11) ---
    for (var s = 0; s < 12; s = s + 1) {
        let pc = (s * 7) % 12;
        let ch = clamp(chroma_val(u32(pc)), 0.0, 1.0);
        total = total + ch;

        let radius = 0.09 + f32(s) * ring_spacing;
        // Mandala petals: a distinct rotating petal count per ring.
        let petals = f32(pc + 3);
        let pet = 0.7 + 0.3 * cos(petals * (ang + base_rot));
        // Thin Gaussian ring band; energy only slightly widens it (bloom lives in brightness).
        let width = 0.0035 + ch * 0.006;
        let d = r - radius;
        let band = exp(-(d * d) / (width * width));

        let ring_hue = key_hue + f32(pc) / 12.0 * 0.5 + pal_shift * 0.3;
        var ring_col = chroma_wheel(ring_hue);
        ring_col = mix(ring_col, ring_col.bgr, minor * 0.25); // cool bias in minor keys

        col = col + ring_col * band * pet * (0.04 + ch * 0.5 * bloom_gain);

        // Representative node for the consonance overlay (independent rotation + breath).
        let na = f32(s) / 12.0 * TAU + node_rot;
        node_pos[pc] = vec2f(cos(na), sin(na)) * node_ring_r;
        node_amp[pc] = ch;
    }

    // --- centre bloom: total chord energy glows, dominant pitch colours it ---
    let cen = exp(-r * r * 45.0) * clamp(total, 0.0, 3.0) * 0.12 * glow;
    col = col + chroma_wheel(key_hue + u.dominant_chroma * 0.15) * cen;

    // --- consonance overlay: living tension edges + nodes on the fifths ring ---
    // Each edge carries a travelling light pulse (tempo-locked). Dissonant intervals pulse
    // faster and shimmer, so a diminished chord visibly trembles while a fifth glows steady.
    if (edges_master > 0.001) {
        let line_w = (0.002 + u.bandwidth * 0.005) * arc_thick; // bandwidth -> line thickness
        for (var i = 0; i < 12; i = i + 1) {
            let ai = node_amp[i];
            if (ai < 0.12) { continue; }
            for (var j = i + 1; j < 12; j = j + 1) {
                let aj = node_amp[j];
                if (aj < 0.12) { continue; }
                let cons = interval_consonance(i - j);
                let diss = 1.0 - cons;

                // distance to the segment + position along it (h in 0..1)
                let a = node_pos[i];
                let ba = node_pos[j] - a;
                let bl = max(dot(ba, ba), 1e-6);
                let h = clamp(dot(p - a, ba) / bl, 0.0, 1.0);
                let seg = length((p - a) - ba * h);
                let line = exp(-(seg * seg) / (line_w * line_w));

                // travelling pulse along the edge; faster and jitterier when dissonant
                let phase = t * (1.5 + diss * 4.5) + u.beat_phase * TAU;
                let flow = 0.45 + 0.55 * sin(h * (5.0 + diss * 12.0) - phase);
                let seed = f32(i * 13 + j) * 1.7;
                let shimmer = 1.0 - diss * 0.5 * (0.5 + 0.5 * sin(t * 24.0 + seed));

                let warm = vec3f(1.0, 0.82, 0.4);
                let tense = vec3f(0.8, 0.2, 0.98);
                let ecol = mix(tense, warm, cons);
                let ew = ai * aj * consonance_gain * 0.6 * mix(0.45, 1.0, cons)
                    * u.key_confidence * (1.0 + u.onset * 0.6);
                col = col + ecol * line * flow * shimmer * ew * edges_master;
            }
        }
        for (var k = 0; k < 12; k = k + 1) {
            let nd = length(p - node_pos[k]);
            let nglow = exp(-(nd * nd) / 0.00025);
            let np = 0.1 + node_amp[k] * (0.7 + u.onset * 0.5);
            col = col + chroma_wheel(key_hue + f32(k) / 12.0) * nglow * np * edges_master;
        }
    }

    // --- feedback: short additive glass trail that clears (geometric series, no smear wash) ---
    col = min(col, vec3f(1.1));
    let prev = feedback(uv);
    var result = col + prev.rgb * (feedback_amount * 0.5);

    // Onset shimmer, gated by chord energy so silence stays still; after blend so it never accumulates.
    result = result + chroma_wheel(key_hue) * u.onset * 0.25 * smoothstep(0.0, 0.3, total);

    return vec4f(min(result, vec3f(1.2)), 1.0);
}
