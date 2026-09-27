# Cloud tasks

Cloud sessions clone from GitHub: **push the branch first**. They can't see local files,
blackboard, adb or the headset, so every brief below stands alone. Paste one as
the prompt: `claude --cloud "$(cat brief.md)"`. Pull the result back with
`claude --teleport <session-id>`.

## Cloud environment (set once)

A sample container measured Sep 27, 2026: Ubuntu 24.04 x86_64, 2 CPUs, 7.8 GB
RAM, about 30 GB free disk, no GPU, Rust 1.95 preinstalled, crates.io and GitHub
reachable, **`static.rust-lang.org` and `dl.google.com` blocked**.

- **Network:** the environment must reach `static.rust-lang.org`, or rustup can't
  install the pinned 1.97.0 toolchain or the Android target.
  `dl.google.com` is only needed to build APKs in the cloud; CI does that instead.
- **Setup script:**

```bash
set -euo pipefail
rustup toolchain install 1.97.0 --profile minimal -c rustfmt,clippy
rustup target add aarch64-linux-android --toolchain 1.97.0
sudo apt-get update -o Acquire::Retries=3
sudo apt-get install -y -o Acquire::Retries=3 libasound2-dev libudev-dev mesa-vulkan-drivers ffmpeg
llvm_lib=$(ls -d /usr/lib/llvm-*/lib 2>/dev/null | sort -V | tail -1); echo "export LIBCLANG_PATH=$llvm_lib" >> ~/.bashrc
cargo install cargo-ndk --locked || true
```

(`libasound2-dev`, `libudev-dev`, `ffmpeg` and `LIBCLANG_PATH` mirror the repo's
CI jobs. `mesa-vulkan-drivers` gives software Vulkan for GPU tests.)

- **Budget:** 2 CPUs makes cold builds slow. Use `cargo check` and dev-profile
  builds; never build the fat-LTO release profile in the cloud.

---

## C1: CI for the `xr` branch + debug APK artifact

*Run after S1 has landed `crates/fosfora-xr/`, `android/` and `scripts/xr/run.sh` on `xr`.*

> You are working in the Fosfora repo (Rust, wgpu 27, pinned toolchain 1.97.0 in
> `rust-toolchain.toml`) on branch `xr`. `crates/fosfora-xr` is a native Android
> OpenXR app (a `cdylib` built with `cargo-ndk` and packaged by the Gradle
> project in `android/`) for Meta Quest 3. Read `CLAUDE.md`, `docs/xr/CLAUDE.md`
> if present, `docs/xr/BUILD_PLAN.md` and `.github/workflows/ci.yml` first.
>
> **Goal:** every push to `xr` runs the desktop CI and produces a downloadable,
> consistently signed debug APK.
>
> **Tasks**
> 1. In `ci.yml`, add `xr` to the `push` branch filter. Change nothing else about
>    the existing jobs. The desktop checks must run unchanged on `xr`.
> 2. New workflow `.github/workflows/xr-apk.yml`:
>    - triggers: push to `xr` touching `crates/**`, `android/**`, `assets/**`,
>      `scripts/xr/**` or `Cargo.*`, plus `workflow_dispatch`;
>    - `ubuntu-latest`, `dtolnay/rust-toolchain@1.97.0` with target
>      `aarch64-linux-android`, `Swatinem/rust-cache@v2`, `actions/setup-java` (Temurin 17);
>    - the NDK preinstalled on the runner image (for example `$ANDROID_NDK_LATEST_HOME`),
>      or a pinned install. State the NDK version in the job log;
>    - `cargo install cargo-ndk --locked` (cached), then the same build command
>      `scripts/xr/run.sh` uses, minus install and launch, then `./gradlew assembleDebug`;
>    - sign with a **stable debug keystore** from secret `XR_DEBUG_KEYSTORE_B64`
>      (plus `XR_DEBUG_KEYSTORE_PASS`), so consecutive builds install over each
>      other with `adb install -r`. If the secret is missing, fail with a clear
>      message saying how to create it (`keytool` command) and add it;
>    - upload `fosfora-xr-debug-<shortsha>.apk` as an artifact (14-day retention).
> 3. Add `cargo clippy -p fosfora-xr --target aarch64-linux-android -- -D warnings` to the new workflow.
> 4. `cargo deny check` must still pass. If the Android deps trip it, update
>    `deny.toml` only to allow MIT, Apache-2.0 or other permissive licenses, and
>    list each addition in the PR.
>
> **Acceptance:** YAML is valid (`actionlint` if you can install it). The first
> push to `xr` shows the desktop jobs and the APK job green, with the APK
> artifact attached. Open a PR into `xr` (not `main`) describing what changed and
> the one manual step Kevin must do (create the keystore secret).
>
> **Don't:** touch release or tag workflows, bump the toolchain, or change any Rust code.

---

## C2: the seam (library target + `desktop` feature)

*Can run in parallel with S1–S2. Needs nothing from them.*

> You are working in the Fosfora repo on a new branch `xr-seam`, cut from `main`
> (not from `xr`). Fosfora is a real-time
> audio-reactive visual engine: about 130K lines of Rust in one **binary** crate,
> `crates/fosfora-app` (wgpu 27, egui 0.33, winit 0.30, cpal 0.17.3), with the
> toolchain pinned at 1.97.0. Read `CLAUDE.md`, `ARCHITECTURE-NOTES.md` and
> `.github/workflows/ci.yml` first, plus the XR plan, which lives on the `xr` branch
> only: `git show origin/xr:docs/xr/PHASE0.md`. Don't commit `docs/xr/` to `xr-seam`.
>
> **Goal:** make Fosfora's core usable as a library that compiles for
> `aarch64-linux-android` with no desktop crates, so a separate
> `crates/fosfora-xr` (a Quest OpenXR app, coming later) can depend on it. **Zero
> behavior change on desktop.**
>
> **Invariants**
> - Every command in `ci.yml` passes unchanged, every clippy feature combination
>   included, with `-D warnings` and workspace pedantic lints. `cargo deny check` passes.
> - The `AudioFeatures` ABI and golden vectors (`audio/features.rs`,
>   `audio/schema.rs`, `GOLDEN_HOPS`) are untouched.
> - Every new `unsafe` block has a `// SAFETY:` comment. None should be needed here.
>
> **Tasks**
> 1. Add `[lib]` (`src/lib.rs`). Move the module declarations there, exposing
>    what `main.rs` and a future XR crate need (`pub mod` for core modules, private
>    or `pub(crate)` where possible). `main.rs` becomes a thin binary over the lib.
>    Keep `#[cfg(test)]` modules and the `test_alloc` arrangement working; check
>    how `main.rs` declares `argv_tests`.
> 2. Features: `default = ["desktop"]`. `desktop` owns the dependencies that are
>    desktop-only or don't build for Android: at least winit, egui-winit, rfd,
>    midir, dark-light, ureq (native-tls), notify / notify-debouncer-mini,
>    egui_code_editor and egui-snarl if only UI uses them, and ctrlc and
>    tungstenite if only desktop paths use them. Existing features that need them
>    (`webcam`, `depth`, `video`, `ndi`, `v4l2`, `spout`, `syphon`, `link`,
>    `analyze`, `release`, `profiling`) must imply `desktop`. Set
>    `required-features = ["desktop"]` on `[[bin]]`. Keep `egui` and `egui-wgpu`
>    unconditional: the XR build renders an egui panel to a texture.
> 3. Gate desktop-only modules with `#[cfg(feature = "desktop")]` at the module
>    declaration where possible (`app`, `ui`, `web`, `midi`, `osc`, `ndi`, `spout`,
>    `syphon`, `v4l2`, `recording`, `depth`, `link`, `analyze`, `download`,
>    `output`: confirm each). Then fix the core→desktop couplings the Android
>    check reports. Starting list (from grep, may be incomplete):
>    - `bindings/{sources,bus,migration}.rs` → `midi`, `osc`
>    - `signal/sink.rs` → `osc`
>    - `gpu/particle/{splat_source,source_loader}.rs` → `rfd` / `ureq`
>    - `trama/persist.rs`
>    - `media/{webcam,video,webcam_ffmpeg,mod}.rs`
>    - `shader/hot_reload.rs` → `notify`
>    - `gpu/frame_graph.rs`, `trama/exec/executor.rs` → `std::process`
>    - `gpu/context.rs`, `gpu/profiler.rs`, `gpu/frame_graph.rs`,
>      `gpu/particle/source_loader.rs` → `winit` / `egui`
>    - `signal/mod.rs` → `ctrlc` (the headless `--signal` loop)
>    - `trama/ui/` → `egui-snarl` (editor UI; gate it, keep trama's runtime)
>
>    Several modules are already gated on their own features (`analyze`,
>    `depth`, `link`, `ndi`, `output`, `spout`, `syphon`, `v4l2`, `test_alloc`);
>    keep those gates, and add `desktop` to them only where they need it.
>
>    Prefer a small trait or an `Option` over scattering `cfg` through function
>    bodies. When a core type holds a desktop handle (for example a MIDI system in
>    the binding bus), gate the field and its source, not the whole type.
> 4. Seams for Android, with no desktop behavior change:
>    - `effect::loader::assets_dir()`: add `set_assets_dir(PathBuf)`, which must be
>      called before first use. It wins over the cwd, exe-dir and macOS-bundle search.
>    - `paths::config_root()`: same override pattern.
>    - Make sure an offscreen renderer can be built from an existing
>      `wgpu::Device` + `Queue` without `GpuContext` (it has a non-optional
>      `Surface` and takes a winit window). `headless/gpu.rs` and
>      `headless/scene_renderer.rs` show the pattern; reuse it rather than
>      inventing another.
> 5. Verify:
>    `cargo check -p fosfora-app --lib --no-default-features --target aarch64-linux-android`
>    (install the target with rustup if it's missing) and the full `ci.yml` clippy and test matrix.
>
> **Acceptance:** both verification commands pass. The diff is `lib.rs`/`main.rs`
> restructuring plus additive `cfg` gating and the two override functions. Open a
> PR into **`main`** (it lands early so a long-running UI branch can absorb it
> while it is small), whose body lists every gated module and dependency and anything you
> were unsure about. Don't write a `CHANGELOG.md` entry (no user-facing change).
>
> **Don't:** change runtime behavior, rename public items beyond what the lib
> split forces, touch shaders, or bump dependency versions. Keep out of the
> internals of `ui/` and `app.rs`; gate them only at their `mod` declarations.
> Another large branch is rewriting the UI, so every line changed there becomes
> a merge conflict.

---

## C3: world-space ports of the lead effects (placeholder)

*Write the full brief with Kevin after S5. A candidate for `/effort ultracode`,
since the work splits into parallel per-effect pieces.*

Outline:
- Input: S5's world-space scatter path and particle-layout decision, the
  measured particle budget, and Kevin's list of 2–3 lead effects.
- Per effect: sim changes for a real `z` (layout per the S5 decision), audio
  mappings unchanged, obstacle modes working in 3D, one preset tuned for
  seated viewing.
- Acceptance per effect: runs at the S5 budget on device (Kevin verifies), and
  desktop CI is untouched.
