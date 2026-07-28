// Etch background — the aluminium powder the stylus scratches through.
//
// Retention is exactly 1.0: the powder keeps whatever the pen has already scratched, until
// the board is shaken. That is both the toy's behaviour and the reason this shader has no
// decay constant — with nothing decaying per frame there is no frame-rate-dependent trail
// length to get wrong (see #1986).
//
// It also means the wipe is the ONLY way anything ever leaves the screen, so it cannot hang
// off `u.drop` alone: drops are rare, and a board that only clears on one left the frame to
// silt up permanently (Kevin, #1991). The shake now runs on its own cycle.

// --- Shake-clean cycle ---
// MUST STAY IDENTICAL to the copy in etch_sim.wgsl (pinned by etch_clear_cycle_matches).
// The sim owns the pen and this shader owns the powder, and they share no buffer, so the
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

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let clear_cycle = param(6u);

    let prev = feedback(uv);

    // A cold-started layer reads back black; treat that as "needs a coat" so the board
    // heals itself on load, on resize, and after an effect switch.
    let uninitialised = max(prev.r, max(prev.g, prev.b)) < 0.02;

    if u.drop > 0.5 || uninitialised || etch_clearing(clear_cycle, u.time) {
        // Fresh powder. The speckle is baked in once per frame of the shake and never
        // re-rolled while the drawing stands, so it cannot smear into the strokes — and a
        // grain running at full frame rate under a held frame is what turns a dropped frame
        // into a visible flash (#1985).
        //
        // Re-seeding the speckle each frame *during* the shake is the point: the grain
        // visibly rattles for the length of the wipe, so the board reads as being shaken
        // rather than cutting to grey.
        let tick = floor(u.time * 70.0);
        let jitter = vec2f(
            phosphor_hash2(vec2f(tick, 1.0)) - 0.5,
            phosphor_hash2(vec2f(tick, 7.0)) - 0.5,
        ) * 8.0;
        let speckle = (phosphor_hash2(uv * 1400.0 + jitter) - 0.5) * 0.055;
        let sheen = (1.0 - uv.y) * 0.045;
        let powder = vec3f(0.615, 0.625, 0.600) + speckle + sheen;
        return vec4f(powder, 1.0);
    }

    return vec4f(prev.rgb, 1.0);
}
