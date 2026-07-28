// Etch (Etch-a-Sketch) particle simulation.
// Re-draws the loaded image/video/webcam frame as a scratched line. Source samples collapse
// onto a small number of horizontal scan lines; within a line the stylus zigzags, and both
// the swing and the stroke density follow the local tone — dense wide scribble in shadow,
// sparse flat line in highlight, which is how anyone actually shades on the toy. The pen
// advances with the music and rattles with the drums; a drop shakes the board clean
// (see etch_bg.wgsl).
// Structs, bindings, and helpers are in particle_lib.wgsl (auto-prepended).

fn unpack_rgba(packed: f32) -> vec4f {
    let bits = bitcast<u32>(packed);
    return vec4f(
        f32(bits & 0xFFu) / 255.0,
        f32((bits >> 8u) & 0xFFu) / 255.0,
        f32((bits >> 16u) & 0xFFu) / 255.0,
        f32((bits >> 24u) & 0xFFu) / 255.0,
    );
}

fn luminance(c: vec3f) -> f32 {
    return dot(c, vec3f(0.299, 0.587, 0.114));
}

// --- Shake-clean cycle ---
// MUST STAY IDENTICAL to the copy in etch_bg.wgsl (pinned by etch_clear_cycle_matches).
// The sim owns the pen and the background owns the powder, and they share no buffer, so the
// only way they can agree on when the board is wiped is for both to derive it from u.time —
// which the app hands to the particle and fragment uniforms from one clock (app.rs:1249).
// The shake lasts a fixed number of seconds rather than a fraction of the cycle, so a short
// cycle cannot shrink it below a frame and silently stop clearing.
// clear_cycle <= 0 clears every frame: pull the slider (or a bound fader) to zero and the
// board stays blank, raise it and the drawing starts again. That is the manual wipe.
const ETCH_SHAKE_SECS: f32 = 0.6;

fn etch_clearing(clear_cycle: f32, t: f32) -> bool {
    if clear_cycle <= 0.0 {
        return true;
    }
    return fract(t / clear_cycle) * clear_cycle < ETCH_SHAKE_SECS;
}

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles {
        return;
    }

    let home = aux[idx].home;
    let home_pos = home.xy;
    let home_color = unpack_rgba(home.z);

    let scan_rows      = clamp(param(0u), 8.0, 200.0); // visible scan lines
    let draw_rate      = param(1u);  // path fraction per second at moderate level
    let scribble_gain  = param(2u);  // how far the pen swings for dark tone
    let polarity       = param(3u);  // 0 = ink the dark (photos), 1 = ink the bright (art on black)
    let line_weight    = param(4u);  // stroke thickness relative to the line spacing
    let ink            = param(5u);  // how black the scratch is
    let clear_cycle    = param(6u);  // seconds between shakes; 0 = hold the board clean
    let tone_floor     = param(7u);  // tone below which the pen lifts entirely

    var p = read_particle(idx);
    let dt = u.delta_time;

    // --- Shared draw progress ---
    // Every particle integrates the same scalar from the same start with the same
    // arithmetic, so they stay in lockstep without any cross-particle communication.
    // The pen rewinds to the top-left whenever the board is being shaken clean, and holds
    // there for the length of the shake so it cannot outrun the powder re-coating under it.
    // A drop wipes on top of that — but a drop is far too rare to rely on as the only
    // eraser, which with retention at 1.0 left the board a one-way trip (Kevin, #1991).
    var prog = p.flags.w;
    let rate = draw_rate * (0.25 + u.rms * 2.0 + u.onset * 1.5);
    let clearing = u.drop > 0.5 || etch_clearing(clear_cycle, u.time);
    if clearing {
        prog = 0.0;
    } else {
        prog = min(prog + rate * dt, 1.05);
    }

    // Aux is filled in raster order over the source, so plain index order *is* scan
    // order — left to right, top to bottom — with no lattice dimensions needed.
    let t = f32(idx) / f32(max(u.max_particles, 1u));

    // --- The stylus is a moving point, not the whole drawn path ---
    // What has already been drawn lives in the powder (the feedback buffer), NOT in the
    // particles. Keeping every passed particle alive re-inks the entire path every frame,
    // and because retention is exactly 1.0 that compounds: a pixel at alpha a is at
    // 1-(1-a)^N after N frames, so everything the pen has touched goes solid black within
    // about a second and the whole tonal range collapses into one dark smear. That is the
    // mush Kevin hit (#1991) — the tone maths was fine, it was just being applied 60 times
    // a second to the same pixels.
    //
    // So a particle is alive only while the stylus is actually passing over it: a couple of
    // frames' worth of path. The window is derived from the same rate*dt the pen advances
    // by, so each sample is inked a bounded number of times at any frame rate, and `ink`
    // controls tone again instead of controlling how fast everything saturates.
    let window = max(rate * dt * 2.5, 1e-6);

    // Transparent padding, not-yet-drawn samples, and everything the pen has already left
    // behind all sit off-screen.
    if home_color.a < 0.01 || t > prog || t < prog - window {
        p.pos_life = vec4f(99.0, 99.0, 0.0, 0.0);
        p.color = vec4f(0.0);
        p.flags.w = prog;
        write_particle(idx, p);
        return;
    }

    // --- Tone ---
    // Which end of the range carries ink is a property of the source, not of the effect:
    // a photograph wants its shadows scribbled, while the bundled art is a bright subject
    // on black and wants the opposite. One knob rather than a guess.
    let lum = luminance(home_color.rgb);
    let raw = mix(1.0 - lum, lum, polarity);

    if raw < tone_floor {
        p.pos_life = vec4f(99.0, 99.0, 0.0, 0.0);
        p.color = vec4f(0.0);
        p.flags.w = prog;
        write_particle(idx, p);
        return;
    }
    let tone = clamp((raw - tone_floor) / max(1.0 - tone_floor, 1e-4), 0.0, 1.0);

    // --- Collapse onto scan lines ---
    let bh = 2.0 / scan_rows;
    let down = 1.0 - home_pos.y;                       // 0 at the top of NDC
    let bidx = clamp(floor(down / bh), 0.0, scan_rows - 1.0);
    let band_y = 1.0 - (bidx + 0.5) * bh;
    // Where this sample sat *within* its band. Only a fraction of it feeds the zigzag
    // phase: several source rows land on each scan line, and if their phases were spread
    // out the strokes would interleave into a solid bar instead of retracing one line.
    // Keeping them near-coincident is what makes the band read as a single stroke.
    let sub = clamp(down / bh - bidx, 0.0, 1.0);

    let amp = tone * scribble_gain * bh * 0.5;

    // Zigzag frequency follows the line spacing, so the hatching keeps its angle at every
    // scan_rows setting instead of needing a second knob kept in sync by hand.
    let zig = scan_rows * 1.1;
    let phase = (home_pos.x + 1.0) * 0.5 * zig + sub * 0.25;
    let tri = abs(fract(phase) * 2.0 - 1.0) * 2.0 - 1.0; // -1..1 triangle
    var pos = vec2f(home_pos.x, band_y + tri * amp);

    // --- Pen rattle ---
    // One shake for the whole frame (a hand holding the toy), not per-particle noise,
    // so the line wobbles coherently instead of dissolving into static. Applied to the
    // stylus rather than to the powder, so the drawing already laid down stays put.
    let frame_seed = floor(u.time * 60.0);
    let rattle = (u.kick + u.percussive_energy * 0.6) * 0.0045;
    pos += vec2f(hash(frame_seed) - 0.5, hash(frame_seed + 17.0) - 0.5) * rattle;

    // Graphite showing through the scratched aluminium powder.
    //
    // Alpha is quadratic in tone rather than linear, and that is load-bearing. A pale
    // stroke has almost no swing, so every source row in the band retraces the same thin
    // line and the overdraw alone would drive it to solid black — highlights would come
    // out as dark as shadows, only thinner. Falling off faster than the overdraw builds up
    // is what keeps a tonal ramp; `ink` scales the whole curve.
    let graphite = vec3f(0.09, 0.09, 0.10);
    let alpha = ink * tone * tone;

    p.pos_life = vec4f(pos, 0.0, 1.0);
    // Sized so consecutive samples along a row overlap slightly at the shipped count —
    // below that the stroke breaks into stipple and stops reading as a drawn line.
    p.vel_size = vec4f(0.0, 0.0, 0.0, bh * 0.18 * line_weight);
    p.color = vec4f(graphite, alpha);
    p.flags = vec4f(p.flags.x + dt, u.lifetime, 0.0, prog);

    write_particle(idx, p);
    mark_alive(idx);
}
