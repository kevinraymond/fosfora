# Build plan: week-1 spike (S0–S7)

**Goal:** by **Sun Oct 4, 2026**, know whether native Rust + OpenXR + wgpu can
carry Fosfora VR to a shippable build. Each step answers one risky question.
Stop at every gate and report (format in `CLAUDE.md`).

| Step | Question it answers | Where | Time box | Depends on |
|---|---|---|---|---|
| S0 | Is the machine ready? | Local | 2 h | — |
| S1 | Can we package and launch a native OpenXR app on Quest 3? | Local + device | 1 day | S0 |
| S2 | Can wgpu render through OpenXR's Vulkan device? | Local + device | 1 day | S1 |
| S3 | Can Fosfora's core compile for Android without desktop crates? | Cloud (C2) or local | 1 day | S0 (runs alongside S1–S2) |
| S4 | Does a real Fosfora effect run in the headset? | Local + device | 0.5 day | S2, S3 |
| S5 | Can the particle renderer go world-space and stereo, and how many particles fit? | Local + device | 1.5 days | S4 |
| S6 | Does audio analysis work on device? | Local + device | 0.5 day | S4 |
| S7 | Are passthrough, hands and room geometry reachable natively? | Local + device | 1 day | S5 |

**Pivot triggers (native-specific).** Pivot to the Unity shell if **S1, S2 or S7**
fails its gate after 1.5× its time box, with no credible fix in sight. S5's
particle ceiling is **not** a pivot trigger: Unity runs on the same GPU. It sets
design budgets instead.

---

## S0: setup (local, 2 h)

**Tasks**
1. Create `./fosfora` as a **git worktree** of Kevin's existing checkout (ask
   him for its path), on a new `xr` branch cut from up-to-date `main`:
   ```bash
   git -C <kevin-checkout> fetch origin
   git -C <kevin-checkout> worktree add "$PWD/fosfora" -b xr origin/main
   ```
   Never check out, rebase or commit on Kevin's other branches (his v2 UI work
   lives on one). If he has no local checkout, fall back to
   `git clone https://github.com/kevinraymond/fosfora fosfora && git -C fosfora switch -c xr`.
   Push `xr` to origin with upstream tracking; cloud sessions need it.
2. Copy this kit's `docs/` to `fosfora/docs/xr/`. Add a short "XR work" section
   to `fosfora/CLAUDE.md` that points to `docs/xr/` and restates invariants I1–I7
   in one line each. Commit (a docs-only commit; CI ignores `**.md`).
3. Seed blackboard: tasks S0–S7 and C1–C3, each with its gate as the done-criterion.
4. Check the toolchain and record the versions in `docs/xr/MEASURED.md`:
   - `rustup show` resolves to 1.97.0 inside the repo, then
     `rustup target add aarch64-linux-android`.
   - Android SDK + NDK present (`ANDROID_HOME`, `ANDROID_NDK_HOME`), and the NDK version.
   - JDK 17+ for Gradle.
   - `cargo install cargo-ndk --locked`.
   - `adb devices` lists the Quest 3. Record the Horizon OS version from the headset.
5. Baseline desktop health: `cargo clippy --all-targets -- -D warnings` and
   `cargo test` on `xr` before any code change. Record the times.

**Gate:** everything above present, and the desktop baseline green. Report
anything missing, with the install command. Don't install system packages without asking.

---

## S1: hello OpenXR (local + device, 1 day)

Package and launch a native OpenXR app that clears each eye to a color.
wgpu isn't involved yet; use `ash` directly. The design is in `XR_DESIGN.md`.

**Tasks**
1. Create `crates/fosfora-xr` (a `cdylib`, `android_main` via `android-activity`
   with the native-activity backend) and add it to the workspace members.
2. OpenXR bring-up:
   - loader init for Android;
   - instance with `XR_KHR_vulkan_enable2` and `XR_KHR_android_create_instance`;
   - system, session, a stage or local reference space;
   - swapchains at the recommended per-eye size;
   - frame loop: `xrWaitFrame` → `xrBeginFrame` → acquire, clear, release → `xrEndFrame` with a projection layer.
3. Session lifecycle: handle READY, SYNCHRONIZED, VISIBLE, FOCUSED, STOPPING and
   EXITING, Android pause and resume, and the Meta button (leave and come back).
4. Packaging: choose `cargo-ndk` + a minimal Gradle project (default), or `xbuild`.
   Log the decision. Bundle the Khronos OpenXR loader (`libopenxr_loader.so`,
   Apache-2.0) and record its source and version.
5. Manifest per `XR_DESIGN.md`, including the VR launcher category.
6. `scripts/xr/run.sh`: build, install (`adb install -r`), launch, and follow the filtered logcat.

**Acceptance criteria**
- Launches from the headset's app library. Both eyes show the clear color, and
  the color changes over time, so the frame loop is visibly alive.
- Holds the default display rate for 60 s with no dropped-frame warnings. Record
  the frame-timing source used (OVR Metrics Tool, or `predictedDisplayPeriod`).
- Leaving to the home screen and returning resumes cleanly. Quitting exits cleanly.
- Desktop CI subset still green (I1).

**Gate question:** is native packaging and launching solved? If yes, S2.

---

## S2: wgpu on OpenXR's Vulkan device (local + device, 1 day)

**Tasks**
1. Create the Vulkan instance and device through `xrCreateVulkanInstanceKHR` /
   `xrCreateVulkanDeviceKHR`. Include the extensions and features wgpu-hal needs.
2. Wrap them in wgpu 27 (`from_hal` / `create_device_from_hal`), and wrap each
   swapchain image as a `wgpu::Texture` (not owned by wgpu).
3. Replace S1's clear with a wgpu render pass per eye: a colored triangle at a
   fixed world position, using per-eye view and projection built from `xrLocateViews`.
4. Log the adapter limits.

**Acceptance criteria**
- The triangle appears in both eyes at a stable world position, with correct
  stereo (no double vision) while you move your head.
- Adapter limits recorded: `max_storage_buffers_per_shader_stage` ≥ 16,
  `max_bind_groups` ≥ 5, and `max_storage_buffer_binding_size` and
  `max_buffer_size` values. If either minimum fails, **stop and report**: it
  changes the core's pipeline layouts.
- No Vulkan validation errors in a debug build, if the validation layer is available on device.
- Display rate held for 60 s.

**Gate question:** is interop solved? This is the step most likely to fail. If
blocked at 1.5 days, report the exact failure and options, one of which is the pivot.

---

## S3: the seam (cloud task C2, or local, 1 day)

Make Fosfora's core usable as a library that compiles for Android without
desktop crates. The full self-contained brief is **C2 in `CLOUD_TASKS.md`**;
this is the summary. It runs on its own branch `xr-seam` off `main` and goes to
`main` by PR (Kevin approves). Then `xr` merges `main`. It is the only XR change
that touches shared structure, so it lands early for Kevin's v2 UI branch to
absorb.

**Tasks**
1. Add `[lib]` to `crates/fosfora-app` (`src/lib.rs`) exposing the shared modules.
   `main.rs` becomes a thin binary on top.
2. New default feature `desktop` that owns the desktop-only dependencies (winit,
   egui-winit, rfd, midir, dark-light, ureq, notify, nokhwa…). Existing features
   that need them (`webcam`, `depth`, `video`, `ndi`, `release`…) imply `desktop`.
   The `[[bin]]` gets `required-features = ["desktop"]`.
3. `cfg(feature = "desktop")` gates at the couplings listed in `PHASE0.md`.
   Prefer gating a whole module over sprinkling `cfg` inside functions.
4. Add seams the XR build will need, with no behavior change on desktop:
   - an override for `effect::loader::assets_dir()` and `paths::config_root()`
     (Android has no cwd, exe dir or XDG config dir);
   - `GpuContext`-free construction of whatever S4 needs. `headless/` already
     shows the pattern.

**Acceptance criteria**
- `cargo check -p fosfora-app --lib --no-default-features --target aarch64-linux-android` passes.
- Every clippy and test command in `ci.yml` passes unchanged (I1). `cargo deny check` passes.
- No behavior change on desktop. The diff is additive or `cfg`-only, apart from the `lib.rs` split.

---

## S4: one Fosfora effect in the headset (local + device, 0.5 day)

**Tasks**
1. Android assets: package the needed subset of `assets/` (shaders, the chosen
   effect, fonts if needed) into the APK. Extract it to internal storage on first
   run (or on version change), and point the S3 `assets_dir` override there.
   Point `config_root` at the app's internal data path.
2. Render one existing 2D effect through the core API into an offscreen
   Rgba16Float texture. Suggested: a light particle effect (Kevin picks). Drive it
   with synthetic audio features, reusing the golden-signal generator idea from
   `audio/mod.rs` tests if that's convenient.
3. Composite that texture onto a world-locked quad 1.5 m ahead and about 1.2 m wide.

**Acceptance criteria**
- The effect animates in both eyes. The display rate is held for 60 s; record
  frame time and GPU time.
- Record which effect was used and the texture size.

---

## S5: world-space particles (local + device, 1.5 days)

The core technical question for Fosfora VR. **Don't port an existing sim yet.**
Their `pos_life.z` is overloaded (see `PHASE0.md`).

**Tasks**
1. Write a small new 3D test sim for XR: curl-noise flow in a 2 m cube 1.5 m
   ahead, with size and speed modulated by the synthetic audio. Reuse the core
   particle buffers, alive list, raster and resolve stages where possible.
2. Add a world-space scatter path: project `xyz` with each eye's view-projection,
   and size the footprint by depth. Run the compute raster once per eye into that
   eye's target, then resolve into the swapchain layer. Keep the desktop 2D path
   untouched (I1).
3. Sweep particle counts from 100K to 2M in steps (for example 100K, 250K, 500K,
   1M, 2M) at 72 Hz and 90 Hz, and record GPU time and whether the display rate held.
4. Try one lower raster resolution (for example 0.75×) at the ceiling, and
   record the gain and how it looks.

**Acceptance criteria**
- Correct stereo depth. Particles sit in front of and behind the S4 quad correctly.
- `MEASURED.md` has the sweep table and the highest count that holds 72 Hz and 90 Hz
  with GPU time ≤ 11 ms.
- A one-paragraph proposal: how existing sims get a real `z` (layout change vs a
  new buffer) and which 2–3 lead effects port first. This feeds cloud task C3.

---

## S6: audio on device (local + device, 0.5 day)

**Tasks**
1. Mic input through cpal's Android (AAudio) backend into the existing ring
   buffer. During development, grant `RECORD_AUDIO` with
   `adb shell pm grant <package> android.permission.RECORD_AUDIO`. The in-app
   runtime-permission request comes later.
2. File playback: bundle one short, freely licensed test track with a known BPM.
   Play it through AAudio output, and tap the same samples into the analysis ring
   buffer so the visuals follow what you hear.
3. Drive the S5 sim from live `AudioFeatures`.

**Acceptance criteria**
- Tempo from the file source is within ±1 BPM of the track's known BPM after the
  lock-in period, and beat pulses are visibly on the beat.
- The mic source reacts to music played in the room.
- A rough audio-to-photon latency: film a click track and the headset lens with a
  phone slow-motion camera; ±10 ms precision is enough. Record it.

---

## S7: passthrough, hands, room (local + device, 1 day)

**Tasks**
1. Passthrough (`XR_FB_passthrough`) under the projection layer. Particles
   composite over the real room with correct alpha.
2. Hand tracking (`XR_EXT_hand_tracking`): 26 joints per hand, fed to the
   obstacle system as spheres. Add a pinch detector (thumb tip to index tip
   distance with hysteresis) that toggles something visible.
3. Room geometry: query scene planes (`XR_FB_scene` and the spatial-entity
   extensions), or the room mesh (`XR_META_spatial_entity_mesh`) if planes are
   harder. Feed the table or floor plane to the obstacle system. The room must be
   set up in the headset's Space Setup first.
4. Check glasses input in Meta XR Simulator on the M3 Ultra (look-and-pinch
   needs `XR_EXT_eye_gaze_interaction`, `XR_EXT_hand_interaction`,
   `XR_META_hand_tracking_microgestures`). **Best effort only:** first confirm the
   macOS simulator accepts a Vulkan-based app. If it doesn't, log it and move on.

**Acceptance criteria**
- In passthrough, particles bounce off a real table and off your hands. Pinch
  toggles an effect parameter.
- `MEASURED.md` records frame time with passthrough, hands and scene all on,
  compared with S5's number at the same particle count.

**Gate question and decision:** write `docs/xr/DECISION.md` (template below) and
stop for Kevin.

### DECISION.md template

```
# Native vs pivot — decision (Oct 4, 2026)
Recommendation: <continue native | pivot to Unity shell>
Gates: S1 <pass/fail> · S2 <…> · S3 <…> · S4 <…> · S5 <…> · S6 <…> · S7 <…>
Budgets: particle ceiling @72 Hz <n>, @90 Hz <n>; frame time with MR on <ms>
What native cost us this week: <hours on interop/packaging/input glue>
Biggest remaining native risk: <one line> — mitigation: <one line>
Next 2 weeks if native: <3–5 bullets>
```

---

## Risks

| Risk | Likelihood | Effect | Mitigation |
|---|---|---|---|
| wgpu 27 interop API differs from the reference example | High | S2 slips | Read the wgpu-hal 27 source in `~/.cargo/registry`; the example is only a map |
| Swapchain image layout wrong at release | Medium | Garbage or validation errors | Make the last use of each swapchain texture per frame a render-pass color write |
| Adreno limits below 16 storage buffers or 5 bind groups | Low–medium | Core pipeline layouts change | Checked in S2; stop and report |
| Android loader or manifest detail wrong | Medium | App won't launch or has no permissions | Follow Meta's native manifest docs; `VERIFY` items in `XR_DESIGN.md` |
| Runtime permissions from native code | Medium | Mic and scene blocked in the product build | `adb pm grant` for the spike; JNI request later |
| Particle ceiling low with MR on | Medium | Visual ambition cut | Design constraint, not a pivot; lower raster resolution |
| The macOS simulator rejects Vulkan apps | Medium | No glasses testing until hardware | Glasses input design stays spec-driven; Quest 3 hand path first |
| Cloud sessions can't reach `static.rust-lang.org` | Medium | Pinned toolchain unavailable in the cloud | Environment network setting; else CI does pinned builds |

## Open questions for Kevin

- [ ] Which effect for S4 (a 2D effect on a quad)?
- [ ] OK to add small core seams (`assets_dir` / `config_root` overrides) on `xr` and merge them to `main` early?
- [ ] Target display rate for the product: 72 Hz (more budget) or 90 Hz (smoother)?
- [ ] Test track(s) for S6: an original, or a CC0 source?
