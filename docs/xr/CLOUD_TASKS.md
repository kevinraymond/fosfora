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

## C3a: world-space particle render path in core (against `main`)

*Approved by Kevin Sep 27 (board #3240) after S5 (board #3192, #3233). Runs
alone; nothing on `xr` depends on it until C3b. It is the only C3 change to
shared structure, so it lands on `main` as its own small PR, like the seam.*

> You are working in the Fosfora repo on a new branch `xr-particles-world`,
> cut from `main` (not from `xr`). Fosfora is a real-time audio-reactive
> visual engine: about 130K lines of Rust in `crates/fosfora-app` (a library
> plus a thin binary; wgpu 27; toolchain pinned at 1.97.0). Read `CLAUDE.md`,
> `ARCHITECTURE-NOTES.md` and `.github/workflows/ci.yml` first. The XR
> measurements that motivate this live on the `xr` branch only:
> `git show origin/xr:docs/xr/MEASURED.md` (section "Particle sweep (S5)",
> especially "What the cost is" and "Proposal"). Don't commit anything under
> `docs/xr/` or `crates/fosfora-xr/` to this branch; don't touch `ui/`,
> `app.rs` or `main.rs`.
>
> **Background.** Fosfora's particle sims write screen-space NDC into
> `pos_life.xy` and the billboard renderer (`gpu/particle/system.rs`,
> `assets/shaders/builtin/particle_render.wgsl`) draws them with
> `draw_indirect`, one instance per alive particle, no camera. A Quest 3 build
> (a separate crate that depends on `fosfora-app` with
> `default-features = false`) needs to draw particles that live in **meters
> in 3D**, once per eye through that eye's view and projection, with a depth
> test against other geometry. Measured on the Quest 3's Adreno 740, the
> instanced draw is per-instance bound: one non-instanced draw where the
> vertex index picks the particle ("vertex pulling") is 1.5x cheaper, and
> 3-vertex sprites another 1.25–1.6x. That is the shape to build.
>
> **Goal:** add a world-space render entry to `ParticleSystem` behind a clean
> API, with **zero behavior change on desktop**: no existing shader, pipeline,
> bind group layout or `.pfx` semantics change, and the new pipeline is
> created only when first used.
>
> **Invariants**
> - Every command in `ci.yml` passes unchanged, every clippy feature
>   combination included, `-D warnings`, workspace pedantic lints.
>   `cargo deny check` passes. No new dependencies.
> - `AudioFeatures` ABI and golden vectors untouched (`audio/features.rs`,
>   `audio/schema.rs`, `GOLDEN_HOPS`).
> - The `Particle` layout (four `vec4f` SoA components, `particle_lib.wgsl`)
>   and `ParticleUniforms` are unchanged. No new per-particle buffer.
> - Every new `unsafe` block has a `// SAFETY:` comment (none expected).
> - US English spelling in code, comments and docs.
>
> **Tasks**
> 1. **Layout convention, documented not enforced.** In `particle_lib.wgsl`
>    (near `struct Particle`) and in the `ParticleSystem` docs, define the
>    world layout a sim may opt into: `pos_life.xyz` = position in meters
>    relative to the effect anchor (+Y up, −Z forward, the OpenXR stage
>    convention), `vel_size.xyz` = velocity in m/s, `vel_size.w` = sprite
>    radius in meters, `color` as today, `flags` as today. Sims that keep
>    per-particle state in `pos_life.z` / `vel_size.z` today (most do:
>    initial size, mass, species, band, height) must move it to `flags.zw`
>    or the `aux` buffer in their world variant; that is C3b's job, not this
>    task's. Nothing changes for the existing 2D sims.
> 2. **Indirect args for a pulled draw.** Extend
>    `builtin/particle_prepare_indirect.wgsl` (or add a sibling) to also
>    write a second 16-byte indirect buffer `[3 * alive, 1, 0, 0]`
>    (vertex count, one instance) from `counters[0]`, so the world draw needs
>    no CPU readback of the alive count. Keep the existing
>    `[6, alive, 0, 0]` buffer as is.
> 3. **Shader `builtin/particle_render_world.wgsl`.** Group 0 = the existing
>    render bind group layout (`render_bgl`: SoA buffers read-only, render
>    uniforms, alive indices), unchanged. Group 1 = a new `WorldCamera`
>    uniform: `view: mat4x4f`, `proj: mat4x4f`, `anchor: vec4f` (xyz added to
>    every position, w unused), `gain: f32` and padding. Vertex entry:
>    `particle = alive_indices[vertex_index / 3u]`, `corner = vertex_index %
>    3u` into a triangle that circumscribes the unit disc
>    (`(-1.732, -1), (1.732, -1), (0, 2)`), position =
>    `proj * (view * vec4f(pos + anchor, 1) + vec4f(corner * radius, 0, 0))`
>    so the sprite has a size in meters and faces the eye. Fragment: the
>    same soft disc as `particle_render.wgsl` mode 0 (`exp(-d²·2)`,
>    discard below 0.01), premultiplied output. Sprite atlas modes, trails
>    and spin are out of scope (mode 0 only; document it).
> 4. **API.** On `ParticleSystem`:
>    ```rust
>    pub struct WorldCamera { pub view: glam::Mat4, pub proj: glam::Mat4, pub anchor: glam::Vec3 }
>    pub struct WorldTarget<'a> { pub color: &'a wgpu::TextureView, pub color_format: wgpu::TextureFormat,
>                                 pub depth: Option<&'a wgpu::TextureView>, pub depth_format: wgpu::TextureFormat }
>    pub fn render_world(&mut self, encoder: &mut wgpu::CommandEncoder, queue: &wgpu::Queue,
>                        target: WorldTarget<'_>, camera: &WorldCamera, load: wgpu::LoadOp<wgpu::Color>)
>    ```
>    It records one render pass (color `load`, depth load, **depth test
>    `Less`, depth write off**) and one `draw_indirect` on the new buffer,
>    using the additive or alpha blend the effect's `blend` selects, as the
>    2D path does. Pipelines are cached per `(color_format, depth_format)`
>    and built on first call; the camera uniform buffer is created on first
>    call too. Which SoA index to bind: the caller may call this either
>    inside the frame (before `flip`, like `render`) or after
>    `SceneRenderer::step` returned (after `flip`). Make the index explicit
>    and safe: for example `render_world` always draws the buffers the last
>    `dispatch` wrote, tracked by a field set in `dispatch` and `flip`,
>    and document it. Also expose `pub fn alive_count(&self) -> u32` if it
>    isn't public already.
> 5. **3D flow sampling.** In `particle_lib.wgsl` add
>    `fn sample_flow_field_3d(pos_m: vec3f, extent_m: f32) -> vec3f` that
>    maps a position within ±extent_m of the anchor to the existing 3D flow
>    texture's [0,1]³ (all three axes; today's `sample_flow_field` maps 2D
>    clip space and scrolls z with time) and returns the xyz velocity. The
>    existing 2D function is untouched.
> 6. **Tests**, using the headless device (`headless/gpu.rs`, software
>    Vulkan is available in CI):
>    - a test sim (`#[cfg(test)]` WGSL string or a file under
>      `assets/shaders/builtin/test/` if that pattern exists) that spawns a
>      handful of particles at fixed world positions, one `dispatch`, then
>      `render_world` into a 256x256 `Rgba8Unorm` target with a known
>      `WorldCamera` (identity view at the origin, a symmetric 90° projection,
>      near 0.05, far 100). Assert each particle's pixel lands within 2 px of
>      the analytically projected point, and that a particle behind the far
>      plane or outside the frustum draws nothing.
>    - the same with a depth texture pre-filled so half the image is
>      "near": assert particles in that half are hidden and the other half
>      visible.
>    - the indirect buffer holds `[3 * alive, 1, 0, 0]` after a dispatch.
>    - `cargo test` (default features), `cargo clippy --all-targets -- -D
>      warnings` for the default build and for `--no-default-features`, and
>      `cargo check -p fosfora-app --lib --no-default-features --target
>      aarch64-linux-android` (install the target with rustup) all pass.
> 7. No `CHANGELOG.md` entry (nothing user-facing on desktop). Commit
>    messages carry the rationale. Open a PR into `main` titled
>    "particles: world-space render entry for the XR build", with the
>    measured motivation in one paragraph and a list of the public API added.
>
> **Deliverable:** the PR URL, its CI run green, and one paragraph on
> anything you had to decide that the brief left open (for example how the
> ping-pong index is tracked). Don't add the render entry to any desktop
> code path.

**Gate (Kevin, then local):** desktop CI green on the PR; `cargo check` for
Android passes; the tests above pass; the diff touches only
`gpu/particle/*`, the two builtin shaders, `particle_lib.wgsl` and tests.
Then Kevin merges, `xr` merges `main`, and C3b starts.

## C3b: world-space ports of Flux, Murmur, Tide (after C3a)

*Lead effects picked by Kevin Sep 27 (board #3239). Brief to write once C3a
is on `main`; it can split into one cloud session per effect.*

Outline:
- Input: C3a's `render_world` + `sample_flow_field_3d`, the S5 budget
  (about 1M sprites at 72 Hz and 750K at 90 Hz on Quest 3 before the
  effect's own passes), and the layout convention from C3a task 1.
- Per effect, an XR variant sim under `assets/xr/shaders/<name>_xr_sim.wgsl`
  with a hidden `.pfx` under `assets/xr/effects/` (the S4 pattern): world
  `xyz` in meters in a ~2 m volume at the anchor, per-particle state moved
  out of the `.z` slots, audio mappings unchanged, obstacle modes working in
  3D where the effect uses them, one preset tuned for seated viewing.
  Flux: curl flow through `sample_flow_field_3d`. Murmur: 3D boids
  (topological neighbors in 3D, predator avoidance in 3D). Tide: height
  becomes real `y`, the sheet pours onto a horizontal plane at the anchor.
- Desktop CI untouched; the variants are hidden and staged into the APK only.
- Acceptance per effect: Kevin verifies on device at the S5 budget
  (`MEASURED.md` row per effect); a headless test renders one frame of each
  variant without validation errors.
