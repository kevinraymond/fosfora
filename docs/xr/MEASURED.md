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
  stays put while moving and turning. (Monocular check; Kevin sees with one eye.)
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
| Audio-to-photon latency (ms, method) | **Visual leads the sound by about 115 ms on the playback path** (filmed, commit `74eb646`+: `debug.fosfora.file click` plays a click track through the headset speakers, `debug.fosfora.flash 1` turns the view white for two frames on each detected beat; phone video with sound, `scripts/xr/latency.py` fits both event grids). 120 BPM, 60 fps: flash minus click 384 ms mod 500 = −116 ms, 46 flashes, circular jitter 18 ms. 100 BPM, 30 fps: 487 mod 600 = −113 ms, 30 flashes, jitter 24 ms. The two grids agree, which resolves the modulo. The app's own log puts the pulse 42–47 ms after the click samples are handed to AAudio (162–164 beats, R 0.96–0.97), so the display chain is short and the sound is late: implied AAudio output latency about 190 ms (default performance mode, cpal). Mic-path latency not filmed (it needs the external mic's output routing fixed first, board #3251); expected positive, capture latency + the same 42–47 ms + display |

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
- **Second film pending** for the 84 ms buffer: expected flash − click
  within the phone's ±17 ms of zero.

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
