# `crates/fosfora-xr`: technical design

This is a design, not a spec of verified API calls. Everything marked **VERIFY**
comes from memory of the ecosystem or from older examples. Check it against
current sources (crate source in `~/.cargo/registry`, Meta's native docs, the
OpenXR spec) before relying on it, and log what you find in blackboard.

## Crate shape

```
crates/fosfora-xr/
  Cargo.toml
  src/
    lib.rs            android_main entry; wires everything together
    xr/
      loader.rs       Android loader init, Entry
      instance.rs     instance, system, required/optional extensions
      session.rs      session + lifecycle state machine
      swapchain.rs    per-eye swapchains → wgpu textures
      frame.rs        wait/begin/locate/render/end
      input.rs        hand tracking, pinch, (later) eye gaze (S7)
      room.rs         passthrough layer, scene anchors as obstacles (S7)
    gfx/
      interop.rs      OpenXR Vulkan instance/device → wgpu (S2)
      eye.rs          per-eye view/projection from XrView (glam)
      quad.rs         world-locked textured quad (S4)
      particles3d.rs  world-space scatter path + test sim (S5)
    platform/
      assets.rs       APK asset extraction → internal storage (S4)
      audio.rs        AAudio mic + file playback feeding the ring buffer (S6)
android/              minimal Gradle app project (NativeActivity, hasCode=false)
scripts/xr/run.sh     build → install → launch → logcat
```

**Dependencies** (versions current as of Sep 2026; **VERIFY** each on crates.io):
- `openxr` with the `loaded` feature. It dlopens `libopenxr_loader.so`, which we bundle.
- `ash`, the same major version wgpu-hal 27 uses (check `Cargo.lock` after
  adding wgpu with Vulkan) so the raw handle types line up.
- `wgpu = "27"`, the same version as `fosfora-app`. Use `wgpu::hal` for interop;
  **VERIFY** the feature flags needed to expose the Vulkan hal on Android.
- `android-activity` with `native-activity`; `android_logger`; `log`; `glam = "0.29"` (matches core).
- `fosfora-app = { path = "../fosfora-app", default-features = false }` (after S3).
- Later: `jni` for runtime permissions.

**Build:** `cargo ndk -t arm64-v8a -o android/app/src/main/jniLibs build -p fosfora-xr`,
then `./gradlew assembleDebug`. Only arm64-v8a; the glasses test page requires
every native library under `arm64-v8a`, with no 32-bit-only dependencies.

## Threads

| Thread | Owns |
|---|---|
| Main (android_main) | Android event polling, OpenXR frame loop, all wgpu submission |
| `fosfora-audio` (existing) | Analysis chain; hands `AudioFrame` to the render side through the existing bounded channel |
| AAudio callbacks | Capture and playback; write only to the SPSC ring buffer (no DSP in callbacks, as on desktop) |

Render is single-threaded for the spike. Don't add a render thread until the
measurements say the CPU side is the bottleneck.

## OpenXR bring-up (Android)

1. **Loader init.** Call `xrInitializeLoaderKHR` with
   `XrLoaderInitInfoAndroidKHR` (JavaVM and activity from `android-activity`)
   before anything else. *Verified (S1):* `openxr` 0.22 wraps it:
   `Entry::load(&AndroidPlatformInfo::new(vm, activity))` runs the loader init,
   and `create_instance(.., &platform)` chains `XrInstanceCreateInfoAndroidKHR`.
2. **Instance.** Required: `XR_KHR_vulkan_enable2` and
   `XR_KHR_android_create_instance` (chain `XrInstanceCreateInfoAndroidKHR`).
   Optional, enabled only if the runtime lists them, each behind a runtime check:
   - `XR_EXT_hand_tracking`, `XR_FB_passthrough`, `XR_FB_display_refresh_rate`
   - `XR_FB_scene`, `XR_FB_spatial_entity`, `XR_FB_spatial_entity_query`,
     `XR_FB_spatial_entity_container`
   - `XR_META_spatial_entity_mesh`
   - `XR_EXT_eye_gaze_interaction`, `XR_EXT_hand_interaction`,
     `XR_META_hand_tracking_microgestures`

   Log the runtime's full extension list at startup. *Measured (S1, v207):*
   84 extensions. Present: `XR_EXT_hand_tracking`, `XR_FB_passthrough`,
   `XR_FB_display_refresh_rate`, `XR_META_spatial_entity_mesh`,
   `XR_META_spatial_entity_room_mesh`, `XR_EXT_spatial_plane_tracking`,
   `XR_EXT_hand_interaction`, `XR_META_hand_tracking_microgestures`,
   `XR_META_environment_depth`. **Absent:** `XR_FB_scene`,
   `XR_FB_spatial_entity`, `XR_FB_spatial_entity_query`,
   `XR_FB_spatial_entity_container`, `XR_EXT_eye_gaze_interaction`.
   *Corrected (S7):* the `XR_FB_scene` / `XR_FB_spatial_entity*` family and
   `XR_EXT_spatial_entity` were hidden only because the manifest lacked
   `com.oculus.permission.USE_ANCHOR_API` (97 extensions once declared; see
   "Android manifest essentials"). `XR_EXT_eye_gaze_interaction` stays absent
   on the Quest 3, as expected.
3. **System** `HEAD_MOUNTED_DISPLAY`, view configuration `PRIMARY_STEREO`,
   environment blend mode `OPAQUE`. Passthrough goes through `XR_FB_passthrough`
   layers on Quest. *Measured (S1):* v207 enumerates both `OPAQUE` and
   `ALPHA_BLEND`; whether `ALPHA_BLEND` alone gives passthrough is an S7 check.
4. **Session lifecycle.** Poll events every frame. On READY, begin the session;
   on STOPPING, end it; on EXITING or LOSS_PENDING, tear down. Render only when
   the session is running; submit layers only when VISIBLE or FOCUSED. Treat
   Android pause and resume as input to this state machine.

## Vulkan interop (XR_KHR_vulkan_enable2 → wgpu 27)

Reference: [philpax/wgpu-openxr-example](https://github.com/philpax/wgpu-openxr-example)
(older wgpu; the names have drifted) and the
[openxrs Vulkan example](https://github.com/Ralith/openxrs/blob/master/openxr/examples/vulkan.rs).

*Verified in S2 against wgpu-hal 27.0.4 / wgpu 27.0.1 (`crates/fosfora-xr/src/gfx.rs`):*

1. `xrGetVulkanGraphicsRequirements2KHR`: Quest v207 reports 1.0 .. 1.2; we
   target 1.2.
2. Instance: `hal::vulkan::Instance::desired_extensions(&entry, version, flags)`
   gives the names (on device: `VK_KHR_surface`, `VK_KHR_android_surface`,
   `VK_EXT_swapchain_colorspace`, `VK_KHR_get_physical_device_properties2`);
   put them in `VkInstanceCreateInfo`, call `xrCreateVulkanInstanceKHR`, then
   `hal::vulkan::Instance::from_raw(entry, ash_instance, version, sdk, None,
   extensions, flags, MemoryBudgetThresholds::default(), false, None)`.
3. `xrGetVulkanGraphicsDevice2KHR` gives the `VkPhysicalDevice`;
   `hal_instance.expose_adapter(phd)` gives the `ExposedAdapter` (info,
   features, limits).
4. Device: `adapter.required_device_extensions(features)` (on device:
   `VK_KHR_swapchain`, `VK_EXT_robustness2`, `VK_KHR_external_memory_fd`), then
   `adapter.physical_device_features(&exts, features).add_to_device_create(info)`
   chains the feature structs; call `xrCreateVulkanDeviceKHR` with that, then
   `adapter.device_from_raw(ash_device, None, &exts, features, &hints, family, 0)`.
5. Wrap: `wgpu::Instance::from_hal::<Vulkan>`, `create_adapter_from_hal`,
   `create_device_from_hal` with the core's limits (16 storage buffers per
   stage, 5 bind groups). All `unsafe`, each with a `// SAFETY:` comment.
6. Session graphics binding `XrGraphicsBindingVulkan2KHR`: the same instance,
   physical device, device, queue family 0, queue index 0 that wgpu submits to.
7. Swapchains: one `R8G8B8A8_SRGB` swapchain per eye at the recommended
   1680x1760, 3 images each. Each image: `device.as_hal::<Vulkan>()
   .texture_from_raw(image, &hal_desc, Some(Box::new(|| {})))` (the no-op drop
   callback keeps wgpu from freeing it), then `create_texture_from_hal`.
8. **Image layout at release.** A render pass with `LoadOp::Clear` /
   `StoreOp::Store` is the last use of each swapchain image per frame, so it
   ends in `COLOR_ATTACHMENT_OPTIMAL`. Measured: zero stale frames over 73 s,
   no wgpu validation errors. No Vulkan validation layer exists on the device;
   bundling `VK_LAYER_KHRONOS_validation` in a debug APK is a follow-up if S5
   shows layout trouble.
9. Frame: `xrWaitFrame` → `xrBeginFrame` → `xrLocateViews` → acquire + wait
   both eyes → one wgpu submit (two render passes) → release → `xrEndFrame`
   with a `CompositionLayerProjection` (plus a passthrough layer first, in S7).

## Per-eye camera

From each `XrView`: pose → view matrix (inverse of the pose); fov
(angleLeft, angleRight, angleUp, angleDown, all asymmetric) → projection.
wgpu uses depth 0..1, so build the projection for that range; reverse-Z optional
(Fosfora uses Depth32Float). Use glam 0.29 to match core. *S2:*
`crates/fosfora-xr/src/math.rs` (host-tested: near/far map to 0/1, fov edges
map to clip edges, view parallax sign).

## World-space particles (S5)

*S4 finding (Quest 3, `crates/fosfora-xr` S4 build, board #3226/#3227):* the
desktop **compute rasterizer is a ~30 ms fixed GPU cost** on the Adreno 740 at
1280x720, independent of particle count (84k or 172k alive), resolution,
feedback passes and post-processing. The **billboard/vertex particle path**
renders the same effect at 4.6 ms with 87k particles and still holds 72 Hz at
1.7M particles (9.4 ms, cost tracking `max_count`). So the S5 world-space path
is built on the billboard renderer (board #3229, accepted); porting the
compute raster stays an optimization spike for later, if ever.

**As built (S5, `crates/fosfora-xr/src/particles3d.rs` + `particles3d_*.wgsl`,
board #3232):** a standalone test sim in the XR crate, not on the core's
`ParticleSystem` (its buffers are private and its positions are screen-space
NDC; a port needs core changes, listed in `MEASURED.md` "Particle sweep (S5)").
- Buffer: one AoS storage buffer, 32 B per particle (`pos: vec3` in stage
  meters, `life`, `vel: vec3`, `seed`). Every slot stays alive: a particle
  that dies or leaves the cube respawns in place, so the draw's instance count
  is the particle count and there is no alive list.
- Sim: one compute dispatch per frame (256 threads per workgroup), analytic
  curl of a two-octave sine potential (divergence-free, a few transcendentals
  per particle), speed and sprite size from the synthetic audio (bass, rms).
- Draw, once per eye, no vertex buffer. Two shapes: instanced
  (`draw(0..6, 0..count)`, one instance per sprite, the shape of the core's
  `draw_indirect`) and **vertex pulling** (`draw(0..6 * count, 0..1)`, the
  vertex index alone picks particle and corner). On the Adreno 740 the
  tiler's per-instance cost dominates the instanced form (board #3233:
  sprite size, swapchain scale, 3-vertex sprites and freezing the sim all
  leave it unchanged); pulling is about 1.5x cheaper at every count and is
  the form the numbers below use. The corner offset is applied **in view
  space** (`view * pos`, then `+ corner * radius_m`, then `proj`), so a
  sprite has a size in meters and perspective sets its footprint. Additive blend into the sRGB swapchain
  image. The vertex stage reads the particle buffer through a read-only
  binding (a writable storage binding in the vertex stage would need
  `VERTEX_WRITABLE_STORAGE`), so the same WGSL is compiled twice with the
  declaration swapped.
- Depth: each eye has a `Depth32Float` attachment at swapchain size. The quad
  writes depth; particles test (`Less`) and never write, so sprites behind the
  quad are hidden and sprites in front draw over it. Additive sprites need no
  order among themselves.
- Evidence for stereo depth without a binocular viewer: `log_depth_probes`
  in `xr.rs` prints, every 5 s, the NDC x and depth of two points 0.5 m nearer
  and farther than the quad center in each eye's matrices; correct depth means
  nearer = smaller depth value and larger disparity. Plus the screencap: the
  quad occludes what is behind it.
- Knobs (`adb shell setprop`): `debug.fosfora.mode` (`particles`, default,
  or `quad` for the S4 path), `count`, `sim` (`0` freezes the sim after a 2 s
  warmup to isolate draw cost), `size` (sprite radius multiplier), `tri`
  (3-vertex sprites), `pull` (vertex pulling), `hz` (requests a display rate
  via `XR_FB_display_refresh_rate` when the session begins) and `eyescale`
  (swapchain size vs recommended; the compositor resamples). The runtime
  switches rates on its own when nothing asks (72 → 90 → 72 seen in one
  launch), so measurements always set `hz`. `scripts/xr/sweep.sh` runs a
  count × rate × scale matrix and summarizes logcat (in-app pacing counters
  plus the runtime's `App=` GPU time, filtered to the app's own pid: the
  runtime logs a `VrApi` line per process).

**Original plan (compute-raster port), kept for the fallback case:** the
desktop scatter (`assets/shaders/builtin/compute_raster_scatter.wgsl`) does
`px = (pos.x * 0.5 + 0.5) * w` on screen-space `pos_life.xy`. The XR path
would add a **separate** scatter/draw shader variant that:
- reads world `xyz` (new test-sim layout; don't reinterpret existing sims' `z`);
- does `clip = view_proj[eye] * vec4(xyz, 1)`, discards if `clip.w <= near` or
  outside the frustum, then NDC → pixels as on desktop;
- sizes the footprint by depth: `radius_px = world_size * focal_px / clip.w`;
- is dispatched once per eye into that eye's raster target (the compute raster
  has no multiview; that's fine).

Resolve into the swapchain layer through a final render pass (layout rule above).
Additive vs alpha resolve follows the effect. Over passthrough (S7), output
premultiplied alpha.

## Audio on device (S6)

*As built (`crates/fosfora-xr/src/audio.rs`, `playback.rs`, board #3193).*
The core's `AudioSystem` runs unchanged on Android: cpal's AAudio host is in
the no-default-features build, `AudioSystem::new` opens the microphones at
the device default (44.1 kHz stereo F32) and the `fosfora-audio` thread,
channel, interpolator and pulse counters work as on desktop. The render side
polls `latest_features(dt)`, `latest_spectrum/mel/dmfcc` and the pulse
counters once per frame and builds the `HopOutput` the headless scene
renderer takes; `recording_ring.peek_latest` feeds the waveform texture.

- **Input preset.** AAudio's default `VOICE_RECOGNITION` preset (cpal sets
  none) runs the Quest's voice processing, which gates music. The XR crate
  opens the microphones itself through `ndk::audio` with an explicit preset
  and feeds `AudioSystem::from_ring`; see `MEASURED.md` for which preset.
- **File playback.** The bundled CC0 track (`assets/xr/audio/`, staged into
  the APK) is decoded with the core's `decode` feature (symphonia) and looped
  through a cpal output stream at the device rate, with linear resampling in
  the callback. Every played frame is also pushed into a ring the core's
  analysis thread reads (`from_ring`), so the visuals follow what the
  speakers play; the callback allocates nothing and bumps the watchdog's
  counter. Knobs: `debug.fosfora.audio synth|mic|micxr|aaudio|file`,
  `debug.fosfora.file <path>`, `debug.fosfora.micpreset`, `micperf`, `micfmt`,
  `micrate`. Measured Sep 27: none of the microphone knobs change what the
  Quest delivers (beamformed, high-passed speech pickup); see `MEASURED.md`.
- **Input events.** The main loop drains `AndroidApp::input_events_iter`
  every pass. Nothing consumes them yet, but an undrained queue makes Android
  flag the app as not responding as soon as a wearer generates input.
- **External microphone.** The headset's experimental external-mic setting
  routes Android's default input to a USB audio class device in the port;
  the core's capture then works unchanged and delivers full-range audio
  (measured Sep 27). Product path for room music: a USB mic or a USB audio
  interface from the mixer. Development over Wi-Fi adb, since the port is
  taken.
- **Latency (measured Sep 27).** Beats flash about 115 ms before the click
  is heard on the playback path: the tap is early by AAudio's ~190 ms output
  latency, the display chain is ~75 ms. Delay the tap by the stream's
  reported output latency (or open the output low-latency) before this is
  a product path; `scripts/xr/latency.py` re-measures from a phone video.
- Later: the in-app `RECORD_AUDIO` runtime request (via `jni`), AEC when the
  headset both plays and listens, and a real render-thread split only if the
  measurements say the CPU side is the bottleneck (it is under 1 ms now).

## Mixed reality (S7)

*As built (`crates/fosfora-xr/src/room.rs`, `input.rs`, `particles3d_sim.wgsl`,
board #3194). Mode `debug.fosfora.mode mr`; each part has its own knob.*

- **Passthrough.** `XR_FB_passthrough`: one `XrPassthroughFB` and one
  reconstruction `XrPassthroughLayerFB`, both created with
  `IS_RUNNING_AT_CREATION` (the `openxr` 0.22 wrapper's `start()` calls the
  pause entry point, so the flag avoids it). Each frame submits the
  passthrough layer first, then the projection layer with
  `BLEND_TEXTURE_SOURCE_ALPHA`; the eye targets clear to alpha 0 and the
  sprites write premultiplied color, so the room shows wherever nothing is
  drawn. The environment blend mode stays `OPAQUE` (the `ALPHA_BLEND` question
  from S1 is moot: on Quest the passthrough *layer* is the mechanism). The
  crate does not re-export the passthrough layer builder, so `xr.rs` fills the
  raw `XrCompositionLayerPassthroughFB` and casts it to the base header, as
  the crate's own builders do.
- **Hands.** `XR_EXT_hand_tracking`, one tracker per hand, joints located in
  the stage space at the predicted display time. Every joint with a valid
  position becomes an obstacle sphere with the runtime's joint radius (up to
  52). Pinch = thumb tip to index tip under 15 mm, released over 30 mm; the
  rising edge toggles a visible parameter (sprite size ×3). Controllers are
  not read (I5).
- **Hand occluders.** The runtime's skinned hand mesh
  (`XR_FB_hand_tracking_mesh`, `xrGetHandMeshFB` once per hand through the
  `openxr-sys` function pointer: 1360 vertices, 2314 triangles, 26 joints
  per hand on v207) is the depth occluder; joint spheres carved a hand-sized
  channel through the cloud but did not read as a hand. The bind-pose mesh
  sits in a static vertex buffer (position, four joint weights, four joint
  indices) and is skinned in the vertex stage from 26 matrices per hand
  uploaded each frame (located joint pose × inverse bind pose), drawn depth
  only with color writes off, like the room boxes, before the sprites. Hand
  scale is estimated from the located bone lengths against the bind pose
  (no `XrHandTrackingScaleFB` call). The sim keeps colliding with the padded
  spheres. `debug.fosfora.handmesh 0` falls back to the sphere impostors;
  `debug.fosfora.handmeshtest "x,y,z"` parks the left mesh in bind pose at
  that offset from the head for an unworn screencap.
- **Room.** `XR_FB_scene` over `XR_FB_spatial_entity_query`, through
  `openxr-sys` function pointers (no safe wrapper exists): `xrQuerySpacesFB`
  with a `LOCAL` storage-location filter once the session is focused,
  results on `SPACE_QUERY_RESULTS_AVAILABLE_FB`, components read with
  `xrGetSpaceComponentStatusFB`, bounds from `xrGetSpaceBoundingBox3DFB`
  (volumes) or `xrGetSpaceBoundingBox2DFB` (planes, given a 4 cm
  thickness), labels from `xrGetSpaceSemanticLabelsFB`. Anchors are located
  once a second (`xrLocateSpace` against the stage space) and become
  oriented boxes. The stage floor (y = 0) is a box too. No anchors →
  `xrRequestSceneCaptureFB` launches Space Setup when
  `debug.fosfora.scenecapture 1`, and the query reruns on
  `SCENE_CAPTURE_COMPLETE_FB`. The global mesh is counted, not used.
- **Sim.** An obstacle uniform block (64 spheres, 32 boxes, restitution,
  margin) read by `collide()` after integration: push out along the sphere
  normal or the nearest box face, reflect the inward velocity (restitution
  0.4), one pass per frame. Gravity 0.15 m/s² in `mr` so particles settle on
  surfaces. Sprites nearer than 0.3 m to the eye are culled in the vertex
  stage: in `mr` the user stands inside the cube and near sprites are pure
  fill-rate cost (numbers in `MEASURED.md`).
- **Not taken.** The `XR_EXT_spatial_entity` + `XR_EXT_spatial_plane_tracking`
  route (future-based, more API surface for the same planes) and
  `XR_META_spatial_entity_room_mesh` (no public binding). Both stay options
  for the product.

## The room first pass (board #3317)

*The first pass of `ROOM_DESIGN.md`: surfaces as emitters and the floor
ripple.*

### Surfaces as emitters

- **Surface lanes.** The two `w` lanes of the box rows that were written 0
  now belong to the aux block contract: `aux[67 + k].w` is the box's
  surface kind as a float (0 none, 1 table = DESK or TABLE, 2 floor, 3
  wall, 4 ceiling, 5 door or window frame, 6 other) and `aux[131 + k].w`
  its emitter weight in 0..1. Row count and offsets are unchanged (163
  obstacle rows, 170 with Murmur's lanes). The depth occluders, the S5
  test sim and Murmur read only `xyz`.
- **Kinds and weights** (`crates/fosfora-xr/src/surfaces.rs`, host-tested).
  `surface_kind` maps the anchor's label list, most specific label first.
  Every room box carries an emitter flag of 1; the synthetic stage floor's
  is 1 only while the room has no FLOOR anchor, so the floor never emits
  twice. Each frame, where the block is moved into the anchor's frame
  (`XrScene::set_world_inputs`), `emitter_weights` turns flags into
  weights: tables by top-face area against the largest table (the desk
  gets `tableweight`, default 1), floors `floorweight` (default 0.5),
  other kinds 0 for now. A box whose top face does not reach into the
  effect's volume (anchor +- `emitter.radius`) weighs 0, so a dragged
  anchor never emits from a surface it would respawn out of. The test is
  on the face point nearest the anchor, not the face center: the synthetic
  floor is 20 m across and centered on the stage origin.
- **The sim** (`flux_xr_sim.wgsl`, `xr_emit`). A preset whose `param(6)`
  is above 0.5 spawns on surfaces when the summed weight is positive;
  otherwise `emit_particle` runs as before. `param(6)`, not `param(0)`:
  the Flux XR presets' first input is `trail_decay` (0.88), and they have
  six inputs, so slot 6 reads 0 for them. A spawn draws over the boxes'
  cumulative weight times the kind's gate (table `0.15 + 0.85 * beat`,
  floor `0.1 + 0.9 * bass`, others 0.3) against the ungated total; a draw
  past the gated sum spawns nothing and the slot stays dead this frame,
  so the emission breathes with the music. The top face is the local axis
  closest to vertical, signed up (tables and scene floors +Z, the stage
  floor +Y), sampled uniformly over its part within the volume's half
  extent of the anchor, 1 cm above it. Floor sparks leave at
  `0.3 + 1.2 * bass` m/s up with a little lateral jitter; table embers at
  3 cm/s sideways, so the settle drift sets them on the top and the flow
  slides them off the edge. Life, color, size and opacity are the volume
  path's. The out-of-volume respawn goes through the same pick and dies
  when the gate is shut.
- **Preset.** `assets/xr/effects/flux_xr_world_room.pfx`, "Flux XR Room":
  Flux XR World plus a seventh input `surface_emit` = 1, same sim, count,
  sizes and emission (hidden; the pinch-hold cycle picks it up by file
  name). Select it with `debug.fosfora.effect 'Flux XR Room'`. Core tests:
  `flux_xr_room_preset_turns_on_surface_emission`, and on a GPU
  `flux_xr_room_emits_on_a_table` and
  `flux_xr_world_ignores_the_surface_lanes`.

### The floor ripple

- **What.** One lit quad on the floor inside the eye pass: rings expand
  from under the wearer on each beat over a soft glow that breathes with
  the bass. Pale and thin, light rather than material: premultiplied warm
  white, peak alpha 0.25 (`ripplegain` scales it), so the real floor shows
  through. Host module `crates/fosfora-xr/src/ripple.rs` (origin smoother,
  ring list, uniform packing and the WGSL, validated by naga in a host
  test); pipeline `build_ripple_pipeline` in `gfx.rs`.
- **Origin.** The head projected onto the floor and low-passed with a
  0.5 s time constant (by time, not frames), so it sits under the chair,
  needs no anchor and stays when the wearer turns. Each ring keeps the
  origin it was born at.
- **Rings.** Up to 8; a beat adds one and replaces the oldest. Radius
  `ripplespeed * age` (2.5 m/s), a 0.15 m gaussian profile, intensity
  `amp * exp(-age / 0.8 s)`, cut at 2.5 s. The amplitude is the low end at
  the beat, `0.3 + 0.7 * max(bass, sub_bass)`, so every beat shows and a
  heavy one shows brighter. The glow is 0.3 x bass over a 0.6 m radius.
- **Why the beat, not the kick.** There is no kick pulse (`kick` is a
  continuous feature), and the beat is the pulse timed to the sound
  through the playback tap (board #3253).
- **Where.** On the scene FLOOR anchor's top face when the room has one
  (the largest), else a 6 m square centered on the origin over the stage
  floor.
- **Draw order and depth.** After `draw_occluders`, so a desk or a hand in
  front of the floor hides it, and before the world sprites, so embers on
  the floor draw over the light. Depth test `Less`, no depth write. Both
  floor occluders write depth at the floor (the stage floor's top at
  y = 0, the scene plane +-2 cm), so the quad sits 2 cm above the highest
  floor top and carries a depth bias toward the camera (constant -16,
  slope -2), the usual decal setup; the lift alone is worth little depth
  at grazing angles.
- **Checked unworn** (`debug.fosfora.rippletest ceiling`, a diagnostic
  that mirrors the floor case under the CEILING anchor for a headset
  lying face up): the rings show without z-fighting against the ceiling
  occluder, and the parked hand mesh (`handmeshtest`) cuts a hand-shaped
  hole in them.
- **Knobs.** `debug.fosfora.ripple` (0/1, default on in `mr` and `world`),
  `ripplegain` (1), `ripplespeed` (2.5). No debug panel row.

## The room second pass (board #3327)

*The second step of `ROOM_DESIGN.md`'s sequencing: the wall spectrum and
the hand instruments.*

### The wall spectrum

- **What.** One lit quad on the wall the wearer faces, the mel spectrum
  as bars of light climbing it: bars across the wall's width (24 by
  default, low bands on the wearer's left), each brightest at its top
  with a bright cap line, over a faint glow along the bottom. The
  ripple's look: premultiplied warm white, peak alpha 0.25
  (`canvasgain`). Host module `crates/fosfora-xr/src/canvas.rs` (wall
  pick, bar model, uniform packing and the WGSL, validated by naga in a
  host test); drawn by `build_surface_pipeline` in `gfx.rs`, the ripple's
  pipeline factored to take a shader and a row count.
- **Which wall** (the "still open" question of `ROOM_DESIGN.md`: it
  follows the head, with hysteresis). The room's `WALL_FACE` boxes, less
  `INVISIBLE_WALL_FACE` (still kind 3 in the surface lanes and still an
  obstacle; `ObstacleBox::hidden` keeps the canvas off it). A wall's
  room-facing face is the face across its thinnest axis whose normal
  points at the head (`TopFace::facing`). The score is the cosine between
  the view direction and the direction to the face's center; walls behind
  the head (score <= 0) are never picked, and another wall takes over only
  after scoring 0.15 above the current one for 1 s, so a glance changes
  nothing. The log names the picked room box and its center when it
  changes and once a second.
- **Bars.** The mel column (`hop.frame.mel`, 64 bands on the device, read
  at runtime) is split evenly into the bars (the bar count, or the mel
  length if smaller) and each bar is the mean of its bands. The bands are
  dB in 0..1 over 80 dB, so the gain is a log-domain offset: each bar's
  top is its own running maximum (instant rise, 8 s decay) and the wall
  shows the 28 dB below it. One top for every bar was the first device
  run: music falls off toward the high bands and the wall read mean
  height 0.11-0.26 with its right half dark; per bar, 0.45-0.47. A bar's
  top is held within 16 dB of the loudest bar's and never under 0.5 (-40
  dB), so a band far below the rest or near silence does not stretch its
  noise over the wall. Heights rise at once and fall at 2.5 wall heights
  per second, by time.
- **Draw order and depth.** Right after the ripple, before the sprites:
  hidden by the occluders in front of the wall, under the embers. The
  wall box's occluder writes depth at its face, so the quad sits 2 cm off
  it with the ripple's depth bias.
- **Checked unworn** (`debug.fosfora.canvastest ceiling`, the ripple's
  trick: the canvas on the CEILING anchor's face toward the head, for a
  headset that sees the ceiling): the bars show with no z-fighting
  against the ceiling occluder.
- **Knobs.** `debug.fosfora.canvas` (0/1, default on in `mr` and
  `world`), `canvasgain` (1), `canvasbars` (24, up to 64),
  `canvastest ceiling`.

## Android manifest essentials

*Verified (S1)* against Meta's public "Android Manifest Settings" page:
`android.hardware.vr.headtracking` required/version 1, the three intent-filter
categories, `com.oculus.supportedDevices` (`quest2|questpro|quest3|quest3s`),
`minSdkVersion 29` / `targetSdkVersion 32`. The page lists no `uses-permission`
requirements and says nothing about the loader `<queries>`; those come from the
Khronos loader docs and work on v207 (the loader falls back to
`/odm/etc/openxr/1/active_runtime.aarch64.json` when both brokers return null).
The live copy is `android/app/src/main/AndroidManifest.xml`; the package id
moved to `android/app/build.gradle.kts` (`namespace` / `applicationId`).

```xml
<manifest ... package="dev.fosfora.xr">
  <uses-feature android:name="android.hardware.vr.headtracking" android:required="true" android:version="1"/>
  <uses-feature android:name="oculus.software.handtracking" android:required="false"/>
  <uses-feature android:name="com.oculus.feature.PASSTHROUGH" android:required="true"/>
  <uses-permission android:name="com.oculus.permission.HAND_TRACKING"/>
  <uses-permission android:name="com.oculus.permission.USE_ANCHOR_API"/>
  <uses-permission android:name="com.oculus.permission.USE_SCENE"/>
  <uses-permission android:name="android.permission.RECORD_AUDIO"/>
  <uses-permission android:name="org.khronos.openxr.permission.OPENXR"/>
  <uses-permission android:name="org.khronos.openxr.permission.OPENXR_SYSTEM"/>
  <queries>
    <provider android:authorities="org.khronos.openxr.runtime_broker;org.khronos.openxr.system_runtime_broker"/>
    <intent><action android:name="org.khronos.openxr.OpenXRRuntimeService"/></intent>
  </queries>
  <application android:hasCode="false" android:label="Fosfora VR">
    <meta-data android:name="com.oculus.supportedDevices" android:value="quest3|quest3s"/>
    <activity android:name="android.app.NativeActivity"
              android:configChanges="density|keyboard|keyboardHidden|navigation|orientation|screenLayout|screenSize|uiMode"
              android:launchMode="singleTask" android:screenOrientation="landscape"
              android:exported="true">
      <meta-data android:name="android.app.lib_name" android:value="fosfora_xr"/>
      <intent-filter>
        <action android:name="android.intent.action.MAIN"/>
        <category android:name="android.intent.category.LAUNCHER"/>
        <category android:name="com.oculus.intent.category.VR"/>
      </intent-filter>
    </activity>
  </application>
</manifest>
```

- The package id is a placeholder. Ask Kevin before the first dashboard upload;
  it is hard to change later.
- The `supportedDevices` value for Meta VR Glasses isn't known yet. Add it when
  Meta publishes it.
- Runtime permissions (`USE_SCENE`, `RECORD_AUDIO`) need a request at runtime.
  Spike: `adb shell pm grant`. Product: a JNI call to `Activity.requestPermissions`.
- **The manifest gates extension enumeration** (measured S7, v207): without
  `com.oculus.permission.USE_ANCHOR_API` the runtime listed 84 extensions and
  none of `XR_FB_scene`, `XR_FB_scene_capture`, `XR_FB_spatial_entity*`,
  `XR_EXT_spatial_entity`, `XR_EXT_spatial_anchor`, `XR_META_spatial_entity_discovery`
  or `_persistence`; with it, 97. Declare every capability the app may use
  before reading the extension list into a design decision.

## OpenXR loader

*Decided (S1, board #3211):* the Khronos Maven AAR
`org.khronos.openxr:openxr_loader_for_android:1.1.63` (Apache-2.0) as a Gradle
dependency in `android/app/build.gradle.kts`. The AAR ships
`jni/arm64-v8a/libopenxr_loader.so`, which Gradle packages into the APK, so no
binary lives in the repo. Building from `KhronosGroup/OpenXR-SDK-Source` stays
the fallback if a newer loader is ever needed before Maven has it.

## Assets on Android

Fosfora finds assets on the filesystem (`effect::loader::assets_dir()`: cwd,
then beside the exe, then the macOS bundle) and config under `dirs::config_dir()`.
Android has none of these. Plan:
1. The APK carries the needed `assets/` subset (Gradle `assets` dir).
2. On first run, or when the app version changes, extract it to
   `internal_data_path()/assets` (`android-activity` gives the path; read through
   the NDK asset manager).
3. The S3 overrides point `assets_dir()` and `config_root()` there before any
   other core call.

*Done in S4 (`crates/fosfora-xr/src/assets.rs`, `android/app/build.gradle.kts`):*
Gradle stages `effects/**`, `shaders/**` and the XR-only `assets/xr/effects/*.pfx`
into the APK with `assets/xr_manifest.txt` (SHA-256 stamp + file list, because
the NDK asset API cannot list directories). 209 files, 0.9 MB, unpack 21 ms.
Scenes are generated at runtime from an effect name (one cue, default params)
under the config dir, so no scene files ship.

## Glasses notes

- 70° × 66° field of view: keep the panel and key visuals inside the central
  region. Use the Meta VR CLI field-of-view simulation on the Quest 3 (see
  Meta's field-of-view doc) once there is UI.
- Eye gaze is only on the glasses; the Quest 3 falls back to head gaze. Design
  selection as "gaze ray (eye if available, else head) + pinch", one code path.
- Eye-tracked foveation has no public native route (Unity only), so it's not
  planned. Fixed foveation only helps fragment passes (see `PHASE0.md`).
