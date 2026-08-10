pub mod audio_textures;
pub mod compositor;
pub mod context;
pub mod frame_capture;
pub mod frame_graph;
pub mod frame_prep;
pub mod fullscreen_quad;
pub mod half;
pub mod helix;
pub mod lattice;
pub mod layer;
pub mod layer_builder;
pub mod particle;
pub mod pass_executor;
pub mod pipeline;
pub mod placeholder;
pub mod postprocess;
pub mod profiler;
pub mod render_target;
pub mod shader_compiler;
#[cfg(test)]
pub mod test_gpu;
pub mod types;
pub mod uniforms;
pub mod volumetric;

pub use context::GpuContext;
pub use pipeline::ShaderPipeline;
pub use uniforms::{ShaderUniforms, UniformBuffer};

/// Wrap an angle to [-PI, PI].
///
/// Any angle that is accumulated frame over frame MUST be wrapped. Unwrapped, it
/// grows without bound, and then two things break: easing or lerping it toward a
/// rest value unwinds backwards through every full turn it ever accumulated
/// (the "model unspins on stop" bug), and `sin`/`cos` lose precision as the
/// argument grows. Wrapping is free and visually identical — rotation is mod TAU.
///
/// The same reasoning forbids `angle = time * rate` in a shader when `rate` is
/// live-editable: `u.time` never wraps, so a rate change jumps the angle by
/// `elapsed × Δrate`. Accumulate on the CPU and wrap instead.
pub fn wrap_angle(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (a + PI).rem_euclid(TAU) - PI
}

#[cfg(test)]
mod wrap_angle_tests {
    use super::wrap_angle;
    use std::f32::consts::{PI, TAU};

    #[test]
    fn wraps_into_range_and_preserves_direction() {
        assert!((wrap_angle(0.5) - 0.5).abs() < 1e-6);
        assert!((wrap_angle(-0.5) + 0.5).abs() < 1e-6);
        // A whole turn is a no-op visually and numerically.
        assert!(wrap_angle(TAU).abs() < 1e-5);
        // Many turns collapse to the equivalent small angle — this is the bit
        // that stops an ease-to-rest from travelling the long way round.
        assert!((wrap_angle(100.0 * TAU + 0.3) - 0.3).abs() < 1e-3);
        for a in [-1000.0f32, -7.0, -PI, 0.0, PI - 1e-4, 7.0, 1000.0] {
            let w = wrap_angle(a);
            assert!((-PI - 1e-5..=PI + 1e-5).contains(&w), "{a} -> {w}");
        }
    }

    // Why the orbit cameras accumulate instead of computing `time * rate`.
    //
    // The built-in "Audio Reactive" binding template maps audio.beat_phase
    // straight onto the motion targets, which include orbit_speed/rotation_speed
    // (bindings/templates.rs). So the rate is not a constant a user sets once —
    // it sweeps every beat. Multiplied by an ever-growing u.time, that is not a
    // rotation, it is a strobe, and it gets worse the longer the app has run.
    #[test]
    fn accumulated_orbit_stays_smooth_when_the_rate_is_modulated() {
        let dt = 1.0 / 60.0;
        let shortest_arc = |a: f32, b: f32| wrap_angle(a - b).abs();

        let (mut phase, mut t) = (0.0f32, 0.0f32);
        let (mut prev_old, mut prev_new) = (0.0f32, 0.0f32);
        let (mut worst_old, mut worst_new) = (0.0f32, 0.0f32);

        // Ten quiet minutes at a fixed rate, then a minute of beat-synced
        // modulation — a realistic set, not a contrived one.
        const SETTLE: u32 = 36_000;
        const MAX_RATE: f32 = 2.0;
        for f in 0..39_600u32 {
            t += dt;
            let rate = if f < SETTLE {
                0.15
            } else {
                (((f - SETTLE) as f32) / 30.0).fract() * MAX_RATE
            };
            phase = wrap_angle(phase + dt * rate);
            let old = t * rate; // what the shader used to compute

            if f > SETTLE {
                worst_old = worst_old.max(shortest_arc(old, prev_old));
                worst_new = worst_new.max(shortest_arc(phase, prev_new));
            }
            prev_old = old;
            prev_new = phase;
        }

        // Accumulated: never moves more than one frame's worth of rotation.
        assert!(
            worst_new <= dt * MAX_RATE + 1e-4,
            "accumulated orbit jumped {worst_new} rad in one frame (max {} expected)",
            dt * MAX_RATE
        );
        // time × rate: jumps most of a turn per frame, ten minutes in.
        assert!(
            worst_old > 1.0,
            "expected the old time*rate form to jump, got {worst_old} — if this \
             ever fails the premise changed, not the fix"
        );
    }
}
