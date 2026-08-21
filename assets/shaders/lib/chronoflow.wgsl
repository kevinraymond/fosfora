// Chronoflow (#1482) shared helpers — temporal-reprojection motion trails.
// Used by chronoflow_velocity.wgsl and the per-effect *_history.wgsl wrappers
// (param indices differ per effect, so the wrappers stay per-effect).
// This file is prepended to EVERY effect shader: keep it free of input0()
// references and param() index assumptions.

// Per-frame history retention for the advected trail image.
// `exposure_p` 0..1 is the effect's trail-length param (0 = almost none,
// 1 = long cinematic streaks), lengthened slightly by loudness so quiet
// passages expose shorter. `snap_p` 0..1 scales the beat shutter snap:
// on the beat retention collapses and the image goes crisp — harder for
// stronger beats.
fn chrono_keep(exposure_p: f32, snap_p: f32) -> f32 {
    let exposure60 = mix(0.72, 0.965, clamp(exposure_p, 0.0, 1.0)) + u.rms * 0.03;
    return chrono_keep_direct(exposure60, snap_p);
}

// Variant taking the per-frame retention directly (no 0.72..0.965 remap) — for
// effects whose legacy trail_decay param range was authored as a raw decay
// factor (flux), where the remap would double the steady-state accumulation
// and wash out a screen-filling additive effect.
fn chrono_keep_direct(keep60: f32, snap_p: f32) -> f32 {
    // Exposure is defined per 1/60 s frame; pow keeps it frame-rate independent.
    let keep = pow(clamp(keep60, 0.0, 0.985), max(u.delta_time, 1e-4) * 60.0);
    let snap = clamp(u.beat * (0.4 + 0.6 * u.beat_strength) * snap_p * 1.6, 0.0, 1.0);
    return keep * (1.0 - snap);
}

// Particle velocity (NDC units/s, y up) -> uv-space displacement per second
// (y down). NDC spans 2 units per uv unit on both axes, so no aspect term.
fn chrono_uv_vel(ndc_vel: vec2f) -> vec2f {
    return vec2f(ndc_vel.x, -ndc_vel.y) * 0.5;
}

// ---- Frame-rate-independent feedback (#1986) ----
//
// Every feedback site in the tree is `out = a*col + k*prev`, where the source
// gain `a` and the retention `k` were authored implicitly at 60 fps. Holding
// BOTH the time constant and the steady state a/(1-k) at a real frame time:
//
//     n = clamp(dt*60, eps, 2)     k' = pow(k, n)     a' = a * (1-k')/(1-k)
//
// Without this, trail length is measured in FRAMES: at k = 0.82 the fraction
// surviving one wall-clock second spans 2.6e-3 at 30 fps to 2.1e-21 at 240,
// so the same preset is a different effect on different hardware.
//
// The bound of 2 is the lesson of the reverted attempt (4b106dd). dt is already
// clamped to 0.05 upstream (app.rs), so an unbounded n reaches 3 and one stalled
// frame took 0.82 -> 0.55 — a NEW single-frame darkening on exactly the hitches
// this is meant to absorb. Two normal frames is the ceiling; the cost is that
// below 30 fps trails run long rather than flashing dark. Correction is exact
// from 30 fps up.
//
// These live here, in a lib already listed in LIB_FILENAMES, rather than in a
// new lib file. assets/ is read at RUNTIME by whatever binary is running, so
// adding to LIB_FILENAMES version-couples assets to the binary and breaks every
// older build mid-session — that is what forced the revert.
fn frame_steps() -> f32 {
    // dt <= 0 means "no frame time recorded" -> behave exactly as authored,
    // not "nothing decays". Also keeps pow(0, 0) out of reach.
    if (u.delta_time <= 0.0) { return 1.0; }
    return clamp(u.delta_time * 60.0, 1e-4, 2.0);
}

// Per-frame retention -> this frame's retention.
// Deliberately NOT clamped to 0.985 like chrono_keep_direct: that clamp bounds
// chrono_keep's audio-summed term, which can reach 0.995. The inputs here are
// authored constants and params, and sumi_velocity legitimately runs at 0.999.
fn frame_decay(keep60: f32) -> f32 {
    let n = frame_steps();
    // Uniform branch (n comes from a uniform), so it costs nothing and makes
    // the 60 fps path bit-exactly the expression that shipped.
    if (n == 1.0) { return keep60; }
    return pow(clamp(keep60, 0.0, 1.0), n);
}

// Per-channel variant. Differential RGB decay is three independent retentions,
// so the pow must see the PRODUCT of decay and tint — pow(k,n)*t != pow(k*t,n),
// and the tint is itself a per-frame rate (Vessel's blue is 1.02, a per-frame
// gain). WGSL has no overloading, hence the noise2/noise3 naming convention.
fn frame_decay3(keep60: vec3f) -> vec3f {
    let n = frame_steps();
    if (n == 1.0) { return keep60; }
    return pow(clamp(keep60, vec3f(0.0), vec3f(1.0)), vec3f(n));
}

// Explicit-Euler spatial diffusion on a 5-point stencil, per frame -> per second
// (#2350). The step is v' = (1-D)*v + D*mean4; with mean4 locally constant the
// deviation from it obeys e' = (1-D)*e, so (1-D) is a per-frame RETENTION and
// compounds exactly like a trail decay. Blur therefore ran to different radii on
// different hardware, the same class of bug as #1986 one dimension over.
//
// D' = 1 - frame_decay(1 - D) keeps the deviation's time constant fixed. It stays
// in [0,1] for D in [0,1], so `blurred` remains a convex combination and cannot
// overshoot into ringing.
//
// The n == 1 early return is NOT redundant with frame_decay's. Without it the
// 60 fps path evaluates 1 - (1 - D), which is not D in f32 for any of the shipped
// weights — 0.12 round-trips to 0.12000000476837158 and 0.16 to 0.15999996662139893.
// Measured: it moved Polycephalum 0.7% at 60 fps before this was added, against a
// contract that says 60 fps is untouched.
fn frame_diffuse(rate60: f32) -> f32 {
    let d = clamp(rate60, 0.0, 1.0);
    if (frame_steps() == 1.0) { return d; }
    return 1.0 - frame_decay(1.0 - d);
}

// Matching source gain, so the steady state a/(1-k) does not move with frame
// rate. `keep60` MUST be the same value handed to frame_decay at this site.
// Only meaningful where the source term is a linear blend inside the shader;
// sites whose source is combined with max() cannot use it.
//
// The Rust-side particle composite used to be the other exception here. It is
// now corrected too (#2349), but from Rust rather than from this helper — the
// particles are added after the fragment passes, so the shader never sees them.
// `ParticleDef::composite_decay` tells Rust which param holds this site's k, and
// `frame_gain` in gpu/particle/types.rs is the twin of this function. If you
// change the formula below, change that one, and vice versa.
fn frame_gain(gain60: f32, keep60: f32) -> f32 {
    let k = clamp(keep60, 0.0, 1.0);
    let d = 1.0 - k;
    // k -> 1 is a lossless integrator; the correction degenerates to n, continuously.
    if (d < 1e-5) { return gain60 * frame_steps(); }
    return gain60 * (1.0 - frame_decay(k)) / d;
}

// ---- Deprecated aliases (pre-rename API, kept so user custom effects keep
// compiling). Do not use in new code; may be removed in a future major release. ----

