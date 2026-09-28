//! Fosfora VR: OpenXR, Android and VR-specific glue (`docs/xr/XR_DESIGN.md`).
//!
//! Everything in this crate is Android-only. On other targets it compiles to an
//! empty library so the desktop workspace (`cargo clippy --all-targets`,
//! `cargo test`) keeps building unchanged (invariant I1). Lint the real code with
//! `cargo ndk -t arm64-v8a clippy -p fosfora-xr` or `scripts/xr/run.sh lint`.
//!
//! Spike state: S1 packaging/launch, S2 wgpu on the runtime's Vulkan device,
//! S4 one core effect on a world-locked quad, S5 world-space particles on
//! the billboard path, S6 live audio, S7 passthrough, hands and room
//! geometry as obstacles.

#[cfg(target_os = "android")]
mod app;
#[cfg(target_os = "android")]
mod assets;
#[cfg(target_os = "android")]
mod audio;
pub mod gesture;
#[cfg(target_os = "android")]
mod gfx;
#[cfg(target_os = "android")]
mod hud;
#[cfg(target_os = "android")]
mod input;
pub mod math;
pub mod palm_panel;
#[cfg(target_os = "android")]
mod particles3d;
#[cfg(target_os = "android")]
mod perf;
#[cfg(target_os = "android")]
mod playback;
#[cfg(target_os = "android")]
mod room;
#[cfg(target_os = "android")]
mod scene;
#[cfg(target_os = "android")]
mod xr;

/// Entry point called by `android-activity`'s NativeActivity glue on its own
/// thread once the activity is created. Returning ends the activity.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
#[allow(
    clippy::no_mangle_with_rust_abi,
    reason = "android-activity's glue declares `android_main` in an extern \"Rust\" block"
)]
pub fn android_main(app: android_activity::AndroidApp) {
    app::run(&app);
}
