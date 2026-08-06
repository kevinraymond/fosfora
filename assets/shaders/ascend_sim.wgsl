// Ascend particle simulation — a spectral ridgeline that rises with brightness (#1801, reworked #1441)
//
// The horizon is a MOUNTAIN RANGE built from the seven frequency bands: sub-bass
// at the left edge through brilliance at the right, each band raising its own
// peak. rolloff (spectral brightness) lifts the whole range — a bright, airy
// passage sends the peaks towering up the frame, a sub-heavy one sinks them to a
// low ridge on the floor. Because the terrain is per-band it is never a rigid
// bar sliding up and down: each peak breathes with its own part of the spectrum.
//
// Unpinned by design (reworked #1441): brightness may lift the range all the way
// up and past the top of the frame. The earlier version stayed in the lower half
// and read as a flat, barely-moving band.
//
// Consumer of `bandwidth` (peak body) and the last of the dormant spectral-shape
// features (#1490), plus the seven band energies for the ridge itself.
//
// NO TRAIL RING: this sim never calls trail_write, and the .pfx sets
// trail_length 0. particle_lib declares trail_buffer at @group(2) @binding(0);
// since #1921 the system always creates that binding (a dummy when trails are
// off), so a sim may reference trail_write with trail_length 0 without failing
// pipeline validation. This sim omits it anyway — it has no trail to write.
// --- Param mapping ---
// param(0) = altitude      (how far brightness lifts the whole range)
// param(1) = relief        (how tall the per-band peaks grow)
// param(2) = shimmer       (zcr-driven horizontal jitter)
// param(3) = flow          (lateral drift speed)
// param(4) = hue           (palette anchor offset)
// param(5) = glow          (brightness gain)
// param(6) = baseline      (resting height of the range)
// param(7) = trail_decay   (bg shader: feedback decay)

// All per-index randomness here uses the lib's uhash/uhash_f (integer mixing).
// fract-sin hash() bands idx-scaled arguments onto near-identical values, which
// in a field this wide reads as vertical seams of clumped particles.

// The seven band energies in spectral order, sub_bass..brilliance. Already
// normalized 0..1 by the analyzer. There is no lib accessor for these (unlike
// band_pan), so index them here.
fn band_amp(i: u32) -> f32 {
    switch i {
        case 0u: { return u.sub_bass; }
        case 1u: { return u.bass; }
        case 2u: { return u.low_mid; }
        case 3u: { return u.mid; }
        case 4u: { return u.upper_mid; }
        case 5u: { return u.presence; }
        default: { return u.brilliance; }
    }
}

// Resting floor of the range, low in the frame. The peaks and the brightness
// lift build UP from here.
fn range_base() -> f32 {
    return -0.92 + param(6u) * 0.5;
}

// Crest height of the ridgeline at a given x, in clip space.
//
// Sum of seven smooth peaks, one per band, spaced across the width. rolloff is
// the 85%-energy frequency (FixedRange, so it holds its last value on silence
// instead of collapsing) — it lifts the whole range, which is what makes the
// terrain track timbre rather than level and keeps climbing on a bright swell.
fn ridgeline_y(x: f32) -> f32 {
    let altitude = clamp(u.rolloff, 0.0, 1.0) * param(0u) * 1.5;

    // Seven band peaks centred from x=-1.05 (sub-bass) to x=+1.05 (brilliance).
    // Width 0.20 is narrow enough that a loud band stands up as its own summit
    // with a real valley beside a quiet neighbour, instead of every band melting
    // into one plateau. A sub-1 power lifts quiet bands into foothills rather
    // than letting them vanish flat.
    var ridge = 0.0;
    for (var i = 0u; i < 7u; i = i + 1u) {
        let cx = -1.05 + f32(i) * 0.35;
        let d = (x - cx) / 0.20;
        let bump = exp(-d * d);
        ridge = ridge + bump * pow(clamp(band_amp(i), 0.0, 1.0), 0.75);
    }
    // A kick punches the whole range up briefly, so a four-on-the-floor pulse
    // reads in the terrain even when the bands themselves are steady.
    ridge = ridge + clamp(u.kick, 0.0, 1.0) * 0.35;
    ridge = ridge * param(1u) * 1.05;

    // Rolling foothills that live even on a flat spectrum, so an evenly-lit
    // passage still reads as terrain rather than a bar. Three incommensurate
    // travelling waves; the fine ripple leans on presence so it only agitates
    // when there is high-frequency content to justify it.
    let swell = sin(x * 1.7 + u.time * 0.28) * 0.06
        + sin(x * 3.9 - u.time * 0.19) * 0.035
        + sin(x * 7.3 + u.time * 0.44) * 0.02 * (0.3 + u.presence);

    return range_base() + altitude + ridge + swell;
}

fn ascend_color(height_t: f32, energy: f32) -> vec3f {
    // Hue rises with altitude: the range warms low on the floor and cools as it
    // climbs, so height is readable even in a still frame.
    let hue_t = fract(param(4u) * 0.5 + 0.55 + height_t * 0.30);
    var col = fosfora_audio_palette(hue_t, 0.5 + 0.5 * clamp(u.centroid, 0.0, 1.0), u.time * 0.015);
    col = mix(col, vec3f(0.85, 0.93, 1.0), clamp(height_t, 0.0, 1.0) * 0.35);
    return col * (0.15 + 1.0 * param(5u) * (0.25 + 0.75 * clamp(energy, 0.0, 1.0)));
}

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let h = uhash(idx * 0x9e3779b9u + u32(u.seed * 4096.0));
    let r0 = f32(uhash(h)) / 4294967296.0;
    let r1 = f32(uhash(h + 1u)) / 4294967296.0;
    let r2 = f32(uhash(h + 2u)) / 4294967296.0;
    let r3 = f32(uhash(h + 3u)) / 4294967296.0;

    // Spread across the full width, a little past the edges so the range has no
    // visible ends.
    let x = r0 * 2.4 - 1.2;

    let crest = ridgeline_y(x);
    // Fill DOWNWARD from the crest so a peak has a body, not just an outline.
    // The body reaches a fraction of the way from the crest to the floor, so a
    // tall peak is a tall mountain and a low one is a mound. bandwidth (spectral
    // spread) fattens it: a pure tone draws a taut ridge, broadband noise
    // inflates it into a massif. A power keeps density high at the bright crest
    // and thins toward the base.
    let reach = 0.35 + clamp(u.bandwidth, 0.0, 1.0) * 0.45;
    let depth = max(crest - range_base(), 0.12) * reach;
    // crest_t: 0 at the bright crest, 1 at the deep base. The power concentrates
    // particles near the crest, so the ridge has a dense lit edge over a thinner
    // body — a mountain silhouette, not a uniform slab.
    let crest_t = pow(r1, 1.6);
    let below = crest_t * depth;
    // A little symmetric fuzz so the crest is a soft glowing edge, not a razor.
    let fuzz = (r2 - 0.5) * 0.03;
    let offset = -below + fuzz;

    let y = crest + offset;

    let vel = vec2f((r2 - 0.5) * 0.05 * param(3u), (r3 - 0.5) * 0.02);
    let init_size = u.initial_size * (0.5 + 0.9 * r2);
    let life = u.lifetime * (0.65 + 0.7 * r3);

    p.pos_life = vec4f(x, y, 0.0, 1.0);
    p.vel_size = vec4f(vel, init_size, init_size);
    p.color = vec4f(ascend_color(0.5, u.rms), 0.0);
    // flags: (age, lifetime, offset-from-crest, crest_t 0=crest..1=base)
    p.flags = vec4f(0.0, life, offset, crest_t);
    return p;
}

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles { return; }

    var p = read_particle(idx);

    if p.pos_life.w <= 0.0 {
        let slot = emit_claim();
        if slot < u.emit_count {
            p = emit_particle(idx);
            write_particle(idx, p);
            mark_alive(idx);
        } else {
            write_particle(idx, p);
        }
        return;
    }

    let age = p.flags.x;
    let max_life = p.flags.y;
    let new_age = age + u.delta_time;
    if new_age >= max_life {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    let dt = u.delta_time;
    let life_frac = new_age / max_life;
    var pos = p.pos_life.xy;
    var vel = p.vel_size.xy;
    let init_size = p.vel_size.z;
    let offset = p.flags.z;
    let crest_t = p.flags.w;

    // Lateral flow, with zcr shimmering the field sideways. zcr rises on noisy /
    // breathy material, so hiss visibly agitates the surface while a clean tone
    // leaves it gliding.
    let shimmer = clamp(u.zcr, 0.0, 1.0) * param(2u);
    vel.x += (uhash_f(idx * 2654435761u + u32(u.time * 60.0)) - 0.5) * shimmer * 0.9 * dt;
    vel.x += 0.04 * param(3u) * dt;

    // Follow the ridgeline as the terrain morphs. The particle keeps its own
    // offset below the crest, so the whole massif rises and falls as one surface
    // instead of each particle chasing an absolute height and shearing it apart.
    let target_y = ridgeline_y(pos.x) + offset;
    vel.y += (target_y - pos.y) * 5.0 * dt;
    vel *= exp(-dt * 2.2);

    pos += vel * dt;

    if abs(pos.x) > 1.35 {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    // Height above the floor, for colour, over the range the crest can now span
    // (base ≈ -0.92 up to well past the top of the frame).
    let height_t = clamp((pos.y + 0.92) / 2.0, 0.0, 1.0);
    let env = smoothstep(0.0, 0.12, life_frac) * (1.0 - smoothstep(0.6, 1.0, life_frac));
    // The crest is the bright lit edge; the body fades into shadow below it, so
    // the terrain has a defined ridgeline instead of reading as a uniform haze.
    let crest_glow = mix(1.7, 0.4, crest_t);
    // Thin the field as it climbs so a lifted range reads as airy rather than as
    // the same slab moved upward. Low per-particle alpha — see panorama_sim for
    // the same reasoning; the additive stack carries the brightness, not the
    // individual particle.
    let alpha = env * (0.03 + 0.10 * u.rms) * (1.0 - 0.3 * height_t) * crest_glow;

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, init_size, init_size * (0.6 + 0.4 * env));
    p.color = vec4f(ascend_color(height_t, u.rms) * (0.7 + 0.6 * crest_glow), alpha);
    p.flags = vec4f(new_age, max_life, offset, crest_t);

    write_particle(idx, p);
    mark_alive(idx);
}
