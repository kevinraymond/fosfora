//! Fosfora VR: OpenXR, Android and VR-specific glue (`docs/xr/XR_DESIGN.md`).
//!
//! Everything in this crate is Android-only. On other targets it compiles to an
//! empty library so the desktop workspace (`cargo clippy --all-targets`,
//! `cargo test`) keeps building unchanged (invariant I1). Lint the real code with
//! `cargo ndk -t arm64-v8a clippy -p fosfora-xr` or `scripts/xr/run.sh lint`.
//!
//! S1 scope: package and launch a native OpenXR app that clears each eye to a
//! color that changes over time. No wgpu yet; `ash` talks to Vulkan directly.

#[cfg(target_os = "android")]
mod app;
#[cfg(target_os = "android")]
mod gfx;
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
