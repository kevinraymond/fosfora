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
