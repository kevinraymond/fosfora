//! Offline (headless) scene rendering for #2027 — no window, no audio device,
//! no wall clock. Dev-side only, behind the `analyze` feature like the rest of
//! the offline toolchain (`--analyze`, `--dump-schema`, `--validate`).

pub mod driver;
pub mod gpu;
pub mod load;
pub mod scene_renderer;
pub mod schedule;
