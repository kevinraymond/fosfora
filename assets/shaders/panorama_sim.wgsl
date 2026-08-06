// Panorama particle simulation — the stereo field as a picture (#1801)
//
// Sound has a POSITION. The screen is a map of the mix: the horizontal axis is
// the stereo image (left edge = hard left, centre = centre) and the vertical
// axis is frequency, sub-bass at the bottom through cymbal shimmer at the top.
// Each band emits its particles at its OWN measured pan, so a kick sits as a
// dot dead centre while hats and reverb tails fan out to the edges — and a
// mono mix collapses the whole picture into a single vertical column.
//
// This is the first consumer of A13b per-band pan (band_pan(i)) and of
// pan/stereo_width/stereo_corr in a particle sim at all — those three have
// existed in the fragment path since A13 (#1464) but the particle ABI carried
// no stereo until the #1801 bump.
//
// The three stereo features drive three separate behaviours, so you can read
// which one is moving:
//   band_pan(i)   -> WHERE each band's particles are born (the image itself)
//   stereo_width  -> how far they fan out from that point (mono = a column)
//   stereo_corr   -> coherence. Correlated content rides tight parallel lanes;
//                    decorrelated content scatters into a cloud.
//
// NO TRAIL RING: this sim never calls trail_write, and the .pfx sets
// trail_length 0. particle_lib declares trail_buffer at @group(2) @binding(0);
// since #1921 the system always creates that binding (a dummy when trails are
// off), so a sim may reference trail_write with trail_length 0 without failing
// pipeline validation. This sim omits it anyway — it has no trail to write.
// --- Param mapping ---
// param(0) = spread        (fan width scale)
// param(1) = band_gap      (0 = whole spectrum on one goniometer row, 1 = full ladder)
// param(2) = drift         (motion speed)
// param(3) = scatter       (how much decorrelation disperses)
// param(4) = hue           (palette anchor offset)
// param(5) = glow          (brightness gain)
// param(6) = centre_pull   (bg shader: vignette toward the centre line)
// param(7) = trail_decay   (bg shader: feedback decay)

const N_BANDS: u32 = 7u;

// How much of the frame the stereo image is spread across. See the note at the
// spawn site: 1:1 would confine ordinary music to a narrow centre strip.
const PAN_GAIN: f32 = 3.2;

// All per-index randomness below uses the lib's uhash/uhash_f (integer mixing).
// fract-sin hash() bands idx-scaled arguments onto near-constant values, which
// is especially dangerous here: clustered spawn positions would read as a stereo
// image the audio does not actually contain — precisely the thing this effect
// claims to show.

// The 7 band energies, in the same order as band_pan(i).
fn band_energy(i: u32) -> f32 {
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

// Vertical position for a CONTINUOUS spectral coordinate `f` in 0..1 (0 = sub
// bass, 1 = air). band_gap collapses the ladder toward y=0, where the effect
// becomes a pure goniometer: the whole spectrum overlaid on one row, which is
// the classic vectorscope read.
fn spectrum_y(f: f32) -> f32 {
    return (f * 2.0 - 1.0) * 0.82 * param(1u);
}

// The measured stereo position and energy at a continuous spectral coordinate,
// interpolated between the two bands it falls between.
//
// Frequency is continuous, so drawing seven discrete lanes drew seven hard-edged
// bars stacked up the frame. Interpolating instead makes the field a single
// ribbon that bends left and right as it climbs — which is both better looking
// and a more honest picture, since nothing about the mix is actually quantised
// into seven steps.
fn spectrum_sample(f: f32) -> vec2f { // (pan 0..1, energy)
    let x = clamp(f, 0.0, 0.9999) * f32(N_BANDS) - 0.5;
    let i0 = u32(clamp(floor(x), 0.0, f32(N_BANDS - 1u)));
    let i1 = min(i0 + 1u, N_BANDS - 1u);
    let t = clamp(x - floor(x), 0.0, 1.0);
    return vec2f(
        mix(band_pan(i0), band_pan(i1), t),
        mix(band_energy(i0), band_energy(i1), t),
    );
}

// Soft bell in -1..1 from three uniforms. A single uniform raised to a power
// still ends at a HARD edge, which is what gave every band a crisp rectangular
// border; averaging gives smooth tails so the cloud fades out instead.
fn bell(a: f32, b: f32, c: f32) -> f32 {
    return ((a + b + c) / 3.0) * 2.0 - 1.0;
}

// Coherence in 0..1. stereo_corr is 0.5 = decorrelated, 1 = mono/in-phase,
// 0 = anti-phase. BOTH extremes are coherent stereo material (anti-phase is a
// deliberate widening trick, not noise), so the distance from 0.5 is what
// matters — not the raw value.
fn coherence() -> f32 {
    return clamp(abs(u.stereo_corr * 2.0 - 1.0), 0.0, 1.0);
}

fn panorama_color(band_t: f32, energy: f32) -> vec3f {
    // Hue walks the spectrum, so the vertical axis is also a colour axis: bass
    // reads warm, air reads cool.
    let hue_t = fract(param(4u) * 0.5 + 0.58 + band_t * 0.42);
    var col = fosfora_audio_palette(hue_t, 0.55 + 0.45 * clamp(u.centroid, 0.0, 1.0), u.time * 0.02);
    // Loud bands read hot and slightly desaturated toward white, so a bright
    // band is legible against a busy field without relying on bloom alone.
    col = mix(col, vec3f(1.0), clamp(energy, 0.0, 1.0) * 0.35);
    return col * (0.20 + 1.10 * param(5u) * clamp(energy, 0.0, 1.0));
}

fn emit_particle(idx: u32) -> Particle {
    var p: Particle;
    let h = uhash(idx * 0x9e3779b9u + u32(u.seed * 4096.0));
    let r0 = f32(uhash(h)) / 4294967296.0;
    let r1 = f32(uhash(h + 1u)) / 4294967296.0;
    let r2 = f32(uhash(h + 2u)) / 4294967296.0;
    let r3 = f32(uhash(h + 3u)) / 4294967296.0;
    let r4 = f32(uhash(h + 4u)) / 4294967296.0;
    let r5 = f32(uhash(h + 5u)) / 4294967296.0;
    let r6 = f32(uhash(h + 6u)) / 4294967296.0;

    // Fixed structural cohort: a slot keeps its spectral coordinate across
    // respawns, so the ribbon has a stable population rather than boiling.
    let f = f32(uhash(idx * 2654435761u)) / 4294967296.0;
    let sample = spectrum_sample(f);
    let energy = sample.y;

    // WHERE that part of the spectrum sits. band_pan is 0..1 with 0.5 = centred;
    // a band with no energy holds exactly 0.5, so a silent region collapses to
    // the centre line rather than wandering — that is the producer's behaviour,
    // not something invented here.
    //
    // AMPLIFIED, not mapped 1:1. Real mixes are mostly centred: on ordinary
    // material the bands live within roughly +/-0.15 of 0.5, which at 1:1 is a
    // 15%-wide strip in the middle of the screen and reads as no stereo image at
    // all. The gain spends the whole frame on the range music actually occupies.
    // Hard-panned content clamps at the edge, which is where it belongs.
    let x_centre = clamp((sample.x - 0.5) * 2.0 * PAN_GAIN, -1.0, 1.0);

    // Fan width from stereo_width. The floor is generous on purpose: a region
    // has to be a visible CLOUD before its position means anything, and
    // stereo_width is a mid/side ratio that stays small on most real material.
    let fan = param(0u) * (0.16 + u.stereo_width * 1.6);
    let x = clamp(x_centre + bell(r0, r1, r2) * fan, -1.15, 1.15);
    let y = spectrum_y(f) + bell(r3, r4, r5) * 0.11;

    // Decorrelated content scatters; correlated content rides tight lanes.
    let scatter = (1.0 - coherence()) * param(3u);
    let drift = param(2u) * (0.10 + 0.30 * u.rms);
    let side = select(-1.0, 1.0, x > x_centre);
    let vel = vec2f(
        side * drift * (0.3 + 0.7 * u.stereo_width) + (r6 - 0.5) * scatter * 0.6,
        (r0 - 0.5) * scatter * 0.4 + drift * 0.12,
    );

    let init_size = u.initial_size * (0.6 + 0.8 * r1) * (0.5 + 0.9 * clamp(energy, 0.0, 1.0));
    let life = u.lifetime * (0.7 + 0.6 * r2);

    p.pos_life = vec4f(x, y, 0.0, 1.0);
    p.vel_size = vec4f(vel, init_size, init_size);
    p.color = vec4f(panorama_color(f, energy), 0.0);
    // flags: (age, lifetime, spectral coordinate, spawn_x) — spawn_x lets the
    // sim spring a particle toward where its part of the spectrum has moved.
    p.flags = vec4f(0.0, life, f, x_centre);
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
    let f = p.flags.z;
    var pos = p.pos_life.xy;
    var vel = p.vel_size.xy;
    let init_size = p.vel_size.z;

    let sample = spectrum_sample(f);
    let energy = sample.y;

    // Track the image as it MOVES. A live particle is pulled toward where its
    // part of the spectrum now sits, so a hi-hat swept left-to-right drags its
    // whole cloud across the screen instead of leaving the old position lit
    // until those particles happen to die. Gentle, so the fan is not sucked
    // into a line.
    let target_x = clamp((sample.x - 0.5) * 2.0 * PAN_GAIN, -1.0, 1.0);
    let home_x = p.flags.w;
    vel.x += (target_x - home_x) * 2.4 * dt;

    // Restoring force toward the particle's own height: keeps the frequency
    // axis readable under drift.
    vel.y += (spectrum_y(f) - pos.y) * 1.6 * dt;

    // Coherent material rides parallel lanes: damp cross-lane motion hard when
    // the mix is correlated, let it wander when it is not.
    let coh = coherence();
    let damp = exp(-dt * (0.8 + 5.0 * coh));
    vel *= damp;

    pos += vel * dt;

    // A particle that leaves the frame is done — no wrap, because a wrapped
    // particle would appear on the wrong side and misreport the stereo image.
    if abs(pos.x) > 1.3 || abs(pos.y) > 1.25 {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    // Fade in fast, out slow; scale by live energy so a spectral region going
    // quiet dims its whole cloud instead of holding a stale bright streak.
    let env = smoothstep(0.0, 0.10, life_frac) * (1.0 - smoothstep(0.55, 1.0, life_frac));
    // Additive: per-particle alpha stays LOW because the stack does the
    // accumulating. At the shipped density a 0.5 alpha saturates the lanes to
    // flat white blocks and the whole image stops reading as particles.
    let alpha = env * (0.03 + 0.16 * clamp(energy, 0.0, 1.0));

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, init_size, init_size * (0.55 + 0.45 * env));
    p.color = vec4f(panorama_color(f, energy), alpha);
    p.flags = vec4f(new_age, max_life, f, home_x);

    write_particle(idx, p);
    mark_alive(idx);
}
