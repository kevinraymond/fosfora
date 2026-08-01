//! Offline (headless) scene rendering (#2027) and loop export (#2063) — no
//! window, no audio device, no wall clock.
//!
//! The render core (gpu, scene_renderer, load, loop_spec, loop_driver) ships
//! in release builds since Phase 2: `--render-loop` is a product feature. The
//! song-driven paths (`driver` = `--render-scene`, `schedule`) stay behind the
//! `analyze` feature with the offline audio toolchain they depend on.

#[cfg(feature = "analyze")]
pub mod driver;
pub mod gpu;
pub mod load;
pub mod loop_driver;
pub mod loop_encode;
pub mod loop_spec;
pub mod scene_renderer;
#[cfg(feature = "analyze")]
pub mod schedule;
