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

*Lead effects picked by Kevin Sep 27 (board #3239). C3a is on `main` (PR
merged Sep 27) and `xr` carries it. One cloud session per effect, Flux first;
the brief below is for Flux and says what changes for the other two.*

> You are working in the Fosfora repo on branch `xr`, in a new branch
> `xr-flux-world` cut from it. Fosfora is a real-time audio-reactive visual
> engine (`crates/fosfora-app`, ~130K lines of Rust, wgpu 27, toolchain
> pinned at 1.97.0) with a Quest 3 build in `crates/fosfora-xr`. Read
> `CLAUDE.md`, `ARCHITECTURE-NOTES.md`, `docs/xr/XR_DESIGN.md` (sections
> "World-space particles (S5)" and "Mixed reality (S7)"), `docs/xr/MEASURED.md`
> (sections "Particle sweep (S5)", "Proposal", "Mixed reality (S7)") and
> `.github/workflows/ci.yml` first. You have no device; the person who
> dispatched you runs the result on a Quest 3 and fills in the numbers.
>
> **Background.** Desktop Fosfora sims write screen-space NDC into
> `pos_life.xy` and draw through a compute raster or the billboard path.
> The Quest build draws world-space particles once per eye through
> `ParticleSystem::render_world` (`crates/fosfora-app/src/gpu/particle/world.rs`,
> `assets/shaders/builtin/particle_render_world.wgsl`, one non-instanced
> pulled draw of 3-vertex sprites, premultiplied output, added by C3a). The
> opt-in world layout is documented in `assets/shaders/lib/particle_lib.wgsl`
> ("World layout"): `pos_life.xyz` = meters relative to the effect anchor
> (+Y up, −Z forward), `vel_size.xyz` = m/s, `vel_size.w` = sprite radius in
> meters; per-particle state a sim keeps in the `.z` slots today moves to
> `flags.zw` or the `aux` buffer. `sample_flow_field_3d(pos_m, extent_m)` in
> the same file samples the volumetric flow-field texture for such a sim.
> The XR crate already drives a core effect headlessly through
> `fosfora_app::headless::scene_renderer::SceneRenderer`
> (`crates/fosfora-xr/src/scene.rs`, the S4 quad path) and has its own test
> sim with obstacles (`crates/fosfora-xr/src/particles3d*`, S5/S7); the
> world-space product path is meant to replace that test sim.
>
> **Measured facts that shape this port (Quest 3, `docs/xr/MEASURED.md`):**
> - Budget with passthrough, hands, room anchors and depth occluders on:
>   **500K sprites at 72 Hz (8.6 ms GPU)**; 250K at 5.5 ms. Plan Flux for
>   ~300K alive so its own passes fit beside the sprites.
> - The sprite draw is 3× cheaper when the eye pass contains a
>   depth-writing opaque draw (the "primer" finding, S7). The XR crate
>   already draws depth occluders and a primer; a port must not rely on
>   turning that off.
> - Over passthrough the eye target clears to alpha 0 and sprites must
>   write premultiplied color; additive color with `OVER` alpha is what the
>   S5/S7 sim uses and it composites correctly.
> - Sprites nearer than ~0.3 m to the eye are culled by the XR draw; the
>   wearer stands inside the cloud, so a port should look right from inside
>   its volume, not only from a seat in front of it.
> - Flux as shipped (compute raster + velocity feedback at 1280x720) ran at
>   6 fps on the quad (S4). The world port keeps the sim and the look of the
>   flow, not the raster resolve.
>
> **Goal (Flux):** an XR variant of Flux that runs its sim in world space
> and renders through `render_world`, with zero change to desktop Flux or
> any shared shader, pipeline or `.pfx` semantics.
>
> **Invariants**
> - Every command in `ci.yml` passes unchanged (`cargo fmt --all -- --check`,
>   `cargo clippy --all-targets -- -D warnings` for the default build and
>   each feature set, `cargo test`), workspace pedantic lints, `cargo deny
>   check`. No new dependencies. `cargo ndk -t arm64-v8a --platform 29 clippy
>   -p fosfora-xr --all-targets -- -D warnings` also passes (install the
>   `aarch64-linux-android` target and `cargo-ndk 4.1.2`; `scripts/xr/run.sh
>   lint` wraps it).
> - `AudioFeatures` ABI and golden vectors untouched. Desktop sims, the 2D
>   render path, `ui/`, `app.rs`, `main.rs` untouched. Nothing under
>   `docs/xr/` changes except a MEASURED.md row left for the device run.
> - Every new `unsafe` block has a `// SAFETY:` comment (none expected).
> - US English spelling. No mention of any event, deadline or prize.
>
> **Tasks**
> 1. **Sim variant.** `assets/xr/shaders/flux_xr_sim.wgsl` from
>    `assets/shaders/flux_sim.wgsl`: positions in meters in a volume about
>    2 m across at the anchor (particles leaving it respawn inside, with a
>    fade near the bounds as in `crates/fosfora-xr/src/particles3d_draw.wgsl`),
>    velocity from `sample_flow_field_3d`, the initial-size state moved out
>    of `pos_life.z` into `flags.zw` or `aux`, audio mappings unchanged
>    (bass → flow speed, rms and beats → size, as the desktop sim does).
>    Keep the desktop file byte-identical.
> 2. **Hidden preset.** `assets/xr/effects/flux_xr_world.pfx` following the
>    S4 variants in that directory (hidden from the desktop library, staged
>    into the APK by `android/app/build.gradle.kts` and
>    `crates/fosfora-xr/src/assets.rs`): points at the XR sim, disables the
>    raster/feedback passes, one preset tuned for a wearer standing in the
>    cloud.
> 3. **Obstacles.** The XR crate feeds hand-joint spheres and room boxes to
>    its test sim through an `Obstacles` uniform (`particles3d_sim.wgsl`,
>    `ObstacleSet` in `particles3d.rs`). Give the Flux variant the same
>    block and `collide()` (copy the WGSL into the XR sim; do not change the
>    core `ParticleSystem` bind group layouts). If that needs a core hook
>    (an extra bind group on the compute pass), stop and write the smallest
>    API you would add instead of adding it.
> 4. **Drive it.** In `crates/fosfora-xr`, a mode `debug.fosfora.mode
>    world` (see the knob list in `app.rs`) that loads the hidden preset
>    through `XrScene`/`SceneRenderer` for the sim step and calls
>    `render_world` once per eye into the eye pass after the occluders and
>    primer, with the S7 passthrough, hands, room and floor exactly as
>    `mode mr` does. Reuse the mixed-mode plumbing; do not fork it.
> 5. **Tests.** A headless test in `fosfora-app` that loads the XR preset,
>    steps the sim a few frames and renders one frame through
>    `render_world` with no validation errors (see the C3a tests in
>    `world.rs`: `prepare_shader_validates`, `particle_lib_flow_3d_validates`
>    for the pattern). It must run in CI without a GPU if the existing
>    headless tests do, or be `#[ignore]` with the same reason they use.
>
> **Deliverable:** one PR against `xr` (not `main`), CI green, Android clippy
> green, the commit body carrying the design rationale (what moved out of
> `pos_life.z`, how the volume and respawn work, what the preset changes). A
> short section at the end of the PR body: "Device run needed: `mode world`
> at 250K and 500K, 72 Hz, with `scripts/xr/sweep.sh --mode world`; fill the
> MEASURED.md row." Do not claim device numbers.
>
> **Murmur and Tide** (later sessions, same shape): Murmur is 3D boids
> (topological neighbors and predator avoidance in 3D; `murmur_history.wgsl`
> stays 2D-free), a murmuration in the room at ~200K; Tide's water height
> becomes real `y` and the sheet pours onto a horizontal plane at the anchor
> and onto the room's table boxes through the obstacle block.

## C3c: 3D spatial hash in core (against `main`, before Murmur)

*Why a core change: Murmur's topological K=7 neighbors need a neighbor
query in three dimensions, and core's spatial hash
(`gpu/particle/spatial_hash.rs`, the three `builtin/spatial_hash_*.wgsl`
passes, the `sh_*` helpers in `particle_lib.wgsl`) bins `pos_life.xy` over
clip space only. Binning a world-layout sim through it would clamp every
position beyond ±1 m to an edge cell and ignore depth entirely. Like C3a,
this is a small addition to shared structure, so it lands on `main` as its
own PR and `xr` merges it. Three desktop effects use the hash today (Murmur,
Symbiosis, Genesis) and `water.rs` patches its grid width; none of them may
change.*

> **Identity first, before you read or change anything.** This clone's git
> user is preset to `Claude <noreply@anthropic.com>`. That preset is wrong
> for this repo and is the thing you are fixing; do not treat "the repo's
> configured git user" as the author to keep. Run, in one shell call:
>
> ```bash
> unset GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL
> git config --global user.name "Kevin Raymond"
> git config --global user.email "kjraym@gmail.com"
> git config user.name "Kevin Raymond"
> git config user.email "kjraym@gmail.com"
> git config --global --unset-all commit.template || true
> git config --unset-all commit.template || true
> git config --show-origin --get-regexp 'user\.|commit\.|trailer\.|core\.hookspath' || true
> ls .git/hooks | grep -v sample || true
> git var GIT_AUTHOR_IDENT; git var GIT_COMMITTER_IDENT
> ```
>
> Both `git var` lines must start with `Kevin Raymond <kjraym@gmail.com>`.
> If they do not, or if a hook or trailer setting shows up, remove what
> overrides it before the first commit. If the environment re-injects
> `GIT_AUTHOR_*` / `GIT_COMMITTER_*` on every shell call, make every commit
> as
> `GIT_COMMITTER_NAME="Kevin Raymond" GIT_COMMITTER_EMAIL="kjraym@gmail.com" git commit --author="Kevin Raymond <kjraym@gmail.com>" ...`.
> After every commit, in the same shell call, run
> `git log -1 --format='%an <%ae> / %cn <%ce>%n%B'` and amend if anything but
> that name and email appears, or if the body carries a `Co-Authored-By`,
> `Claude-Session` or other trailer, a "Generated with" line or a model
> name. The same applies to the PR title and body: write the body from a
> file, then `gh pr view --json body -q .body | grep -iE 'claude|anthropic|generated|co-authored'`
> must print nothing (fix with `gh pr edit --body-file`). `CLAUDE.md` says
> the same; it overrides your defaults.
>
> You are working in the Fosfora repo on a new branch `xr-hash-3d` cut from
> `main` (not from `xr`). Fosfora is a real-time audio-reactive visual
> engine: about 130K lines of Rust in `crates/fosfora-app`, wgpu 27,
> toolchain pinned at 1.97.0. Read `CLAUDE.md`, `ARCHITECTURE-NOTES.md` and
> `.github/workflows/ci.yml` first. Don't commit anything under `docs/xr/`
> or `crates/fosfora-xr/`; don't touch `ui/`, `app.rs` or `main.rs`.
>
> **Background.** `SpatialHashGrid` (`crates/fosfora-app/src/gpu/particle/spatial_hash.rs`)
> is a count / prefix-sum / scatter pipeline over a `GRID_W x GRID_H` grid
> that maps `pos_life.xy` in clip space [-1,1]² to cells
> (`builtin/spatial_hash_count.wgsl`, `spatial_hash_scatter.wgsl`,
> `spatial_hash_prefix_sum.wgsl`; the grid constants are patched into the
> source by `patch_grid_constants`, and into the sim's `particle_lib.wgsl`
> `SH_GRID_W/H` by `effect/loader.rs`). A sim opts in with
> `"interaction": true` in its `.pfx` and queries neighbors with
> `sh_pos_to_cell(vec2f)` and `sh_cell_range(gx, gy)` over group 3.
> `grid_dims` sizes the grid for ~16 particles per cell, clamped to
> [40, 256] (or the `.pfx`'s `grid_max`). World-layout sims (documented
> under "World layout" in `assets/shaders/lib/particle_lib.wgsl`) keep
> `pos_life.xyz` in meters within ±`emitter_radius` of an anchor; a Quest
> murmuration (a later branch) needs K-nearest neighbors in that volume.
>
> **Goal:** a 3D mode of the same spatial hash, opt-in per effect, with
> zero behavior change for every effect that does not opt in.
>
> **Invariants**
> - Every command in `ci.yml` passes unchanged, every clippy feature set,
>   `-D warnings`, workspace pedantic lints; `cargo deny check` passes. No
>   new dependencies. No new `unsafe` (if one is needed it gets a
>   `// SAFETY:` comment).
> - `AudioFeatures` ABI and golden vectors untouched.
> - The 2D hash is byte-identical in behavior: the three existing hash
>   shaders and the `sh_pos_to_cell` / `sh_cell_range` helpers do not
>   change; Murmur, Symbiosis and Genesis load and step exactly as before
>   (their `.pfx` files are untouched). `water.rs` keeps its patch.
> - The `Particle` layout, `ParticleUniforms` and the `.pfx` semantics of
>   every existing field are unchanged.
> - US English spelling.
>
> **Tasks**
> 1. **Preset field.** `"interaction_3d": true` on the particle block
>    (`ParticleDef` in `gpu/particle/types.rs`, `#[serde(default)]`,
>    implies `interaction`). Document it beside `interaction` and
>    `grid_max`: cells cover the cube within ±`emitter_radius` meters of
>    the anchor on all three axes (the world layout's volume), so a sim
>    that opts in must keep `pos_life.xyz` in meters.
> 2. **Grid sizing.** `grid_dims_3d(max_particles, grid_max)`: one edge
>    `d = cbrt(max_particles / 16)`, clamped to [8, 64] (or `grid_max`),
>    `num_cells = d³`. 200K particles give 23³ = 12,167 cells. The Blelloch
>    scan in `spatial_hash_prefix_sum.wgsl` chunks `NUM_CELLS` over one
>    workgroup, so it already handles any count; check it, don't assume.
> 3. **Count and scatter.** New files `builtin/spatial_hash_count_3d.wgsl`
>    and `builtin/spatial_hash_scatter_3d.wgsl` (the 2D files stay
>    byte-identical). Same bindings as the 2D pair; the `Uniforms` struct
>    extends the prefix it already mirrors far enough to read
>    `emitter_radius` from the live `ParticleUniforms` (same buffer, same
>    offsets; add a `const _` or a test that pins the offset). Cell =
>    `clamp(u32((pos.xyz / extent * 0.5 + 0.5) * GRID_D), 0, GRID_D-1)` per
>    axis, index `(z * GRID_D + y) * GRID_D + x`, with `extent =
>    max(u.emitter_radius, 1e-3)`. `SpatialHashGrid::new` takes the mode
>    (an enum, not a bool) and picks the pair and the dims; `dispatch` is
>    unchanged.
> 4. **Query helpers.** In `particle_lib.wgsl`, after the 2D helpers:
>    `const SH_GRID_D: u32 = 1u;` (patched like `SH_GRID_W/H`, stays 1 for
>    2D effects), `fn sh_pos_to_cell_3d(pos: vec3f, extent: f32) -> vec3i`
>    and `fn sh_cell_range_3d(c: vec3i) -> vec2u` (out of range → (0, 0)),
>    both using the same mapping and index order as the count pass. The 2D
>    helpers keep their bodies. `effect/loader.rs` patches `SH_GRID_D`
>    where it patches `SH_GRID_W/H` (both places).
> 5. **Tests** on the headless device (`headless/gpu.rs`; software Vulkan
>    is in CI):
>    - a `#[cfg(test)]` sim (or a test `.pfx` + WGSL under the pattern the
>      C3a tests in `gpu/particle/world.rs` use) with `interaction_3d`,
>      eight particles at the corners of a ±0.5 m cube plus one at the
>      origin, `emitter_radius` 1.0: after one `dispatch`, read back
>      `cell_counts` and `sorted_indices` and assert each corner sits in
>      its own cell at the expected index, the origin in the center cell,
>      and the counts sum to 9.
>    - the sim side: a query from a corner particle through
>      `sh_cell_range_3d` over its 27-cell neighborhood finds exactly the
>      origin particle when the grid edge is small enough, and nothing
>      when it is not (pick two `grid_max` values that make that true).
>    - a regression guard: `grid_dims` for the 2D path returns what it
>      returned before for 40K, 200K and 1.2M particles, and the three
>      2D hash shader sources still contain their `const GRID_W: u32 = 40u;`
>      anchors.
>
> **Deliverable:** one PR against `main`, CI green, the commit body carrying
> the rationale (why a separate 3D pair, why `emitter_radius` is the extent,
> why the 2D path is untouched). No `CHANGELOG.md` entry: nothing user-facing
> changes until a 3D effect ships. Do not mention any event, deadline or
> prize anywhere.
>
> Before opening the PR, verify: `git log --format='%an <%ae> / %cn <%ce>' main..HEAD`
> shows only `Kevin Raymond <kjraym@gmail.com>` on every line, and
> `git log --format=%B main..HEAD | grep -iE 'co-authored|claude|anthropic|generated'`
> prints nothing. If either check fails, rewrite the commits
> (`git rebase` / `git commit --amend --reset-author`) until both pass, then
> re-run both checks and paste their output at the end of your final report.
> The PR body follows `.github/PULL_REQUEST_TEMPLATE.md` if there is one and
> contains no model names or session links.

Then Kevin merges (squash), `xr` merges `main`, and C3b Murmur starts.

## C3b Murmur: the murmuration in the room (after C3c)

*Same shape as the Flux port. It inherits everything the Flux world path
built on `xr`: `mode world`, the `debug.fosfora.effect` knob, the aux
obstacle block with the head, the settle drift lane, hand spheres and room
boxes, the near fade, and the eye-pass hook (`prepare_world` before the eye
pass, `draw_world` inside it). Nothing in `crates/fosfora-xr` should need to
change; if it does, it is one knob default at most.*

> **Identity first, before you read or change anything.** This clone's git
> user is preset to `Claude <noreply@anthropic.com>`. That preset is wrong
> for this repo and is the thing you are fixing; do not treat "the repo's
> configured git user" as the author to keep. Run, in one shell call:
>
> ```bash
> unset GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL
> git config --global user.name "Kevin Raymond"
> git config --global user.email "kjraym@gmail.com"
> git config user.name "Kevin Raymond"
> git config user.email "kjraym@gmail.com"
> git config --global --unset-all commit.template || true
> git config --unset-all commit.template || true
> git config --show-origin --get-regexp 'user\.|commit\.|trailer\.|core\.hookspath' || true
> ls .git/hooks | grep -v sample || true
> git var GIT_AUTHOR_IDENT; git var GIT_COMMITTER_IDENT
> ```
>
> Both `git var` lines must start with `Kevin Raymond <kjraym@gmail.com>`.
> If they do not, or if a hook or trailer setting shows up, remove what
> overrides it before the first commit. If the environment re-injects
> `GIT_AUTHOR_*` / `GIT_COMMITTER_*` on every shell call, make every commit
> as
> `GIT_COMMITTER_NAME="Kevin Raymond" GIT_COMMITTER_EMAIL="kjraym@gmail.com" git commit --author="Kevin Raymond <kjraym@gmail.com>" ...`.
> After every commit, in the same shell call, run
> `git log -1 --format='%an <%ae> / %cn <%ce>%n%B'` and amend if anything but
> that name and email appears, or if the body carries a `Co-Authored-By`,
> `Claude-Session` or other trailer, a "Generated with" line or a model
> name. The same applies to the PR title and body: write the body from a
> file, then `gh pr view --json body -q .body | grep -iE 'claude|anthropic|generated|co-authored'`
> must print nothing (fix with `gh pr edit --body-file`). `CLAUDE.md` says
> the same; it overrides your defaults.
>
> You are working in the Fosfora repo on branch `xr`, in a new branch
> `xr-murmur-world` cut from it. Fosfora is a real-time audio-reactive
> visual engine (`crates/fosfora-app`, ~130K lines of Rust, wgpu 27,
> toolchain pinned at 1.97.0) with a Quest 3 build in `crates/fosfora-xr`.
> Read `CLAUDE.md`, `ARCHITECTURE-NOTES.md`, `docs/xr/XR_DESIGN.md`
> (sections "World-space particles (S5)" and "Mixed reality (S7)"),
> `docs/xr/MEASURED.md` (sections "Mixed reality (S7)" and "C3b: Flux in
> world space") and `.github/workflows/ci.yml` first. You have no device;
> the person who dispatched you runs the result on a Quest 3 and fills in
> the numbers.
>
> **Background.** The Flux world port is the template: read
> `assets/xr/shaders/flux_xr_sim.wgsl` and `assets/xr/effects/flux_xr_world.pfx`
> end to end before writing anything, and `crates/fosfora-xr/src/scene.rs`
> (`new_world`, `set_world_inputs`) for what the app feeds the sim. The
> aux buffer's first 163 rows carry the XR inputs (head + near-fade radius,
> the obstacle header with restitution / margin / hand kick / settle drift,
> 64 hand-joint spheres, 32 room boxes), so a world sim cannot keep
> per-particle state in `aux`; it has `flags.zw` (Flux uses them for the
> initial radius and the base opacity). Desktop Murmur
> (`assets/shaders/murmur_sim.wgsl`, `assets/effects/murmur.pfx`) is 2D
> boids over the spatial hash: K=7 topological neighbors from a 9-cell
> scan, Vicsek noise driven by bass, adaptive separation, a predator on
> onsets and kicks, roost centering, a heading angle in `flags.z`, birth
> time in `flags.w`, initial size in `pos_life.z`, dark silhouettes with a
> key-tinted rim light, drawn through the compute raster with a velocity
> field and a history (streak) pass. The `main` merge that brought C3c
> gives core a 3D spatial hash: `"interaction_3d": true` in the `.pfx`,
> `sh_pos_to_cell_3d(pos, extent)` and `sh_cell_range_3d(cell)` in
> `particle_lib.wgsl`, cells over ±`emitter_radius` on all three axes.
>
> **Measured facts that shape this port (Quest 3, `docs/xr/MEASURED.md`):**
> - Flux in world space at 400K costs 9.3–10.7 ms at 72 Hz with the room,
>   hands and passthrough on; its sim is ~3.3 ms at 300K (two 3D flow
>   samples, two noise lookups, a 69-obstacle collide loop) and the draw
>   ~5.8 ms. The frame budget at 72 Hz is 13.9 ms. Murmur's neighbor scan
>   is the expensive part: 27 cells × up to `MAX_PER_CELL` candidates per
>   bird per frame. Plan for **200K birds** and make the per-cell cap and
>   K constants at the top of the file, so a device sweep can trade them.
> - The wearer stands inside the volume: the flock must look right from
>   inside, and the near fade (0.15 m around the head) already handles
>   sprites at the eyes.
> - `render_world` has an alpha pipeline (`blend: "alpha"`): the fragment
>   writes straight color and alpha over a target cleared to alpha 0, which
>   composites as a premultiplied layer over passthrough. Dark birds with
>   alpha ~0.9 therefore read as **dark silhouettes against the real
>   room**, which is the look: keep the desktop's near-black palette and
>   the rim light, drop the twilight sky.
>
> **Goal (Murmur):** an XR variant of Murmur whose flock lives in meters
> around the wearer, avoids the wearer's hands as predators, wheels around
> the room's furniture, and renders through `render_world`, with zero
> change to desktop Murmur or any shared shader, pipeline or `.pfx`
> semantics.
>
> **Invariants**
> - Every command in `ci.yml` passes unchanged (fmt, clippy per feature set
>   with `-D warnings`, tests), workspace pedantic lints, `cargo deny check`.
>   No new dependencies. `scripts/xr/run.sh lint` (Android clippy for
>   `fosfora-xr`) passes; install the `aarch64-linux-android` target and
>   `cargo-ndk`.
> - `AudioFeatures` ABI and golden vectors untouched. Desktop sims, the 2D
>   render path, the spatial hash, `ui/`, `app.rs`, `main.rs` untouched.
>   `assets/shaders/murmur_sim.wgsl`, `murmur_history.wgsl`, `murmur_bg.wgsl`
>   and `assets/effects/murmur.pfx` byte-identical. Nothing under
>   `docs/xr/` changes except a MEASURED.md row left for the device run.
> - No core change. If the port needs one, stop and write the smallest
>   API you would add in the PR body instead of adding it.
> - Every new `unsafe` block has a `// SAFETY:` comment (none expected).
> - US English spelling. No mention of any event, deadline or prize.
>
> **Tasks**
> 1. **Sim variant** `assets/xr/shaders/murmur_xr_sim.wgsl`, from the
>    desktop sim, in the world layout:
>    - `pos_life.xyz` meters relative to the anchor in the same 3 m cube
>      as Flux (`emitter.radius` 1.5, read as `u.emitter_radius`);
>      `vel_size.xyz` m/s, `vel_size.w` sprite radius in meters.
>    - **Heading is the velocity direction** (`normalize(vel_size.xyz)`),
>      not a stored angle: every steering term, including the boundary and
>      roost terms, goes into `target_vel` before the heading low-pass, and
>      the low-pass runs on the unit vector (slerp-free: normalize the
>      blend, keep `frame_diffuse` for the rate). That frees `flags.z` for
>      the initial size (was `pos_life.z`) and keeps `flags.w` as birth
>      time. Per-bird speed variation stays a hash of `idx`.
>    - **Neighbors** from `sh_cell_range_3d` over the 27 cells around
>      `sh_pos_to_cell_3d(pos, u.emitter_radius)`, center cell first, the
>      same K=7 insertion sort, `MAX_PER_CELL` 16 to start; alignment from
>      neighbor velocity directions, cohesion toward their center of mass,
>      the adaptive separation from the K-th distance, the edge factor, all
>      as the 2D sim does but in 3D.
>    - **Roost** at the anchor plus 0.5 m up (eye height for a standing
>      wearer), quadratic beyond 0.5 m as today; **bounds** a soft
>      repulsion within 0.3 m of the cube's faces (the 2D edge term per
>      axis) and the Flux respawn when a bird still leaves.
>    - **Vicsek noise** as a random unit-vector perturbation of the target
>      direction with the same `eta` from bass; use `xr_rand3` /
>      `uhash` (copy them from the Flux sim), never fract-sin on `idx`.
>    - **Predators: the hands.** Copy the aux layout constants and
>      `xr_collide()` from the Flux sim verbatim. Every hand-joint sphere
>      in the block repels birds within 0.4 m of it, weighted by
>      `predator_strength()` and a steady 1.0 intensity (a hand is always a
>      hawk), added to `target_vel` before the heading low-pass; the
>      audio predator keeps its onset/kick intensity and becomes a 3D
>      Lissajous around the roost with strikes into the flock. `xr_collide`
>      then runs as in Flux so no bird passes through a hand, a table or
>      the floor (the room boxes are hard obstacles; a bird that touches
>      one slides). Ignore the settle drift lane (`aux[2].z`): birds do not
>      settle. Emission near a donor bird uses the 3D probe cell.
>    - **Look**: dark silhouette colors from the preset's gradient, alpha
>      0.9 with the spawn fade and the opacity curve, the rim light from
>      the edge factor and `dominant_chroma`, the near fade around the
>      head and the edge fade toward the bounds exactly as Flux does them
>      (opacity evaluated from the base each frame, never compounded).
>      No depth-based sizing (depth is real now); size from `flags.z`,
>      neighbor density and rms as today, in meters.
> 2. **Hidden preset** `assets/xr/effects/murmur_xr_world.pfx` beside the
>    Flux one: `"interaction": true, "interaction_3d": true`, `blend:
>    "alpha"`, `render_mode: "billboard"`, `velocity_field: false`, no
>    velocity or history passes (keep only `background`, as the Flux preset
>    does), `max_count` 200000, `emit_rate` sized so the flock fills in
>    ~4 s, `emitter.radius` 1.5, `initial_size` 0.012 m, `size_end` 0.012,
>    `lifetime` 15, the same inputs and audio mappings as desktop, and
>    `"hidden": true`. The gradle staging and `assets.rs` pick it up from
>    the directory; check they do.
> 3. **Drive it.** `adb shell setprop debug.fosfora.effect "Murmur XR World"`
>    with `mode world` must load it with no change to `crates/fosfora-xr`.
>    If a default in `app.rs` (near fade, hand pad, kick) is wrong for
>    birds, change that one default only under a `world` + effect-name
>    check and say why in the commit body.
> 4. **Tests** in `fosfora-app` next to `flux_xr_world_steps_and_renders`
>    in `gpu/particle/world.rs`: the preset parses with the fields above,
>    the sim compiles (`interaction_3d` builds the 3D hash), steps a few
>    frames and renders one frame through `render_world` with the alpha
>    pipeline and no validation errors; plus one pixel assertion that a
>    bird placed in front of the camera lands dark (rgb < 0.1) with alpha
>    > 0.5 in the target. Same CI rules as the C3a tests.
>
> **Deliverable:** one PR against `xr` (not `main`), CI green, Android
> clippy green, the commit body carrying the design rationale (heading from
> velocity, the 3D neighbor scan and its caps, hands as predators, what the
> preset drops). End the PR body with: "Device run needed: `mode world`,
> `debug.fosfora.effect "Murmur XR World"`, `scripts/xr/sweep.sh --mode
> world --counts "100000 150000 200000 300000"` at 72 Hz; fill the
> MEASURED.md row; worn gate: the flock splits around a hand and wheels
> around the desk." Do not claim device numbers.
>
> Before opening the PR, verify: `git log --format='%an <%ae> / %cn <%ce>' xr..HEAD`
> shows only `Kevin Raymond <kjraym@gmail.com>` on every line, and
> `git log --format=%B xr..HEAD | grep -iE 'co-authored|claude|anthropic|generated'`
> prints nothing. If either check fails, rewrite the commits
> (`git rebase` / `git commit --amend --reset-author`) until both pass, then
> re-run both checks and paste their output at the end of your final report.
> The PR body follows `.github/PULL_REQUEST_TEMPLATE.md` if there is one and
> contains no model names or session links.

Then: device sweep, worn gate, squash-merge into `xr`, and Tide (same
shape; its brief follows once Murmur's numbers are in).
