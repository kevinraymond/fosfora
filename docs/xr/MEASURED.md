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
| S1 | `c45a9c8` | clear color, both eyes, 1680x1760 sRGB per eye | 72 (runtime default for a new app; 72/80/90/120 offered) | Yes: 74 s, 5362 frames, 1 long frame at session start, max wait-to-wait 17.3 ms after the first second, 0 `should_render=false` | not measured (S4 adds it) | 0.25 (runtime `App=`, the app's GPU time) | in-app `predictedDisplayPeriod` counters (logcat `fosfora_xr`, 1 s windows) + runtime `VrApi` line (`FPS=72/72 Stale=0 Tear=0`) |
| S2 | `32b4628` | stereo triangle through wgpu, one render pass per eye | 72 | Yes: 73 s, 5287 frames, 0 long frames, max wait-to-wait 15.9 ms after the first second, 0 `should_render=false` | not measured | 0.26 (runtime `App=`) | same as S1 |
| S4 | `fad08e4` | **Flux as shipped** (compute raster, High, ~1.3–2M alive) at 1280x720 on a 1.2 x 0.675 m quad, both eyes, bright and animating | 72 | **No: 6 fps**, every frame long, runtime `Stale=72` | 168 avg (in-app; waiting on the GPU) | 154–170 (runtime `App=`) | same as S1, plus `particles alive` log |
| S4 | `fad08e4` | `Flux XR Vertex` (billboard path, Low, 87k alive): holds rate but the image is almost black (Flux's look comes from the raster resolve + velocity feedback), so a renderer-cost datum, not a Flux result | 72 | Yes: 78 s, 5511 frames, 1 long frame, max wait-to-wait 14.8 ms, 0 stale | 2.19 avg / 3.49 max | 4.60 median / 5.56 max | same |
| S5 | `47fb3d0` | S5 test sim, 750K world-space sprites (vertex pulling, 3-vertex), both eyes at 1680x1760 with depth, static quad | 72 | Yes: 63 s, 4555 frames, 0 long frames, 13 stale | 0.67 avg | 10.1 median / 12.9 max (runtime `App=`) | same, via `scripts/xr/sweep.sh` |
| S5 | `47fb3d0` | same, 750K | 90 | Yes: 63 s, 5678 frames, 0 long frames, 0 stale | 0.59 avg | 8.1 median / 8.8 max | same |
| S7 | `031956b` | mixed reality: passthrough layer + hand joints + scene anchors + floor as obstacles, 100K unoccluded sprites, near cull, no quad | 72 | Yes: 30 s, 2029 frames, 0 long, 16 stale | 0.77 avg | 5.4 median / 6.6 max | `sweep.sh --mode mr` |
| S7 | `031956b` | same, 250K | 72 | No: 61.5 frames/s, 167 long, 431 stale | 1.32 avg | 13.5 median / 18.4 max | same |
| S7 | `031956b` | same, 250K, passthrough off | 72 | Yes: 30 s, 2026 frames, 2 long, 9 stale | 0.62 avg | 9.4 median / 10.1 max | same |
| C3b | `8e20998` + eye-pass hook | `Flux XR World` (`mode world`): Flux's sim in a 2 m volume around the wearer, drawn through `prepare_world` / `draw_world` inside the eye pass, over the `mr` setup (passthrough, hands, room, floor, occluders, primer); unworn, no room anchors in this run | 72 | 250K: Yes (2032 frames, 0 long, 0 stale) · 300K: Yes (1961 frames, 0 long, 0 stale) · 500K: No (69.4 frames/s, 9 long, 104 stale) | 1.5 avg | 250K 8.4 / 9.4 · 300K 9.2 / 9.7 · 500K 11.7 / 13.3 | `sweep.sh --mode world --counts "250000 300000 500000" --hz 72`; the split below |

### S4 notes (commit `fad08e4`, Quest 3 v207)

Effect: **Flux** (Kevin's pick, board #3202). Offscreen texture 1280x720
`Rgba8UnormSrgb` (the headless renderer's capture format), copied each frame
onto a 1.2 x 0.675 m quad at (0, 1.5, -1.5) m in STAGE space. Synthetic
120 BPM features (`headless::loop_driver::synth_features` plus pumping band
energies). Assets: 209 files, 0.9 MB, unpacked in 21 ms; 58 effects load.
Startup to first frame ~300 ms.

**Gate result: the effect runs and animates in both eyes (Flux on the quad
reads mean RGB 142/106/64, max 247, in the screencap) but the display rate is
not held: 6 fps.** Sweep
(`setprop debug.fosfora.{effect,quality,scene,emit}`; GPU ms = runtime `App=`):

| Variant | Quality (max_count) | Offscreen | Alive | fps | GPU ms |
|---|---|---|---|---|---|
| Flux (compute raster) | High (2M) | 1280x720 | ~2M | 6 | 154–170 |
| Flux | Low (500k) | 1280x720 | ~500k | 17 | 51 |
| Flux | Low | 640x360 | ~500k | 17 | 61 |
| Flux | Medium (1M) | 640x360 | ~1M | 10 | 93 |
| Flux, emit 6000/s | Low | 1280x720 | 84k | 24 | 30–38 |
| Flux, emit 12500/s | Low | 1280x720 | 172k | 22 | 41 |
| Flux, emit 6000/s | High | 1280x720 | 84k | 18 | 50 |
| Flux XR NoFeedback (background pass only) | Low | 1280x720 | 84k | 25 | 46 |
| Flux XR NoFields (no flow/velocity field) | Low | 1280x720 | 84k | 29 | 30 |
| Flux XR NoPost (no bloom/vignette) | Low | 1280x720 | 84k | 24 | 37 |
| **Flux XR Vertex (billboard path)** | Low | 1280x720 | 87k | **72** | **4.6** |
| Flux XR Vertex | Low | 1280x720 | 346k | 72 | 4.6 |
| Flux XR Vertex | High | 1280x720 | 822k | 72 | 9.8 |
| Flux XR Vertex | High | 1280x720 | 1.7M | 72 | 9.4 |

Reading: the compute rasterizer is a ~30 ms fixed GPU cost on the Adreno 740
at this size, independent of alive count, offscreen resolution, feedback
passes and post. The billboard path holds 72 Hz to 1.7M particles and its GPU
time tracks `max_count` (buffer/dispatch size), not the alive count. But the
billboard variant of Flux renders almost nothing (max pixel 98 at 1.7M alive):
Flux's image is the raster resolve plus the `@particles.velocity` feedback,
which the billboard path does not produce. So the billboard numbers bound the
renderer, not the effect. The XR variants are hidden effects in
`assets/xr/effects/`, staged into the APK only.

Pipeline check: Aurora on the quad reads mean RGB 66/93/69 (max 249), so
capture → copy → quad sampling is correct in both eyes. Convergence log
(`stereo:`) unchanged from S2, target now the quad center.

### S2 notes (commit `32b4628`)

- Screencap: the triangle appears in both eyes with opposite-sign parallax
  (centroid 62% across the left-eye image, 39% across the right-eye image),
  consistent with an object 1.5 m ahead.
- World-locking: verified by Kevin wearing the headset (Sep 27): the triangle
  stays put while moving and turning. (A monocular check; stereo
  convergence is verified numerically below.)
- Convergence, numeric (commit `eefdc33`, tracked, flags 7): runtime
  IPD 65.0 mm, triangle center 1.52 m from the eye midpoint → expected
  disparity 2.44°; from our per-eye matrices the center sits at +0.62° in the
  left eye and −1.86° in the right eye, disparity 2.48°, convergent. Left-eye
  fov [−54°, +40°, +44°, −55°], so a straight-ahead object sits at NDC x ≈ +0.25
  in the left image, which is what the screencap parallax showed.

## Particle sweep (S5)

Setup (commit `47fb3d0`, Quest 3 v207, board #3192): `debug.fosfora.mode
particles`, the S5 test sim (`crates/fosfora-xr/src/particles3d*`: curl-noise
flow in a 2 m cube centered on the S4 quad at (0, 1.5, −1.5) m, every slot
alive, 32 B per particle), drawn once per eye at the recommended 1680x1760
per-eye swapchain with depth against a static test-card quad. Sprite radius
4 mm × audio (rms). Rates set through `XR_FB_display_refresh_rate` per run.
GPU ms = runtime `App=` median over 1 s windows after a 5 s warmup, from the
app's own `VrApi` line; frames/s and long frames from the in-app counters;
"Held" = frames/s at the target and 0 stale frames. Runs are 30 s (20 s for
diagnostics) unless marked 60 s. `scripts/xr/sweep.sh` produced every row.

### Instanced quads (one instance per sprite, the shape of the core's `draw_indirect`)

| Particles | Hz target | GPU ms (med / max) | frames/s | Held? |
|---|---|---|---|---|
| 100K | 72 | 4.8 / 5.0 | 72.0 | Yes |
| 250K | 72 | 9.4 / 9.9 | 72.0 | Yes |
| 300K | 72 | 9.7 / 10.2 | 72.0 | Yes |
| 350K | 72 | 10.7 / 11.1 | 72.0 | Yes |
| 400K | 72 | 11.5 / 12.1 | 72.0 | Yes (over the 11 ms budget) |
| 450K | 72 | 12.6 / 14.0 | 68.7 | No |
| 500K | 72 | 13.9 / 15.8 | 61.8 | No |
| 1M | 72 | 28.3 / 34.4 | 31.4 | No |
| 2M | 72 | 164 / 182 | 5.9 | No (see note) |
| 100K | 90 | 4.7 / 5.0 | 90.0 | Yes |
| 250K | 90 | 8.1 / 8.5 | 90.0 | Yes |
| 300K | 90 | 8.7 / 9.0 | 90.0 | Yes |
| 350K | 90 | 10.1 / 10.8 | 84.8 | No |
| 400K | 90 | 11.4 / 12.5 | 74.3 | No |
| 500K | 90 | 14.4 / 15.9 | 59.8 | No |
| 1M | 90 | 28.2 / 33.7 | 30.0 | No |
| 2M | 90 | 166 / 183 | 5.8 | No |

Instanced ceiling with GPU ≤ 11 ms: **350K at 72 Hz, 300K at 90 Hz.**

### What the cost is (72 Hz, 20 s runs, GPU ms median)

| Variant | 250K | 500K | 1M |
|---|---|---|---|
| Instanced quads (baseline) | 9.4 | 13.9 | 28.3 |
| Sim frozen after a 2 s warmup (draw only) | 9.0 | 13.6 | 26.2 |
| Swapchain 0.75× (1260x1320 per eye) | 9.2 | 13.6 | 26.7 |
| Sprite radius × 0.5 | — | 14.0 | 27.9 |
| Sprite radius × 0.25 | — | 12.0 | — |
| 3-vertex sprites, instanced | 9.1 | 13.6 | 26.9 |
| **Vertex pulling** (one non-instanced draw, quads) | **6.3** | **9.6** | **17.0** |
| **Vertex pulling, 3-vertex sprites** | — | **7.7** | **10.8** |

Reading: the sim is under 2 ms even at 1M; raster resolution and sprite
size barely matter (not fill-bound); halving the vertex count of an
instanced draw does nothing. The tiler's per-instance cost dominates the
instanced form. One draw of `verts × count` vertices where the vertex index
picks the particle (vertex pulling) is 1.5× cheaper at every count, and with
pulling the vertex count matters, so 3-vertex sprites take another 1.25–1.6×.
Both eyes render in separate passes; multiview was not tried.

### Vertex pulling, 3-vertex sprites (the S5 default from `47fb3d0`)

| Particles | Hz target | Run | GPU ms (med / max) | frames/s | Long frames | Stale | Held? |
|---|---|---|---|---|---|---|---|
| 500K | 72 | 20 s | 7.7 / 8.4 | 72.0 | 0 | 0 | Yes |
| 750K | 72 | 60 s | 10.1 / 12.9 | 72.0 | 0 of 4555 | 13 | Yes (13 stale frames in 63 s) |
| 1M | 72 | 20 s | 10.8 / 11.5 | 72.0 | 0 | 0 | Yes |
| 1M | 72 | 60 s | 11.2 / 16.3 | 72.0 | 13 of 4400 | 103 | Marginal: rate held, but long frames and stale frames over 60 s, median over budget |
| 1.25M | 72 | 20 s | 13.8 / 15.6 | 59.9 | 3 | 174 | No |
| 1.5M | 72 | 20 s | 15.6 / 19.1 | 52.4 | 51 | 299 | No |
| 2M | 72 | 20 s | 21.7 / 24.8 | 40.0 | 527 | 452 | No |
| 500K | 90 | 20 s | 7.6 / 8.1 | 90.0 | 0 | 0 | Yes |
| 500K | 90 | 60 s | 7.4 / 8.7 | 90.0 | 1 of 5697 | 4 | Yes |
| 750K | 90 | 20 s | 8.4 / 8.9 | 90.0 | 0 | 0 | Yes |
| 750K | 90 | 60 s | 8.1 / 8.8 | 90.0 | 0 of 5678 | 0 | Yes |
| 1M | 90 | 20 s | 11.3 / 12.9 | 75.1 | 2 | 203 | No |

**Ceiling with GPU ≤ 11 ms: 750K at 72 Hz (10.1 ms) and 750K at 90 Hz
(8.1 ms).** 1M holds 72 Hz in a 20 s run at 10.8 ms but not cleanly over
60 s. The GPU runs faster per frame at 90 Hz than at 72 Hz for the same
content (750K: 8.1 vs 10.1 ms; 500K: 7.4 vs 7.7), presumably the runtime's
dynamic GPU clock (`VrApi` shows GPU level 4 at 640 MHz vs 492 MHz), so the
90 Hz budget of 11.1 ms is not the handicap it looks like.

### 0.75× raster

Swapchain at 0.75× the recommended size, instanced quads: 250K 9.2 ms, 500K
13.6 ms, 1M 26.7 ms at 72 Hz (vs 9.4 / 13.9 / 28.3), 500K at 90 Hz 13.5 ms
(vs 14.4). A 2–5 % gain; the compositor's upscale is visible as softer sprite
edges in the screencap. Not worth the resolution on this path.

### Notes

- 2M instanced: 164 ms is not the 2× of 1M (28 ms) but 6×; not investigated
  (the count is far past the ceiling). The 64 MB particle buffer is within
  the 128 MiB storage binding limit.
- The runtime changes the display rate on its own when the app requests none
  (72 → 90 → 72 within one launch), so every row above requested its rate.
- Stereo depth evidence (`depth probe` log, tracked, IPD 64.9 mm, eye midpoint
  1.53 m from the quad): a point 0.5 m nearer than the quad center projects to
  depth 0.9506 in both eyes vs the quad's 0.9672 and the 0.5 m farther point's
  0.9755, with NDC x disparities 0.543 (near) > 0.524 (quad) > 0.514 (far).
  Nearer = smaller depth value and larger disparity, in both eyes, as required.
  The screencap at 500K shows the quad as a sparser rectangle in both eyes:
  sprites behind it fail the depth test, sprites in front draw over it.
- CPU per frame 0.6–0.8 ms at every count (the draw is one call per eye).
- **15-minute soak at 750K, 90 Hz** (commit `0dec960`, `scripts/xr/soak.sh`,
  headset unworn on AC power, samples every 15 s): the rate held for the
  whole run (90–91 fps in every sample). Stale frames only in the first
  3 minutes (four samples with 3–14 stale, 35 of the run's 52 long frames
  in the first 4 minutes, 0 stale after that); GPU 7.7–8.5 ms throughout;
  GPU clock alternating 640 / 599 MHz from minute 4 on (no drop in frame
  rate); runtime power level 0 and thermal status 0 throughout (no
  throttling). Battery temperature 32.0 → 43.0 °C, still rising about
  0.7 °C/min at the end, so a longer session is untested; charging over USB
  adds heat, so a worn, unplugged run would differ in both directions.
  The Android thermal service's per-zone values (SoC, board, surface) never
  refreshed over adb on v207, so the battery sensor is the only live one.

### Proposal: how existing sims get a real `z` (feeds C3)

Both "reserved" slots are taken: `pos_life.z` holds the initial size in Flux,
Murmur, Cymatics, Tesla, Cascade, Array, Genesis, Accretion (and Tide's water
height, Mycelium's generation); `vel_size.z` holds size, mass, species or
band in Accretion, Tide, Genesis, Ascend, Cleave, Panorama, Cascade, Vessel,
Array. So no global reinterpretation is safe, and a new SoA component would
touch every compute layout. **Recommendation: a per-effect layout
convention, not a core layout change.** An XR variant of a sim
(`assets/xr/shaders/<name>_xr_sim.wgsl`, hidden `.pfx` as in S4) writes
`pos_life.xyz` = position in meters relative to the effect anchor and
`vel_size.xyz` = velocity, and moves whatever it kept in the `.z` slots to
`flags.zw` or the existing per-particle `aux` buffer (unused by Flux, Murmur,
Tide, Cascade, Phosphor). Desktop sims and the 2D path are untouched (I1).
What core must add for the port, behind a clean API (the stop-and-ask for
C3): (1) a world-space render pipeline variant of `particle_render.wgsl`
taking a per-eye `view` + `proj` uniform, billboarding in view space and
using **vertex pulling** (`alive_indices[vertex_index / 3]`) instead of
`draw_indirect` instances; (2) a `ParticleSystem::render_eye(encoder, color,
depth, view, proj)` entry (or getters for the current SoA buffers, alive list
and counters) so `fosfora-xr` can drive it once per eye after
`SceneRenderer::step`; (3) the flow field sampled in 3D (the texture is
already `texture_3d`; `sample_flow_field` maps 2D clip space today). Lead
effects to port first: **Flux** (curl flow, 3D is its natural form, the
flow-field texture is already volumetric), **Murmur** (a murmuration in the
room is the strongest VR image; boids generalize to 3D directly), and
**Tide** (already tracks a height `z` for contact and is billboard-rendered;
a waterfall onto a real table in S7). Budget for a port at 72 Hz: about 1M
sprites per effect with pulling + 3-vertex sprites, 500K at 90 Hz, before
the effect's own sim and feedback passes.

## Audio (S6)

Setup (commit `01fde39`, Quest 3 v207, board #3193): the core's
`AudioSystem` and `HopAnalyzer` unchanged, driven on device two ways.
`debug.fosfora.audio file`: the bundled test track looped through cpal's
AAudio output (44.1 kHz stereo F32, the device default), every played frame
also pushed into a ring that the core's analysis thread reads
(`AudioSystem::from_ring`, added for this). `debug.fosfora.audio mic`: the
core's own cpal capture (`AudioSystem::new`). `aaudio`: a raw AAudio input
stream with a chosen input preset. Features drive the S5 sim (bass → flow
speed, rms and the beat pulse → sprite size) or the S4 quad's effect.

| Item | Value |
|---|---|
| Test track, known BPM | "Ember Glow" by oglsdl, CC0, OpenGameArt; seconds 30–75 bundled as `assets/xr/audio/ember_glow_excerpt.ogg` (45 s, 48 kHz stereo Vorbis, 670 KB; source, checksum and license in `assets/xr/audio/LICENSE.md`). Stated 140 BPM; measured 140.0 ±0.25 BPM over the whole track by onset autocorrelation at 0.25 BPM lag resolution, in every 30 s window |
| Decoded on device by | the core's symphonia path (`fosfora_app::decode`, new `decode` feature, no desktop crates); 45.0 s at 48 kHz, resampled to the 44.1 kHz output by the playback callback |
| Detected BPM after lock (file tap) | 139.7–140.0 (`raw_bpm`, two runs: 139.7–139.9 and 139.7–140.0), steady for the rest of each 70 s run |
| Lock-in time (s) | first published value at 13.9 s, at 140 by ~19 s (the published BPM ramps through the 0.5 s / 1 s smoother); the desktop `--analyze` on the same file: anchor earned at 3.4 s of audio, 140.24 BPM |
| Beat pulses on the beat? (file tap) | 98 beats after lock in the second run: phase concentration R 0.80 against the 140 BPM grid, circular jitter 46 ms, inter-beat intervals 5–95 % 395–480 ms (median 412). First run (same content from a raw file): R 0.16, 131 ms, 395–646 ms. Desktop `--analyze` on the excerpt: R 0.46, 85 ms, 405–444 ms. Beat times are the playhead position at the render frame that saw the pulse counter change (±14 ms) |
| Mic source reacts? | Yes: with music in the room (Kevin, headset worn) the core's capture reads peaks 0.07–0.08 with bass/onset following, and an XR-owned stream at I16 or F32, 44.1 or 48 kHz likewise. Input preset comparison (`debug.fosfora.audio loop`: the clip on the headset speakers, analysis on the AAudio microphones): **not measurable unworn**. With the headset off, even with the proximity fake and playback confirmed running at media volume 8/15 to the speaker, `VoiceRecognition`, `Unprocessed` and `Generic` all read a peak of 0.0000 for 60 s each. The Quest mutes the microphones when the headset is not worn, below Android (the recording client is "not silenced", mic mute is off, the app op is allowed). **Worn, the loopback still reads zero** for all three presets (one 0.008 peak in 3 minutes): the headset cancels its own speaker output from the microphone input, so a loopback cannot measure presets; only an external source can. **Worn, room music from desk speakers with a subwoofer, normal-loud level, head straight (Kevin, Sep 27):** cpal I16 48 kHz, raw AAudio `VoiceRecognition` (default mode), `Unprocessed` (default mode) and `Unprocessed` (low latency), 35 s each, all read peaks of 0.0005–0.02, `bass` 0.00 in every sample, `rms`/`onset` twitching on the normalizer; no tempo lock in 35 s. Earlier close-range captures (voice, music near the visor) reached 0.07–0.29. Tipping the head back so the visor's underside faces the speakers made the visuals react (Kevin). Reading: the Quest 3 mic pipeline beamforms toward the wearer's mouth and high-passes for speech, and none of the AAudio knobs escape it, so room music arrives quiet and without its kick; on this device the microphones are a weak source for music and the playback tap is the reliable one |
| **External USB microphone** (Quest experimental setting "external mic", Blue Microphones USB mic on a USB-A-to-C adapter in the headset's port; adb over Wi-Fi) | Works through the core's default capture with no code change (Android routes the default input to the USB device: 44.1 kHz stereo F32). Room music from desk speakers, headset worn: peaks 0.33–0.52, `bass` 0.2–0.95, tempo lock within ~20 s. The 140 BPM test track played from the desk speakers: **139.6–139.8 BPM** through the room (gate ±1), bass 0.4–0.8 throughout. Built-in array under the same conditions: peaks 0.0005–0.02, bass 0 |
| Audio-to-photon latency (ms, method) | **Fixed Sep 28 (`xr-audio-latency`, "Playback latency" below): flash on the sound to +6–12 ms, within the phone's 17 ms frame.** As measured in S6: **Visual leads the sound by about 115 ms on the playback path** (filmed, commit `74eb646`+: `debug.fosfora.file click` plays a click track through the headset speakers, `debug.fosfora.flash 1` turns the view white for two frames on each detected beat; phone video with sound, `scripts/xr/latency.py` fits both event grids). 120 BPM, 60 fps: flash minus click 384 ms mod 500 = −116 ms, 46 flashes, circular jitter 18 ms. 100 BPM, 30 fps: 487 mod 600 = −113 ms, 30 flashes, jitter 24 ms. The two grids agree, which resolves the modulo. The app's own log puts the pulse 42–47 ms after the click samples are handed to AAudio (162–164 beats, R 0.96–0.97), so the display chain is short and the sound is late: implied AAudio output latency about 190 ms (default performance mode, cpal). Mic-path latency not filmed (it needs the external mic's output routing fixed first, board #3251); expected positive, capture latency + the same 42–47 ms + display |

### S6 notes

- **Silent capture, explained.** The first mic runs read a peak of 1e-4
  (digital silence) whether the headset was worn or not. The room was quiet
  and AAudio's default input preset (`VOICE_RECOGNITION`, which cpal cannot
  change) runs the Quest's voice processing, which floors a quiet room to
  zero. With music the same stream alternates between real audio and hard
  zeros every few seconds: the voice preset gates music as noise. The XR
  crate therefore opens the microphones itself through the `ndk` crate's
  AAudio binding with a selectable preset (`debug.fosfora.micpreset`).
- **App not responding.** The main loop never drained Android input events;
  the first time a wearer generated input the queue backed up and Android
  showed "Fosfora VR is not responding". Fixed by draining
  `input_events_iter` every loop (nothing consumes them yet; hands come
  through OpenXR in S7).
- **Core additions (board #3244, on `xr`, additive, desktop unchanged):**
  `AudioSystem::from_ring(ring, sample_rate, callback_count, …)` runs the
  existing analysis thread over a caller-fed ring (auto-reconnect off, a
  new `CaptureBackend::External`), and a `decode` feature (`symphonia`,
  implied by `analyze`) mounts `analyze/decode.rs` at `fosfora_app::decode`
  so the XR build decodes without desktop crates.
- **Sound is late, not the visuals.** On the playback path the tap feeds
  the analysis about 190 ms before the speaker emits the same sample, so
  beats flash 115 ms early. Fixed on `xr-audio-latency` (Sep 28, board
  #3253, filmed: −115 → +41 ms with the low-latency output alone, then a
  buffer sized to the chain), see "Playback latency" below. A mic-driven show has the opposite
  sign and needs the visuals as early as possible.
- **External USB mic: the fix.** With the Quest's experimental external-mic
  setting and a USB audio class mic in the port, the built-in pipeline is
  bypassed: full-range, unprocessed, tempo locks through the room. The
  port is shared with adb, so device work runs over Wi-Fi
  (`adb tcpip 5555` on the cable once, then `adb connect <ip>:5555`). A USB
  audio interface fed from the mixer would be the direct-line version of
  the same path (untested).
- **Built-in microphones are marginal on Quest 3.** Level 10×
  below close-range speech, `bass` always 0 (speech high-pass), pickup
  strongly directional (the visor's underside). Preset, API path and
  performance mode make no difference. Product consequence: drive the
  visuals from what the headset plays (the tap) or from a paired source;
  treat the mic as ambience, not as the beat source.
- **Unworn = muted microphones.** The `prox_close` fake keeps the app
  running but not the microphones: every unworn run reads a peak of 1e-4 or
  0, every worn run reads real audio. Mic measurements need a wearer;
  file-tap measurements do not.
- `RECORD_AUDIO` is in the manifest and granted with `adb shell pm grant`;
  the in-app runtime permission request is still to do.

### Playback latency (`xr-audio-latency`, Sep 28, board #3253)

The playback output moved from cpal to ndk's raw AAudio binding, which
exposes the stream: its performance mode, `AAudioStream_getTimestamp` and
`frames_written`. Measured with the 120 BPM click track, 60 s runs, the
400K Flux world sim running, from the app's own log (the beat pulse's
playhead position against the click grid; "offset" = pulse time minus the
moment the click's frames were handed to AAudio):

| Output stream | Burst / buffer (frames) | Handed → speaker (AAudio timestamps) | Tap delay | Beat pulse offset (median, 5–95 %) | Underruns |
|---|---|---|---|---|---|
| cpal, legacy (S6, `tapdelay 0`) | 1922 / 3844 | not exposed | 0 | 40 ms, 20–80 | 0 |
| AAudio legacy (`playperf none`) | 1922 / 3844 | 137–215 ms raw, run mean 173 | mean − 76 ms, 2 steps | 161 ms, 140–200 | 0 |
| AAudio low latency, AAudio's buffer (`playbuf 0`) | 192 / 1536 | 36 ms, constant | 0 | 36 ms, 20–36 | 1 at start |
| **AAudio low latency, 84 ms buffer (default)** | **192 / 4032** | **88 ms, constant** | **11 ms** | **44 ms, 28–60** | **1 at start** |

- **The film (Kevin, Sep 28, phone at 60 fps, low latency with AAudio's
  1536-frame buffer, no tap delay): the flash trails the click by 41 ms**
  (peak-frame offsets cluster at 24 / 41 / 58 ms, the phone's frame
  quantum; 55 of 70 flashes in the cluster, median 41; the grid fit says
  52 because bright ember frames over passthrough pass the flash threshold
  on some beats and 9 clicks have no frame over it). Against −115 ms in
  S6. With the pulse at 32 ms and the speaker at 36, that makes the
  display chain after the pulse ~44 ms (two 72 Hz frames plus the phone's
  quantum), so the whole analysis-and-display chain is ~76 ms.
- **Low latency is the fix, plus a buffer sized to the chain.** The MMAP
  stream reports its latency exactly (36.0 ms on every read). Its own
  buffer is shorter than the 76 ms chain, which leaves the flash 41 ms
  behind the sound with no delay to trade, so the default asks for 84 ms
  of buffer (`set_buffer_size_in_frames`; the stream's capacity is
  requested at twice that): the speaker is then 88 ms behind the
  handover and the tap delay lines the chain up (`88 − 76 = 11 ms`). The
  default-mode stream's timestamps saw-tooth by ~80 ms over ~4 s (the
  reported DAC position runs ~2 % ahead of the frames handed over, then
  snaps back), so a 5 s median still swung ±20 ms; only a run-long mean
  holds still.
- **The tap delay line stays as the fallback**: an output that is not
  MMAP-capable (a USB or Bluetooth device, another headset) gets its
  frames handed to the analysis `latency − 76 ms` after AAudio, from the
  run mean, set by the first estimate and then moved only in steps
  > 15 ms and at most every 2 s.
- **Cost:** one underrun as the stream starts (the first bursts), none
  after, in every 60 s run under the 400K world sim; logged in the
  playback status line. The frame time is unchanged (App 9.45 ms at
  400K). ndk 0.9 reports AAudio's positive "size set" return from
  `AAudioStream_setBufferSizeInFrames` as an error; the stream's getter is
  the truth.
- **Second film (Kevin, Sep 28, the 84 ms buffer, 11 ms tap delay): flash
  − click = +12 ms by the grid fit (60 flashes, circular jitter 14 ms),
  +6 ms median per click** (59 of 61 clicks have a flash; the peak frame
  is the click's own frame for 35, within one 16.7 ms frame for 50). The
  flash is on the sound to within the phone's frame quantum. The whole
  path: −115 ms (S6) → +41 ms (low latency alone) → +6–12 ms (buffer
  sized to the chain). This recording's click detector also caught 227
  transients at 124 ms spacing, so the per-click numbers above use only
  the transients on the 500 ms grid; `latency.py`'s grid fit is robust to
  that, its per-flash spread is not (board #3289).

## Mixed reality (S7)

Setup (commits `101cdd6`..`031956b`, Quest 3 v207, board #3194): `debug.fosfora.mode
mr` = `XR_FB_passthrough` reconstruction layer under the projection layer,
`XR_EXT_hand_tracking` joints as obstacle spheres, `XR_FB_scene` anchors and the
stage floor as obstacle boxes, gravity 0.15 m/s², near-sprite cull 0.3 m, no test
quad, sim cube 2.2 m centered at (0, 1.1, −0.9) m (the user stands inside it).
Same harness as S5 (`scripts/xr/sweep.sh --mode mr --set …`, headless, 30 s runs,
GPU ms = runtime `App=` median after 5 s). The headset sat unworn on the desk
(eyes at y ≈ 0.96 m), so no hands were tracked and, with no room set up, no
anchors: the obstacle set in these runs is the floor box alone. Hands and
anchors cost the same shader loop, so their cost is bounded below by these rows
and measured separately once a room exists (see "Functional gate" below).

### Extension gating (the S1 list was a manifest artifact)

The S1 app saw 84 extensions and none of the `XR_FB_scene` /
`XR_FB_spatial_entity*` family. Adding `com.oculus.permission.USE_ANCHOR_API` to
the manifest (no code change) makes the runtime list **97**: the 13 that
appeared are `XR_FB_scene`, `XR_FB_scene_capture`, `XR_FB_spatial_entity`,
`_query`, `_storage`, `_storage_batch`, `_container`, `XR_EXT_spatial_entity`,
`XR_EXT_spatial_anchor`, `XR_EXT_spatial_persistence`,
`_persistence_operations`, `XR_META_spatial_entity_discovery` and
`_persistence`. `XR_EXT_eye_gaze_interaction` is still absent (Quest 3 has no
eye tracking). Passthrough layer, both hand trackers and the scene query all
came up on the first run with the permission declared (board #3258).

### Frame time with MR on (72 Hz, unoccluded sprites)

| Particles | Passthrough | GPU ms (med / max) | frames/s | Long | Stale | Held? |
|---|---|---|---|---|---|---|
| 100K | on | 5.4 / 6.6 | 72.0 | 0 | 16 | Yes |
| 250K | on | 13.5 / 18.4 | 61.5 | 167 | 431 | No |
| 500K | on | 26.5 / 41.6 | 30.9 | 723 | 1040 | No |
| 750K | on | 40.0 / 63.2 | 21.5 | 505 | 833 | No |
| 100K | off | 5.5 / 6.4 | 72.0 | 0 | 6 | Yes |
| 250K | off | 9.4 / 10.1 | 72.0 | 2 | 9 | Yes |
| 500K | off | 15.9 / 21.4 | 50.7 | 132 | 511 | No |
| 750K | off | 26.1 / 36.8 | 33.4 | 769 | 1007 | No |

**Budget with passthrough, hands and room on: ~150K sprites at 72 Hz (100K at
5.4 ms holds; 250K at 13.5 ms does not).** Without passthrough the same scene
holds 250K (9.4 ms). Passthrough's cost is not a fixed overhead: 0 ms at 100K,
+4 ms at 250K, +11 ms at 500K, +14 ms at 750K, i.e. the app's GPU time
stretches by ~1.5× under load, which reads as GPU contention with the
passthrough pipeline rather than compositor work billed to the app.

### What each S7 feature costs (750K, S5 cube at (0, 1.5, −1.5), gravity 0)

| Configuration | GPU ms (med / max) | frames/s |
|---|---|---|
| `mode particles` (S5 as measured: static quad on) | 9.7 / 12.3 | 70.0 |
| `mode mr`, quad on, every S7 feature off | 9.4 / 13.8 | 70.1 |
| `mode mr`, quad off, every S7 feature off | 30.2 / 43.4 | 29.5 |
| `mode particles`, quad off | 31.1 / 47.4 | 29.1 |
| + passthrough | 45.8 / 70.9 | 19.0 |
| + hands + room + floor (no passthrough) | 29.2 / 45.9 | 29.2 |
| + passthrough + hands + room + floor | 44.2 / 74.1 | 18.8 |
| `mr` defaults (MR cube, gravity, near cull 0.3, all on) | 40.0 / 63.2 | 21.5 |
| same, near cull off | 41.3 / 61.2 | 20.6 |
| same, passthrough off | 26.1 / 36.8 | 33.4 |

- **Obstacles are free at this granularity:** the collide pass with 1–53
  spheres and up to 33 boxes is inside run-to-run noise (29.2 vs 30.2 ms).
- **The near cull saves ~1.3 ms** in the MR cube (41.3 → 40.0 ms).
- **Correction to S5 (board #3259): the S5 ceiling depended on the test
  quad.** The same 750K sprites cost 9.4–9.7 ms with the opaque quad in the
  scene and 30–31 ms without it, in either mode. The quad covers ~6 % of the
  view from the desk, so plain occlusion cannot explain a 3× change; the
  diagnostic rows below isolate the mechanism.

### The quad effect, isolated (750K, `mode particles`, 72 Hz)

| Configuration | GPU ms (med / max) | frames/s |
|---|---|---|
| quad at the S5 place (0, 1.5, −1.5): in view, occludes some sprites | 9.6 / 11.6 | 70.0 |
| quad at (0, 1.5, −4.0): in view, behind every sprite, occludes nothing | 10.6 / 11.7 | 70.1 |
| quad at (0, 1.5, +4.0): behind the head, not in view, zero fragments | 10.4 / 12.8 | 70.1 |
| no quad, sprites depth-tested (`Less`) | 31.1 / 47.4 | 29.1 |
| no quad, sprites with depth compare `Always` | 31.3 / 46.9 | 26.7 |

**Occlusion is not the mechanism.** A depth-writing opaque draw anywhere in
the eye pass, even one that produces no fragments, makes the 750K additive
sprite draw 3× cheaper; the sprites' own depth compare is irrelevant. This
smells like an Adreno/driver render-pass mode decision (binning or
LRZ setup) keyed on the pass having a depth-writing pipeline. Consequence:
every world-space pass should carry a depth-writing draw. `mode mr` now draws
a 1 mm "primer" quad below the floor behind the user by default
(`debug.fosfora.primer`); its effect on the MR budget is in the next table.

### MR budget with the primer (72 Hz, `mr` defaults, `031956b` + primer)

| Particles | Passthrough | GPU ms (med / max) | frames/s | Long | Stale | Held? |
|---|---|---|---|---|---|---|
| 250K | on | 8.2 / 8.7 | 72.0 | 0 | 1 | Yes |
| 500K | on | 13.1 / 16.0 | 64.6 | 74 | 311 | No |
| 750K | on | 17.6 / 23.7 | 44.7 | 640 | 773 | No |
| 500K | off | 11.6 / 18.3 | 68.6 | 40 | 229 | Marginal |
| 750K | off | 17.6 / 23.5 | 48.6 | 347 | 657 | No |

**MR budget: 250K sprites at 72 Hz with passthrough, hands, room and floor on
(8.2 ms, 1 stale frame in 30 s); ~350K is the edge (500K at 13.1 ms drops to
65 fps).** Against S5's 750K at 10.1 ms this is the honest number for the
product: it is what a wearer inside the cloud sees, over passthrough, with
hands. With the primer, passthrough's extra cost is inside the noise at 750K
(17.6 vs 17.6 ms) and ~1.5 ms at 500K; the earlier 1.5× stretch was the slow
sprite path amplified. Hands, room anchors and the floor remain below the
noise floor.

### Final numbers with the room loaded (72 Hz, `mr` defaults, `794d7df`)

Headless, but with the room from Space Setup: 16 anchors + floor as obstacle
boxes and depth occluders, primer, near cull, flow ×0.35, drift 0.5 m/s.

| Particles | GPU ms (med / max) | frames/s | Long | Stale | Held? |
|---|---|---|---|---|---|
| 250K | 5.5 / 6.2 | 72.0 | 0 of 2032 | 0 | Yes |
| 500K | 8.6 / 9.5 | 72.0 | 0 of 2026 | 4 | Yes |

Better than the earlier 250K at 8.2 ms: the room's walls, floor and ceiling
now write depth, so sprites behind real surfaces are rejected, and the
slower flow keeps more of the cloud settled on surfaces. **MR budget with
everything on: 500K sprites at 72 Hz (8.6 ms).**

### Hand mesh occluder (72 Hz, `mr` defaults, Sep 27 evening)

`XR_FB_hand_tracking_mesh` on v207 (with `USE_ANCHOR_API` declared, the
97-extension list): `xrGetHandMeshFB` returns **1360 vertices, 2314
triangles, 26 joints per hand**, wrist at the mesh origin, the hand 19 cm
long along +X (left) / −X (right). Drawn skinned, depth-only, in place of
the joint spheres.

Unworn, so no hand is tracked: the draw path was exercised with
`debug.fosfora.handmeshtest "0,0,-0.5"` (the left mesh in bind pose parked
0.5 m ahead of the eyes). The screencap shows a hand-shaped void with spread
fingers carved out of the cloud, passthrough showing through it.

Cost of one mesh in both eyes, `scripts/xr/sweep.sh --mode mr` (headset
face-up on the desk, floor + primer on, the room query returned no anchors
in this session, so the baseline is lower than the "final numbers" above):

| Particles | GPU ms med / max, no mesh | with one mesh | frames/s | Long | Stale |
|---|---|---|---|---|---|
| 250K | 2.65 / 3.09 | 2.92 / 3.27 | 72.0 | 0 | 0 |
| 500K | 5.53 / 6.14 | 5.55 / 6.12 | 72.0 | 0 | 0 |

Two hands are two such draws: **under 0.3 ms per hand, in the noise at
500K.**

**Worn gate (Kevin, Sep 27, 500K, room loaded with 16 anchors, passthrough,
both hands):** the screencap shows both real hands cut out of the cloud
with individual fingers, the gaps between them and the palm, in both eyes;
the mesh outline runs a few mm past the real fingertips in places (dark
slivers where the occluder hides sprites but the passthrough shows
background). Pinch still works through the same joints. Frame stats with
hands in view: 72.0 frames/s, 0 long frames per window once donned (the
101 long frames of the first 836 are the donning and Space Setup query),
7.3–8.0 ms GPU (`App=`), CPU 0.8 ms avg. The bone-length scale estimate
logged 1.07–1.09 for the left hand and stayed within ±2 % of 1.0 for the
right, so it is pose-noisy at the few-percent level; the mesh looked right
either way.

### C3b: Flux in world space (`mode world`, 72 Hz, Sep 27 late)

**Indirect draw empty on the Adreno.** The first device run of the C3b
port drew nothing: the frame was bare passthrough at every count, with 4×
sprites, with every depth writer off, and with the sim forced to opaque
white 3 cm sprites. The world path's indirect arguments read back correctly
every frame (`[899994, 1, 0, 0]` at 300K), yet `vkCmdDrawIndirect` produced
no fragments: raw, through wgpu's `VALIDATION_INDIRECT_CALL` copy, and
through a 16-byte transfer copy alike. The same count as a direct draw
rendered the ember cloud at once. Root cause (driver or wgpu-hal) unknown;
no earlier step had exercised an indirect draw on the device (S4 Flux ran
the compute raster, the S5 sim draws directly). `render_world` now draws
`3 × max_particles` directly and the vertex shader drops sprites past the
alive count (`8e20998`; a core test reproduces the stale-alive-list case).

**Cost with the cloud visible** (unworn, floor + primer on, no room anchors
returned in this run, hands untracked):

| Particles | GPU ms (med / max) | frames/s | Long | Stale | Held? |
|---|---|---|---|---|---|
| 250K | 12.8 / 16.0 | 63.2 | 63 of 1785 | 277 | No |
| 300K | 15.6 / 18.7 | 53.6 | 127 of 1567 | 506 | No |
| 500K | 24.7 / 32.2 | 32.4 | 741 of 1052 | 976 | No |

Against the S5 test sim at 500K in 8.6 ms with the room, hands and
passthrough all on, Flux is about 3× the cost per particle. Before the
fix the same runs, drawing no fragments, cost 7.1 ms (250K) and 7.3 ms
(300K): sim plus core's per-dispatch passes plus the empty draw.

**The split, 300K:**

| Run | GPU ms (med / max) | frames/s | Long |
|---|---|---|---|
| sim on, passthrough on | 15.6 / 18.7 | 53.6 | 127 |
| sim frozen after warmup (`sim 0`) | 9.2 / 14.7 | 72.0 | 19 |
| passthrough off, sim on | 10.3 / 11.2 | 72.0 | 1 |

- **Sim: ~6 ms at 300K.** Two `sample_flow_field_3d` reads, two `noise2`,
  the collide loop, respawn, plus whatever core's `dispatch` runs beside
  the sim. The S5 sim's compute at 500K was a fraction of that.
- **Draw: passthrough costs ~5 ms here** against ~1.5 ms in S7, where the
  sprite draw shared a pass with a depth-writing draw. `render_world`
  records its own pass with no depth-writing draw in it (the S7 "primer"
  finding, up to 3×), and the eye pass now stores its depth for the world
  pass to reload (2 × 1680×1760 D32 per frame). The PR named this risk.

**The eye-pass hook.** `ParticleSystem::prepare_world` (pipeline for the
pass's formats, camera slot; before the pass) and `draw_world` (inside
the caller's pass) replace the separate pass in the Quest build: the
sprites now draw last in the eye pass, after the primer and the
occluders, and the eye pass discards its depth again. Same runs:

| Particles | GPU ms (med / max) | frames/s | Long | Stale | Held? |
|---|---|---|---|---|---|
| 250K | 8.4 / 9.4 | 72.0 | 0 of 2032 | 0 | Yes |
| 300K | 9.2 / 9.7 | 72.0 | 0 of 1961 | 0 | Yes |
| 500K | 11.7 / 13.3 | 69.4 | 9 of 1952 | 104 | No |

The split at 300K after the hook: passthrough off 9.8 ms (no penalty
left, as in S7 with the primer), sim frozen 5.8 ms, so the sim is ~3.3 ms
and the draw ~5.8 ms. **Flux in world space: 300K at 72 Hz with 4.7 ms
of headroom (unworn, no room anchors in this run).** The brief planned
Flux for ~300K.

**Tuning so hands and the room read** (`xr-flux-tune`, board #3276).
Worn, the merged port looked good but showed no reaction to hands or
furniture, although the log had 52 hand spheres and 16 room boxes reaching
the sim every frame: Flux had no drift, so nothing settled on a table; its
near fade (0.3–0.45 m) thinned the cloud exactly at hand distance; and its
volume ended at the floor, where a settled particle respawns. Changes: a
settle drift at integration (the `mr` gravity, 0.5 m/s, through the aux
block), a 3 m volume so the floor and tables are inside it, and world-mode
defaults of a 0.15 m near fade, a 10 cm hand pad and a 0.4 m/s kick.
Preset raised to 400K to keep the cloud dense in the larger volume.

| Particles | GPU ms (med / max) | frames/s | Long | Stale | Held? |
|---|---|---|---|---|---|
| 300K | 7.6 / 8.2 | 72.0 | 0 of 1959 | 0 | Yes |
| 400K | 9.3 / 9.8 | 72.0 | 0 of 2028 | 0 | Yes |

Cheaper than before at the same count: the sprites spread over 3.4× the
volume overlap less.

**Worn (Kevin, Sep 27 late):** the hand channel reads, the desk partly
(his stationary space was not well set), but the cloud crowded the bottom
of the volume: a uniform 0.5 m/s drift over a 12 s life brings nearly
every particle to the floor. Fix (`af3fc7b`): the collide step reports a
push-out through an upward face, and a resting particle ages 2× faster on
top of real time, so it lands, slides for a few seconds, fades and
respawns up in the volume. Sweep of the fix at 400K, 72 Hz, unworn, room
loaded (16 anchors + floor, 52 hand spheres):

| Run | GPU ms (med / max) | frames/s | Long | Stale | Held? |
|---|---|---|---|---|---|
| Sep 27 23:58, cold | 9.3 / 10.0 | 72.0 | 0 of ~2000 | 0 | Yes |
| Sep 28 00:13, cold | 9.7 / 13.1 | 72.0 | 3 of 2021 | 7 | Yes |
| Sep 28 00:15, back to back | 10.7 / 12.4 | 71.9 | 8 of 2000 | 36 | Yes |
| Sep 28 00:16, back to back | 10.5 / 13.2 | 72.0 | 1 of 1955 | 4 | Yes |

Cost unchanged by the aging (one bool per box test); the spread is the
device: runs 3 and 4 followed run 2 without a pause (battery 38 °C) and
sit ~1 ms above the cold runs. 400K holds 72 Hz with 3 ms of headroom at
worst. **Worn gate closed (Kevin, Sep 28): fine for now.** PR #183 merged.

**Next:** the same hook serves the Murmur and Tide ports; the sim trim
(one flow sample) only if 500K in the small volume is wanted.

### Functional gate (wearer, Kevin, Sep 27, ~11 worn runs)

- **Passthrough:** the real room shows behind the particles (confirmed by
  the wearer; a headset screencap shows the desk, monitors and hands under
  the cloud).
- **Room:** Space Setup launched from inside the app (`xrRequestSceneCaptureFB`)
  and returned 18 anchors: 5 tables, 3 storage units, floor, ceiling, 4
  walls (one invisible), door and window frames, and a global mesh of
  87,530 triangles. Anchors arrive with `LOCATABLE` off and are enabled with
  `xrSetSpaceComponentStatusFB`. Local +Z is world up for tables and the
  floor, down for the ceiling, horizontal for walls (logged), so the
  bounded-box placement is right.
- **Bounce off a real table:** particles land on the desk top and slide
  off it, not through it (wearer). With gravity as an acceleration nothing
  settled: the flow blend damps it to a few cm/s; as a 0.5 m/s drift with
  the flow at ×0.35 it reads.
- **Hands:** both tracked, 26 joints each, as padded spheres (+6 cm): the
  cloud parts around the hands and particles bounce off them (wearer).
  Visually the joint occluders are still wrong (cubes read as cubes; sphere
  impostors are smoother but not a hand): a follow-up with
  `XR_FB_hand_tracking_mesh` as the depth occluder.
- **Pinch:** thumb-index pinch on either hand toggles the sprite size ×3
  (wearer: "toggles the bloom"), 11–15 mm on, 30–120 mm off.
- **Bugs found only by wearing it:** the first frame after donning has no
  view pose and `xrEndFrame` rejects a projection layer (was fatal); the
  runtime answers the first scene query after focus with 0 anchors for a
  room it finds 3 s later (was a Space Setup popup loop); the LOCAL storage
  filter fails validation on v207; and passthrough has no depth, so without
  depth occluders every collision void is hidden behind the particles that
  are behind the object.
- **Not done:** the Meta XR Simulator glasses-input check (macOS only; this
  session is Linux). Logged, no effect on the decision.

### C3b: Murmur in world space (`mode world`, `effect "Murmur XR World"`, 72 Hz, Sep 28)

The port as delivered (PR #187: K=7 topological neighbors from the 3D hash,
27 cells x `MAX_PER_CELL` 16 candidates per bird, hands as predators), 30 s
runs, unworn, 52 hand spheres and 17 boxes reaching the sim:

| Particles | GPU ms (med / max) | frames/s | Long | Stale | Held? |
|---|---|---|---|---|---|
| 100K | 25.1 / 34.7 | 33.3 | 748 of 1058 | 892 | No |
| 150K | 47.5 / 68.7 | 17.3 | 400 of 617 | 1369 | No |
| 200K | 80.5 / 108.5 | 10.9 | 257 of 422 | 1474 | No |
| 300K | 123.8 / 134.4 | 7.1 | 192 of 315 | 1665 | No |

About 0.25 ms per thousand birds, 5-10x over the frame at the brief's
200K. A stripped scan (K=5, `MAX_PER_CELL` 2: 54 candidates, 8x fewer)
is only about twice cheaper, so the per-bird fixed cost rules: 27 cell
range lookups, two passes over the sphere block (predator, then collide),
the neighbor positions re-read after the sort.

| Particles, K=5 / 2 per cell | GPU ms (med / max) | frames/s | Long | Held? |
|---|---|---|---|---|
| 100K | 13.7 / 19.1 | 63.0 | 78 of 1703 | No |
| 150K | 23.3 / 36.0 | 36.1 | 782 of 1086 | No |
| 200K | 31.7 / 48.9 | 24.0 | 567 of 795 | No |

Flux XR World at the same counts, for scale: 100K 3.4 ms, 150K 4.6, 200K
5.9, 300K 8.7 (whole frame, 72 Hz throughout).

**Decision (Kevin, Sep 28):** the port is not going to budget, and the
product pivots to room-native effects (`ROOM_DESIGN.md`). Murmur ships
small, as a flock that lives in the room and flees the hands, if it reads
worn; the full scan stays (K=7, 16 per cell), with the roost 1 m ahead
of the anchor and the head as a hawk (review fixes, `f898f07`):

| Particles | GPU ms (med / max) | frames/s | Long | Stale | Held? |
|---|---|---|---|---|---|
| 30K | 9.2 / 9.9 | 72.0 | 0 of 2031 | 0 | Yes |
| **40K (preset)** | **9.7 / 12.7** | **72.0** | **2 of 2024** | **17** | **Yes** |
| 50K | 11.5 / 15.2 | 70.0 | 25 of 1956 | 126 | No |

Worn gate: the flock keeps out of the face, splits around a hand and
wheels around the desk.

## Hands-first interaction (I5)

### Pinch gestures, worn gate (Kevin, Sep 28, `mode world`, Flux XR World 400K, 72 Hz)

Worn, both hands, no controllers. One pinch at a time is a tap, a drag
or a hold (`crates/fosfora-xr/src/gesture.rs`): past 2.5 cm of pinch-point
travel it is a drag, which moves the cloud anchor 1:1 with the hand; held
within 2.5 cm for 0.7 s it is a hold; released before either it is a tap.

- **Pinch detection:** every attempt registered, on either hand: 28
  pinches engaged, at 3.9–14.9 mm tip distance (median 9.8 mm), released
  at 30–89 mm. The tips close to 1–8 mm in a held pinch. No near-misses:
  in every one-second window without a pinch the closest approach was
  28 mm or more. So the 15 mm threshold is not the problem. The earlier
  "pinch does nothing" in `mode world` was the consumer: its only effect
  was the S5 test sim's sprite size, and world mode runs that sim at
  count 0.
- **Gestures:** the 28 pinches became 16 taps, 7 drags and 2 holds; the
  other 3 were the second hand pinching while the first owned a gesture.
- **Drag:** 7 drags, left and right, 0.09–0.53 m net each; the anchor
  followed the hand (it ended at (0.19, 1.29, −0.27) from (0, 1, 0)). A
  drag whose hand lost tracking mid-motion ended cleanly, and the next
  pinch started a new drag. The second hand's pinch was ignored while the
  first hand dragged.
- **Hold:** 2 of 2 fired, once each, at 0.7 s.
- **Frame time, worn (41 s, hands in view):** GPU (`App=`) 11.4 ms median,
  14.6 ms max; 70 fps median, 53 min; 220 stale frames. **Unworn right
  after:** 8.6 ms, 72 fps. The gestures do not cause it: the first drag
  ran at 72 fps and 10.3 ms, and the drops start in the tap-only stretch
  before any drag. Flux XR World at 400K was measured unworn only
  (9.3 ms, C3b above); worn, with the wearer inside the cloud and hands in
  view, it runs over the 13.9 ms budget in bursts. Open.

### Pinch-hold cycles the world effects (Sep 28, unworn, proximity faked)

A pinch-hold switches to the next world-layout preset (every
`*_xr_world*.pfx`, in file-name order): today Flux XR World (400K) and
Flux XR World Coarse (150K larger embers in a 2 m volume, a test variant
until Murmur lands). `debug.fosfora.cycletest 8` switched every 8 s for
the measurements.

- **Rebuilding on each switch:** 495–507 ms per switch (6 switches), one
  frozen frame of ~0.5 s each time. Rejected.
- **Built once at startup, swapped:** each effect builds in 507–530 ms at
  launch; a switch then costs 0–2 long frames (worst window 29.8 ms max
  interval, the first switch), 72 fps otherwise. A parked effect resumes
  where it stopped. Memory: 424 MB PSS with both effects built against
  392 MB with one.

### Debug panel (Sep 28, unworn with `debug.fosfora.hudtest 1`, Flux XR World 400K)

Development only and opt-in: `adb shell setprop debug.fosfora.hud 1`
before launch (off by default; an app setting may replace the knob).

An egui panel above the left palm (shown while the palm faces the wearer,
poked with the right index finger): fps, long frames, the runtime's app
GPU time and utilization (`XR_META_performance_metrics`: 17 counters on
v207, no app CPU counter, so the frame loop's own CPU time stands in), a
two-second GPU-time graph against the 13.9 ms budget, particles, hands,
gesture, anchor with Recenter, audio levels, Prev/Next effect, and four
live sliders (settle, near fade, hand pad, hand kick). 640 x 1024 texture
on a 20 x 30 cm quad, drawn after the sprites and depth-tested against the
hand occluders.

- **Cost while shown:** frame-loop CPU 2.23 ms against 1.50 ms hidden
  (+0.7 ms, the egui pass); GPU 9.61 ms median against 9.73 ms hidden (no
  measurable difference); 72 fps, 0 stale either way (25 s each).
- **Check without a wearer:** `hudtest` parks the panel ahead of the view
  and dumps its texture to `files/config/hud.rgba` at frame 720; the dump
  read back pixel-exact (layout, values and graph as intended).
- **Since then:** the texture grew to 640 x 1472 (20 x 46 cm) for the
  steppers and rows added later, and board #3402 put the controls in two
  columns with smaller text; see "Particle pitcher and the panel" below.

### Operating the panel without depth judgment (Sep 28, worn)

Requirement: the panel has to work without stereo depth perception, and
without it judging a fingertip's distance to a floating panel is
guesswork. Three rounds, from the log:

- **egui widgets, poke only:** "hard to use"; presses registered, clicks
  were luck (a poke drifts past egui's click tolerance between press and
  release).
- **plus a hand ray (shoulder through index knuckle) with pinch, still
  egui widgets:** ~45 presses, all classified as pokes, 0 button actions
  fired. Pointing at a panel held over the other hand brings the fingertip
  within the 10 cm poke band, so the poke took the pointer from the ray
  every time; and egui's click-on-release failed for the same drift. No
  ray was visible.
- **big rows, fire on press, visible beam:** full-width rows 2.5 cm tall
  picked by the pointer's height alone, left/right halves for Prev/Next and
  -/+, the target under the pointer fires when the press starts and stays
  locked until release, held -/+ repeats; poke only within 3 cm; a beam
  from the knuckle to the hit point; no cloud gestures while the panel is
  up. **57 of 57 presses fired a target** (12 ray, 45 poke): 10 Next,
  2 Recenter, 212 steps including repeats; 0 cloud gestures started while
  the panel was up (17 outside it). Wearer: "much better".

## Seated reach: Go-Go arm extension (board #3308)

### Worn gate (Sep 28, seated, both hands, no controllers, Murmur XR World 40K and Flux)

The shoulder is estimated from the head (0.15 m out to the hand's side,
0.20 m down, the head's yaw only, low-passed at 0.15 s). Within a
threshold of it the hand is 1:1; beyond it the whole joint set moves along
the shoulder-to-palm line to `d + gain * (d - threshold)^2` (capped at
3 m, low-passed at 0.05 s). The far joints feed the sim's spheres (Murmur's
predator and every effect's collide); the skinned-mesh occluder, pinch
gestures and the debug panel stay on the real hand. Past 3 cm of extension
the far hand shows as small sprites on its joints with a faint beam from
the real wrist.

- **The wearer's arm, from the log:** a stretched palm sits 0.52 m (p90)
  to 0.62 m (max) from the estimated shoulder, short of the 0.70 m the
  first pass was designed for. With threshold 0.45 m and gain 20.8 the far
  hand reached 0.89 m (p90) and 1.01 m (max): not far enough for the
  flock.
- **Tuned worn from the debug panel to threshold 0.30 m, gain 40**, now
  the defaults: the far hand reached 2.48 m (p90) and the 3 m cap (57 s at
  these settings).
- **Frame time, worn with reach on (GPU `App=` median / max, fps median /
  min):** Murmur 40K 10.0 / 12.5 ms, 72 / 55 fps (314 s); Flux Coarse
  8.8 / 15.1 ms, 72 / 60 (212 s); Flux 10.4 ms median over 29 s, its
  47 ms max at an effect switch.
- **Verdict (wearer):** "pretty great". The flock 1 m ahead is reachable
  from the chair; control of the far hand is "excellent"; near and far
  interactions both work; the wrist beam is "a little bit bright" (alpha
  0.25 -> 0.15 in the same change).

### Hand menu (Sep 28)

Turning the left palm up always shows a hand menu, for now one row: the
debug panel's on/off toggle, saved in the app's config (`hand_menu.json`)
until turned off again; `debug.fosfora.hud 0|1`, when set, overrides at
launch and is saved too. With debug on the same quad grows upward into the
debug panel (20 x 43 cm), the toggle its bottom row; with it off the quad
is the 20 x 6 cm menu strip. The quad keeps its bottom edge on a resize
and the toggle sits the same distance above it in both (64 px against 63
px in the texture dumps), so the toggle stays under the pointer. The
runtime's performance counters run only while debug is on, following the
toggle during the session.

## Why the hands never scoop the flock (board #3311)

Worn report (Sep 28): the flock always scatters from a hand at once; the
"push lets you scoop and part the flock" expectation (#3309) never shows.
A hand-scare lane (aux[2].w, stored as calm so an unwritten 0 keeps the
full scare; debug panel row, `debug.fosfora.handscare`) scales Murmur's
hand predator, for an A/B in the headset.

- **The lane works (GPU test, 20K birds, five 2 cm joint spheres at the
  roost, no pad, no kick):** birds within 0.25 m of the palm: 1238 with no
  hand, 1838 with the hand calmed (scare 0), 0 with the calm factor
  removed from the shader (the break-it check).
- **Worn, scare 0 alone:** "pretty much the same" as scare 1. The hand
  still carries its collide: 52 joint spheres padded by 0.10 m (a
  hand-sized ~0.3 m blob) that throw a touched bird out at 0.4 m/s, swept by
  the seated-reach far hand at several times the real hand's speed.
- **Worn, scare 0 with kick ~0.1 and pad 0-0.02:** the far hand passes
  behind and through the flock without disturbing it. The log does not
  separate pad from kick (both were lowered within seconds).
- **Nothing ever carries a bird:** each frame a bird's velocity is rebuilt
  from its own heading at cruise speed, so a contact only pushes it out
  and turns it. Parting works; scooping needs a hold behavior (an
  attractor or containment), which is the pose design: fist = predator,
  open hand = push only, palm to the ceiling or two hands closing = hold.

## Pose-driven hands (board #3314)

The hand's pose picks what it does to Murmur's flock: a fist is the
predator, an open hand only pushes, and a palm turned to the ceiling or two
palms facing each other close together hold part of the flock and carry it.
Poses are read on the real hand; the behavior acts at the seated-reach far
hand. Knobs: `debug.fosfora.poses`, `openpad`, `openkick`, `holdradius`.

- **Worn gate (Sep 28, seated, both hands, no controllers): pass.** Kevin:
  "It looks great. Everything working." Fist scatters, open hand parts,
  palm-up and two-hand holds carry a group, the hand menu still opens with
  the left palm toward the face. 176 s at 72.0 fps, 8 long frames of 12747.
- **Holds, worn:** 8 in the session, 6 palm up and 2 with both hands,
  lasting 3.9 to 15.0 s and carrying their center 1.5 to 10.7 m (the far
  hand's path). Two-hand holds opened at radius 0.13-0.14 m (half the real
  palms' distance).
- **Pose readings, worn:** an open hand's curl (mean fingertip-to-palm
  distance) sits at 0.126-0.147 m, a fist's reaches 0.033-0.037 m, so the
  fist thresholds (on under 0.055 m, off over 0.070 m) have wide margins on
  both sides.
- **The hold latches (GPU test, 20K birds):** a hold that let every bird in
  and none out was a trap that cohesion kept filling: at the roost it took
  70-80 % of the flock in a second. Latching only the birds inside its
  radius in its first 0.15 s holds about a fifth (3.9-4.6K). Moved 0.8 m at
  2 m/s it arrives with 3.7-4.4K inside its radius, against 1-340 on the
  spring alone (no carry velocity) and 1-207 with no hold.
- **Open-hand pad and kick (GPU sweep, a flat 25-joint hand):** at far-hand
  speeds (1.5 and 3 m/s) a pad of 0.05 m or more leaves an empty wake
  (0 birds against 484-922 with no hand) and 0.00-0.02 m slips between the
  joints; nothing is displaced more than 0.5 m. At 0.5 m/s the kick is what
  plows: 447 birds displaced with kick 0.4 against 8 with none. Open-hand
  defaults: pad 0.05 m, kick 0.1 m/s; the fist keeps 0.10 m and 0.4 m/s.
- **Cost:** unworn 40K, 10.03 / 10.40 ms GPU median / max at 72 fps
  (before: 9.7 / 12.7).

## Room first pass: surfaces as emitters and the floor ripple (board #3317)

The room's real surfaces emit (the "Flux XR Room" preset: tables shed
embers on the beat, the floor sparks with the bass; the surface kind and
weight ride in the box block's unused w lanes) and one lit quad on the
real floor shows rings from under the wearer on each beat. Knobs:
`debug.fosfora.floorweight`, `tableweight`, `ripple`, `ripplegain`,
`ripplespeed`, `rippletest ceiling`.

- **Worn gate (Sep 28, seated, Flux XR Room 400K + ripple, the bundled
  track on the playback tap): pass.** Kevin: "a good v1, everything
  working as designed." The room located 16 anchors; emitter weight 1.29
  (the desk 1, the other tables by area, the scene floor 0.5); 375-388K
  alive. Runtime App GPU over the 5 min worn window (which ends with a
  cycle through Murmur): 9.50 ms median, 11.85 p90, 16.99 max; 72 fps
  median, p10 69, min 48; 713 stale and about 115 long frames of ~22K.
- **Unworn (72 Hz, 30 s sweeps, headset face up, room not located so only
  the stage floor emitted at weight 0.5):** Flux XR Room 400K 7.01 / 7.76
  ms GPU median / max, 72 fps, 0 long, ~330K alive; ripple off 7.00 / 7.88
  (the floor is out of view unworn, so this pair says nothing about the
  ripple's cost). Murmur XR World 40K, ripple off: 9.95 / 10.92, 72 fps.
  So the room in view (surfaces, the ripple, tracked hands) is worth about
  2.5 ms over the unworn floor-only run.
- **Ripple, in view (unworn ceiling stand-in, rough, not a sweep):** 7-8 ms
  App GPU with the quad filling the view at gain 4 against 5.2-6.7 without
  it, before empty ring slots were skipped in the shader. Rings show with
  no z-fighting on a face whose occluder writes depth; the parked hand
  mesh cuts a clean hand-shaped hole in them.
- **Emission (GPU test, Flux XR Room at 20K, one table box):** after 5
  frames every particle sits on the table top; after 20, each is on it or
  falling off its edge, at least 90 % still on top. "Flux XR World" with
  the surface lanes written spawns exactly as before.
- **Rate:** at the volume preset's 53333/s the room settled near 60K alive,
  because a claimed slot is declined while its surface's gate is closed;
  at 160000/s it holds 330K unworn (floor only) and 380K worn (16 anchors).

## Room second pass: the wall spectrum and the hand instruments (board #3327)

The spectrum climbs the wall the wearer faces (a canvas quad on one
WALL_FACE, picked with hysteresis, 24 bars from the mel bands, each bar
normalized against its own running top), and the hands play the room: a
short pinch-release throws a streak from the far hand along the line from
the head through the pinch that bursts where it hits a surface; an open
palm, down and still, lifts embers off the surface under it. Instrument
rows 170..173 follow the hand lanes (WORLD_AUX_ROWS 173). Knobs:
`debug.fosfora.canvas`, `canvasgain`, `canvasbars`, `canvastest ceiling`,
`throw`, `burstcount`, `throwtest N`, `lift`, `liftradius`.

- **Worn gate (Sep 28, seated, Flux XR Room 400K with the ripple, the
  spectrum and both instruments, the bundled track on the tap): pass.**
  Kevin: "this is awesome, everything seems to be working as intended."
  The pick held on one wall 5.25 m ahead for the whole 6 min; bar height
  mean 0.58, max 0.99. 511 throws logged, hitting the floor, a table and
  the far wall at 4.7 m; about 97 lifts. Alive ranged 7K to 399K (median
  256K): a throw steals from the living cloud and the surface gates breathe.
  Runtime App GPU 9.75 ms median, 10.93 p90, 15.32 max; 72 fps median,
  p10 70, min 56; 623 stale frames. **Known failure in that pass: the
  throw.** Kevin saw a small burst at the hand that fell a short way and
  faded, no streak to the aimed surface and nothing at it; the 511 log
  lines are the aiming ray's hits, not visible impacts (board #3329).
- **Throw range:** the aiming ray first looked only as far as a miss ends
  (3 m), so the wall 4-5 m ahead read as a miss and the burst hung in the
  air. The ray now looks 12 m; a real miss still bursts 3 m out.
- **Unworn (72 Hz, 30 s sweeps, headset face up):** Flux XR Room 400K with
  the spectrum in view on the ceiling stand-in 8.09 / 8.75 ms median / max,
  and with the canvas off 9.09 / 10.05, both 72 fps, 0 long. The order is
  the anchor-load noise between runs (whether the room's surfaces emitted),
  so the spectrum's cost is under that floor.
- **GPU tests (core, Flux XR Room at 20K):** a burst of 2000 lands every
  newborn within its 0.12 m radius, and a burst outside the volume keeps
  1990 or more of them out there 10 frames later (burst particles are
  free of the volume); embers settled on a table at mean y -0.472 rise to
  -0.410 after 20 frames of lift, against -0.468 without it.
- **Bars, device:** one running top for the whole wall left the high bands
  dark (mean height 0.11-0.26); a top per bar, held within 16 dB of the
  loudest and never below -40 dB, gives mean 0.45-0.58 and max 0.8-1.0.
- **Throw fix (board #3329, unworn, Sep 28):** the flight and the impact
  were right all along: with the cloud thinned to 100K and a 30K burst
  the ball appeared exactly at the logged ceiling hit, raining down under
  the settle drift. Two faults hid it at the product scale. Burst
  particles were drawn like the cloud (5 mm sprites, opacity 0.08-0.13,
  the same color), so a 6000 ball was 1.5% of a 400K cloud and the 300 a
  frame streak puffs (3 cm) vanished; what Kevin saw at the hand was the
  near end of the streak (sprites six times larger on screen at 0.5 m)
  falling at 0.5 m/s. And a free particle died 3 half extents (4.5 m)
  from the anchor, so a burst on the far wall (5.25 m) died on its first
  frame. Now a burst particle is drawn at twice the sprite size, opacity
  0.35 and its color half way to warm white (`XR_BURST_SIZE`, `_ALPHA`,
  `_WHITE`), and dies 13 m from the anchor (`XR_FREE_REACH_M`, past the
  ray's 12 m). Screencaps with `throwtest 6` on Flux XR Room 400K show a
  bright ball at the ceiling hit with the streak's puffs trailing below
  it, then a spray raining from the hit. Cost: App GPU 9.52 ms median,
  9.82 p90, 10.31 max over 78 s with 13 throws, 72 fps, 2 long frames of
  5658, 5 stale; within the pass's 9.75. A first try at 3x size, opacity
  0.6 and 0.7 white drowned the view in yellow with throws every 2 s. GPU
  test (core, Flux XR World at 20K): a flight's rows walked over 20 frames
  to a hit 5 m out leave one puff of 300 per frame along the ray, the
  farthest at the current center and none beyond it; the impact's 6000
  land in the ball at the hit; all survive 10 frames on (the test fails
  at the old reach).
- **Worn re-gate (Kevin, Sep 28, 405d0e8): pass**, "I see it working
  now." His wish: the streak should get smaller and dimmer as it travels
  away, then explode into the current colors. So each frame's burst
  carries a brightness (row 172 w): 1 at the hand falling to 0.3 at the
  hit for the streak (`STREAK_END_BRIGHTNESS`), 1 for the impact; the sim
  mixes sprite size, opacity and the pull toward white between the cloud's
  and the burst's by it. Unworn, same setup: a white ball at the hit over
  a dimmer, warmer trail. App GPU 9.56 ms median, 9.91 p90, 10.32 max
  over 68 s with 11 throws, 72 fps, 4 long of 4928. Kevin notes the
  effect may be partly lost on him without stereo vision.

## Rescan the room (board #3323)

The hand menu carries "Rescan the room" under "Recenter the cloud"
whenever the room is on. It relaunches Space Setup
(`xrRequestSceneCaptureFB`) and the query that follows its completion
replaces the anchors. `debug.fosfora.rescan 1` does the same once the
first query has returned anchors; `debug.fosfora.rescan query` reruns the
query alone, so the replace path can be exercised over adb with nobody
wearing the headset.

- **Handle reuse (Quest 3, v207, Sep 29):** a requery returns the same
  `XrSpace` handle for every anchor it returned before. Destroying the old
  anchors before reading the new results left every new handle invalid
  (`ERROR_HANDLE_INVALID` from `xrDestroySpace`, "no bounded component"
  for all 18) and the room went to 0 boxes. The anchors are now
  reconciled by handle: one the new results still carry is rebuilt from
  them with its pose kept, one they no longer carry is destroyed.
- **Unworn, `rescan query`, Flux XR Room 400K:** the second query
  returned 18 results, "16 kept, 0 dropped", 16 anchors (8 planes, 8
  volumes), 17 boxes (room 16 + floor) as before; no warnings beyond the
  usual startup ones; 72 fps, 0 long frames.
- **Unworn, `rescan 1`:** the runtime accepts the request
  (`GuardianSDKServer::RequestSceneCaptureFlow 0`) and the app keeps
  running with its 17 boxes, but the Space Setup flow shows nothing with
  nobody wearing the headset, so `SceneCaptureComplete` never arrives.
  The full loop (Space Setup, then the query, a changed room's anchors
  replacing the old) is the worn gate. Worn gate pending.

## Live environment depth as an occluder (board #3324)

Phase 1: `XR_META_environment_depth` as a depth occluder, so things the
scan does not know (an unscanned chair, a person, the wearer's own body,
a moved object) hide the sprites. Phase 2, the depth map as a collision
source for the sim, is not in this pass.

**Setup** (`crates/fosfora-xr/src/env_depth.rs`). With `envdepth` (on
by default in `mr` and `world`), `envdepthshow` or `envdepthcheck` on, the session creates a depth provider and its
swapchain (a two-layer `D16_UNORM` array per image, left eye then right,
wrapped as wgpu textures), sets hand removal when the system supports it
and starts the provider. Each frame, after the views are located, the
frame loop acquires the depth image for the predicted display time in
the stage space. The eye pass then draws one full-screen triangle first,
right after the clear (depth compare Always, depth write on, color off):
each fragment takes the point 2 m along its eye ray, looks the map up
where that point lands in the depth camera (layer = eye), decodes the
OpenGL-convention depth to meters, scales the depth camera's ray to that
distance and writes its depth in the eye as `frag_depth`. The primer, the
room boxes and the hand mesh follow as before. Texels with no data
(`d >= 1`), distances under `envdepthnear` and points outside the depth
fov are discarded; a frame with no depth image (`NOT_AVAILABLE`, just
after start) skips the draw. The knobs are read at startup:

```
adb shell setprop debug.fosfora.envdepth 0        # the occluder off (default on in mr and world, off elsewhere)
adb shell setprop debug.fosfora.envdepthshow 1    # the diagnostic: the same pass drawn as gray
adb shell setprop debug.fosfora.envdepthhands 0   # keep the hands in the depth map (default 1: removed)
adb shell setprop debug.fosfora.envdepthnear 0.2  # discard distance, m
adb shell setprop debug.fosfora.envdepthflipv 1   # read texture row 0 as the top (default 0: the bottom, verified)
adb shell setprop debug.fosfora.envdepthcheck 1   # the numeric self-check (below); draws nothing by itself
```

With `envdepth`, `envdepthshow` and `envdepthcheck` all 0 no provider is
created: the baseline is the app without it. A failed provider creation
is retried every 5 s from the frame loop, up to 12 times, each attempt
logged.

**Reading the diagnostic.** `envdepthshow 1` writes the decoded distance
as opaque gray over the view (the sprites still draw over it): 0 m black
to 4 m white, linear in the stored bytes, so a screencap's gray level /
255 x 4 is the distance in meters. Upright and aligned means the gray
edges of the desk, the door frame and a hand held out sit on the
passthrough edges in both eyes; a map upside down (the floor bright at
the top) means the row order is wrong (`envdepthflipv`). A screencap
never shows passthrough, hence the self-check below. Logcat's `environment depth:` lines give
the swapchain length and size, whether hand removal was applied, the
first frame (after how many not-available frames and ms), its near/far
and fov, any near/far change, and a count every 720 frames.

**The numeric self-check** (`envdepthcheck 1`), because a screencap never
shows passthrough: once a second, with the room's boxes located, a
compute pass copies a 40 x 40 grid of each layer (every 8th texel of
320) into a buffer that is mapped a few frames later, without stalling
the loop. Each texel's ray, from the acquired pose and fov of its layer,
is cast against the room's boxes (hidden walls left out) and the stage
floor (y = 0); texels with no depth data or no hit are skipped. Per
layer, one `envdepth check` line gives the texels with depth data and,
for three readings of the texture (row 0 at the top, as the shader reads
it with `envdepthflipv 1`; row 0 at the bottom; columns mirrored), the
texels compared, the fraction within 10 cm and 25 cm and the median of
map minus room depth. The reading with the high agreement is the right
one.

**Check verdict (Quest 3, `7421282`, unworn on the desk, 17 boxes + the
floor, six consecutive checks, all stable):**

| Reading | Layer 0: 10 cm / 25 cm / median | Layer 1: 10 cm / 25 cm / median |
|---|---|---|
| Row 0 at the top | 12 % / 24 % / −0.170 m | 8 % / 16 % / −0.12 m |
| Row 0 at the bottom | 42 % / 52 % / +0.006 m | 42 % / 59 % / +0.017 m |
| Columns mirrored | 3 % / 10 % / −0.188 m | 4 % / 11 % / −0.29 m |

Texture row 0 is the **bottom** of the view: the runtime renders the map
in its own GL framebuffer (the MR service's log shows
`GL_DEPTH_COMPONENT16` framebuffers), so the rows come in GL order
whatever the app's graphics API. `envdepthflipv` now defaults to 0. Read
that way the map matches the scanned room to a median of 1-2 cm; the 42 %
within 10 cm is the real room against 4 cm slabs and unscanned objects,
not a misalignment.

**Runtime facts (Quest 3, v207, Sep 29).**

- The first three launches failed at `xrCreateEnvironmentDepthProviderMETA`
  with `ERROR_RUNTIME_FAILURE`. The runtime's mixed reality service
  (`mrsystemservice`, up 42 h) logged, for every client including the OS
  shell's own provider: "MUSTFIX: MIXEDREALITY.Framebuffer: Failed to
  create swapchain. Requested format 33189, 320x320, length 4, arraySize
  2". A headset reboot fixed it (hence the creation retry above).
- After the reboot: "swapchain 4 images, 320x320 D16 x 2 layers, hand
  removal on (supported true)"; the first depth frame 3138 ms after
  creation, with 0 not-available frames; near 0.1 m, far inf (an
  infinite projection); layer 0 fov [-54.0 40.0 44.0 -55.0] degrees
  (left, right, up, down); 720 acquires per 10 s, 0 errors.
- The diagnostic gray renders in both eyes with plausible near/far
  structure; orientation is for the self-check to settle.

**Cost** (Quest 3, `mode world`, 72 Hz, unworn on the desk, the room's
17 anchors located, 60 s runs with the first 5 s skipped; App GPU ms):

| Effect | Config | GPU ms med / p90 / max | fps (med) | Long | Stale |
|---|---|---|---|---|---|
| Flux XR Room 400K | baseline | 9.25 / 9.66 / 9.73 | 73 | 0 | 0 |
| Flux XR Room 400K | occluder | 9.36 / 9.68 / 9.99 | 73 | 0 | 0-1 |
| Flux XR Room 400K | occluder, second run | 9.39 / 9.70 / 10.04 | 73 | 0 | 0-1 |
| Flux XR Room 400K | diagnostic gray | 9.20 / 9.66 / 10.02 | 73 | 0 | 0 |
| Murmur XR World 40K | baseline | 10.11 / 10.80 / 12.41 | 73 (min 71) | 3 | 9 |
| Murmur XR World 40K | occluder | 10.39 / 10.71 / 11.75 | 72 (min 68) | 6 | 26 |
| Murmur XR World 40K | diagnostic gray | not run | | | |

The full-screen reprojection costs 0.1-0.3 ms, inside run-to-run noise;
no early-z gain shows unworn. `envdepth` is therefore on by default in
`mr` and `world`. A first Flux baseline run, taken before the room query
had returned anchors, read 6.00 ms with only the stage floor emitting:
that is the emitter weight, not the occluder, so it was rerun.

**Worn gate: pending.** An unscanned object, a person and the wearer's
own body hiding the cloud; no halo on head turns; the hands with
removal on (`envdepthhands 1`) against off.

| Depth map | Value |
|---|---|
| Size (per layer) | 320 x 320 |
| Swapchain length | 4 |
| Near / far (m) | 0.1 / inf (infinite projection) |
| First depth frame | 3138 ms after creation, 0 not-available frames |
| Hand removal supported / applied | yes / yes |
| Layer 0 fov (deg, l r u d) | -54.0 40.0 44.0 -55.0 |

## Live depth as a collision source (board #3352)

Phase 2 of #3324, route B'1 (#3350, #3351), then B'2: the environment
depth map reaches the world sim through the core particle sim's existing
obstacle texture slot. The sim's compute pipeline can sample one texture
for this (group 1 binding 2, RGBA8, declared by `particle_lib.wgsl` as
`obstacle_tex`). B'1 read the atlas back to the CPU and uploaded it (no
core change); B'2 copies it on the GPU, with one core change: the
accessors `ParticleSystem::obstacle_texture` / `obstacle_size` (its own
commit, for main). So:

1. **The atlas** (`env_depth.rs`, `ATLAS_WGSL`, CPU twin
   `encode_atlas`). Each frame with an acquired depth image, a compute pass
   condenses both layers into `W x 2H` RGBA8 texels in a storage buffer:
   layer 0 in rows `0..H`, layer 1 below, each in the map's own row order
   (row 0 the bottom of the view; `envdepthflipv 1` is honored by reading
   the map upside down). The distance along the depth camera's -Z over
   5 m is a 16-bit fraction: high byte in R (the 2 cm reading on its
   own), low byte in B. G is 255 where there is data; a texel without
   data, nearer than `envdepthnear` or at or past 5 m has G 0 (so the
   range never reads as a wall). A is 0, which keeps the core's
   luminance-to-alpha pass off the bytes. At the default 160 per side each
   texel is the nearest of its 2x2 block of the 320 map: surfaces grow,
   never shrink. Eight bits were the plan; with them the sim's gradient
   normals on a seat 0.8-2 m out tilted up to 28 degrees (p90 9), which
   the settle drift turns into a slide toward the viewer; 16 bits hold
   them under 1 degree at no cost (GPU test below).
2. **The copy (B'2).** In the frame's one command encoder, before the
   sims' dispatch, the atlas pass is followed by `copy_buffer_to_texture`
   into the world effect's obstacle texture (`obstacle_texture()`). Buffer
   rows are padded to the 256-byte copy alignment (160 texels = 640 bytes,
   padded to 768; 320 = 1280, aligned). The texture is checked every frame
   (the pinch-hold swaps in another effect with its own): at any size but
   the atlas's, `update_obstacle_webcam` with a zeroed image of that size
   allocates it and rebinds the sim's bind group once, then the copy
   fills it. No CPU round trip and no lag: the rows carry the same
   frame's poses, acquired before the sim's inputs are written.
   (B'1, measured below: three staging buffers mapped asynchronously, the
   atlas uploaded through `update_obstacle_webcam` a frame later.)
3. **The aux rows** (`WORLD_AUX_ROWS` 173 -> 180, `DepthCollide::rows`):
   `aux[173]` = (the stride as `u32` bits: low 16 `depthcollideevery`
   N, 0 when no atlas is in the texture, high 16 the frame's phase, frame
   mod N; thickness band m; range m; texels per side); per layer k, `aux[174 + 3k]` = camera
   position relative to the anchor and the near plane, `aux[175 + 3k]` =
   orientation quaternion, `aux[176 + 3k]` = fov tangents (left, right,
   up, down). An atlas older than 100 ms (the depth stopped), or none yet
   in this effect (a parked effect just swapped in), zeroes them.
4. **The collide** (`flux_xr_sim.wgsl`, `xr_depth_collide`, after the box
   collide). Particle `i` is tested only when `(i + phase) % N == 0`, so
   each is tested every N frames (N = 1: every frame). Layer 0, else
   (outside layer 0's view) layer 1: the first layer whose view holds the
   particle decides, with data or without. It collides when it lies
   between `margin` in front of the surface and `thickness` behind it
   (deeper is left alone: the occluder hides it). The normal is the depth
   gradient (central differences over the texel's four neighbors,
   unprojected), facing the camera; at a silhouette (a neighbor without
   data or 0.5 m away) the ray toward the camera. The particle moves out
   along the normal onto the plane through the point `margin` in front of
   the surface on its ray; the inward normal speed is reflected with the
   header's restitution, as for a box; the tangential speed is damped by
   0.9; an upward normal counts as resting (ages faster, like a table).
   `textureLoad`, not a filtered sample: interpolating across a
   silhouette would invent a surface between a person and the wall
   behind. The atlas side comes from the rows, not `textureDimensions`
   (the texture is sized before the first copy; the 1x1 placeholder's
   zeros read as no data anyway). Murmur ignores the rows.

Also in this pass, for the occluder: the lookup is edge-aware bilinear by
default (`envdepthfilter 1`): the 2x2 texels around the ray are
interpolated when their distances are within 0.25 m of each other, else
the nearest of those with data is taken. Within a surface the depth steps
go; a silhouette is still cut on the texel grid (moved half a texel out),
so `envdepthfilter 2`, bilinear across silhouettes too, is there as the
comparison. (Board #3402 made 2 the default after Kevin found the object
edges "very chonky" worn; see the next section.) `envdepthhands` is now live: polled once a second, applied
with `xrSetEnvironmentDepthHandRemovalMETA` on a change, and the log line
(`environment depth: hand removal asked on: the runtime answered
SUCCESS`) says what was asked and what the runtime answered.

```
adb shell setprop debug.fosfora.depthcollide 0|1          # world mode only; default 1 wherever the depth map is on; 1 implies the provider
adb shell setprop debug.fosfora.depthcollideres 160|320   # atlas texels per layer side (default 160)
adb shell setprop debug.fosfora.depthcollidethick 0.15    # the thickness band behind a surface, m
adb shell setprop debug.fosfora.depthcollideevery 2       # test each particle every N frames, 1..8 (default 2), up to N-1 frames late
adb shell setprop debug.fosfora.depthcollideupload 0|1    # diagnostic: 0 runs the atlas pass but skips its copy into the texture (nothing collides), default 1
adb shell setprop debug.fosfora.envdepthfilter 0|1|2      # occluder lookup: nearest texel | edge-aware | bilinear everywhere (default since board #3402)
adb shell setprop debug.fosfora.envdepthhands 0|1         # live: polled once a second
```

Logcat: the creation line gains `filter ...` and `depth collide on
(160x320 atlas, thickness 0.15 m, every 2 frames)`; every 10 s `depth
atlas: N copied into the obstacle texture (N after sizing it), N frames
without a texture to copy into, N not copied (depthcollideupload 0), N
frames without a depth frame · 160x320`.

**Tests (desktop).** Unit (`env_depth.rs`): the atlas encode (each layer
in its half, row order, the flip, the 2x2 nearest with holes, too-near
and out-of-range texels, R/G/B values, the spans), the aux rows' layout,
the filter on a synthetic 2x2; the atlas and occluder shaders validate
in naga. GPU (`tests/depth_collide_gpu.rs`, ignored like the core's
probes; lavapipe or any adapter): the atlas pass on a D16 array, copied
into an RGBA8 texture through the padded rows, matches its CPU twin at
160 and 320, upright and flipped; the Flux sim run
headless behind the loader's preamble with the core's bindings: a wall
1 m ahead over the lower half of the view stops 20K particles flying at
1 m/s (six frames in, at the margin bouncing back at the
restitution; forty frames in none behind it, all flying away) while the
upper half, without data, flies through; with the switch row off
everything flies through; with `every 2` about half bounce on the frame
the wall is reached and all of them two frames later; outside layer 0's
view layer 1 answers, and layer 0 in view over no data decides alone; the 1x1 placeholder collides with nothing; particles falling onto a
horizontal seat bounce up within 3 degrees of vertical and none falls
through (a few cm early along the ray where the seat is seen at a
grazing angle, the texel's nearest depth). The core's Flux and Murmur
world probes still pass.

**Cost** (Quest 3, `aa8fb45`, `mode world`, Flux XR Room 400K, 72 Hz,
unworn, the room's 17 anchors located, 60 s runs; App GPU ms; each
particle tested every frame, before `depthcollideevery`):

```
adb shell "setprop debug.fosfora.effect 'Flux XR Room'"
scripts/xr/sweep.sh --mode world --counts 400000 --hz 72 --seconds 60 --set "depthcollide=0"
scripts/xr/sweep.sh --mode world --counts 400000 --hz 72 --seconds 60 --set "depthcollide=1;depthcollideres=160;depthcollideevery=1"
scripts/xr/sweep.sh --mode world --counts 400000 --hz 72 --seconds 60 --set "depthcollide=1;depthcollideres=320;depthcollideevery=1"
scripts/xr/sweep.sh --mode world --counts 400000 --hz 72 --seconds 60 --set "depthcollide=1;depthcollideres=160;depthcollideevery=2"
```

| Config | Battery | App GPU med / p90 / max | CPU avg / max ms | fps (min) | Long | Stale | Atlas lag |
|---|---|---|---|---|---|---|---|
| collide off (occluder on) | 43 C | 10.28 / 10.98 / 11.95 | 1.59 / 2.5 | | 2 | 4 | n/a |
| collide on, 160 (same pair) | 43 C | 10.84 / 12.15 / 12.91 | 2.02 / 4.0 | 64 | 29 | 134 | 1 frame |
| collide on, 160 (earlier, cooler) | | 10.44 / 11.54 / 12.21 | | | | 35 | 1 frame |
| collide on, 320 (earlier, cooler) | | 10.59 / 11.66 / 13.49 | | | | 61 | 1 frame |
| collide on, 160, every 2 | | see `d39bb4c` below | | | | | |

**`d39bb4c`, back to back** (battery 42 -> 46 C over the three runs, GPU
clock 640 MHz throughout; atlas 678 uploads / 10 s, lag 1 frame, 0
failed):

| Config | App GPU med / p90 / max | CPU avg ms | Long | Stale |
|---|---|---|---|---|
| collide off | 10.38 / 11.01 / 11.79 | 1.67 | 9 | 14 |
| collide on, 160, every 1 | 11.21 / 12.14 / 12.74 | 2.07 | 41 | 209 |
| collide on, 160, every 2 | 11.27 / 12.27 / 13.29 | 2.06 | 56 | 270 |

Testing each particle every other frame changed nothing: the
per-particle test is not the cost. **The cost is the atlas plumbing:**
its own command encoder and submit per frame, the 200 KB
`write_texture` upload through `update_obstacle_webcam`, and the map and
poll on the CPU. Next pass: the atlas pass and its copy are recorded into
the frame's one command encoder (one submit per frame), the staging
buffer is mapped after it, and `depthcollideupload 0` (a diagnostic:
everything but the upload, so nothing collides) splits the pass from the
upload.

**`44243f0`, one submit, heat-adverse order** (43 -> 47 C: on with the
upload first, off last):

| Config | App GPU med / p90 / max | CPU avg ms |
|---|---|---|
| collide on, 160, upload | 11.48 / 12.29 / 13.38 | 1.91 |
| collide on, 160, `depthcollideupload 0` | 10.97 / 11.87 / 13.01 | 1.76 |
| collide off | 10.80 / 11.76 / 12.54 | 1.68 |

So the upload costs ~0.5 ms GPU and ~0.2 ms CPU, the atlas pass ~0.2 ms.
The single submit also made the read-back land 2 frames late (84-94
atlases superseded per 10 s). Kevin approved B'2, which removes the round
trip: the GPU copy above (`dc72ac5` core accessor, then the XR change).

**B'2 on the device** (`1122a85`, Flux XR Room 400K, `mode world`,
72 Hz, unworn, cooled to 40 C first, four 60 s runs back to back,
battery 40 -> 45 C across the series):

```
scripts/xr/sweep.sh --mode world --counts 400000 --hz 72 --seconds 60 --set "depthcollide=0"
scripts/xr/sweep.sh --mode world --counts 400000 --hz 72 --seconds 60 --set "depthcollide=1;depthcollideres=160"
scripts/xr/sweep.sh --mode world --counts 400000 --hz 72 --seconds 60 --set "depthcollide=1;depthcollideres=160;depthcollideupload=0"
```

| Run (in order) | App GPU med / p90 / max | CPU avg ms |
|---|---|---|
| collide on, 160 (GPU copy, every 2) | 9.20 / 9.69 / 10.06 | 1.63 |
| collide off | 9.27 / 9.78 / 10.54 | 1.56 |
| collide on, 160, again | 9.27 / 9.80 / 10.16 | 1.63 |
| collide on, 160, `depthcollideupload 0` | 9.36 / 9.73 / 10.46 | 1.60 |

Atlas: 721 copied into the obstacle texture per 10 s, 0 after sizing
(one sizing at start), 0 frames without a texture, 0 frames without a
depth frame. **The collision source now costs nothing measurable: at
most 0.1 ms GPU and +0.07 ms CPU**, against ~0.7 ms GPU, 0.4 ms CPU and
1-2 frames of lag with the CPU upload (B'1). Caveat: the room query
returned 0 anchors in all four runs (the headset had just woken from a
doze), so only the stage floor emitted (~330K alive) and the absolute
numbers sit below the located-room baseline of 9.25-9.38 ms; the pairs
are consistent. Stale frames were 179-235 in every run, collide off
included, so they are not the feature's.

The atlas path works on the device: 709 uploads per 10 s, lag exactly 1
frame, 0 failed, 0 skipped. Hand removal: the creation line read
"supported true, asked false" with `envdepthhands 0`. **The collide
costs about 0.6 ms of App GPU at the median, 1.2 ms at p90, and 0.4 ms
of frame-loop CPU** (likely the atlas's 200 KB read back, copied again by the
core's alpha check and uploaded); on a hot device that is enough for 134
stale frames a minute, which judders worn. Heat moves the baseline by a
millisecond: collide off read 9.25 ms at 35 C this morning (the phase 1
table) and 10.28 at 43 C for this pair, so compare only back-to-back
pairs. Hence `depthcollideevery` (default 2) and the per-particle
trims: the side from the rows instead of `textureDimensions`, no layer 1
attempt once layer 0's view holds the particle, the stride test before
any other work and the fov rows and collide header read only when they
are needed.

**Worn gate: pending.** Embers land on a hand-held object, a person and
an unscanned chair; they slide off a shoulder; none trapped inside
people; occluder edges with `envdepthfilter` 1 against 2; hand removal
with `envdepthhands` 0 against 1 (live).

## Particle pitcher and the panel (board #3402)

Kevin's worn verdict on the depth collide: "overall the interactions were
working well", but from inside a 400K cloud he could hardly judge them,
and the occluder's object edges were "very chonky". So: a stream he can
aim, a way to thin the cloud while he does, a denser panel, and softer
edges.

**The pitcher** (`instruments.rs`, `Pitcher`). Turned on from the hand
menu's Pitcher toggle (not saved across launches), the right far palm
(Go-Go reach applied; the left holds the menu) pours `rate x dt`
particles a frame, the fraction carried over (4000/s at 72 Hz: 55 or 56
a frame), born within 2 cm of the palm and flying along the palm normal
at the pitcher's speed, within a 6 degree cone. He aims by turning the
palm; no pose or pinch. A pour particle has the burst look (twice the
sprite, opacity 0.35, half way to warm white, so the stream reads
through the cloud) and the full lifetime (a throw's live half), is free
of the volume like a burst, and collides with the room's boxes and the
live depth map like any particle: under the presets' drag (0.98 a 60 Hz
frame) a 1.5 m/s stream poured level carries at most about 1.2 m before
the settle drift (0.5 m/s) takes it down; it lands, slides and rests. The debug panel's "pitcher /s" stepper sets the
rate; the pour is not scaled by the cloud density.

**The rows.** The pour rides the burst rows (row 170 x the count, row 171
the far palm and the 2 cm nozzle, row 172 w brightness 1) plus a row of
its own after the depth rows: row 180 = the pour's direction (unit, the
palm normal) and w its speed (`XR_AUX_POUR`; `WORLD_AUX_ROWS` 181). With
its w > 0 `xr_burst` gives the newborn the pour's velocity, uniform over
the cone's cap (`XR_POUR_SPREAD`, a shader constant: the burst rows have
no free lane), and the full lifetime; with w = 0 a burst is a throw's,
unchanged. A throw's burst wins the rows: the pour drops that frame's
particles and counts it.

**The cloud density** (the debug panel's "cloud density" stepper, 0.05
to 1 by 0.05): the showing world effect's emission rate is set to the
rate `new_world` gave it (the preset's, scaled with the count) times the
density, when it changes and when a pinch-hold swaps in an effect last
set for another value. The cloud thins or refills over a lifetime (12 s).

**The panel** (`panel_grid.rs`, `hud.rs`; texture unchanged, 640 x 1472
at 1.6 px per point on the 20 x 46 cm quad). The controls are a block at
the bottom of what the quad shows: rows 34 pt (1.7 cm) tall, 6 pt apart
(before: 50 pt, the width of the panel each). From the top: Prev/Next
(full width); the nine steppers two to a row, each cell half the width
with its own -/+ boxes (40 pt, before 64) and its label (12 pt) over its
value (16 pt); Recenter and Rescan; Pitcher and Debug, the bottom row of
both the debug panel and the hand menu. The pointer picks a row by its
height (each row owns half the gap on either side, so the rows tile the
block), a column by the side of the panel's center, then a half of the
cell; press, lock, repeat and the show/hide hysteresis are unchanged.
The bottom row sits 10 pt above the visible bottom edge in the 84 pt
menu strip (4.2 cm, before 6.2) and in the full panel alike, so it stays
under the pointer when debug toggles (before, matched by eye: 64 against
63 px). Nothing is under 12 pt: the title is 15 (was 17), the graph's
labels 12 (were 11), the header lines egui's body text (12.5, the audio
bars' names monospace 12). Widths with egui
0.33's font: the widest label, "reach 1:1 within m", 97 pt at 12 pt in a
107 pt middle; "Recenter the cloud" 135 pt at 16 in a 187 pt cell. The
header (about 250 pt) ends well above the controls' top (593 pt with all
eight rows); if it ever runs into them the app logs it once.

**Soft occluder edges.** `envdepthfilter` now defaults to 2: bilinear
over the 2x2 texels across silhouettes too, so an object's edge is
blended instead of cut on the 320 x 320 map's grid. 1 (edge-aware) stays
for the A/B.

```
adb shell setprop debug.fosfora.pitcher 0|1          # the pitcher at launch (default 0); the hand menu's Pitcher toggle after
adb shell setprop debug.fosfora.pitcherrate 4000     # particles per second, 500..10000 (the panel's "pitcher /s")
adb shell setprop debug.fosfora.pitcherspeed 1.5     # the stream's speed, m/s
adb shell setprop debug.fosfora.pitchertest 1        # diagnostic: pitcher on, pouring from 0.5 m ahead of the view, 30 degrees down, untracked
adb shell setprop debug.fosfora.density 1.0          # the cloud density, 0.05..1 (the panel's "cloud density")
adb shell setprop debug.fosfora.envdepthfilter 0|1|2 # occluder lookup: nearest | edge-aware | bilinear everywhere (default)
```

Logcat: the instruments line gains `pitcher on|off (4000/s at 1.5 m/s)`;
`pitcher on|off` on the toggle; every 10 s while on `pitcher on at
4000/s, 1.5 m/s: N frames pouring, N particles asked, N frames skipped
for a throw (10 s)`; `cloud density 0.50: 'Flux XR World' emits 26666/s
(preset 53333/s)` on a change.

**Tests (desktop).** Unit (`instruments.rs`): the pitcher pours its
rate to the particle at 72 and 90 Hz in frames of the whole part of
rate x dt, a 500/s pitcher on some frames only; off or untracked nothing
and nothing carried; a throw's frames are dropped and counted; the rows
it packs. `panel_grid.rs`: the rows tile the block bottom up, the bottom
row sits as far above the edge in the menu and the panel, a pair row's
cells, halves and end boxes, no font under 12 pt, the header's room.
GPU (`tests/depth_collide_gpu.rs`, ignored): 200 a frame for 30 frames
from the anchor straight down onto a floor box 1 m below, with the
presets' drag and lifetime and the settle drift: the newborns fly at
1.5 m/s within 6 degrees and live 12 s; each lands no wider than the
nozzle plus 1 m x tan 6 degrees; 2.5 s in all 6000 rest the margin above
the box, none below; the same rows with row 180 zero give the throw's
outward ball at half the lifetime. The core's throw and lift tests pass
unchanged.

**Cost** (Quest 3, `f78b00d`, Flux XR Room 400K, full cloud, `mode
world`, 72 Hz, unworn with `pitchertest 1`, 40 s runs, battery 31-37 C;
App GPU ms):

| Pitcher | App GPU med / p90 / max | fps | Long | Asked per 10 s |
|---|---|---|---|---|
| off | 9.62 / 10.29 / 10.41 | 73 | 0 | |
| 4000/s | 9.89 / 10.61 / 11.32 | 72 | 6 | ~40100 (720 frames pouring, 0 skipped for a throw) |
| 20000/s | 12.21 / 14.07 / 15.17 | 66 (min 56) | 44 | ~209K-226K |

4000/s costs about 0.3 ms. 20000/s into a full cloud is over budget:
with ~398K alive there are almost no dead slots, so every frame the pour
takes its particles from the living through the steal path (row 170 w),
and its sprites are twice the cloud's. The rate is therefore capped at
10000/s (stepper 500..10000 by 500, the knob clamped the same); above
about that into a full cloud the steal path costs frames, and the
intended use is with the cloud density lowered, which leaves dead slots
for the pour. Screencaps: a bright white-yellow stream reading clearly
through the cloud, pouring down and forward and spreading on the floor;
the panel's two columns legible with every stepper and the paired rows
in place. Stale frames were high in every run, pitcher off included
(the headset had just woken from a doze), so they are not the
feature's.

**Worn gate: pending.** The stream pours from the right palm where it
points; it lands on a chair, a person and the left hand and slides off;
the cloud thins with the density stepper; the panel's two columns are
legible and every stepper still steps (-/+ and the held repeat); object
edges with `envdepthfilter` 2 against 1, the stream at a chair's edge.

## Space size (board #3325)

A conceptual zoom of the space the cloud lives in: the half extent of the
cube around the anchor that the world effect's particles live in and
respawn out of (the presets' `emitter.radius`: 1.5 m in Flux XR World,
Flux XR Room and Murmur XR World, the 3 m volume of board #3276; 1 m in
Flux XR World Coarse), set live from the debug panel or a knob.

**What it scales.** The world sims read the volume from the core uniform
`emitter_radius`, which the particle system copies from its def every
update; `XrScene::set_space_half` writes the def's radius (no core
change) and the half the surface weights are taken against. Flux spawns
uniformly in the new cube, respawns a particle that leaves it and fades
opacity over its outer 30 %; a surface emits only when its top face
reaches into the cube, so a table or wall outside the volume stops
emitting and one inside starts, the next frame. Changing the size does
not reset the cloud: a particle outside a smaller cube respawns inside on
its next step, a larger one fills over a lifetime (12 s) at the same
emission rate, so the same count spreads thinner (8x the volume at twice
the half extent).

**What it does not.** The near fade (0.15 m around the head), the settle
drift, the hand pad and kick, the Go-Go reach and a throw's or the
pitcher's particles (free of the volume, `XR_FREE_REACH_M` 13 m) do not
read the radius. Nor does the particle count or the emission rate: the
cloud density stepper stays the way to thin it.

**Murmur.** The 3D spatial hash spans +-`emitter_radius` with a fixed
cell count, so a larger volume means larger cells and, with
`MAX_PER_CELL` capped at 16, a coarser neighbor sample in a dense flock;
the cruise speed, the audio predator's reach and the roost's pull are in
half extents too (so the flock keeps its proportions), so the flock flies
faster and wider in a larger volume. The hash is unchanged.

**The control.** The debug panel's tenth stepper, "space half m", 0.5 to
6 m by 0.25, fills the empty cell of the fifth pair row, so the panel
keeps its eight rows and its texture. Until the knob or the stepper asks
for a size each effect keeps its preset's, and the stepper shows it (1.50
for Flux XR Room); once asked, the size holds for every effect a
pinch-hold swaps in (each scene stores what it was asked for and the app
applies the asked size to the one showing, as the cloud density does).

```
adb shell setprop debug.fosfora.space 1.5   # the half extent, m, 0.5..6, every world effect; unset keeps each preset's
```

Logcat: `space half 1.50 -> 3.00 m` from the scene and `space size:
'Flux XR Room' at 3.00 m half extent` from the app on each change or
swap; `space half ... (re-applied)` if a rebuild of the particle system
reset the def to the preset's.

**Tests (desktop).** `space.rs`: the presets kept until asked, an asked
size applied to a swapped-in effect and shown on the stepper, the knob
clamped to the range (0 unset), the write-back after a rebuild, the floor
of 0.05 m. `surfaces.rs`: a table whose top face starts 2 m out weighs 0
at half 1.5 and 0.5 at 2.5; the desk emits at 0.75, 1.5 and 2.5.
`panel_grid.rs`: the debug panel's row count derives from the stepper
count (`hud.rs` checks it at compile time) and is still 8, and the
tiling, cell and font tests pass unchanged.

**Worn gate: pending.** At 0.75 m the cloud hugs the wearer and the desk
still emits (its top face is inside); at 3 m it fills the room and embers
reach the far wall's spectrum (a wall reaches into the cube only within
the half extent of the anchor on each axis: the far wall of board #3327
was 4.7-5.25 m ahead, which needs about 5 m); the change is live with no
reset of the cloud; the panel still fits and the new stepper steps (-/+
and the held repeat).

**Cost** (Quest 3 right after a reboot, `8895ed7`, `mode world`, 72 Hz,
unworn, 40 s runs, battery 41-43 C; App GPU ms). The room query returned
0 anchors in these headless runs, so only the stage floor emitted:

| Effect | Space half | App GPU med / p90 / max | fps | Notes |
|---|---|---|---|---|
| Flux XR Room 400K | preset (1.50) | 9.20 / 9.74 / 9.88 | 73 | 0 stale |
| Flux XR Room 400K | 0.75 m | 9.96 / 10.06 / 10.13 | | the floor left the volume (emitter weight 0.00); 400K alive packed in the small cube |
| Flux XR Room 400K | 1.5 m | 9.25 / 9.62 / 9.69 | | |
| Flux XR Room 400K | 3 m | 9.33 / 9.64 / 9.73 | | |
| Flux XR Room 400K | 6 m | 9.29 / 9.58 / 9.66 | | |
| Murmur XR World 40K | 1.5 m | 10.33 / 11.00 / 11.62 | 69-73 | 5 long |
| Murmur XR World 40K | 3 m | 10.17 / 11.39 / 11.62 | | 7 long |

The control itself costs nothing: 1.5 m set through the stepper's path
matches the preset, and 3 and 6 m are within run-to-run noise of it. A
small volume costs about 0.7 ms at 0.75 m, and that is density, not the
control: the same 400K sprites packed into 1/8 of the volume overdraw
more. Murmur is unchanged at 3 m (the median slightly lower, the p90
within noise). An earlier sweep on a degraded runtime (14 ms at every
size) was the device, not the build, and is not recorded here.

## Room preset editor, step 1: the surface lane (board #3326)

Option B of the editor (`ROOM_DESIGN.md`, "The room preset editor"), the
first of its two steps: every room surface runs a **behavior** chosen per
surface instead of the fixed rule per kind, keyed by the anchor's UUID,
saved per room, restored when the room's anchors come back. Step 2 (the
beam pick, the highlight and the panel page) is separate and carries the
worn gate; this step's gate is unworn by design: a knob that assigns a
behavior over adb and a GPU test that a lane value moves the emission to
the right box.

**What was built** (`xr-room-lane`, board #3444). A lane block of 32 rows
after the pour row (aux 181 to 213, one `vec4` per obstacle box: behavior
id + 1 with 0 = unset, strength, two spare parameters), the Flux world sim
gated by the lane (embers on the beat gate, sparks on the bass gate,
anything else closed) with the kind rule as the default for an unset
lane, the emitter weights taken for the same behaviors, the anchor UUID
kept from `xrRetrieveSpaceQueryResultsFB` and carried on each box, the
room id (FNV-1a 64 of the sorted UUIDs), `rooms/<room id>.json` under the
config dir with kind defaults and per-anchor entries, the wall spectrum
restricted to walls on `spectrum` and the floor ripple pinned by a floor
on `ripple`, and `debug.fosfora.surface`. Desktop tests cover the file,
the room id, the knob grammar, the weights and the lane row; five GPU
tests in `tests/depth_collide_gpu.rs` run the sim with lanes set and
unset (a mutation that puts the kind rule back fails four of them).

```
adb shell setprop debug.fosfora.surface "dc83ba94=none,wall=none,87277f7a=spectrum@0.7"
adb shell setprop debug.fosfora.surface "table=none"     # the class: the kind default and every table
adb shell setprop debug.fosfora.surface "#7=ripple"      # a box by index, the stage floor too
adb shell setprop debug.fosfora.surface clear
```

**On the Quest 3** (`61a2273`, v207, `mode world`, Flux XR Room 400K, 72
Hz, unworn on the desk, Sep 29). The query returned 19 results, 17
anchors (7 planes, 10 volumes), room id `a03160e5a4a3b311`; the lane table
logged once per rebuild names every box:

```
room a03160e5a4a3b311: 0 TABLE dc83ba94 table embers 1.00 | 1 STORAGE a5438e6b other none 1.00 |
  2 WALL_FACE 87277f7a wall spectrum 1.00 | ... | 7 FLOOR 36614aec floor sparks 1.00 |
  8 WINDOW_FRAME 57898c62 frame none 1.00 | 10 CEILING 41d44ee8 ceiling none 1.00 | ... |
  16 WALL_FACE 4fd306df wall spectrum 1.00 | 17 stage floor 00000000 floor sparks 1.00
```

- `table=none`: the summed emitter weight fell from 0.99 to 0.50 (the
  five tables off, the floor's 0.5 left); `clear` put 0.99 back.
- `floor=ripple`: "floor ripple: on box 7 (pinned by its lane)", the
  weight 0.99 to 0.49 (the floor's sparks off, the tables left).
- `wall=none,8f7ada5c=spectrum`: "wall spectrum: on box 12 (1 walls)",
  the other three walls dropped. A wall pinned behind the head
  (`87277f7a`) gave "no wall": the hysteresis pick still refuses a wall
  the wearer faces away from, so a pin behind the chair draws nothing
  until the wearer turns.
- `dc83ba94=none,wall=none,87277f7a=spectrum@0.7`: three log lines, one
  per assignment, and a file with five entries (four walls, one table)
  and `"wall": "none"` as the kind default. A relaunch logged "loaded
  rooms/a03160e5a4a3b311.json (5 anchors assigned)" and the same lane
  table, before the anchors had all located (2/17 on the first pass).
- `zz=drips`: "'zz' is not a UUID (at least 8 hex digits), #<index> or a
  kind; nothing applied". Reserved names are refused the same way.
- With the room off (`room 0`): the stage floor takes `#0=…` and the log
  says "not saved (no room)".

Unworn, the scene query alternates between 17 anchors and 0 across
launches (0 from 12:07 to 12:12, 17 before and after) and the located
count climbs over the first seconds (2, 8, 16 of 17). The knob waits for
the anchors while the room is on, so a value set before the room is back
applies once it is.

**Cost** (App GPU med / p90 / max in ms, 60 s runs, the first 5 s
skipped; the room query returned 0 anchors in the sweeps, so only the
stage floor emitted, as in the space size sweep):

| Build | Effect | Device state | App GPU med / max | long | stale |
|---|---|---|---|---|---|
| `abb9e1c` (before) | Flux XR Room 400K | 32 C, 2 h up | 9.65 / 10.22 | 0 | 138 |
| `61a2273` (lanes) | Flux XR Room 400K | 35 C, back to back | 9.55 / 10.39 | 0 | 169 |
| `61a2273` | Murmur XR World 40K | 40 C | 11.89 / 14.60 | 83 | 558 |
| `61a2273` | Flux XR Room 400K | right after a reboot, 42 C | 9.05 / 9.89 | 0 | 0 |
| `61a2273` | Murmur XR World 40K | after the reboot | 10.18 / 11.95 | 2 | 5 |

The lane costs nothing measurable: back to back with the previous build
Flux XR Room is within noise (9.65 to 9.55), and after a reboot both
effects match this morning's pre-lane numbers on the same device state
(board #3420: Flux XR Room 9.20 / 9.88, Murmur 10.33 / 11.62). The warm
Murmur row (11.89 at 40 C, 83 long frames) is the device's heat, not the
build: the same build gave 10.18 after the reboot, and Murmur reads no
lane (its sim stops at row 170; the only change for it is 512 bytes more
in the per-frame aux upload).

**Worn gate:** none for this step (step 2's: point at the desk and turn
its embers off and on, put the spectrum on the side wall, relaunch and
find the room as left).

## Room preset editor, step 2: point, pick, assign (board #3326)

The second step of option B (`ROOM_DESIGN.md`, "The room preset editor"):
an **Edit room** mode on the hand menu. With it on, the right far hand's
ray (the throw's: from the head through the far pinch point, so the reach
extension puts far walls within pointing range from the chair) casts
against the room's boxes, the hit face carries a bordered tint, a pinch
advances that surface's behavior one step through the catalogue (none,
embers, sparks, spectrum, ripple), a pinch-hold applies it to every
surface of its kind, and the menu's status cell names the pointed surface
and its behavior ("desk: embers"). The assignments go through step 1's
lanes and room file. The right hand's tap, hold and drag belong to the
editor while the mode is on; the left hand and the panel are unchanged.
The hit has hysteresis (0.15 s to take a new box, 0.3 s to drop a miss).

```
adb shell setprop debug.fosfora.editroom 1   # the mode on at launch (the menu's toggle otherwise)
adb shell setprop debug.fosfora.picktest 3   # diagnostic: the ray 0.5 m ahead of the head, 20 degrees down, a tap every 3 s
```

**Unworn on the Quest 3** (`3ff2de2`, v207, `mode world`, Flux XR Room
400K, 72 Hz, the headset on the desk facing the room, Sep 29):

- With the room's 17 anchors, `picktest 3` logged "pointing at storage 9
  (STORAGE 037a4a92) at (-3.24, 0.21, -1.89)" and then, every 3 s, the
  cycle in order: embers, sparks, spectrum, ripple, none, embers, ..., each
  with the lane module's own line and a save of
  `rooms/a03160e5a4a3b311.json`. A tap with nothing hit logs "tap with no
  surface under the beam, nothing changed" and touches nothing.
- The highlight shows in screencaps: a bright rectangular border framing
  the storage unit's face the ray hit, the fill a shade lighter, with the
  environment depth occluder on and off alike (the real unit and its
  anchor box coincide, so the decal a centimeter off the face survives
  the live depth). The beam is edge-on from the head in this diagnostic,
  so it does not show; worn, it runs from the far hand to the hit.
- The hit wandered between wall 5, table 14 and floor 7 within seconds in
  one run: the unworn head pose drifts (the runtime's tracking with the
  headset lying still), and the hysteresis absorbed the flicker. To be
  judged worn.
- Friendly names read "storage 9", "wall 5", "table 14", "floor 7": the
  runtime's label in words, the box index appended where the room has
  several of a kind.

**Cost** (App GPU med / max ms, 60 s, right after a reboot, 41-42 C; the
room query returned 0 anchors in these runs, so the ray had only the
stage floor to miss and the highlight was not drawn):

| Setting | App GPU med / max | long | stale |
|---|---|---|---|
| editor off | 8.84 / 9.74 | 0 | 0 |
| editor on, `picktest 3` | 8.90 / 9.78 | 0 | 0 |
| editor on, idle | 9.05 / 9.85 | 0 | 0 |

The cast, the beam and the mode cost nothing measurable. The highlight is
one quad through the ripple's pipeline; a warm pair with it drawn (40 C,
after 30 min of runs) was inside the device's thermal noise (10.28 with a
97 ms stall and 124 long frames off, 11.02 with 33 long frames on) and is
not evidence either way. The hand menu grew by one row (84 to 124 points)
in every mode.

**Worn gate (Kevin):** point at the desk and turn its embers off and on;
put the spectrum on the side wall; relaunch and find the room as left;
the border and the status cell read with one eye.

## Surfaces as effects, D1: the surfaces pass (board #3472)

The first step of `SURFACES_DESIGN.md`: one render pass that draws a
fragment effect on each room surface's acting face, one uniform slot per
box and one shader switching on the surface's behavior; two shaders,
**rings** (the floor ripple folded in, now on any face) and
**streamlines** (lines of light flowing along a curl-noise field, the
mockup's tabletop); a per-kind palette; the catalogue widened so tables and
storage cycle none, embers, sparks, streamlines, floors none, sparks,
rings, streamlines, walls none, spectrum, streamlines, rings.

```
adb shell setprop debug.fosfora.surface "table=streamlines,wall=streamlines,other=streamlines,ceiling=rings,frame=rings"
```

(The property holds 91 bytes: `floor=rings` is the default and `frame`
can be left out to fit.)

**Cost** (`ad9f850`, Quest 3 v207, `mode world`, Embers 400K with
Particles off so the sprites do not draw, 72 Hz, unworn, 45 s runs at
37-38 C, App GPU med / max ms):

| Lit faces | App GPU med / max | long | stale |
|---|---|---|---|
| none (the stage floor's rings alone, no room) | 8.84 / 9.81 | 56 | 52 |
| 1 (stage floor rings) | 8.77 / 9.70 | 47 | 54 |
| 6 (5 tables streamlines, the floor rings; the room's 17 anchors) | 8.75 / 10.30 | 71 | 124 |
| every face lit | not measured: the query returned 0 anchors in eight launches after the 6-face run |
| none again | 8.97 / 10.05 | 27 | 21 |

Six faces lit (five tabletops of streamlines and the room's floor of
rings) cost nothing measurable against none: the pass is fill, and the
tabletops are a small part of the eye buffer. The every-face number, with
four walls and the ceiling covering most of the view, is the one the
design's gate (under 2 ms) is about, and it needs the room's anchors,
which the unworn headset stopped returning; it is taken worn, from the
debug panel's GPU graph, or on the next unworn run that has them.

**Look:** unworn screencaps show only the stage floor (no room), so the
streamlines' look on a table is judged worn, first by Kevin.

## Surfaces as effects, D2a: the catalogue (board #3488)

The catalogue grown to curls (6) and pulse (7), the wall spectrum folded
into the surfaces pass as shader 3, the mockup's room as the per-kind
defaults, the lane's spare parameters as color and band, the room file at
version 2, a scan label. The slot is 34 rows (544 bytes), 32 slots.

**Per shader on the stage floor alone** (`0074f9d`, Quest 3 v207, `mode
world`, Embers 400K with the cloud off, `room 0` so the synthetic stage
floor is the only surface and fills the lower half of the view from the
desk, music on, 72 Hz, unworn, 30 s runs, 28 to 38 C rising through the
list, App GPU med / max ms):

| Stage floor | App GPU med / max | over none |
|---|---|---|
| none | 4.88 / 4.99 | |
| rings | 5.65 / 5.75 | 0.8 |
| streamlines | 6.58 / 6.75 | 1.7 |
| curls | 7.17 / 7.34 | 2.3 |
| pulse | 5.41 / 5.60 | 0.5 |

The stage floor from the desk is the largest face the pass will ever
draw, so these are per-shader worst cases for one face, in the order the
shaders' per-pixel work predicts (curls: three noise octaves, the streaks
and the fill; streamlines: two octaves and the streaks; rings: eight
gaussians; pulse: one falloff). Curls over a full view costs more than the
design's 2 ms room budget on its own; on a chair seat it covers a few
thousand pixels. All five runs held 72 fps with 0 long and 0 stale frames.

**Every face lit** (`bca4618`, the same setup with `room 1`, 45 s runs,
30 to 35 C, after Kevin wore the headset for a minute so the room's 17
anchors came back; App GPU med / max ms):

| Room | lit | App GPU med / max | stale |
|---|---|---|---|
| 17 anchors, every kind none | 0 | 7.36 / 7.60 | 32 |
| 17 anchors, every kind on a shader (`table=streamlines,floor=rings,wall=spectrum,other=curls,frame=pulse,ceiling=rings`) | 14 (rings 2, streamlines 5, curls 5, pulse 1, spectrum 1) | 8.36 / 8.67 | 61 |
| 0 anchors (the query flaked again), every kind none | 0 | 7.03 / 7.32 | 57 |

Fourteen faces lit, five of them curls, cost **1.0 ms** over the same room
with nothing lit: inside the design's 2 ms gate. The 17 anchors as
occluders and depth cost about 2 ms over no room (7.36 against 5.21),
which is the room's known price, not the pass's. The three earlier
no-anchor runs (5.21, 5.31, 5.42) say the pass costs nothing with no
face to draw. The version 1 room file loaded with its kind defaults
dropped, as logged ("version 1: its kind defaults give way to the
built-ins").

**Look (unworn screencaps, stage floor):** the streamlines as D1; curls
draw as a violet wash of small closed loops with the fill between them;
the rings as the ripple, faint; the pulse reads as a flat violet rectangle
over the whole floor, paint rather than light, which on a frame-sized face
may be right and on a floor is not. Judged worn.

## Surfaces as effects, D2b: the desktop's fragment effects on faces (board #3489)

Eight single-pass desktop effects (aurora, prism, shards, astrolabe,
bezel, fenestra, reticle, tessera; ids 8 to 15) composed through the
core loader's preamble with their fragment entry wrapped for a face, one
pipeline each, drawn by the surfaces pass.

**On the device** (`98b1deb`, Quest 3 v207, right after a reboot): the
eight pipelines validate and compile on the Adreno 740 with group 0 as
the uniform alone, "surface ports: 8 pipelines in 339 ms" at launch, and
each draws ("surfaces: 1 lit (… ports 1)").

**Per effect on the stage floor alone** (`mode world`, Embers 400K with
the cloud off, `room 0`, music on, 72 Hz, unworn, 30 s runs, App GPU med
ms). The first pass ran the eight in a row from a cold headset (28 C):

| Stage floor | App GPU med | over the opening none (4.75) |
|---|---|---|
| aurora | 6.22 | 1.5 |
| prism | 6.07 | 1.3 |
| shards | 5.98 | 1.2 |
| astrolabe | 6.44 | 1.7 |
| bezel | 6.41 | 1.7 |
| fenestra | 8.94 | (drift, see below) |
| reticle | 7.28 | (drift) |
| tessera | 6.10 | (drift) |
| none again | 6.46 | |

The closing none was 1.7 ms above the opening one (36 C by then, stale
frames from 0 to 64): the baseline stepped up partway through, so the
last rows were rerun interleaved with baselines at 36 to 40 C:

| Run | App GPU med | over its neighbors' none |
|---|---|---|
| none | 6.70 | |
| fenestra | 9.00 | 2.2 |
| none | 6.88 | |
| reticle | 8.11 | 1.2 |
| none | 6.90 | |
| aurora | 8.23 | 1.2 |
| none | 7.18 | |

So a ported effect costs 1.2 to 1.7 ms over a full-view face and
fenestra 2.2, the range of the streamlines (1.7) and the curls (2.3):
the same order as the purpose-built shaders, as fill should be. Every
run held 72 fps with 0 long frames (1 in the last none).

**Every kind on ported effects, the room on:** not measured. The scene
query returned 0 anchors in five 45 s runs, and by then the headset sat
at 41 to 43 C with 24 to 74 long frames a run on every row, the none
rows included (7.75, 7.92, 8.20), so the numbers say nothing about the
pass. Taken on a cool headset that has the room's anchors.

**Look:** unworn, the headset lay low with the stage floor mostly behind
the live depth occluder; aurora, prism, shards and tessera show as light
on the strip of floor in view with the real floor showing through their
dark parts, bezel and reticle (drawn at the face's edges and center) are
out of the view. Judged worn.
