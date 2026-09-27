# Measured on device

Numbers only, newest step at the bottom of each table. Each row names its
commit. A claim in a report that isn't here doesn't count (invariant I4).

## Environment (S0)

| Item | Value |
|---|---|
| Dev host OS / arch | Debian 13 (trixie), Linux 6.12, x86_64, 32 threads |
| Rust toolchain (in repo) | 1.97.0 + `aarch64-linux-android` target |
| Android NDK | 27.0.12077973 (`~/Android/Sdk/ndk`); SDK platforms to android-36.1, build-tools to 36.1.0 |
| JDK | OpenJDK 21.0.11 |
| cargo-ndk | 4.1.2 |
| Quest 3 Horizon OS version | v207 (`207.0.0.297.1234`), Android 14 / SDK 34, `ro.ovr.os.api.version` 160 |
| OpenXR loader (source, version) | Khronos `org.khronos.openxr:openxr_loader_for_android` 1.1.63 from Maven Central (Apache-2.0), packaged by Gradle from the AAR (`jni/arm64-v8a/libopenxr_loader.so`, AAR sha256 `622419d2…ded73`) |
| Desktop baseline: clippy / test wall time | ci.yml lint+test matrix all green on `3f7cce0` (warm cache, 32 threads): fmt 1s, clippy ×8 sets 110s, test ×7 sets 368s (default: 1022 passed, 0 failed, 111 ignored), `cargo deny` ok; total 479s |

### S1 notes (commit `c45a9c8`, Quest 3 v207)

- Runtime `Oculus 207.297.0`, system `Meta Quest 3`, 84 extensions (list in the
  app's startup log). Blend modes offered: `OPAQUE`, `ALPHA_BLEND`.
- Vulkan through the runtime: `Adreno (TM) 740`, API 1.3.295, runtime requires
  1.0–1.2. Swapchain: `R8G8B8A8_SRGB`, 3 images per eye.
- Startup to first FOCUSED frame: ~250 ms after `android_main`.
- Lifecycle: Guardian's tracking-lost dialog and the BACK-key exit dialog take
  the session FOCUSED → VISIBLE (frames continue with empty `xrEndFrame` while
  `should_render` is false) and, for Guardian, on to STOPPING → IDLE with
  `xrEndSession`; the same session resumed READY → FOCUSED afterwards. HOME
  destroys the activity: Destroy → `android_main` returns ("exited cleanly"),
  and a relaunch from the library re-initializes in the same process. Choosing
  Exit in the system dialog kills the process (runtime destroys the client).
- Headless test setup: headset unworn, `prox_close` broadcast + `debug.oculus.guardian_pause 1` (board #3215).

## Adapter limits (S2)

| Limit | Value | Required |
|---|---|---|
| max_storage_buffers_per_shader_stage | 16777216 | ≥ 16 |
| max_bind_groups | 7 | ≥ 5 |
| max_storage_buffer_binding_size | 134217728 (128 MiB) | |
| max_buffer_size | 2147483647 | |
| Swapchain format / per-eye size | `R8G8B8A8_SRGB` (wgpu `Rgba8UnormSrgb`), 1680x1760, 3 images per eye | |

Commit `32b4628`, Quest 3 v207, wgpu 27.0.1 / wgpu-hal 27.0.4 over the
runtime-created Vulkan 1.2 device, Adreno 740 (driver build 810c66ceb7,
06/24/26). Also: max_texture_dimension_2d 16384, max_compute_workgroup_storage_size
32768, max_compute_invocations_per_workgroup 1024,
max_compute_workgroups_per_dimension 65535, max_uniform_buffer_binding_size 65536,
max_push_constant_size 256. Device extensions wgpu-hal enabled:
`VK_KHR_swapchain`, `VK_EXT_robustness2`, `VK_KHR_external_memory_fd`
(`VK_EXT_memory_budget` absent, warning only). No Vulkan validation layer on
the device; wgpu's own validation reported nothing over the run.

## Frame timing

| Step | Commit | Content | Display Hz | Held 60 s? | CPU ms | GPU ms | Tool |
|---|---|---|---|---|---|---|---|
| S1 | `c45a9c8` | clear color, both eyes, 1680x1760 sRGB per eye | 72 (runtime default for a new app; 72/80/90/120 offered) | Yes: 74 s, 5362 frames, 1 long frame at session start, max wait-to-wait 17.3 ms after the first second, 0 `should_render=false` | 0.25 (runtime `App=`) | n/a (clear only; runtime `CPU&GPU=0.79`) | in-app `predictedDisplayPeriod` counters (logcat `fosfora_xr`, 1 s windows) + runtime `VrApi` line (`FPS=72/72 Stale=0 Tear=0`) |
| S2 | `32b4628` | stereo triangle through wgpu, one render pass per eye | 72 | Yes: 73 s, 5287 frames, 0 long frames, max wait-to-wait 15.9 ms after the first second, 0 `should_render=false` | 0.26 (runtime `App=`) | n/a (runtime `CPU&GPU=1.19`) | same as S1 |
| S4 | | 2D effect on quad | | | | | |

### S2 notes (commit `32b4628`)

- Screencap: the triangle appears in both eyes with opposite-sign parallax
  (centroid 62% across the left-eye image, 39% across the right-eye image),
  consistent with an object 1.5 m ahead.
- World-locking: verified by Kevin wearing the headset (Sep 27): the triangle
  stays put while moving and turning. (Monocular check; Kevin sees with one eye.)
- Convergence, numeric (commit `eefdc33`, tracked, flags 7): runtime
  IPD 65.0 mm, triangle center 1.52 m from the eye midpoint → expected
  disparity 2.44°; from our per-eye matrices the center sits at +0.62° in the
  left eye and −1.86° in the right eye, disparity 2.48°, convergent. Left-eye
  fov [−54°, +40°, +44°, −55°], so a straight-ahead object sits at NDC x ≈ +0.25
  in the left image, which is what the screencap parallax showed.

## Particle sweep (S5)

| Particles | Raster scale | Hz target | GPU ms | Held? | Notes |
|---|---|---|---|---|---|
| 100K | 1.0 | 72 | | | |
| 250K | 1.0 | 72 | | | |
| 500K | 1.0 | 72 | | | |
| 1M | 1.0 | 72 | | | |
| 2M | 1.0 | 72 | | | |

Ceiling at 72 Hz: — · at 90 Hz: — · at 0.75× raster: —

## Audio (S6)

| Item | Value |
|---|---|
| Test track, known BPM | |
| Detected BPM after lock | |
| Lock-in time (s) | |
| Audio-to-photon latency (ms, method) | |
| Mic source reacts? | |

## Mixed reality (S7)

| Configuration | Particles | GPU ms | Held 72 Hz? |
|---|---|---|---|
| Passthrough off (S5 baseline) | | | |
| Passthrough on | | | |
| + hands as obstacles | | | |
| + scene planes / mesh | | | |
