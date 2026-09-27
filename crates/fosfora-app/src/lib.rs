//! Fosfora's engine as a library: audio analysis, effects, bindings, scenes and the
//! GPU renderer. The `fosfora` desktop binary (`main.rs`) is a thin shell over it.

// These lints only fire on exported API, which this code was not until the binary
// crate was split into a lib. They ask for API additions (Default impls, #[must_use],
// hasher generics), not fixes, so they are held at the pre-split posture rather than
// churning ~45 items across modules another branch is actively rewriting.
#![allow(
    clippy::new_without_default,
    clippy::return_self_not_must_use,
    clippy::pub_underscore_fields,
    clippy::implicit_hasher,
    clippy::len_without_is_empty
)]

#[cfg(feature = "analyze")]
pub mod analyze;
pub mod app;
pub mod audio;
pub mod bindings;
#[cfg(feature = "depth")]
pub mod depth;
pub mod download;
pub mod effect;
pub mod gpu;
pub mod headless;
#[cfg(feature = "link")]
pub mod link;
pub mod media;
pub mod midi;
#[cfg(feature = "ndi")]
pub mod ndi;
pub mod osc;
#[cfg(any(
    feature = "ndi",
    all(target_os = "linux", feature = "v4l2"),
    all(target_os = "windows", feature = "spout"),
    all(target_os = "macos", feature = "syphon")
))]
pub mod output;
pub mod params;
pub mod paths;
pub mod preset;
pub mod recording;
pub mod scene;
pub mod settings;
pub mod shader;
pub mod signal;
#[cfg(all(target_os = "windows", feature = "spout"))]
pub mod spout;
#[cfg(all(target_os = "macos", feature = "syphon"))]
pub mod syphon;
#[cfg(test)]
mod test_alloc;
pub mod trama;
pub mod ui;
#[cfg(all(target_os = "linux", feature = "v4l2"))]
pub mod v4l2;
pub mod web;
