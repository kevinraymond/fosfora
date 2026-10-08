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
- `jni` 0.21 for the runtime permissions (see "Android manifest essentials").

**Build:** `cargo ndk -t arm64-v8a -o android/app/src/main/jniLibs build -p fosfora-xr`,
then `./gradlew assembleDebug`. Only arm64-v8a; the glasses test page requires
every native library under `arm64-v8a`, with no 32-bit-only dependencies.

## Threads

| Thread | Owns |
|---|---|
| Main (android_main) | Android event polling, OpenXR frame loop, all wgpu submission |
| `fosfora-audio` (existing) | Analysis chain; hands `AudioFrame` to the render side through the existing bounded channel |
| AAudio callbacks | Capture and playback; write only to the SPSC ring buffer (no DSP in callbacks, as on desktop) |
| `fosfora-voice` (board #3751) | Loads the speech model at launch, then transcribes each closed push-to-talk window; results back over a channel the frame loop polls |
| `fosfora-agent` (board #3751, V3) | One per sentence the grammar cannot match: the HTTPS call to the language model `voice.json` names and its parse; the answer back over a channel the frame loop polls |
| `fosfora-local` (board #3751, V5) | The on-device provider: loads the decision model's spec, tokenizer and ONNX Runtime session after the speech model, then runs the cascade for each sentence the grammar misses (ONNX Runtime's intra-op threads under it, `debug.fosfora.localthreads`); answers over a per-sentence channel the frame loop polls. Never at the same time as a transcription: no voice window opens while it decides, and a sentence waits while a window is open or transcribing. Its session is parked for the process when the app goes away, never released |

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
- Knobs (`adb shell setprop`): `debug.fosfora.mode` (`world`, the default since board #3553: an unset knob shows the room; `particles`,
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
- The in-app `RECORD_AUDIO` request is built (see "Runtime permissions"
  under "Android manifest essentials"). Later: AEC when the
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
  52). Pinch = thumb tip to index tip under 15 mm, released over 30 mm,
  counted only when the tips closed to get there (`pinch.rs`, board #3336;
  the gesture section below). With `XR_FB_hand_tracking_aim` the aim state
  is chained onto the joint locate (a raw `xrLocateHandJointsEXT` through
  the function pointer, since the crate's safe locate cannot chain a
  `next`): while its `SYSTEM_GESTURE` flag is on, the runtime's own menu
  gesture owns that hand and its pinch is suppressed here, so a pinch the
  system took never starts a gesture of ours. `debug.fosfora.handlead <ms>`
  chains `XrHandJointVelocitiesEXT` onto the same locate and moves every
  joint ahead along its linear velocity by that long (board #3753, an A/B
  for the trailing mesh on fast motion; default 0, off). Controllers are
  not read (I5).
- **Thumb microgestures (board #3336, step 4 of the gesture map).**
  `XR_META_hand_tracking_microgestures`, enabled together with
  `XR_EXT_hand_interaction` (the registry makes it depend on that extension,
  whose profile its paths extend) when the runtime offers both; the launch
  log says `microgestures (XR_META_hand_tracking_microgestures on
  XR_EXT_hand_interaction): true`. `microgestures.rs` holds the app's only
  action set, `fosfora`: five boolean actions (`swipe_left`, `swipe_right`,
  `swipe_forward`, `swipe_backward`, `tap_thumb`), each with
  `/user/hand/left` and `/user/hand/right` as subaction paths, suggested on
  `/interaction_profiles/ext/hand_interaction_ext` at
  `/user/hand/{left,right}/input/{swipe_left,swipe_right,swipe_forward,swipe_backward,tap_thumb}_meta/click`
  (the registry's subpaths). No controller profile is bound (I5). The set
  is attached in `XrSession::new`, after `xrCreateSession` and before the
  READY event begins the session (`xrAttachSessionActionSets` must precede
  `xrBeginSession`, once per session, so a later action set joins this
  one). A refused binding is logged and the run goes on without it;
  `INTERACTION_PROFILE_CHANGED` logs each hand's bound profile. Each frame,
  right after the hands are located and only while the session is
  `FOCUSED`, one `xrSyncActions`; each action's rising edge per hand
  (changed since the last sync and now on) is the frame's microgesture, the
  first in left, right, forward, backward order when two swipes fire at
  once. In world mode with more than one effect a swipe right steps to the
  next world effect and a swipe left to the previous, either hand, by the
  path the menu's `<` `>` take; nothing while the hand menu is up, nothing
  from the right hand in Edit room. Forward and backward are logged and
  reserved; the thumb tap opens the voice window (board #3751, V4, the
  Voice bullet below). The decision (`consume`) is plain data with desktop
  tests. Knob `debug.fosfora.micro 0|1`, default 1; 0 creates no action set.
- **Voice (board #3751, `VOICE_DESIGN.md`, "V1 as built" to "V4 as built").** The left fist
  held is a push-to-talk window, and (V4) a thumb tap on either hand opens
  one that closes on silence; the transcription shows on the label. The
  hand menu's Voice toggle (the Music row's right cell) turns it on and
  off, saved in `hand_menu.json`. Knobs, read at launch:
  - `debug.fosfora.voice 0|1`: forces the hand menu's Voice toggle and saves it (on when the file lacks it); voice needs the speech model installed, else off with a log.
  - `debug.fosfora.voicequiet <level>` (V4): a tap window closes once speech was heard and the microphone's level (RMS of the last 50 ms, ±1 scale) has stayed under this for 0.8 s; default 0.01 (-40 dBFS), tuned against the room's noise.
  - `debug.fosfora.voicethreads <n>`: whisper's threads for a transcription, 1..6, default 3.
  - `debug.fosfora.voicefile <path>`: a 16 kHz mono 16-bit WAV transcribed 3 s after the model loads, as if a window had closed (the unworn gate).
  - `debug.fosfora.say "<sentence>"` (V2): a sentence fed to the grammar as if heard, polled once a second, fed once per value.
  - `debug.fosfora.agent 0|1` (V3, `VOICE_DESIGN.md`, "V3 as built"): a sentence the grammar cannot match goes to the language model `voice.json` names; V5: with no `voice.json`, or one naming no `provider`, to the on-device model when its files are installed ("V5 as built"); default 1 whenever a provider is configured or installed, 0 keeps the agent off for sweeps.
  - `debug.fosfora.localthreads <n>` (V5): the on-device provider's ONNX Runtime intra-op threads, 1..6, default 3 (as whisper's).
  - `debug.fosfora.localmin <p>` (V5): the on-device provider's confidence floor on the action kind and the value, 0..1, default 0.35; an answer under it is the grammar's "Didn't catch that" with its hint, and changes nothing.
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
- **Anchor replay (board #3536), a measurement aid, not a product
  feature.** Unworn on a desk the runtime mostly answers the query with no
  anchors, so unworn sweeps could not light the room's surfaces. The room
  saves its located boxes to `debug/anchors.json` under the config dir
  (`files/config/` in the app's data) whenever the located set changes: at
  once when every anchor is located, after 2 s when a partial set holds.
  Per anchor: UUID, label as the runtime gave it, center, rotation and
  half extents in the base space; plus a version and the room id. Log:
  "room: saved N anchors for replay (debug/anchors.json)". Knob
  `debug.fosfora.anchors replay` (read at launch; unset or any other value
  is off): the first query that completes with no anchors reads the file
  once and its boxes become the room, static, with the same kinds,
  hidden walls and UUIDs, so the room id and `rooms/<id>.json` resolve as
  live and the scan label reads "Room: n surfaces". Log: "room: replaying
  N saved anchors (debug.fosfora.anchors replay)". The retries keep
  running; a query or rescan that returns live anchors drops the replay.
  A missing or refused file is logged and the run has no room. A replayed
  room is never saved back. Off, only the save differs from before. The
  boxes are only as good as the stage space: a changed boundary moves
  them, so a worn launch refreshes the file.
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

### The hand instruments

- **What.** Two instruments on the Go-Go far hand and the poses, acting
  on the Flux world sims (World, Coarse, Room; Murmur ignores them): a
  pinch tap throws a burst of embers that flies to where the hand points
  and bursts on the surface it hits, and an open palm held still, facing
  down, lifts embers off the surface under it toward the palm. The drag
  that moves the cloud's home is the anchor drag, as before. Host module
  `crates/fosfora-xr/src/instruments.rs` (the ray cast, the flights, the
  lift and the rows, host-tested).
- **Instrument rows.** Three rows appended after Murmur's hand lanes,
  170..173 (`XR_AUX_INSTRUMENTS`, `XR_AUX_INSTRUMENT_ROWS` in
  `flux_xr_sim.wgsl`; the upload is now 173 rows, `WORLD_AUX_ROWS` in
  `scene.rs`, and the core pins follow): `aux[170]` x = burst count (u32
  bits), y = lift strength 0..1, z = lift radius (m), w = steal fraction;
  `aux[171]` the burst center (anchor-relative), w its radius;
  `aux[172]` the lift point (the far palm). All zero is no instrument, so
  every writer from before the rows, and every core test that uploads
  163 or 170 rows, is unchanged.
- **The throw.** A tap not taken by the hand menu (while the menu shows,
  its pinches are its own) casts a ray from the head through the far
  pinch point, from that point, against the room's boxes: a slab test per
  oriented box, the nearest entry, a box containing the pinch point
  skipped, the stage floor only while the room has no FLOOR anchor. A
  miss ends 3 m along the ray. The burst flies from the pinch point to
  the hit at 6 m/s (at least 0.12 s), shedding a 300-particle streak per
  frame 3 cm across, then bursts 6000 (`burstcount`) over 4 frames, 0.12
  m in radius, centered one radius plus 2 cm off the surface along its
  normal: the ball is born whole in front of the surface, and the half
  flying at it bounces off it, instead of half of it being born inside
  the wall box and lost behind it. One flight per hand, a new tap
  replacing it; the rows carry one burst a frame, so two flights take
  turns frame by frame, and an impact counts only its packed frames, so
  it still delivers its whole count. Each throw logs the hit box, its
  kind, the point, the normal, the distance and the reach.
- **The burst in the sim.** In the dead-slot path, before the surface
  pick, the first `burst count` claimed slots spawn at the burst:
  uniform in the ball, flying radially out at 0.3..1.2 m/s (scaled by
  radius / 0.12, so the streak stays thin), the volume path's color and
  size at full opacity from birth (past the fade-in), half the lifetime.
  Other slots take the existing path unchanged. A burst particle's life
  lane is `XR_FREE` (2.0; the renderer reads only xyz): it is not bound
  to the volume (no respawn at the bounds, no edge fade), since walls sit
  outside the 1.5 m volume, and it dies past three half extents. Flux XR
  World runs near its particle count (53333/s over a 12 s life for 400K
  slots), so only the few hundred slots that die each frame are free to
  claim; the scene therefore sets the steal fraction, (count - dead
  slots) / alive from the last alive readback, at most 5 %, and a living
  particle respawns at the burst with that probability. Flux XR Room
  keeps a pool of declined slots (380K of 400K alive worn) and steals
  nothing.
- **The lift.** A far hand that reads open on the real hand, palm normal
  against world up under -0.6, far palm speed (low-passed, 0.1 s) under
  0.35 m/s for 0.25 s, turns the lift on at the far palm; it stays on
  while the hand stays open and palm down and slower than 0.7 m/s, so the
  embers can be led; strength ramps 0 -> 1 over 0.3 s and back. The
  menu's left hand never lifts while the menu is up; with both hands
  lifting, the later one wins. In the sim, a particle under the palm
  within the radius horizontally accelerates toward the palm at
  strength x 4 m/s^2 x (1 - (r / radius)^2), its lateral speed damped (3
  per second times the same factor), and it does not rest (no rest
  aging); above the palm nothing acts. Lift on and off are logged per
  hand.
- **Tests.** Host: the ray hits the nearest face with its normal, a
  flight reaches its hit in distance / 6 m/s at 72 and 90 Hz and its
  impact sums to the count, two flights alternate and each delivers its
  count, the lift needs a still, open, palm-down hand and ramps by time,
  a fist, a palm up, a sweep and the menu hand never lift, the rows are
  all zero with nothing playing. GPU (core): `flux_xr_throw_bursts_at_the_point`
  (2000 at a point: the first frame's 2000 newborns all within the
  radius; outside the volume, 2000 still there ten frames later) and
  `flux_xr_lift_raises_resting_embers` (embers settled on the room table
  at y -0.472, top -0.480; 20 frames of lift raise the column's mean to
  -0.410, against -0.468 without it).
- **Knobs.** `debug.fosfora.throw` (0/1, default 1), `burstcount`
  (6000), `lift` (0/1, default 1), `liftradius` (0.35).

## Hands: the gesture vocabulary (board #3336)

*An audit after Kevin's worn pass of Sep 29: the pinch-hold that cycles the
world effect fired when he did not mean it, thumb and index merely close.
What the hands do today, where the actions collide, what the runtime offers
beyond raw tip distance, and a proposal. Steps 1, 3 and 5 are built (see
"As built" at the end); steps 2 and 4 are pending.*

### What one pinch feeds today

- **The detector** (`pinch.rs`, fed by `input.rs`): thumb tip to index tip
  under 15 mm is a pinch, over 30 mm releases it. Since board #3336 a
  crossing counts only when the tips **closed** to get there: at least 8 mm
  nearer than at their widest within the last 0.5 s. A relaxed hand's tips
  sit 20 to 30 mm apart and a deliberate pinch closes 15 mm or more in a
  tenth of a second, a slow, careful one over a whole second still 10 mm in
  half of it; a hand settling on a desk drifts a millimeter or two a second
  and is reported once in the log as `pinch … not taken`, nothing fires. The check runs on every frame under the threshold, so a hand already
  resting close that then pinches hard still fires.
- **The gesture machine** (`gesture.rs`): one pinch at a time, either hand.
  Released before moving 25 mm and before 0.7 s is a **tap**; moving past
  25 mm is a **drag** until release; still for 0.7 s is a **hold**, which
  fires once.
- **The consumers** (`app.rs`, the gesture block): a tap not taken by the
  hand menu **throws** a burst where the far hand points, and toggles the
  S5 test sim's sprite size (a spike leftover that logs on every throw and
  draws nothing in world mode); a drag **moves the cloud's home**; a hold
  **cycles the world effect**. With Edit room on, the right hand's tap
  **cycles the pointed surface's behavior**, its hold **applies it to the
  kind**, its drag is swallowed (a short one counts as a tap), and the left
  hand keeps the world vocabulary. While the hand menu shows (left palm
  facing the head), the right hand's pinches are the menu's: a ray pinch
  clicks or drags on the panel, a fingertip pokes it.
- **The poses** (`pose.rs`, with 0.1 s dwell and hysteresis): fist (finger
  curl under 55 mm), palm up (normal within 37° of up), open; hands together
  under 0.30 m. Murmur's hand behaviors read them; the Flux lift reads an
  open far palm held still, facing down. With voice on (board #3751, V4)
  the left fist is the voice path's opener everywhere, so Murmur sees an
  open left hand and only the right fist is a predator; with voice off,
  either fist is.

### Where the vocabulary collides

1. **A resting hand is a pinch, and a still pinch is a hold.** The complaint.
   Fingers settling together crossed 15 mm, and a hand that then stays still
   on the desk reaches 0.7 s and cycles the effect. The closing gate removes
   the first half; a deliberate pinch held still (aiming a throw, thinking)
   still cycles the effect after 0.7 s with no warning but the HUD's
   progress figure, which the menu-off wearer never sees.
2. **A slow throw is a hold.** A pinch, a pause to aim, then the release: if
   the pause reaches 0.7 s the effect cycles and the throw never happens.
3. **The hold is destructive-adjacent.** It changes what the whole room
   shows, from a gesture a hand makes by accident, with no arm step. The
   editor's hold (apply to the kind) rewrites every surface of a kind the
   same way.
4. **The menu pose is the system gesture.** The Quest's own hand menu arms
   on a palm facing the head and opens on a pinch of that hand. Our hand
   menu shows on the same left palm; a left pinch while it shows is read by
   the runtime first. We never see `SYSTEM_GESTURE` today, so a pinch the
   runtime took can still start a gesture here.
5. **Tap toggles a dead parameter.** Every throw logs `sprite size x3`.
   Noise in the log, nothing visible in world mode.

### What the runtime offers (v207 lists all of these)

- **`XR_FB_hand_tracking_aim`**: chained to `xrLocateHandJointsEXT`, an aim
  pose and flags per hand: `VALID`, `INDEX_PINCHING` … `LITTLE_PINCHING`,
  `SYSTEM_GESTURE` (the runtime's menu gesture is armed or in progress),
  `DOMINANT_HAND`, `MENU_PRESSED`, plus a pinch strength 0..1 per finger with
  the runtime's own filtering and hysteresis. No extra call per frame. The
  `openxr` crate exposes the flag and the struct (`HandTrackingAimStateFB`);
  the chain needs a raw locate, as the scene query does.
- **`XR_EXT_hand_interaction`**: an interaction profile with `pinch_ext`
  (value and ready), `aim_activate_ext`, `grasp_ext` and `poke_ext` poses,
  through action sets and `xrSyncActions` each frame; the runtime tunes the
  thresholds and hysteresis. The app has no action sets today.
- **`XR_META_hand_tracking_microgestures`**: thumb swipes (left, right,
  forward, backward) and a thumb tap on the index, as input paths on the
  hand interaction profile. Small, deliberate, hard to make by accident:
  the natural home for "next effect".
- `XR_MSFT_hand_interaction` (select and squeeze), `XR_FB_hand_tracking_capsules`,
  `XR_META_hand_tracking_frequency_hint` are available and not needed.

### Proposal: a per-action map

| Action | Today | Proposed |
|---|---|---|
| Throw a burst | pinch tap | pinch tap (gated pinch), unchanged |
| Move the cloud's home | pinch drag | pinch drag, unchanged |
| Next / previous world effect | pinch hold 0.7 s | thumb swipe right / left (microgestures) **and** the menu's `<` `>` row; the bare hold goes |
| Hand menu | left palm faces the head | unchanged; a hand flagged `SYSTEM_GESTURE` contributes no pinch that frame |
| Edit room: cycle the surface | right tap | unchanged (an explicit mode) |
| Edit room: apply to the kind | right hold 0.7 s | right hold with the in-world label showing the ring filling, and a second hold within 2 s to confirm (hold-to-arm for anything room-wide) |
| Sprite size x3 | every tap | removed from world mode (stays in `particles` for the sweeps) |

The order to build it: (1) the closing gate, done, worn check pending; (2)
`SYSTEM_GESTURE` from the aim extension dropping that hand's pinch, cheap and
removes a double-read; (3) the effect cycle onto the menu row (the row
exists in the debug panel; it moves into the hand menu) and the hold off
the world; (4) microgestures through a minimal action set, measured on the
device (an `xrSyncActions` per frame; expect noise); (5) the editor's
confirm. Each is its own PR with a worn gate.

**Decision for Kevin.** (a) Take the map as proposed, or keep the hold with a
longer dwell (1.2 s) and the ring visible? (b) Microgestures and the menu
row, or the menu row alone (one fewer extension, one more reach for the
menu)? (c) Is a confirm step on apply-to-kind worth the second hold? Default
taken if silent: the map as proposed, both homes for the cycle, the confirm.
**Taken (Oct 8):** Kevin away, his default stands ("merge and move on, test
and refine later"): the map as proposed, both homes for the cycle (the menu
row now, microgestures a later step), the confirm on apply-to-kind.

**As built (steps 1, 3, 5).** Step 1, the closing gate, landed in PR #234
(worn check pending). Step 3: the bare hold does nothing in the world (it
logs `gesture: hold <hand> (unassigned in the world since board #3336)`), and
the effect cycle is the hand menu's top row, the debug panel's `<` `>` row
("Embers  1/3") moved under the menu's title; the menu is five rows, eight
with Edit room on (244 and 364 points tall, its bottom rows where they were),
and outside world mode the row is a status, "One effect in this mode", so the
height does not change with the mode; the debug panel keeps its 11 and 14
rows. Step 5: the editor's hold is armed first. A first right hold on a
surface arms the class cycle (the highlight pulses twice, the label at the
hit says what a second would do, "desk: hold again for all tables ->
streamlines", the status cell appends " · armed"); a second hold on the same
surface within 2.5 s (the proposal said 2 s) fires it; the time running out,
a tap, a gesture with no surface, the beam moving to another surface or Edit
room off disarm without firing, and the panel up holds the surface while the
arm's clock runs on. The label is the arm's cue; no ring fills. And a tap's
sprite size toggle runs only in `mode particles` (the S5 test sim the sweeps
use); in mr and world a tap throws and nothing else. Steps 2 (`SYSTEM_GESTURE`
from the aim extension) and 4 (microgestures, the cycle's second home) are
pending, each its own PR with a worn gate.

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
  <uses-permission android:name="android.permission.INTERNET"/>
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
- `INTERNET` (the voice path's agent, board #3751) is a normal permission:
  granted at install, no runtime ask.
- Runtime permissions (`USE_SCENE`, `RECORD_AUDIO`) need a request at runtime;
  the app makes it (below). The development scripts never relied on the
  ask: the permissions were granted once with `adb shell pm grant`, which
  survives `adb install -r`.

**Runtime permissions** (*built*, board #3264; `crates/fosfora-xr/src/permissions.rs`).
At launch, after the asset install and before the session, the app checks
`USE_SCENE` and `RECORD_AUDIO` with `Activity.checkSelfPermission` (JNI
through the `jni` crate on the `android_main` thread; no Java source) and
logs each: `permission USE_SCENE: granted`, `missing, asking`, `missing, not
asking (debug.fosfora.ask 0)`, or for the microphones `missing, not needed
(audio synth)`. It asks for the missing ones in one
`Activity.requestPermissions` call: `USE_SCENE` always (the room is the
product), `RECORD_AUDIO` only when `debug.fosfora.audio` is a source that
opens the microphones (`mic`, `micxr`, `aaudio`, `loop`). The bring-up does
not wait for the answer: the session, passthrough, hands and the stage floor
need neither permission. Without `USE_SCENE` the scene query returns nothing;
without `RECORD_AUDIO` the microphones stay closed and the synthetic groove
runs. The result callback (`onRequestPermissionsResult`) never reaches native
code, so the frame loop polls the grants once a second until nothing is
missing or 120 s pass (`permissions: … still missing after 120 s: polling
stopped, the fallbacks stay`); a denial leaves the fallback for the run. The
pickup, without a relaunch: a `USE_SCENE` grant while the room has no anchors
reruns the scene query (the query alone, as `debug.fosfora.rescan query` runs
it, not Space Setup, with a fresh set of empty-result retries), logged
`permission USE_SCENE granted after N s: querying the room`; a `RECORD_AUDIO`
grant opens the microphones the way the launch would have, logged
`permission RECORD_AUDIO granted after N s: opening the microphones`. While
`USE_SCENE` is awaited and the room has no anchors, the scan label reads
"Allow spatial data to see the room" (`room scan: NotAllowed` in the log)
instead of "Scanning the room…". `debug.fosfora.ask 0` skips the request for
unworn runs, where the dialog would sit over the view; the checks, the log and
the pickup stay. `scripts/xr/sweep.sh` and `soak.sh` set it for their runs
and clear it afterwards. Should the JNI layer fail, the app takes everything
as granted and runs as it did before it asked.

Device check (the reviewer's recipe):

```
adb shell pm revoke dev.fosfora.xr com.oculus.permission.USE_SCENE
adb shell pm revoke dev.fosfora.xr android.permission.RECORD_AUDIO
# launch, screencap the dialog, then
adb shell pm grant dev.fosfora.xr com.oculus.permission.USE_SCENE
# the log says "granted after N s: querying the room" and "room scan: Found(…)"
```

With the default `debug.fosfora.audio synth` only `USE_SCENE` is asked for;
set `debug.fosfora.audio aaudio` before the launch to see both dialogs and the
microphone pickup (`adb shell pm grant dev.fosfora.xr
android.permission.RECORD_AUDIO`).
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

## ONNX Runtime (board #3751, V5)

The voice path's on-device provider (`local.rs`) runs its decision model on
ONNX Runtime 1.28.0 (MIT), from the official Maven Central AAR
`com.microsoft.onnxruntime:onnxruntime-android:1.28.0`, the way the OpenXR
loader comes: a Gradle dependency, no binary in the repo. Only the AAR's
`jni/arm64-v8a/libonnxruntime.so` (28.6 MB) reaches the APK's
`lib/arm64-v8a/` (`android/app/build.gradle.kts` extracts it): its Java API
and JNI binding serve Java callers, and the app has no code. The `ort`
crate (`=2.0.0-rc.11`, the version the core pins; `load-dynamic`) dlopens
it by name at the provider's load, which the classloader namespace resolves
in the app's native library directory, as for the loader; the Activity's
`ApplicationInfo.nativeLibraryDir` is the fallback path (`permissions.rs`
has the JNI). The library needs only `libc`, `libm`, `libdl`, `liblog` and
`libandroid`, so it does not need `libc++_shared.so`.

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

*The speech model (board #3751):* `ggml-base.en.bin` (148 MB, MIT; source
in `assets/xr/models/LICENSE.md`) is fetched with its checksum by
`scripts/xr/fetch-model.sh` into the git-ignored `assets/xr/models/`,
staged as `xr/models/ggml-base.en.bin` and stored uncompressed in the APK.
The install streams every file in chunks, so the model never sits whole in
memory; like every asset it is copied again only when the stamp changes.

*The decision model (board #3751, V5):* `s1-17m-int8.onnx` (29 MB, ours,
Apache-2.0), `s1-17m-tokenizer.json` and `s1-17m-tokenizer_config.json`
(Apache-2.0) are listed with their SHA-256s in `assets/xr/models/MODELS.txt`,
which `scripts/xr/fetch-model.sh` reads for every model (the speech model's
line too). Their URL is a placeholder until the model is hosted: the script
then accepts a file already present whose SHA-256 matches (a dev build
copies them in from the training export) and otherwise warns and goes on,
so the APK builds without them and the provider is off with a log line.
`s1-17m-spec.json` (the provider spec, ours and small) is committed. All
four are staged as `xr/models/…`, the model stored uncompressed.

*The agent's config (board #3751, V3):* `voice.json` under the config dir
(`files/config/voice.json` in the app's data) names the language model a
sentence the grammar cannot match goes to: `provider` (`anthropic` or
`openai`), `base_url`, `model`, `api_key`, and for `openai` an `extra`
object merged into the request (examples in `VOICE_DESIGN.md`, "V3 as
built"). It holds a key, so it is never committed (git-ignored wherever
it sits) and never logged; it is read once at launch. Put it on the
headset with
`adb shell "run-as dev.fosfora.xr sh -c 'cat > files/config/voice.json'" < voice.json`
and relaunch; without it the agent is off and says why in the log. V5:
without it, or with one naming no `provider`, the agent is the on-device
provider whenever its files are installed (`"provider": "local"` names it);
a file naming `anthropic` or `openai` keeps V3's behavior.

## Glasses notes

- 70° × 66° field of view: keep the panel and key visuals inside the central
  region. Use the Meta VR CLI field-of-view simulation on the Quest 3 (see
  Meta's field-of-view doc) once there is UI.
- Eye gaze is only on the glasses; the Quest 3 falls back to head gaze. Design
  selection as "gaze ray (eye if available, else head) + pinch", one code path.
- Eye-tracked foveation has no public native route (Unity only), so it's not
  planned. Fixed foveation only helps fragment passes (see `PHASE0.md`).
