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
| `fosfora-music` (board #5385) | The Music row's tracks: decodes each whole, walks the music folders, and frees the clips the frame loop lets go of; results back over a channel the frame loop polls ("Your music") |
| `fosfora-voice` (board #3751) | Loads the speech model at launch, then transcribes each closed push-to-talk window; results back over a channel the frame loop polls |
| `fosfora-agent` (board #3751, V3) | One per sentence the grammar cannot match: the HTTPS call to the language model `voice.json` names and its parse; the answer back over a channel the frame loop polls |
| `fosfora-local` (board #3751, V5) | The on-device provider: loads the decision model's spec, tokenizer and ONNX Runtime session after the speech model, then runs the cascade for each sentence the grammar misses (ONNX Runtime's intra-op threads under it, `debug.fosfora.localthreads`); answers over a per-sentence channel the frame loop polls. Never at the same time as a transcription: no voice window opens while it decides, and a sentence waits while a window is open or transcribing. Its session is parked for the process when the app goes away, never released |

Render is single-threaded for the spike. Don't add a render thread until the
measurements say the CPU side is the bottleneck.

## Thermal governor (board #3789)

After an hour worn the Quest 3 holds its GPU at 456 MHz instead of 640, and
content that fits the 72 Hz budget cool runs at 24 to 30 fps (`MEASURED.md`,
"Worn A/B of the frame cost"). The frame loop gives ground on its own
(`governor.rs`, the decision logic, host-tested; `app.rs` feeds it once a
frame in world mode). **It watches** the runtime's app GPU time
(`XR_META_performance_metrics`, the mean over the last second); the
governor keeps those counters on whether the debug panel is up or not.
Without them it falls back to the frame loop's one-second window: over below
95 % of the display rate or at 3 long frames, under at 98 % or above with no
long frame. **The budget** is the display period times a margin: over above
0.92 of it (12.8 ms at 72 Hz) for 3 s steps the density down by 0.1,
to 0.3 at the lowest; under 0.75 of it (10.4 ms) for 20 s steps it back up
by 0.1, to the wearer's own setting at most. Between the two it holds, and
each step restarts the clock, as does a gap of over 0.5 s with no frame (the
session paused). Over for another 3 s at the floor raises a `Starved` level,
the seam for the next stage (the faces' update rate, board #3788, not
built); it is the first thing given back. **The stepper**: the debug panel's
"firefly density" shows the governed value marked, "firefly density 0.60
(gov)", and a wearer's step starts from it and becomes the new ceiling, the
governor's debt cleared. Each step logs a line (`governor: gpu 15.2 ms over
12.8 for 3 s · density 1.00 → 0.90`), and the thresholds once the display
rate is known. **Knobs** (read at launch): `debug.fosfora.gov 0|1` (default
on in world and mr; mr has no density, so it idles there),
`debug.fosfora.govdown 0.92`, `debug.fosfora.govup 0.75` (held at or below
`govdown`).

**What the density is now** (board #5470). It was the cloud's emission
against its preset's. The cloud effects are gone, and the density is the
share of the fireflies' count that is asked for ("Fireflies and the hand
particles", below). The governor still steps it, and it now gives back
little: the fireflies are a few thousand sprites. The stage after it, the
faces' update rate, is the one that would give back a hot headset's frame.

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
- **File playback.** The CC0 test clip (`assets/xr/audio/`, staged into
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

### Your music (board #5385)

*As built (`playlist.rs`, `player.rs`, `playback.rs`).* The Music row plays
the wearer's own files. While the music folders are empty it plays the
tracks the app ships with (board #5528).

- **With no music of your own.** The app ships full tracks, each with a
  build, a drop and a breakdown: "Let the Night Come In", "Slow Blue" and
  "Second Wind" (`assets/xr/audio/`, CC0, staged into the APK). While both
  folders hold no track the app can read, they are the playlist. Their
  order is one list in the code (`music::BUNDLED`), the first track first,
  not the folder's order or the alphabet's. They go through the same
  playlist and player as the wearer's files, so everything below holds for
  them: the title from the file name, `<` and `>`, the voice, playing
  through, the saved track, the decode on the music thread.
- **Your music wins.** When a scan finds a track in either folder (at
  launch, when the permission arrives, on "scan my music"), the playlist is
  the wearer's tracks only, and no bundled track is in it. When the folders
  are empty again the bundled ones return. At either change the first track
  of the new playlist takes over: at once while the music plays, and at the
  next play when it is stopped. A scan that leaves the playlist in the same
  hands leaves the playing track alone, and keeps the row on its track when
  that file is still there.
- **`debug.fosfora.bundled 1`.** The playlist is the bundled tracks whatever
  the folders hold, so they can be reached on a headset that has music.
  Read at launch. It saves no track: each launch starts on the first
  bundled track, and the wearer's saved track is still there without it.
- **The test clip.** `ember_glow_excerpt.ogg` (45 s, 140 BPM) stays in the
  APK for the measurement sources, `debug.fosfora.audio file` and `loop`,
  and for the numbers in `MEASURED.md` made with them. It is never in the
  playlist. The row falls back to it, looped, only when no track plays at
  all, which takes every bundled file missing or not decoding. A bundled
  file missing from the assets is logged at launch and left out of the
  list. One that does not decode is skipped like any other track.
- **The two folders.** The shared `Music/` folder
  (`/storage/emulated/0/Music`) and the app's own folder
  (`Android/data/dev.fosfora.xr/files/Music`). The app asks Android for both
  paths through JNI (`Environment.getExternalStoragePublicDirectory` and
  `Context.getExternalFilesDir` with `DIRECTORY_MUSIC`) and creates its own
  folder at launch. Each is walked four folders deep, 2000 tracks at most,
  hidden files skipped.
- **Putting files there.** Plug the headset into a computer over USB and
  allow file access in the headset. Copy the files into `Music` at the top of
  the headset's storage, or into `Android/data/dev.fosfora.xr/files/Music`.
  The files the app reads are `.mp3`, `.ogg`, `.flac`, `.wav` and `.m4a`.
- **The permission.** Reading `Music/` needs a runtime permission, asked for
  at launch with the room's and the microphone's. The app targets SDK 32, so
  it asks for `READ_EXTERNAL_STORAGE`, which Android 13 and later grant as
  `READ_MEDIA_AUDIO` and the other media permissions. The name follows the
  target SDK (`permissions::storage`), and the manifest declares both.
  Without the grant the app's own folder still plays, and the launch log says
  `Music/` was not scanned. A grant within 120 s scans it without a relaunch.
- **The order.** The shared folder's tracks come first, then the app
  folder's. In each, a folder's own files come before its subfolders', and
  names sort the way people read them ("Track 2" before "Track 10"). The
  title is the file name without its extension and its leading track number
  ("01 - One More Time.mp3" is "One More Time"). Tags are not read.
- **The hand menu.** The Music cell keeps play and stop in its middle, with
  the press's words over the track's title, cut to fit. Its end boxes are
  `<` and `>`, the previous and the next track, wrapping. A step while the
  music plays decodes the new track while the old one plays on. Stopped, a
  step only picks the track for the next play.
- **The words.** "next song" (also "skip", "next track"), "previous song"
  ("last song"), "play {song}" ("play the song {song}"), "what is playing",
  and "scan my music" to pick up files added since the launch. A song is
  found by its title with case and punctuation folded: the whole title wins,
  else a unique run of its words. Several matches get a "Which one: …?"
  reply that names them. These are grammar phrases only. The on-device
  decision model's label set is closed, so it still knows only "music on"
  and "music off".
- **Play through.** With two tracks or more, the next one decodes 30 s
  before the playing one ends, and the playback swaps it in where the last
  one ends. One track, or the test clip, loops. The stream, its tap and
  the analysis on the tap carry on across a swap.
- **The limits.** A track is decoded whole on the music thread, about 23 MB a
  minute (48 kHz stereo f32; a 5-minute track measured 115.2 MB on the
  desktop, decoded in 0.21 s). While the next track waits, both are in
  memory. A track longer than 15 minutes is refused and skipped, and so is a
  file that does not decode. A file larger than any 15-minute track can be
  (518 MB, 96 kHz 24-bit) is refused before its decode. The core's decoder has
  no length probe yet, so a long but small file (a two-hour MP3) is decoded in
  full before the length check refuses it. Streaming decode is a later step.
- **Remembered.** The current track and the last play or stop are saved in
  `music.json` under the config dir, beside `hand_menu.json`. A launch
  resumes that track from its top, or stays quiet after a stop.
  `debug.fosfora.music` overrides the saved state at launch.
  `debug.fosfora.file`, when set, plays at launch in place of the saved track,
  so the measurement scripts get the same clip whatever the folders hold.
- **The log.** At launch, `music: 14 tracks (12 in Music, 2 in the app
  folder) · resuming "<title>"`. With empty folders, `music: no tracks in
  the music folders · bundled: "Let the Night Come In", "Slow Blue",
  "Second Wind" · playing "Let the Night Come In"`. With the knob the line
  starts `music: debug.fosfora.bundled 1 over 14 tracks (...)`. With no
  track at all, `music: no tracks found and no bundled track, playing the
  test clip (put files in Music/)`. A scan that changes the playlist's
  hands logs `music: the playlist is now the tracks in the music folders`,
  or `... the bundled tracks: the music folders are empty`. Then one line
  per track start with its length and decode time: `music: playing
  "<title>" · 3:42 · decoded in 1.8 s`.

### Interface sounds (board #5460)

*As built (`ui_sound.rs`, `playback.rs`).* Bare hands give no click and no
haptic, so a short sound tells the wearer that a press or a gesture
registered, and that the microphone is open. The sounds are synthesized at
launch from plucked notes in E major. There are no audio files.

| Cue | When | Notes | Length | Peak, default gain |
|---|---|---|---|---|
| Press | a hand menu press that does something; a thumb swipe that stepped the theme; a surface row step or an Edit room pinch that changed a surface | B6 tick with a woodblock partial | 30 ms | -14 dBFS |
| Listening | the voice window opened | E6, then B6 | 120 ms | -16.5 dBFS |
| Closed | the voice window closed | B6, then E6 | 120 ms | -16.5 dBFS |
| Understood | every clause of a sentence matched; an agent reply's actions all resolved; the room editor's second hold applied | G#6 | 160 ms | -16.5 dBFS |
| Not understood | a clause missed; an empty transcript; an agent miss or error | B5, a plain sine with a softer attack | 180 ms | -16.5 dBFS |
| Armed | the room editor's first hold armed the class cycle | F#6 twice | 110 ms | -16.5 dBFS |

A press that does nothing makes no sound: an effect step with no other
effect, a track step with no tracks, an Agent pick the headset doesn't
have, Roam already as asked, a stepper at its end, a surface row with no
surface under the beam. The voice's actions don't tick, because the
sentence already sounded. The debug panel's steppers other than density
make no sound either; they send the app no action.

- **After the tap.** The app analyzes the music it plays, so a cue in the
  analysis would flash the room. The output callback writes each frame to
  the speaker and into the delay line toward the tap, and only then adds the
  cues to the speaker's frames (`ui_sound::output_block`). The tap never
  sees them. A test runs that block with and without a cue and compares the
  tap's frames bit for bit.
- **The audio thread.** The frame loop asks for a cue by setting its bit in
  an atomic, and the callback picks the bits up at its next buffer. The mix
  allocates, locks and logs nothing. While a cue sounds, each sum goes
  through a soft knee above 0.85 that stays under full scale. Frames without
  a cue are left as the music made them. A cue asked while no stream runs,
  or picked up more than 300 ms after the ask, is dropped rather than played
  late.
- **Music stopped.** A stop closes the music's stream, so the cues get a
  low-latency stream of their own while the music is stopped. It has no tap,
  plays at half the gain because nothing masks it, and opens and closes on
  its own thread so the frame loop never waits. Between tracks nothing
  changes: the next track swaps in on the same stream, which plays silence
  if nothing is queued yet.
- **The microphones.** While the analysis runs on the microphones (the
  `mic`, `micxr`, `aaudio` and `loop` sources), the cues are off. The voice
  window's microphone opens just before the rising notes are asked for, so
  they play during its first 200 ms or so. The headset cancels its own
  speaker output from its microphones (the loopback read zero worn, see
  `MEASURED.md`), so they should not reach whisper or the tap window's quiet
  closer. Only a worn window with `debug.fosfora.voicedump 1` can confirm it.
  The falling notes play after the window's stream has closed.
- **The knob.** `debug.fosfora.uisound` is the gain from 0 to 1, read live.
  Unset it is 0.5, and 0 silences the cues. The log has `ui sound: gain 0.50
  (default)` at launch and on each change, and each cue at the debug level
  (`ui sound: press`).
- **Listening without the headset.** `UI_SOUND_WAV=/tmp/cues.wav cargo test
  -p fosfora-xr --lib write_the_cues -- --ignored` writes every cue at both
  levels to a WAV.

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
  Since board #3793 a frame over passthrough submits three layers, bottom
  first: passthrough, the faces layer (the lit room surfaces at a fraction
  of the eye resolution, `BLEND_TEXTURE_SOURCE_ALPHA`, knob
  `debug.fosfora.faceslayer`), then the main projection layer;
  `SURFACES_DESIGN.md`, D2c. Without passthrough there is no faces layer.
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
  once. In world mode a swipe right steps to the next theme and a swipe
  left to the one before, either hand, by the path the menu's Theme control
  takes (board #5470; until then it stepped the world effect); nothing
  while a pointer is on the hand menu
  (board #5459; with `debug.fosfora.menustay 0`, while it is up), nothing
  from the right hand in Edit room. A swipe acts 0.15 s after it fires
  and is dropped if its hand is a fist by then (board #5531,
  `microgestures::SwipeGate`): a hand closing into a fist drags the thumb
  along the index, and that stepped the theme. Forward and backward are
  logged and reserved; the thumb tap opens the voice window (board #3751,
  V4, the Voice bullet below), and a tap from a hand closing into a fist
  gives that fist's window (`VOICE_DESIGN.md`, "The fist holds the
  window"). The decision (`consume`) is plain data with desktop
  tests. Knob `debug.fosfora.micro 0|1`, default 1; 0 creates no action set.
- **Voice (board #3751, `VOICE_DESIGN.md`, "V1 as built" to "V4 as built").** A fist on
  either hand holds a voice window open (board #5531: it opens under the
  fist and what was said is sent when that hand opens, or at 20 s), and
  (V4) a thumb tap on either hand opens one that closes on silence. The
  answer shows on the label. The
  hand menu's Voice toggle (the Music row's right cell) turns it on and
  off, saved in `hand_menu.json`. The Agent row (board #3776, under the
  effect's) picks the agent's provider among what the headset has (local,
  the `voice.json` provider, off), saved there too and switched at once
  (`VOICE_DESIGN.md`, "The provider toggle"). Knobs, read at launch:
  - `debug.fosfora.voice 0|1`: forces the hand menu's Voice toggle and saves it (on when the file lacks it); voice needs the speech model installed, else off with a log.
  - `debug.fosfora.voicequiet <level>` (V4): a tap window closes once speech was heard and the microphone's level (RMS of the last 50 ms, ±1 scale) has stayed under this for 0.8 s; default 0.01 (-40 dBFS), tuned against the room's noise.
  - `debug.fosfora.voicethreads <n>`: whisper's threads for a transcription, 1..6, default 3.
  - `debug.fosfora.voicefile <path>`: a 16 kHz mono 16-bit WAV transcribed 3 s after the model loads, as if a window had closed (the unworn gate).
  - `debug.fosfora.voicedump 1` (board #5375, `VOICE_DESIGN.md`, "The window's audio in the log"): each window's audio as whisper gets it (16 kHz mono) written by the voice thread as a 16-bit WAV to `files/voice/window-<NNN>.wav`, numbered from 001 per launch, one `voice: window dumped to <path> (<bytes> bytes)` line each; the `voicefile` clip is not dumped; default off.
  - `debug.fosfora.say "<sentence>"` (V2): a sentence fed to the grammar as if heard, polled once a second, fed once per value.
  - `debug.fosfora.labels 1` (board #5378, `ROOM_DESIGN.md`, "Surface labels"): every surface shows its voice name and its behavior on itself all the time, not only with Edit room on, around a voice window and after a miss on a name; default off.
  - `debug.fosfora.agent 0|1` (V3, `VOICE_DESIGN.md`, "V3 as built"): a sentence the grammar cannot match goes to the language model `voice.json` names; V5: with no `voice.json`, or one naming no `provider`, to the on-device model when its files are installed ("V5 as built"); default 1 whenever a provider is configured or installed, 0 keeps the agent off for sweeps (not saved; the Agent control then reads "Agent: off" and a press turns it on).
  - `debug.fosfora.agentpick local|anthropic|openai|off` (board #3776): the Agent control pressed to that pick for the unworn gate, polled once a second, fed once per value as `say` is, switched and saved as a press is; a pick the headset cannot make logs `voice agent: agentpick "<v>" not available`.
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
- **Geometry first (board #5416, `geom_first.rs`).** Two occluders write
  depth in the eye pass: the live depth map (`env_depth.rs`, 320 x 320
  stretched about five times over an eye and blended across silhouettes)
  and the room's boxes. Where both cover a surface, the map's blended
  silhouette spills past the box's exact edge, so even a scanned table
  gets a soft, wandering edge. With geometry first (the default; `debug.fosfora.geomfirst 0` turns it off) the map
  stops occluding where a box explains what it sees, and the box's own
  edge shows. The map keeps everything no box explains.
  - **The rule.** A map sample is explained by a box when the ray from its
    depth camera meets the box, or the box grown sideways by
    `geomfirstmargin` texel widths at the sample's depth (default 1.5),
    and the sample lies no more than `geomfirsttol` (default 4 cm) plus
    the sensor's noise at its range (0.5 cm out to 1.6 m, 1.8 cm at 3 m)
    in front of where the ray first meets it. A sample on the box, behind
    it or in the blended fringe just outside its silhouette is the box's;
    one clearly in front of it is not. A ray that meets the box itself is
    held to the box's surface, so the margin never widens the tolerance
    in front of a face.
  - **Which boxes.** The ones drawn as occluders, read from the same
    obstacle uniform the occluder draws from: the scanned boxes (hidden
    walls included), the live planes and closed boxes, and the stage
    floor. None with `debug.fosfora.occluders 0`. Hands are not tested.
    The runtime removes them from the map, and the hand mesh, drawn
    after, keeps its own edge.
  - **Where it runs.** Per vertex of the occluder mesh, in a compute pass
    over the eye mesh's vertices of both layers, once a frame in the
    frame's one submission. In the vertex stage the box loop would run in
    the binning pass and again for every bin a triangle touches. A second
    small pass per target (the eye pass, and the faces pass at twice the
    step) marks the explained vertices to drop, and the mesh's vertex
    stage pushes those past the far plane, so the clipper removes every
    triangle that has one before it is binned. One exception, the keep
    rule: an explained vertex that shares a triangle with an unexplained,
    nearer vertex stays where it is. That triangle is the soft edge of
    something no box explains, a person in front of a scanned wall, and
    cutting it would leave the mesh's grid as the edge. `geomfirst 2`
    drops them all, for the A/B. The full-screen pass (`envdepthmesh 0`)
    has no vertices to test, so the feature needs the mesh.
  - **Knobs**, read at launch and polled once a second: `geomfirst 0|1|2`
    (default 1, on with the keep rule, since the worn comparison; 0 is the
    map occluding everywhere), `geomfirstmargin` (texel widths, 0 to 8), `geomfirsttol`
    (m, 0 to 0.5) and `geomfirstshow 1`, a magenta tint over the eye
    pass's map occluder where it still occludes, on or off with the
    feature, for an unworn screen capture (it includes the passthrough).
    The `geometry first:` log line gives the state at launch and each
    change.
  - **What it does not do.** It never hides anything new: the boxes draw
    as before and the map draws less. Off, the frame is what it was, with
    no compute pass and the occluder's own pipeline and uniform bytes. A
    box a few centimeters off its object gives a crisp edge in the wrong
    place. Something real within the tolerance in front of a box, or
    within the margin beside its silhouette (a phone on a table, a cable
    along a wall), loses its occlusion. Past a box's edge, a background no
    box explains loses its occlusion for up to one mesh cell plus the
    margin, which matters only for what is behind that background. It
    does not sharpen an edge no box explains.
- **Depth light (board #5439, `depth_light.rs`).** With the room dark,
  what the live depth map still occludes was a black cutout with the map's
  coarse edge: a vacuum's handle a couple of feet away, a chair, a pile, a
  person, the wearer's own legs. No box or plane fits a vacuum's handle,
  but the map already is a surface on every object. So the occluder's mesh
  now draws those things as points of light fixed in the room, and with
  the room dark the sky shows through between them. The first version lit
  them as a fill fading to dark at the silhouette; worn, that read as
  chunky shifting outlines around every thing and between the items of one
  pile, and a shelf 3 m out was a black block. It is on by default
  (`debug.fosfora.depthlight 0` turns it off). On the `xr-sonar` branch a
  sweep of light crosses these points on the downbeat: see
  [The sweep](#the-sweep-lines-of-light-from-the-wearer-on-the-downbeat-board-5511).
  - **What is lit.** The mesh, where it still occludes. With geometry
    first on, that is what no room box explains. The eye pass's mesh writes
    the same depth as before, so the particles are hidden behind real
    things as they were. The hands are not part of it, since the runtime
    removes them from the map and the hand mesh occludes them.
  - **Where the points draw.** In the faces layer when there is one (the
    default with passthrough), from its own mesh at twice the eye's step:
    at half the eye's resolution that is a quarter of the fragments, and
    the runtime's upsample keeps the soft points soft. The eye pass's mesh
    then writes the depth and the veil of its own light (the edge bars).
    Without a faces layer the eye pass draws the points.
  - **The veil: what shows through what.** The map no longer hides the
    surfaces' light (the sky, the lit faces, the edge bars) with its hard
    cutout at every level. Those draw first, tested against the room's
    boxes and the hands alone; the mesh then erases the share `1 - level`
    of them where it lies in front (in color and alpha, so a pixel it
    erases stays clear over the passthrough) and adds its points. With the
    room visible (level 0), a real thing hides the surfaces' light behind
    it as before and the passthrough shows the thing, with a faint light
    on it. At level 0.5 half of the surfaces' light shows through it, over
    the passthrough at half. With the room black (level 1) all of it shows,
    and the thing is only its points. The faces layer gets the same erase
    from its own mesh. At level 0 the veil erases all, so both passes draw
    the mesh first, as without the light, and nothing behind a real thing
    shades; at level 1 it erases nothing, so the eye pass's mesh writes
    depth alone. The points add their light to what is behind rather than cover it; an
    "over" of the points' own coverage needs it in the layer's alpha, which
    takes a second output or a second draw. In between the veil still
    follows the map's coarse silhouette, in proportion. A fold of the mesh
    erases twice. `depthlightveil 0` keeps the hard cutout at every level,
    the mesh drawn before the surfaces as in the first version.
  - **The points.** A tile of soft points made once and uploaded as a
    256 x 256 texture, 16 cells a side with a point anywhere in each: per
    texel, the distance to the nearest point's center, and that point's
    size, twinkle phase and rank. The fragment reads it once per plane and
    octave at the world position and makes the point by thresholds: the
    fill's radius times its size, swelling with the music, shrinking where
    a mass ends, present where its rank is under the fill, twinkling at
    its own rate with a triangle wave. Each plane and octave reads the tile
    from its own place; it repeats every 16 cells, 64 cm 1 m from the eye.
    The first version hashed four cells per plane and octave in every
    fragment, with a sine and a square root each, and cost about 4 ms on
    the Quest 3 for a pile filling 15 % of the view. A surface uses the
    planes it faces (the normal's fourth powers; a second plane only
    between about 35 and 55 degrees off an axis), so the map's noise,
    which moves the surface along the view ray, barely moves a point: on a
    surface turned 20 degrees from the eye, 4 mm of noise moves one by 5 %
    of a cell at most, a pixel or two. A point
    depends on the world position alone, so it stays put as the head moves
    and as the mesh's vertices slide over the surface. A quarter of a
    surface is covered (`depthlightfill`). A cell is 4 cm across 1 m from
    the eye, about 30 pixels, and doubles with each octave of the
    distance, the next octave fading in over the last quarter of each, so a
    thing keeps its look on screen near or far and the noise, larger far
    away, stays small against the cells. Where cells shrink to a few pixels
    (a surface seen edge-on, the fringe that leans back at a step) the
    points fade out; fading them into their mean instead drew a faint line
    of glow along every step in renders of recorded depth. The height
    bands of the first version stay as a slow wave of light through the
    points, one every 20 cm, drifting up, a triangle wave rather than a
    cosine, the crests tinted toward the
    top's color. A surface that faces up takes the top's color. The colors
    are the theme's: the color it gives other surfaces, and the key's tint
    where the theme uses it. Aurora draws teal and the key's tint; a room
    with no theme other's own green.
  - **Masses.** Inside a mass a step between things draws nothing: the
    normal is the one-sided gradient toward the neighbor nearer in depth,
    so a step does not tilt it, and nothing thins. A step of half a meter,
    or a quarter of the distance when that is larger, parts two masses:
    the far side keeps no points at its edge and the near side half their
    area, so the fringe between them carries the near side's points
    shrinking out and the mass behind starts again a cell away, with the
    sky between. A pile stays one mass; the pile and the shelf behind it
    are two. Something thinner than a mesh step is all near side, so a
    vacuum's handle carries points at half their area.
  - **Far things keep their color.** The light is all there to 1.5 m, falls
    to 0.35 of it by 3 m, and stays there as far as the map has data: a
    shelf 3 m out is a dim constellation in the theme's color.
  - **How bright.** A point's center rises with the immersion level from
    0.06 with the room plain to 0.8 with it black, times `depthlightgain`,
    the wave and the twinkle. At a quarter covered the light averages well
    under a lit surface's.
  - **Knobs**, read at launch and polled once a second: `depthlight 0|1`
    (default 1; 0 is the frame as it was, depth only with no color write
    and the old order), `depthlightgain` (0 to 4, default 1),
    `depthlightfill` (0.05 to 0.33, default 0.25: the largest point must
    stay within half a cell), `depthlightveil 0|1` (default 1) and
    `depthlightpoints 0|1` (default 1; 0 draws no points but keeps the
    light's passes and order, to cost the points apart). It needs the
    mesh. With the full-screen pass (`envdepthmesh 0`, or
    `envdepthfilter 0` or 1) or under `envdepthshow` it is off, and the
    `depth light:` launch line says so. The `geomfirstshow` tint replaces
    it while on. The log gives the state at launch, each change, and the
    colors when the theme changes.
  - **Cost.** Per vertex of the mesh that draws the points (the faces
    layer's, 6.6K an eye at step 4; the eye's, 26K, without a faces
    layer), in the vertex stage where the map is already read: four more
    texel loads, five rays, two one-sided differences, a cross product, a
    rotation, two square roots and three smoothsteps. None of it feeds the
    position. Per fragment of that mesh: one `fwidth`, a normalize, the
    planes' weights, a `log2` and a square root, then one read of the tile
    (two in the last quarter of an octave or between two axes, at most
    six) with about forty arithmetic operations each, then the wave and
    the blend; no sine or cosine. No discard and no `frag_depth`, so early
    depth testing works as before. The eye pass's mesh adds a constant
    fragment while the veil erases (levels between 0 and 1) and nothing
    over the depth-only occluder otherwise. Between those levels the
    surfaces behind a real thing shade where its depth rejected them
    before. A person 1 m away covers about 15 % of an eye's pixels and a
    quarter at 0.7 m. The first points measured 55 to 63 fps on the Quest
    3 in such a view; this version is not yet measured.
  - **What it does not do.** It does not change what occludes the
    particles: the depth written, geometry first's rule and the hand
    removal are as before. It does not sharpen the map's edge, which still
    shows between levels 0 and 1 as the veil's coarse outline, in the faces
    layer the coarser one of its own mesh. Where no box explains a wall (no
    scan, roam, geometry first off) the wall is points too. In the faces
    layer the masses come from its mesh at twice the step, so a thing
    narrower than about four depth texels can fall between its vertices
    and carry no points.
- **Edge bars (board #5420, `edge_bars.rs`).** Bars rise and fall along
  the edges of each plane and box, so the room's outline is drawn as the
  spectrum. It is on by default (`debug.fosfora.edgebars 0` turns it
  off). Each kind of surface draws in the mode its theme names (board
  #5431, `SURFACES_DESIGN.md`, Themes): on the face, standing, in (a
  tunnel behind the face, below) or off.
  - **What is drawn where.** The shapes are the boxes drawn as occluders
    this frame: the scanned boxes, and the live planes and closed boxes
    while they are shown, at their fade. Hidden walls and the stage floor
    occlude but get no outline, the hands are not boxes, and with
    `debug.fosfora.occluders 0` nothing is outlined. The occluder uniform
    carries neither a hidden wall's flag nor a live shape's fade, so the
    frame hands the boxes over where it assembles the obstacle block. A
    plane outlines its face toward the head. A volume outlines the faces
    turned toward the head, one to three. Each edge is a strip on its
    face, lifted 1 cm as the surfaces are, and the bars grow in from the
    edge. The strips of a face meet at its corners on a 45 degree miter,
    so they never overlap. An edge two visible faces share gets bars on
    both, which reads as the edge itself glowing. `edgebars 2` stands the
    bars off the edge along the face's normal instead, a fence of light
    on a table's rim.
  - **The bars.** They are the wall spectrum's bars, normalized per bar,
    running from an edge's low end to its high end. A box corner is a low
    end when an even number of its signs along the box's axes is
    positive. So the direction alternates around every face, the same
    band meets itself at every corner, and an edge two faces share runs
    the same way on both. An edge shows every bar the canvas has while
    each stays at least 3 cm wide; a shorter edge merges neighbors by
    their mean. With the default 24 bars an edge over 2.9 m gets bars
    wider than 12 cm (16.7 cm on a 4 m wall). A line 4 mm wide runs along
    the whole edge while music plays, so the outline stays when a band
    is at zero. In silence (immersion's quiet) nothing draws. The look is
    the wall spectrum's: brightest at the tip, a cap line, soft sides.
  - **The scale rule.** The tallest bar is `edgebarsheight` (default
    0.12) of the face's smaller side, clamped to 2 to 20 cm, so a 40 cm
    box and a 4 m wall read alike: 4.8 cm on the box, 20 cm on the wall.
    A face under 4.4 cm across gets strips half its width, which meet in
    its middle.
  - **Color and brightness.** The surface's own color, from its lane's
    color index. A live shape takes its kind's palette color. The peak
    alpha is 0.25 with the room plain and rises to 0.7 at full immersion,
    times `edgebarsgain` and a live shape's fade.
  - **Clear of the neighbors.** A scene plane is a 4 cm slab centered on
    the surface. Where a wall meets the floor or another wall, the
    neighbor's slab covers the first 2 cm of the face along that edge
    and would hide the line. Each edge moves in along its face until it
    is out of every other box grown by 5 mm, by at most 6 cm. At a wall's
    junctions that is 2.5 cm.
  - **Occlusion and cost.** The strips draw in the eye pass after every
    occluder and the lit faces, before the sprites, with the surfaces'
    pipeline state (depth-tested, no depth write, the same bias). The
    face's own box never hides its strips: the 1 cm lift alone keeps a
    strip over 400 float steps in front of the face's depth at 4 m, more
    at grazing angles, and the bias only adds to that. One uniform of
    33,456 bytes holds up to 384 strips and tunnels together, a tunnel in
    a strip's 80 bytes, after the strips. A frame writes only the rows it
    uses: 2,736 bytes of bar sums (this frame's and the in mode's
    history, 2,448 more than before the in mode, in every mode) and 80 per
    strip or tunnel. Each eye draws the strips in one instanced call and
    the tunnels in another, so a frame of mixed modes is still one write
    and at most two draws per eye. The strips cover 27 % of a 4 x 2.5 m
    wall, up to 46 % of a square face under 1.7 m, and 24 % of a 4 x 4 x
    2.5 m room's walls, floor and ceiling. The fragment has no loop: a
    merged bar's mean comes from two running sums.
  - **In** (`edgebars 3`, board #5431). The owner's reference is an LED
    infinity mirror: a frame of lights whose copies recede into the box
    behind the glass, each smaller with distance, a tunnel of light with a
    dark center. Each outlined face becomes a window into such a tunnel: a
    box behind the face with the face's outline as its opening (moved
    clear of its neighbors as the strips are), its four inner walls
    running straight back along the inward normal. The bars lie on the
    inner walls and grow away from the rim into depth.
    - *How it is drawn.* Nothing is drawn behind the face: the face's own
      occluder writes depth there, and whatever stands in front of the
      face must still hide the tunnel. The face's quad is drawn instead,
      lifted and depth-tested as the strips are, and its fragment finds
      where the view ray through the pixel meets the inner walls. The ray
      crossed the opening inside it, so it leaves the box through the wall
      ahead of it along each of the face's axes, or through the end. The
      nearer of those two walls is the one it meets: the nearest of the
      four planes, with the two behind the ray left out, which costs two
      divisions and a minimum, no loop and no march. That gives a wall, a
      point along its edge and a depth. Where the ray reaches the end
      first nothing is drawn (alpha 0), so the center stays dark and the
      passthrough or the surface's own effect shows there. The eye
      pass's camera uniform carries only `view_proj`, so the vertex stage
      solves the eye's position from it: the one point it maps to clip x,
      y and w all 0. The tunnel is therefore at its true place for each
      eye and each head position: lean and the far rungs shift against
      the rim, which holds with one eye, by motion alone.
    - *Rungs.* On each inner wall the spectrum runs along the edge as on
      the face: the same bands, ends, merging and corners. A bar is a
      stack of lit rungs at even depths, like the mirror's receding
      frames, not a solid fill: ten at full height, each 0.3 of the
      spacing wide, a rung lit where its bar reached its depth (half lit
      when the bar's height times the depth is exactly there). The rungs
      line up across all four walls, so a loud moment reads as whole
      frames lit deep into the wall and a quiet one as the rim alone. The
      rim rung runs along the whole edge while music plays, gaps included,
      at the line's 0.6 and up to 1 where its bar is loud. Each rung is 0.8
      of the one in front (the last 0.13), so the far end fades into dark.
      A rung thinner than a pixel dims by the share of the pixel it covers
      instead of flickering, and a face seen nearly edge-on fades out
      between 0.05 and 0.2 of the cosine of the view ray to its normal
      (87 to 78 degrees off it).
    - *Sound travels in.* Rung k shows the bars from k steps of 60 ms ago,
      so a hit starts at the rim and reaches the last rung in 0.54 s. The
      history keeps each bar's loudest over each step, so a hit between
      two steps is not lost, and steps by time, so 72 and 90 Hz behave
      alike. Its rows' running sums go in the same uniform as this
      frame's, one block of 17 rows per rung. The fragment reads one row's
      two sums either way, so the history is the mode, not a knob.
    - *Scale.* The tunnel's depth is `edgebarsdepth` (default 0.6) of the
      face's smaller side, clamped to 0.15 to 1.5 m, so a 40 cm box and a
      4 m wall both read as a tunnel: 24 cm into the box, 1.5 m into a 4 x
      2.5 m wall. `edgebarsgain` applies as on the face.
    - *Cost.* A tunnel covers its whole face, where the strips covered
      its rims. Seated in a room whose walls and ceiling go in, that is
      most of each eye's 1680 x 1760 pixels, each shaded once (what stands
      in front fails the depth test, and the shader never discards, so
      early depth testing holds). The fragment: one intersection, a floor
      for the rung, the bar's mean from two running sums of that rung's
      row, two smoothsteps across the bar's slot, a power for the dimming
      and a square root for the grazing fade; no texture, no loop. That
      is less work than the surfaces pass's rings (a loop of eight) or
      streamlines (noise) and far less than a ported effect. For scale,
      `MEASURED.md` records Drift (streamlines on every wall, rings on the
      ceiling) at 0.10 ms of App over the saved room and Aurora (the
      ported effect on every wall and the ceiling) at 0.37 ms. The other
      modes draw as they did; they write the 2,448 bytes of history too.
    - *Tested on the desktop:* the intersection square on, grazing and
      from each side, the center ray reaching the end, the parallax (the
      same world point on a far rung from two head positions), the rung
      spacing and dimming, the history's stepping at 72 and 90 Hz, the
      depth scale and clamps, the fragment's Rust copy at chosen points,
      the shader validated with naga and its struct size checked. An
      ignored GPU test (`tests/edge_bars_gpu.rs`) renders the tunnel and
      compares it with the Rust copy pixel by pixel.
    - *To capture and measure* (not yet run on the device). The mode knob
      is live, so one seated launch in the scanned room captures every
      mode at the same spot: `music 1`, `gov 0` and `immersefix 1` set
      first, then `edgebars 0`, `1`, `2` and `3` in turn, a screen capture
      2 s after each, and `edgebars ""` with `theme mirror` last (back up
      `rooms/` first: the theme writes the room). For the cost, one launch
      per mode: `scripts/xr/gpu-stages.sh --with
      "music=1;gov=0;immersefix=1" --cond "off=edgebars=0" --cond
      "face=edgebars=1" --cond "stand=edgebars=2" --cond "in=edgebars=3"
      --cond "off2=edgebars=0" --only "off,face,stand,in,off2"`, and the
      eye surface's render is the number to read.
  - **Knobs**, read at launch and polled once a second: `edgebars`
    (unset each kind in its theme's mode; 0 off, 1 on the face, 2
    standing, 3 in, each forced on every kind; the hand menu's Edges
    control holds its choice until the value changes), `edgebarsgain` (0
    to 4, default 1), `edgebarsheight` (0 to 0.5, default 0.12) and
    `edgebarsdepth` (0 to 2, default 0.6). The `edge bars:` log lines give
    the state at launch, each change, the mode per kind when it changes,
    and what is outlined.
  - **What it does not do.** It is a layer over every shape, not a lane
    behavior: the lanes keep drawing, and no room file or lane format
    changes. Off, the frame is what it was, with no strips, no buffer
    write and no draw. The outline is where the box is, so a box a few
    centimeters off its object draws its outline that far off. Where
    boxes overlap, an edge inside another box moves in by up to 6 cm and
    is hidden if it is still inside. A face larger than the wall it sits
    on draws its outline past the wall's edge, where a neighboring wall
    hides it or it floats in the open.
  - **Where the tunnel looks wrong.** Where boxes overlap, only the
    nearest face's tunnel shows (a face behind another box's occluder
    fails the depth test), so a tunnel is cut where a neighbor's box pokes
    through its face. Two faces within the 1 cm lift of each other, such
    as a wall scanned twice, both draw, and two tunnels of different
    depths add up. A box off its object puts the opening where the box
    is: the rim floats off the real edge, and the passthrough through the
    dark center shows the mismatch. A face at a grazing angle shows a
    sliver of the far wall's first rungs and then fades out. A tunnel is
    drawn whatever is behind the face, so 1.5 m behind a thin wall, or 24
    cm behind the front of a shallow shelf, it reaches into the next room
    or out through the back; the illusion does not know. A volume's
    tunnels behind two of its faces cross inside it, each seen only
    through its own face. `edgebars 3` forces the floor in too, a pit
    under a seated wearer's feet.
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

## Fireflies and the hand particles (board #5470)

*Built and tested on the desktop, on a desktop GPU too, and the device code
passes the Android lint. Nothing here has run on the headset yet: every
size, speed and time below is the code's constant, not a measurement, and
the defaults are a first guess for a worn tuning pass.*

**What was cut, and why.** The owner wore the app and decided the show is
the room: the lit surfaces, the sky and the depth points. The three cloud
effects were not wanted. Seated, the default one (Embers) had almost
nothing alive, Flux Cloud was 300,000 sprites of smoke between the wearer
and the room, and Flock used 92% of the GPU. So Flux Cloud, Embers and
Flock are gone for the wearer: not in the menu, not by voice, not by swipe,
and roam no longer swaps to one. With them went the hand menu's effect row,
roam's fill rule, and embers and sparks as things a surface can be given.
What is left is one sparse layer, the fireflies, and the particles a hand
makes.

### What the wearer sees

Seated, soft points of light hang all around, from the floor to the
ceiling and in every direction, about one every 0.36 m. Each drifts on a
slow loop of its own and glows and fades on its own rhythm of 4 to 9 s. With
music they are a little brighter and a little larger in the loud parts. In
roam they are the same, around the head: the layer's center glides after
the wearer as the cloud's did (`roam::Follow`). A pinch tap still throws a
streak that bursts where it lands. The burst now arcs, falls and lies
where it lands, and what reaches an edge runs along it. The Particles
toggle hides both.

### One pool, two caps

The world effect is one preset, `assets/xr/effects/fireflies_xr_world.pfx`,
and one sim, `assets/xr/shaders/fireflies_xr_sim.wgsl`. Nothing in the core
changed: the core runs the preset as it ran the others. `fireflies.rs`
holds the caps and the knobs.

- `fireflies::FIREFLY_CAP` is 3,000: the most fireflies alive at once.
- `fireflies::POOL_CAP` is 20,000: the most particles alive at once,
  fireflies and hand particles together.

The sim keeps both by where a particle may be born. The pool's slots are
all there is. A firefly is born only in a slot whose index is under the
fireflies' count, which the sim takes as 3,000 at most, and a hand
particle only in a slot at or past it. So no room, song, theme or gesture
can put more of either alive. `XrScene::new_world` holds the pool to the
cap whatever the preset or `debug.fosfora.count` says, and a throw's burst
is held to the 17,000 slots the fireflies never take. When the hands'
slots are all alive a new burst takes the oldest hand particles, never a
firefly.

### The fireflies

- **Where.** Uniform in the volume: 0.9 of the space size across (the
  preset's 3 m half extent, so 5.4 m square) and from 1.05 m under the
  anchor to 1.65 m over it, which around an anchor 1 m up is the floor to
  2.65 m. They fade toward the bounds and near the head (from 0.45 m to
  0.3 m with the default near fade). One inside a scanned box or behind
  the depth map is hidden by the occluders. They collide with nothing.
- **Motion.** A firefly's velocity is a sum of slow sines of time, on
  rates and phases taken from its slot index: a circle of 15 to 30 s, a
  slower one a third its size on top, and a slow rise and fall. It reads
  no flow field and no noise. Its speed stays between 0.6 and 1.3 of its
  own drift speed, which is 0.6 to 1.4 of the knob's, and its direction
  turns half a degree a frame at most. At the default drift its loop is
  about a meter across.
- **Light.** Each glows from 12% of its peak to its peak and back on its
  own period. The tint is a warm yellow to a yellow green, fixed: the
  cloud did not take the theme's colors either, and a warm point reads
  against Aurora's greens and against a black room alike.
- **Music.** The loudness lifts the light by up to 60% and the radius by
  up to 35%, each times the music knob (default 0.5). The light reads the
  loudness as the flash limit lets it through (`flash.rs`), and the sim
  adds no envelope of its own and reads no beat phase, so the fireflies'
  light only ever moves the way the limit moves the level. The radius
  reads the loudness as the limit glides it, so it swells and does not
  step.

### The hand particles

A throw's burst and the pitcher's pour are born as they were
(`instruments.rs`). The streak's 300 particles a frame are born along the
stretch the throw flew since the frame before (`Burst::back`, carried in
the pour row at speed 0), not in a 3 cm ball at the frame's point: a frame
of flight is 8 cm, and with nothing stirring the particles the balls stayed
a row of clumps all the way down. What else changed is how they move.

- **No jitter.** Nothing pushes a hand particle but gravity, the air's
  drag, the hands and the surfaces. There is no flow field, no turbulence
  and no kick on the beat.
- **Gravity** is an acceleration now, 2.5 m/s² by default, where the cloud
  had a 0.5 m/s settle drift. A burst arcs and falls in about a second.
- **No bounce.** Where a particle meets a box or the depth map the speed
  into the surface is taken away. On a face that holds it up it slides,
  slows and ages three times as fast, so it fades where it lies.
- **Edge flow.** An edge is where a face that holds a particle up meets a
  second surface: the floor and a wall, the floor and a box standing on
  it, a table's top and its rim, a table's top and the wall behind it. The
  collide already tests the boxes near a particle. It now also notes each
  box face within 5 cm, and the rims within 5 cm of a face that holds the
  particle up. With a face under it and a second surface across it, the
  particle is steered along the edge at 0.18 to 0.42 m/s and in toward
  it, a rim brakes it and stops it short of sliding off, and it ages at
  the clock's rate, so it runs until it fades. Which way it runs is settled
  once, from the way it was moving, and kept, so at a corner it turns onto
  the next edge and goes on round. Two upright surfaces of two boxes (two
  walls, a wall and a cabinet) make an edge it slides down.
- **Where it cannot work.** A box's own upright edges are not edges here.
  The depth map gives a surface but no edge: a particle on something only
  the depth map knows lies there and fades. In roam with no room there is
  the stage floor alone, and the particles arc, fall and fade. Live planes
  are boxes like any other, so in roam an edge is there as soon as a plane
  is found.
- **The hands** still push the particles they touch, but not in a
  particle's first half second: a burst is born at the pinch point, inside
  the hand's pad, and used to jump out of it.

### The swipe, the menu and the toggle

- **The thumb swipe steps the theme**: right is the next, left the one
  before, wrapping, from the theme the menu's control shows
  (`themes::Shown::step`, `lanes::RoomLanes::step_theme`). It is the path
  the Theme control and the spoken "next theme" take, and it saves with
  the room. The caption names the new theme ("Aurora"). The first run's
  swipe hint keeps its words and is taught by a swipe that changed the
  theme.
- **The hand menu** has no effect row. It was six rows, five on the
  release surface, until the Limit flashing row went in at the top (board
  #5534): seven and six.
- **The Particles toggle** hides the fireflies and the hand particles,
  with Edit room on too. The sim steps on, so on shows them as they would
  have been.
- **The voice** answers a sentence that asks for an effect with "The
  effects are gone" (`VOICE_DESIGN.md`, "The effects are gone").

### The knobs

Each is polled once a second and logged on a change
(`fireflies::Knobs`), for a worn tuning pass.

| Knob | Default | Range | What |
|---|---|---|---|
| `debug.fosfora.fireflies` | 2000 | 0 to 3000 | How many fireflies. Lowered, the ones past it fade out over 1.5 s. |
| `debug.fosfora.fireflysize` | 0.02 | 0.002 to 0.04 | Sprite radius, m. Each is 0.7 to 1.3 of it. |
| `debug.fosfora.fireflygain` | 0.8 | 0 to 2 | Opacity at the top of a firefly's rhythm with no music. |
| `debug.fosfora.fireflydrift` | 0.06 | 0 to 0.5 | Drift speed, m/s. |
| `debug.fosfora.fireflymusic` | 0.5 | 0 to 1 | How strongly they answer the music. 0 is not at all. |
| `debug.fosfora.handgravity` | 2.5 | 0 to 9.81 | The hand particles' gravity, m/s². |
| `debug.fosfora.edgeflow` | 1 | 0 to 2 | Edge flow. 0 is off. It scales the 5 cm band and how fast a particle turns onto the edge. |

The size is twice the sprite a thrown burst has, picked from the headset's
captures at 0.01, 0.02 and 0.03 (`MEASURED.md`), and the drift is in the
family of the cloud's 0.05 m/s start speed. The count, the brightness, the
music's share, the gravity and the edge flow's band, speed and steer are
guesses. `debug.fosfora.density` (the debug panel's "firefly density") and
the governor scale the count. `debug.fosfora.count` sets the pool, under
its cap, and `debug.fosfora.burstcount` a throw's burst, under its own.

### What it should cost

Not measured. With nothing alive the app runs 7.05 ms a frame seated on
Aurora and 6.73 ms in roam (`MEASURED.md`, the reboot table), and Flux
Cloud's 300,000 sprites added 1.9 to 2.5 ms to that, sim and draw. Scaled
by count alone, a full pool of 20,000 is under 0.2 ms. The sprites are
twice the cloud's radius, so if the whole of the cloud's cost were fill the
pool would count four times over: 0.07 ms for 2,000 fireflies, about 0.4 ms
for the fireflies and a throw in flight (some 9,000 hand particles), and
about 0.7 ms for a pool hammered full, or 0.9 ms on a hot headset. The
truth should sit between the two scalings, under 1 ms in every case. A
firefly's step samples no texture and tests no box. A hand particle's
tests the boxes in its cell and the depth map every other frame, as the
cloud's did.

### Checking it unworn

With `debug.fosfora.begin 1`, on the debug APK:

- The fireflies are there from the launch. The log has
  `fireflies: 2000 fireflies (cap 3000) · …` at launch and once a second
  `particles alive 2000 of a pool of 20000 · fireflies asked 2000 (cap
  3000)`.
- Each knob logs `fireflies: … (was: …)` within a second of a `setprop`.
  `debug.fosfora.fireflies 500` thins them, and the alive count follows.
- `debug.fosfora.throwtest 3` throws every 3 s from ahead of the view.
  The alive count rises by the streak and the 6,000 of the burst and
  falls back within 6 s. `debug.fosfora.handgravity` and
  `debug.fosfora.edgeflow` change the next throw.
- `debug.fosfora.swipe right` logs `microgesture: swipe right (right
  hand) -> next theme`, the lanes' `theme … (a thumb swipe)` line and
  `the caption reads "…"`. `left` steps back.
- `debug.fosfora.say 'next effect'` logs the reply "The effects are
  gone".
- `debug.fosfora.cloud 0` logs `particles off: the fireflies and the hand
  particles are hidden`.

### Tests

`fireflies.rs`: `the_caps_hold`,
`each_knob_keeps_its_range_and_a_bad_value_the_default`,
`the_rows_carry_the_knobs_in_the_sims_lanes`,
`the_sim_reads_the_rows_the_app_writes`,
`the_sim_has_no_jitter_no_bounce_and_no_beat_phase`,
`the_preset_is_the_hidden_world_effect` and
`a_burst_steals_from_the_hands_share_alone`.

On a desktop GPU, `#[ignore]`d (`cargo test -p fosfora-xr --test
fireflies_gpu -- --ignored`):
`neither_cap_gives_under_a_pool_hammered_with_bursts`,
`the_count_knob_fills_and_thins_the_fireflies`,
`a_firefly_drifts_slowly_and_never_turns_on_a_frame`,
`the_music_lifts_the_light_and_the_size_a_little`,
`a_hand_particle_arcs_falls_and_does_not_bounce`,
`a_hand_particle_runs_along_the_foot_of_a_wall`,
`a_hand_particle_runs_round_a_tables_rim`,
`a_hand_particle_slides_down_a_corner`,
`a_burst_takes_the_hands_slots_and_never_a_firefly` and
`the_preset_runs_through_the_cores_particle_system`, which loads the
preset through the core's loader and draws it with `render_world`.

Beside them: `microgestures.rs`'s
`a_swipe_steps_the_theme_both_ways_and_wraps`, `themes.rs`'s
`names_and_the_cycle`, `lanes.rs`'s
`the_control_steps_through_the_themes_and_wraps`, `first_run.rs`'s
`the_swipe_caption_names_the_new_theme` and
`the_swipe_hint_is_taught_by_a_swipe_that_changed_the_theme`,
`panel_grid.rs`'s menu tests, `room_edit.rs`'s
`the_particles_follow_the_toggle_alone`, `surfaces.rs`'s cycle tests,
`intent.rs`'s `the_effect_phrases_answer_that_the_effects_are_gone` and
`the_help_lists_nothing_that_is_gone`, `agent.rs`'s
`the_room_names_no_effect_and_no_particle_behavior`, and the two that an
old file still loads: `palm_panel.rs`'s
`a_menu_file_that_names_a_cut_effect_still_loads` and `room_file.rs`'s
`a_room_that_names_a_cut_effect_still_loads`.

### Left in place

The core's tests read the cut effects' four presets and two sims
(`flux_xr_world.pfx`, `flux_xr_world_room.pfx`, `flux_xr_coarse.pfx`,
`murmur_xr_world.pfx`, `flux_xr_sim.wgsl`, `murmur_xr_sim.wgsl`), so the
files stay in the repo. The APK leaves them out. In the XR crate these are
still built and nothing reaches them: Flock's hand poses and holds
(`pose::decide`, the hand lanes), Embers' surface weights and the room fit
of its space size, the soloed surface (`room_edit::solo`), and the
behaviors embers and sparks, which a saved room may still name and which
show nothing. Their knobs are still read.

### What only a worn check can tell

Whether 2,000 reads as sparse or as busy, and whether 20 mm at 0.8 reads
with one eye against a black room and against a dimmed one. Whether the
loudness lifting the light reads as a swell: it rises on the frame the
level does, and the size glides. Whether the fireflies gliding after the
wearer in roam reads as their own motion or as the layer being dragged.
Whether 2.5 m/s² feels like falling or like sinking, and whether a 5 cm
band and 0.3 m/s make the edges read as lit lines or as stragglers. What
the frame costs.

## The sweep: lines of light from the wearer on the downbeat (board #5511)

*A prototype behind a knob, on the `xr-sonar` branch. Built and tested on
the desktop, on a desktop GPU too, and the device code passes the Android
lint. Nothing here has run on the headset: every speed, width and time
below is the code's constant, not a measurement, and most are a first
guess for a worn tuning pass.*

**Why.** Worn, the depth light's points were too sparse. The things the
room scan does not know (a chair, a shelf, a pile, a vacuum) did not read
as shapes, and they did not move with the music enough. The sweep is one
way to make them read as forms and flow. The code is `sonar.rs`, and the
drawing is in the depth light's shader (`env_depth::ENV_DEPTH_WGSL`).

### What the wearer sees

- **On a downbeat** a thin shell of light leaves the head and travels out
  through the room. Where it crosses a thing the depth light draws, that
  thing shows a bright line: its points at one distance from where the
  sweep began. As the shell moves on, the line crawls over the thing and
  traces its shape, the way a contour line on a map does. Behind the line
  the points stay swollen and bright for a moment and then settle, with a
  faint glow under them, so a thing just crossed glows and fades. Between
  two things nothing shows.
- **During a build** standing lines come up between the sweeps: faint
  lines of equal distance from the wearer, drifting slowly outward. As
  the build grows a second set fades in halfway between them and then a
  third, so the spacing tightens from 0.6 m to 0.3 m to 0.15 m near the
  wearer. This is the look's way of saying something is coming.
- **On the drop** the standing lines fade in 0.3 s and one sweep at full
  strength leaves. See "What it cannot do": on the headset no drop reaches
  the sweep yet.

### A sweep

- **Origin.** The head's position when it started, fixed in the room from
  then on. Turning or moving the head does not move a line already
  traveling.
- **Speed.** Fixed at its start: 4 m in one bar of four beats at the
  song's tempo, between 1 and 3 m/s (60 and 180 BPM), so sweeps follow
  each other 4 m apart. 2 m/s before the first tempo lock.
- **Starts.** On a downbeat the flash limit let through, while the music
  is not quiet (the immersion's quiet, or the show waiting for the
  wearer). Never less than 0.7 s after the last start, whatever asks, and
  never with three alive. A sweep is alive until its radius reaches
  4.5 m.
- **The line.** 2 cm to each side near the wearer, and three times the
  depth map's noise model (0.002 times the squared range, `MEASURED.md`)
  where that is more: 5.4 cm to each side at 3 m, 9.6 cm at 4 m. Its light
  is all there to 1.5 m, half at 3 m and gone at 4.5 m. A downbeat's sweep
  is at 0.7 of a drop's. The peak alpha runs from 0.25 with the room plain
  to 0.9 with it black, times `sonargain`.
- **The tail.** The light behind the crest falls to nothing over the
  distance the shell travels in `sonartail` seconds: 0.35 s, 0.7 m at
  2 m/s. A point under the crest or the tail grows by up to 35% of its
  radius, as far as half a cell lets the largest, and brightens to the
  line's peak whatever the distance had dimmed it to. The solid glow under
  the points is 0.12 of the line's.
- **Color.** The depth light's own: the color the theme gives other
  surfaces, with the crest tinted 0.6 of the way toward the key's tint
  where the theme uses it. Aurora draws a teal line with a crest toward
  the key.
- **Off the mesh's staircase.** The solid light fades out toward a mass's
  edge and on a triangle of the mesh seen within about 7 to 17 degrees of
  edge-on. That is the fringe that leans back at a step between two
  depths. A line there traced the mesh's steps and not the thing in
  renders of a depth recording, and it is where the map changes most from
  one frame to the next.

### The standing lines

- **When.** While `buildup` is over the level the immersion reads as
  tension (`immerse::BUILD_FROM`, 0.4). They are at full strength from a
  `buildup` of 0.58. The second set fades in from 0.49 to 0.67 and the
  third from 0.70 to 0.91. They center on where the head was when they
  came up.
- **The pattern limit.** Regular light and dark stripes are a hazard of
  their own, beside the flash limit, unless they flow smoothly one way.
  So the standing lines never flash, reverse or stand still. They drift
  outward at 0.12 m/s all the time they show, the way the sweeps travel,
  and the drift never turns back (half that speed while the flash limit
  slows the looks' own motion). The spacing tightens by sets that fade in
  between the lines, so no line moves inward when it tightens. Their
  strength changes only by slow ramps: 1.5 s from nothing to all, and the
  build the sets read moves at most a quarter of its range a second. The
  one short ramp is the drop's 0.3 s, after which they stay away for 4 s.
- **Faint.** A tenth of a sweep's line, and never over an alpha of 0.09
  whatever the gains. That is under the change of luminance the flash
  rule counts, so a line drifting over a wall that faces the wearer, which
  lights all of it at once, is no transition by itself.
- **How many.** The finest spacing is 0.15 m and the code holds it over a
  floor of 0.12 m. The third set shows only to 2.5 m and the second to
  3.5 m, where a line is still thin against its spacing. A surface that
  faces the wearer and fills the view (104 by 96 degrees) shows, from its
  nearest point out to its far corner:

  | Distance | Loosest (the build just begun) | Tightest (a full build) |
  |---|---|---|
  | 1 m | 2 | 7 |
  | 3 m | 1 | 2 |

  The most at any distance is 8, at 1.2 m. A straight cut from corner to
  corner through the nearest point crosses each line twice.

### The flash limit

A sweep starts on a trigger the limiter let through (`flash.rs`), as the
floor's rings do, and then runs by itself. The standing lines read
`buildup` as the limiter hands it on. The limiter lets a downbeat through
at most every 0.75 s, and the sweep's own floor is 0.7 s whatever asks,
the test knob included. So a place sees a line pass at most twice in any
second, and each pass is one rise and one fall: two flashes at most from
the sweeps, on a surface that faces the wearer and lights all at once.

The limit is the wearer's setting since board #5534, on unless they turn
it off ("The first run" below). With it off the downbeats and `buildup`
reach the sweep as the analysis has them. The sweep's own 0.7 s floor
does not depend on the limit, so the two passes a second still hold.

`flash_room.rs` counts it on the depth light's Rust copy (which the GPU
test holds to the shader), behind the limiter as `app.rs` runs it: 10
degree regions on a facing wall 1.2 m away, the floor, a far wall and a
slanted wall, at 72 frames a second.

| Source | General flashes in the worst second | Red |
|---|---|---|
| Pulse trains at 2, 4, 8 and 16 Hz | 1.5 | 0 |
| The same, a sweep every 0.7 s, `sonargain` and `depthlightgain` at 4 | 2.0 | 0 |
| No beat, a sweep every 0.7 s, both gains at 4 | 2.0 | 0 |
| A full build with a drop asked on every frame | 1.5 | 0 |

### The knobs

Each is read at launch, polled once a second and logged on a change
(`sonar::SonarOptions`).

| Knob | Default | Range | What |
|---|---|---|---|
| `debug.fosfora.sonar` | 1 | 0, 1, 2 | 0 off: the depth light as it was. 1 the depth light's things. 2 is for the scanned room too, which is not built, so it draws as 1. |
| `debug.fosfora.sonarspeed` | 1 | 0.25 to 4 | A factor on a sweep's speed. |
| `debug.fosfora.sonarwidth` | 1 | 0.25 to 4 | A factor on the lines' width. |
| `debug.fosfora.sonargain` | 1 | 0 to 4 | Brightness of the line and of the points under it. |
| `debug.fosfora.sonartail` | 0.35 | 0 to 2 | The tail, s. 0 leaves the crest alone. |
| `debug.fosfora.sonarcontours` | 1 | 0 to 1 | The standing lines' strength. 0 never shows them. |
| `debug.fosfora.sonartest` | unset | 0.7 and up | A sweep every that many seconds whatever the music does, for unworn captures. |
| `debug.fosfora.sonarbuild` | unset | 0 to 1 | The build the standing lines read, held there whatever the music does, for unworn captures. |

The 0.7 s between starts, the three alive and the 4 m a bar are the
requirement's. The noise model is measured. The rest are guesses: the
speed's range, the 2 cm, the three noise widths, the fade from 1.5 to
4.5 m, the tail, the glow, the strengths and alphas, the swell, the tint,
the spacings and where the sets come in, the drift and the ramps.

### Off is off

With `debug.fosfora.sonar 0`, and on every frame with no sweep alive and
no standing line showing, the sweep's rows of the uniform are all 0 and
the mesh draws with `fs_mesh_light`, which is left exactly as it was
before the sweep. The sweep has its own fragment, `fs_mesh_light_sonar`,
in a second pipeline, and the pass takes it only on a frame something
shows. A first version put the sweep in `fs_mesh_light` behind a branch on
a uniform, and with the rows at 0 it still wrote 30,126 values that
differed from before on an RTX 4090: the compiler contracts the same
arithmetic differently once the function changes shape. The GPU test holds
`fs_mesh_light` to a copy of the old fragment bit for bit.

### What it should cost

Not measured. It adds no pass, no render target and no texture read. Per
fragment of the mesh that draws the points (the faces layer's, at half the
eye's resolution, by default), on a frame a sweep or a standing line
shows:

- two more derivatives, a cross product, three dot products, a division
  and two smoothsteps, for the solid light's share;
- per live sweep, at most three: a square root, a smoothstep and about a
  dozen multiplies and adds. They are three calls on plain rows, with no
  loop and no array;
- for the standing lines, while they show: a square root, two divisions,
  two floors, two smoothsteps and about thirty multiplies and adds;
- for the blend: two more color mixes and about fifteen multiplies and
  adds.

With one sweep alive that is about as much arithmetic as one more read of
the points' tile without the read. The points with the whole map lit cost
at most 0.8 ms (`MEASURED.md`), so the sweep should sit well under that.
The uniform grows by 12 rows, 192 bytes, written where the light's rows
already are.

### Checking it unworn

On the debug APK, with the depth light drawing (something unscanned in
view, or `debug.fosfora.geomfirst 0`):

- The log has `sonar: on, the depth light's things · …` at launch, and
  each knob logs `sonar: … (was: …)` within a second of a `setprop`.
- `debug.fosfora.sonartest 2` starts a sweep every 2 s with no music. At
  debug level the log has `sonar: a sweep leaves (Test)` for each, and
  `(Downbeat)` with music.
- `debug.fosfora.sonarbuild 0.7` brings the standing lines up at the
  middle spacing, and `1` at the tightest, within 2 s. Unset, they follow
  the music.
- `debug.fosfora.sonar 0` leaves the depth light as it was.

### Tests

`sonar.rs`: `a_sweep_starts_on_a_downbeat_from_the_head_and_keeps_its_origin`,
`starts_keep_the_gap_whatever_asks`,
`at_most_three_are_alive_and_none_starts_in_silence`,
`the_speed_crosses_four_meters_a_bar_within_its_range`,
`the_line_is_a_crest_with_a_tail_behind_it`,
`the_line_widens_with_the_maps_noise_and_fades_with_distance`,
`the_contours_come_up_with_the_build_and_tighten_by_halves`,
`the_spacing_has_a_floor_and_a_full_view_shows_few_lines`,
`the_drift_runs_outward_and_never_reverses`,
`a_drop_fades_the_contours_and_sends_a_full_sweep` and
`the_knobs_read_within_their_ranges_and_say_so`.

`flash_room.rs`: `the_sweep_over_the_depth_light_stays_within_three_flashes`.

On a desktop GPU, `#[ignore]`d (`cargo test -p fosfora-xr --test
depth_light_gpu -- --ignored`):
`with_the_sweep_off_the_light_draws_what_it_drew_bit_for_bit`,
`the_sweep_changes_the_lights_things_alone`,
`a_line_sits_at_its_radius_from_where_the_sweep_began`,
`the_swept_fragment_matches_its_cpu_mirror` and
`the_recorded_room_under_the_sweep`, which renders a depth recording with
`FOSFORA_SONAR_REC=<file.lprec>` and writes the pictures to
`FOSFORA_SONAR_DUMP=<dir>`.

On a recording of a still headset (`debug.fosfora.liveplanesrec`, 64 by 64
read-backs a quarter of a second apart), with the lines held still, this
much of their light moved from one read-back to the next:

| Lines | Their half-width | Moved, mean and worst of 8 | A shift of a quarter of the half-width moves |
|---|---|---|---|
| A sweep 0.75 m out | 2.0 cm | 0.11, 0.14 | 0.37 |
| A sweep 2.75 m out | 4.5 cm | 0.06, 0.08 | 0.38 |
| A sweep 3.75 m out | 8.4 cm | 0.15, 0.19 | 0.39 |
| Standing, a mid build | 2.0 cm | 0.16, 0.18 | 0.29 |
| Standing, a full build | 2.0 cm | 0.15, 0.17 | 0.31 |

So the map's noise moves a line by about a tenth of its half-width, a
couple of millimeters near. At 1.75 m that recording has almost no
surface: what the sweep lights there is a few silhouette cells, and about
half of that little light changes between read-backs.

### What it cannot do

- **The scanned room.** It draws only where the depth light draws. With
  geometry first on, that is not the scanned walls, floor and tables.
  Drawing it there (`sonar 2`) is not built. The faces pass draws three
  kinds of fragment (the surfaces pass's, each ported effect's, the
  sky's), and a branch in each would change what they write with the
  sweep off, as it did here. The way that keeps off off is one more small
  pipeline in the faces pass: each scanned face a shell or its tail
  reaches, drawn once more with the sweep's lines alone, added over what
  is there and scaled by one less the immersion level.
- **The drop.** On the headset the hop is built with no drop
  (`audio.rs`), so no drop reaches the flash limit, and the full sweep and
  the 0.3 s fade never fire there. The standing lines then leave by their
  slow ramp when the build ends. The same gap decides whether the
  immersion's drop hold runs, so it is left for a decision of its own.
- **Songs without downbeats.** A song the beat tracker has not locked
  onto gets no sweeps.
- **Facing and grazing surfaces.** A surface that faces the origin
  squarely is all at one distance, so it lights at once instead of showing
  a line. A surface seen nearly edge-on (the floor far away, a wall seen
  along its length) carries the points' share and no solid line.
- **Thin things.** The line is as coarse as the depth map's mesh, and a
  thing narrower than a mesh cell may miss it.

### What only a worn check can tell

Whether a line 4 cm across that moves 2.8 cm a frame reads as a line that
crawls or as a row of separate lines, with one eye. Whether the sweep
makes a pile or a chair read as a form, or as a flash that passes. Whether
0.7 against the points' own light is enough with the room black, and too
much with it half visible. Whether the glow behind the line reads as the
thing glowing or as a fill with the mesh's blocky edge. Whether the
standing lines read as tension or as clutter, whether their drift is seen
at all, and whether the map's noise makes them shimmer. Whether a sweep on
every bar is too often. What the frame costs.

## The room first pass (board #3317)

*The first pass of `ROOM_DESIGN.md`: surfaces as emitters and the floor
ripple.*

### Surfaces as emitters

*Cut by board #5470 ("Fireflies and the hand particles", below). Embers,
the effect that spawned on the room's surfaces, is gone, and no sim the app
runs reads the emitter weights. The lanes are still written and the code is
still there. What follows is the record of the pass.*

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

- **What.** Two instruments on the hand (the Go-Go far hand while Edit
  room is on, "The reach" below) and the poses: a pinch
  tap throws a burst that flies to where the hand points and bursts on the
  surface it hits, and an open palm held still, facing down, lifts the
  particles lying under it toward the palm. They were built on the Flux
  world sims. Since board #5470 their particles are the fireflies sim's
  hand particles, which reads the same rows; how they move after they are
  born is in "Fireflies and the hand particles", below. The drag that
  moves the world effect's home is the anchor drag, as before. Host module
  `crates/fosfora-xr/src/instruments.rs` (the ray cast, the flights, the
  lift and the rows, host-tested).
- **The reach** (board #5531, `reach::live`). The Go-Go far hand was
  made for particle effects that are gone, so it is live only while Edit
  room is on. With the editor off each hand is its real hand: no offset,
  no ghost, no beam from the wrist, and the throw, the lift and the
  pitcher act at the real pinch and palm. With the editor on the reach
  is as it was, for both hands. `debug.fosfora.reach 0` still turns it
  off everywhere, and 1 now means "with Edit room". Turning the editor
  on or off with an arm stretched does not jump: the extension glides
  through the reach's own low-pass (0.05 s), 63% of the way in 0.05 s
  and 95% in 0.15 s. From a full stretch (1.94 m of extension) the ghost
  is gone in 0.22 s and the hand is exactly the real hand in 0.39 s. The
  log says `reach: on with Edit room` and `reach: off with Edit room`.
  Tests (`reach.rs`): `the_reach_is_live_only_while_the_room_is_edited`,
  `turning_the_editor_on_or_off_with_an_arm_stretched_does_not_jump`.
- **What the reach is worth, in numbers**
  (`without_the_reach_the_same_arm_aims_about_a_meter_lower_on_a_wall_3_m_away`).
  The editor's ray and the throw's run from the head through the pinch
  point, so both are a line of sight with or without the reach. Seated,
  the right arm at a full stretch level with the shoulder, a wall 3 m
  ahead: the ray lands 0.96 m up the wall with the reach and 0.05 m up
  without, 1.14 m apart. Raised 20 degrees: 2.03 m and 1.06 m. A
  centimeter of hand to the side moves the aim 6 cm either way. Without
  the reach the wearer aims by holding the pinch over the target, at eye
  height for a point at eye height. A throw from the level arm hits the
  foot of that wall after 2.75 m and 0.46 s, where the far hand hit it
  0.96 m up after 0.55 m. A hand low in front lands its throw on the
  floor 1.04 m ahead, against 1.39 m with the reach. A hand within 0.30 m
  of the shoulder throws the same either way.
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
- **The switch** (board #5531, `instruments::Play`). Whether the throw
  and the lift stay in the product is open, so one switch covers both:
  `debug.fosfora.throw 0`. At 0 a pinch throws nothing and a palm lifts
  nothing. Everything else a pinch does is untouched: the begin, the
  hand menu's rows and the room editor. The default is one constant,
  `instruments::THROW_DEFAULT`, so taking them out is a change of that
  value. The launch's `instruments:` line says `throw off` and `lift off
  with the throw`. The pitcher and the `throwtest` diagnostic do not read
  the switch. Test: `one_switch_turns_the_throw_and_the_lift_off`.
- **Knobs.** `debug.fosfora.throw` (0/1, default 1, the lift with it),
  `burstcount` (6000), `lift` (0/1, default 1, the lift alone),
  `liftradius` (0.35).

## Hands: the gesture vocabulary (board #3336)

*Board #5470 changed what two of these gestures reach. There is one world
effect now, so nothing cycles it: the hand menu has no effect row, and the
thumb swipe steps the theme. The audit and the proposal below are as they
were written.*

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
  hand keeps the world vocabulary. While the hand menu shows (the left palm
  summons it; since board #5459 it stays in the air, "The hand menu: summon
  and stay" below), the right hand's pinches are the menu's: a ray pinch
  clicks or drags on the panel, a fingertip pokes it.
- **The poses** (`pose.rs`, with 0.1 s dwell and hysteresis): fist (finger
  curl under 55 mm), palm up (normal within 37° of up), open; hands together
  under 0.30 m. Murmur's hand behaviors read them; the Flux lift reads an
  open far palm held still, facing down. With voice on (board #3751, V4)
  a fist on either hand holds the voice window everywhere (board #5531),
  so Murmur sees open hands and no fist is a predator. With voice off,
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
rows. (Board #3776 added the Agent row under the effect's: the menu is six
rows, nine with Edit room on, 284 and 404 points; the debug panel 12 and
15, its header allowance 320 → 300 points, the dumps' header being about
250.) (Amended Oct 20, board #4192: Recenter and Rescan, the debug panel's
row alone until then, joined the menu under the effect's row, over the
Agent's, so the scan label's "Rescan from the hand menu" has a row to
find: the menu is seven rows, ten with Edit room on, 324 and 444 points;
the debug panel stays 12 and 15, the row being one it had.) Step 5: the
editor's hold is armed first. A first right hold on a
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

**Since board #5470.** The effect row is gone with the effects it stepped.
The menu is six rows, nine with Edit room on (284 and 404 points), and the
release surface's five and eight (244 and 364). Recenter and Rescan are the
top row. The debug panel is 11 and 14 rows and its header keeps one row of
headroom. The bottom rows are where they were.

**Since board #5534.** The Limit flashing toggle is the top row, on both
surfaces: one button across the row that reads "Limit flashing: on" or
"Limit flashing: off" (`flash_choice::row_label`). It is the wearer's
photosensitive setting ("The first run" below, and `SURFACES_DESIGN.md`,
"Photosensitive safety"). A press changes it at once and saves it in
`hand_menu.json` as `"limit_flashing"`; nothing confirms an off. The menu
is seven rows, ten with Edit room on (324 and 444 points), and the release
surface's six and nine (284 and 404). The debug panel is 12 and 15 rows,
which uses the header's last row of headroom. The row went in at the top,
so the rows a wearer already reaches for are where they were.

### The hand menu: summon and stay (board #5459)

The app is used lying back in a chair. The menu used to live on the left
palm and hide when the palm turned away, so every press needed both arms
raised for as long as it took, and nothing in the menu worked with one
hand. Now the palm summons the menu and the menu stays in the world
(`palm_panel.rs`, "Summon and stay"; `hud.rs`). Knob
`debug.fosfora.menustay 0|1`, default 1, polled once a second and logged
on each change. 0 is the menu on the palm as it was, so the two can be
compared worn in one sitting.

- **Summon.** The left palm turning to the face (cosine to the head over
  0.6, and facing the head at least as much as the ceiling) shows the menu
  at the palm, and it follows the palm as before. Showing the palm again
  later brings it back to the palm from wherever it is, at once; a pointer
  on it holds it still until the pointer leaves.
- **Stay.** The palm turning away (cosine under 0.3, or facing the ceiling
  0.1 more than the head) or the hand dropping out of tracking leaves the
  menu where it is. It turns to face the wearer once, then holds still,
  and the right hand's beam or fingertip works it as before.
- **The spoken open.** "Menu" or "open the menu" (also "show the menu",
  "bring up the menu", "hand menu") parks it 45 cm ahead of the head along
  the view and 5 cm below, facing it, with no palm, so one hand works it.
  "What can I say" lists "open the menu" on its first page.
- **The closes.** There is no new row. In the air, no pointer on it for
  8 s closes it; on the palm the count never runs. A pinch that starts
  away from it closes it: the right hand's ray landing outside the panel
  grown by half its size (1.5 times its half extents) or missing it. The
  left hand's pinch never closes it, since that hand is on its way down
  after summoning the menu. The closing pinch is masked from the
  gesture machine until it is released, so it does nothing else. A pinch
  that just misses a row, inside that margin, does nothing and the menu
  stays. "Close the menu" (also "hide the menu", "menu off") closes it. A
  close while the palm is still up holds until the palm turns away and
  back.
- **What it owns while up.** The same as the palm's menu: both hands'
  pinches for the cloud gestures (one already in progress goes on), the
  throw, the room editor's tap, hold and beam (held on its surface),
  Murmur's left-hand pose and two-hand hold, and the left hand's lift. The
  closes bound it: at most 3 s after the last pointer left it (frame time;
  a frame counts at most 1/30 s), and at once on a pinch away. Thumb
  swipes keep working while it is up with no pointer on it; with the knob
  at 0 they stop whenever it is up, as before.
- **The frame lying back.** `Placement::facing` builds the panel's right
  from world up crossed with the line of sight, which turns anywhere with
  the hand straight above the face. `Placement::facing_view` keeps world
  up's frame beyond 30° from the vertical and within it blends to the
  head's right projected onto the panel, whole within 15°, as the room
  labels do. The world-up right is turned to agree with the head's, so
  the panel never flips over as the line of sight passes the vertical. On
  the palm the panel also grows along its own up near the vertical
  (`Placement::above_view`), where world up would put it behind the hand.
- **The log.** "hand menu shown", "hand menu in the air", "hand menu on
  the palm", "hand menu parked ahead (spoken)", and "hand menu hidden" with
  why: "(no pointer for 3 s)", "(a right pinch away from it)", "(a left
  pinch away from it)", "(spoken)". With the knob at 0 the spoken open and
  close reply "The menu is on your left palm" and do nothing.
- **Tests (desktop).** In `palm_panel.rs`, the palm turning away leaves it
  in the air, the idle close, the pinch away closing and swallowed, the
  near miss doing nothing, the palm bringing it back, the spoken open and
  close, the knob at 0, and the frame overhead with the head looking up.
  In `intent.rs`, `open_and_close_the_menu`. The worn comparison is
  pending.

## The labels a new wearer meets (board #5461)

*Built.* Everything the app tells a wearer is on one in-world label
(`label.rs`), white text on a dark ground. One label shows at a time, in
this order: the voice window's, a thumb swipe's caption, the room
editor's at the surface, a hand menu reply, the room scan's, the begin
card or the opening's message, the controllers label, and last a first
run hint. Those up to the scan's hold the opening; the controllers label
and the hints never do. The begin card, the caption and the hints are
"The first run" below.

What a wearer can meet, in the order of a first launch:

| When | Words | Where | How long |
|---|---|---|---|
| The room's scene query runs | Scanning the room… | 1.2 m ahead, level | until it returns |
| It finds the room | Loading your room | same | 3 s |
| It finds none, roam goes on | No room here: roaming. Rescan from the hand menu | same | 6 s |
| The wearer said no to spatial data | Fosfora can't see the room. Allow spatial data in Settings, then restart Fosfora. | same | 6 s |
| The scan's label has gone, and the wearer has not chosen whether to limit flashing | This show has flashing lights that follow the music. They can trigger seizures in people with photosensitive epilepsy. Put a hand over one and pinch: Limit flashing / Full show | 1 m ahead, at the center of the view | until the wearer answers, 60 s at most |
| The question is answered or has had its time (on a later launch, the scan's label has gone) | Pinch to begin | same | until the wearer begins, 60 s at most |
| The music first plays, no label up | Sit back and relax | 1.2 m ahead, level | 4 s, then the show |
| No hand tracked since the launch, 4 s after the scan's label | Put the controllers down. Fosfora uses your hands. | under the begin card while the show waits, else 1.2 m ahead, level | until a hand is tracked; alone, 10 s at most |
| Music `<` or `>` with no songs, the bundled tracks missing too | No songs on the headset. Put them in its Music folder. | 1 m out, 20 degrees above the view | 4.8 s |
| The same, music permission missing | Can't read the Music folder. Allow music files in Settings. | same | 4.5 s |
| Room editor: a tap, a hold, a menu step | desk: sparks, all tables: none | at the surface | its reading time |
| Room editor: a first hold | desk: hold again for all tables -> curls | at the surface | the arm's 2.5 s |
| A voice reply or miss | theme: Aurora, Didn't catch that… | the voice window's place | its own time or its reading time |
| A thumb swipe stepped the theme | Aurora | the voice window's place | its reading time |
| A first run hint, the label free for 5 s | Swipe your thumb along your finger | the voice window's place | until the gesture is made, 20 s at most |
| The hint's gesture was made | Got it | same | 2.5 s |
| The session ends on an error | Fosfora stopped: the session was lost | 1.2 m ahead, level | 4 s, then the app exits |

**Reading time.** A label shows for 1.5 s and 0.3 s a word, at most 6 s
(`label::read_s`). Most labels held 1.5 s before, about a second at full
strength once the fade is taken off. A caller's own time is a floor
(`Label::show_for`): "Listening…" stays the whole voice window, a voice
reply its 1.5 s or its reading time, whichever is longer. The one label
held to its caller's time exactly (`Label::show_held`) is the editor's
"hold again": it goes when the arm's 2.5 s window closes, since it names
a second hold that would no longer work. Its seven words need 3.6 s, and
the second hold itself takes 0.7 s of the window, so a slow reader can't
finish it in time. The window's length is unchanged; widening it is a
separate decision.

**Lines.** A label of one line takes up to 88 px of its 1024 px texture.
One that would shrink below 64 px to fit wraps instead (`label::layout`):
a sentence a line, a sentence that still runs over onto as few lines as
it takes, as even as they come, a line ending at a colon when that costs
no extra line. Two lines take up to 76 px, more lines 64. The widths are
the font's own advances (`text_width.rs`, Ubuntu-Light, which egui lays
out without kerning), so the wrap is tested on the desktop. Before, "No
room found. Rescan from the hand menu" drew at 46 px on one line, "desk:
hold again for all tables -> streamlines" at 48, and the theme miss
(`No theme called "blue sky": Classic, Drift, Tide, Aurora or Mirror`)
ran past both ends at the 40 px floor. They now draw at 73, 76 and 64.

**Spatial data denied.** The OpenXR session stays idle under the
system's dialog (`MEASURED.md`), so when frames run the wearer has
answered it. The label held for the whole 120 s watch before, and the
opening waited behind it. It now holds 6 s and fades, and the show
starts without a room, in roam, as after a scan that finds none. A grant
made in Settings during the watch is picked up as it comes; a later one
needs a restart, which the words say. The watch's end, spatial data still
missing, shows no second label two minutes into the show.

**Controllers.** The app binds no controller profile and reads only the
hand-tracking extension, so a wearer holding the controllers has no
menu, no voice and no gesture. When no hand has been tracked since the
launch and the scan's label has been gone 4 s, the controllers label
shows after the opening's message and stays until a hand is tracked, 10 s
at most. Once a hand has been tracked, or the label has had its time, it
never shows again in that run, so a wearer whose hands rest out of view
isn't told to put down controllers they don't hold, and one lying back
with their hands at rest since the launch doesn't keep it in front of the
show. A wearer who picks the controllers up later sees
nothing. `debug.fosfora.handshint 0` keeps it off; `scripts/xr/sweep.sh`
and `soak.sh` set it for their unworn runs, where no hand is ever
tracked. The log says `controllers label: shown, no hand tracked since
the launch` and `controllers label: gone, a hand is tracked`.

**Music with no songs.** Music `<` `>` with no track in either folder
did nothing visible (the log said so). It now shows the reply on the
label, placed once at the press 1 m from the head along the view raised
20 degrees (`label::menu_reply_at`), so it sits over the menu the wearer
is looking at rather than behind it. The words name the permission when
the shared Music folder is there but unread for want of it
(`music::no_songs`); a spoken "next song" replies the same words. Board
#5528: with the bundled tracks there is always a song to step to, so a
wearer with no music of their own steps through those and never sees
these words. They show only when no bundled track plays either. Nothing
in the app then tells that wearer where their own music goes, or that
the Music folder is unread for want of the permission; "Putting files
there" above is where it is written down.

**The menu's words.** The Edges control reads "Edges: theme" in a cell
of its own and "Edges" over the state in half a cell (the developer
surface, beside "All: none"), where "Edges: theme" is 97 points in 90.5.
The editor's status cell is empty with Edit room off (it read "edit room
off" beside the "Edit room: off" button) and reads "Point at a surface"
with nothing under the beam. "Particles: off (shown: editing)", 208
points in a 187-point cell, is "Particles: off (editing)". A status too
long for its cell at 16 points takes two lines at 12 and is cut past two.
`panel_grid`'s `every_menu_word_fits_its_cell` measures every fixed word
against its cell.

**What only a worn check can tell.** Whether 76 and 64 px lines read at
1.2 m and at a wall; whether the menu reply sits clear of the menu for a
real palm and reach; whether the runtime reports no hands while the
controllers are held (the label's whole premise); how the session
behaves when the wearer leaves for Settings and comes back; and whether
the 4 s wait after the scan's label is long enough for a wearer who put
the controllers down at launch.

## The first run (board #5452)

*Built and tested on the desktop, and the device code passes the Android
lint. Nothing here has run on the headset yet: every time and size below
is the code's constant, not a measurement.*

A wearer who had never seen the app had no say in when the show started
and was never told what their hands could do. The music played while
they were still settling, and the swipe, the thumb tap and the palm menu
were found by accident or not at all. Now the show waits for them, and
each gesture is taught once, when there is room on the label for it. The
rules are `first_run.rs`, plain data tested on the desktop; `app.rs`
feeds them once a frame.

Since board #5534 a new wearer is also warned that the show flashes and
asked, before anything flashes, whether to limit it. That is "The flash
question" below (`flash_choice.rs`); like the rest of this section it has
not run on the headset yet.

### What the wearer meets

1. **The launch, as before.** The permission dialog, then "Scanning the
   room…" and "Loading your room" (3 s), or one of the no-room labels
   (6 s). From the first frame the room is dimmed a little: the immersion
   level goes to 0.25 over about a second and a half (`immerse::WAIT_LEVEL`,
   time constant `WAIT_S` 0.5 s). No music plays. The looks get silence in
   place of the audio, so no surface pulses. The fireflies drift and glow
   on their own rhythm, with no music to lift them.
2. **The flash question**, on a launch where the wearer has not chosen
   (board #5534). When the scan's label has gone the card at the center
   of the view reads, on six lines: **"This show has flashing lights"**,
   **"that follow the music. They can"**, **"trigger seizures in
   people"**, **"with photosensitive epilepsy."**, **"Put a hand over one
   and pinch:"** and, at the two ends of the last line, **"Limit
   flashing"** and **"Full show"**. The wearer holds a hand over one of
   the two, as they see it, and the fifth line changes to **"Pinch to
   choose Limit flashing"** or **"Pinch to choose Full show"**. A pinch
   then answers, with the press cue, and the card gives way to the next
   one. "The flash question" below has the rules.
3. **The card.** When the scan's label has gone, or the question is
   over, a card sits at the
   center of the view, 1 m ahead and 0.4 m wide: **"Pinch to begin"**.
   It follows the head the way the voice label does
   (`voice::Follow::centered`), so it is in view lying back and never
   rigid on the head.
4. **No hands.** If no hand has been tracked for 4 s with the card up,
   the controllers line goes under it: **"Put the controllers down.
   Fosfora uses your hands."** It stays while no hand is tracked and goes
   with the first one. It does not show on its own while the show waits.
   On the question card the fifth line reads **"Put the controllers
   down."** instead.
5. **The begin.** A pinch closing on either hand begins the show. It
   ticks (the press cue) and does nothing else (board #5531): it throws
   no burst, and no drag, no hold and no tap come from that pinch until
   it opens. The card goes, the music
   starts (the track was decoded during the wait), and the opening runs:
   **"Sit back and relax"** for 4 s at the wait's 0.25, then the glide to
   0.35 over 3 s.
6. **The swipe hint.** Once the label has been free for 5 s with the show
   on (about 9 s after the pinch), ahead of the view and 10 degrees under
   the line of sight: **"Swipe your thumb"** over **"along your finger"**.
   The wearer swipes, the theme steps with its tick, and the caption
   names it: **"Mirror"**. Then **"Got it"** for 2.5 s with the
   understood cue.
7. **The talk hint**, 5 s of free label later, only with voice on and the
   microphone allowed (board #5531; until then the tap hint, "Tap your
   thumb to talk."): **"Make a fist, say a command,"** over **"then open
   your hand."**, **"Or tap thumb on index finger."** and **"Try: what
   can I say"**. A fist opens the voice window and holds it, and the
   label reads "Listening…" over "Open your hand when done". A window
   with something said in it answers the hint. "Got it" waits 3 s for the
   label and is dropped behind an answer that stays longer, and its cue
   never plays into an open microphone.
8. **The palm hint**, last: **"Left palm to your face"** over **"for the
   menu. It stays there."** The palm brings the menu, then "Got it" and
   the cue.

A hint that has been in view for 20 s gives up. Made or given up, it is
saved as taught and never shows again on that install.

### The flash question (board #5534)

The app limits how fast its light may flash: at most three flashes a
second, whatever the music does (`flash.rs`, and `SURFACES_DESIGN.md`,
"Photosensitive safety"). The limit costs the show: fast beats land at
half time and the light moves in steps. So the limit is the wearer's
choice. It is on unless they turn it off, and a new wearer is warned and
asked before anything flashes. The rules are `flash_choice.rs`.

- **The setting.** "Limit flashing", on or off, saved in `hand_menu.json`
  as `"limit_flashing": true` or `false`. A file without the field means
  the wearer has not chosen: the limit is on, and the next launch asks.
- **When it is asked.** On a launch with nothing saved, as the first
  card, before "Pinch to begin". The show is still waiting behind it, so
  no music plays, the looks get silence and no sweep starts: nothing the
  audio drives lights while the card is up.
- **The words** are in "What the wearer meets" above: four lines of
  warning, one line that says how to answer, and the two answers named
  for what the wearer gets. All six lines draw at 64 px, the size of the
  voice's help card, and no state of the card moves the answers.
- **Answering by hand.** The wearer holds a hand over one answer, as they
  see it, and pinches. The app reads the line from the head through the
  pinching fingers to the card (`flash_choice::over`). Left of the gap
  between the answers is "Limit flashing" and right of it is "Full show",
  with a 6 cm dead band between them. Each answer reaches about 15 cm
  past the card's end, two lines up and three lines down. Either hand gives either
  answer, so one hand is enough. The hand has to be 32 to 90 cm from the
  head.
- **Not by accident.** A pinch answers only when its hand has been over
  the same answer for 0.3 s (`DWELL_S`). By then the fifth line names that
  answer, so the wearer reads what the pinch will do before making it. A
  hand at rest in the lap, a hand at the headset, a pinch in passing and
  the other hand's pinch answer nothing. A hand that slides from one
  answer to the other starts its 0.3 s again.
- **Which eye.** The app reads the head, and the wearer looks with an eye
  about 3 cm to one side of it. For a hand 32 cm out that moves what the
  app reads by up to 7 cm on the card. The 10 cm gap between the answers
  and the dead band are sized so that a hand covering any part of one
  answer, for either eye, never reads as the other. At worst it reads as
  neither and the fifth line does not change.
- **The pinch that answers does nothing else.** While the question is up
  the wait is handed no card and no start gesture
  (`flash_choice::wait_input`), and the wait begins on a pinch closing. So
  the pinch that answered has to open and close again to begin the show.
  It throws nothing: every pinch of the wait is held back until it opens.
- **Answering by voice.** "Limit flashing" or "full show", in a voice
  window opened by the thumb tap or the fist as anywhere else
  (`VOICE_DESIGN.md`, "Limit flashing by voice"). It answers whether the
  card shows or not.
- **Nobody answers.** The question waits 60 s (`ASK_MAX_S`), counted only
  while the card is the label showing. Then it counts as no answer: the
  limit stays on, nothing is saved, and "Pinch to begin" comes up with its
  own 60 s. The next launch asks again. Nobody chose, and a wearer who was
  not ready for the card should still get the choice. The cost falls on a
  wearer who can neither pinch nor speak: their show starts after two
  minutes instead of one, on every launch.
- **The show begun another way** while the question is up (a spoken
  "begin", "play" by voice, `debug.fosfora.begin 1` set live) ends it the
  same way: the limit on, nothing saved, asked again.
- **After the answer** the card reads "Pinch to begin" and the first run
  goes on as before.
- **Changing it later.** The hand menu's top row ("Limit flashing: on")
  and the same two sentences by voice. Either takes effect on its frame
  and is saved. Turning the limit off does not ask twice. A wearer who
  sets it from the menu has chosen and is not asked at a launch.
- **The knob.** `debug.fosfora.flashlimit 0|1` overrides the setting while
  it is set, for an A/B and for unworn runs. Unset, the setting rules.
  The log says which decided, at launch and on each change: `flash limit:
  on · … · the wearer's setting`, `· the wearer has not chosen`, or `·
  debug.fosfora.flashlimit 0 overrides the wearer's setting`. The
  measurement strobe (`debug.fosfora.flash 1`) still needs the knob at 0.
  The wearer's "Limit flashing: off" alone never starts it.
- **The log.** `flash question: asked before the begin, nothing is saved`
  at launch, or `not asked, the wearer's setting is saved (Limit flashing:
  on)`, or `not asked (debug.fosfora.begin 1), nothing saved by this
  launch`. Then how it ended: `"Full show", the limit off (saved) by a
  pinch on the right hand`, `"Limit flashing", the limit on (saved) by
  voice`, `nobody answered in 60 s: the limit stays on, nothing saved,
  asked again next launch`.

What was considered and left out for the hands' answer:

- **Which hand pinches** (left for one answer, right for the other). The
  wait can read it today, but a wearer with one hand could give only one
  answer.
- **A pinch against a held pinch, or one pinch against two.** A hand at
  rest can hold a pinch and tracking can flicker one into two, and the
  accident would be one of the answers.
- **The hand menu's ray or fingertip on a panel.** The menu's pointer is
  the right hand's only, and a second panel needs drawing that can't be
  checked without the headset.
- **A ray from the hand to the card.** The label draws no beam or cursor,
  so the wearer could not see what the ray is on. With a hand over the
  answer the hand itself is the cursor.

### The rules

- **Every launch waits**, not only the first. Until the begin a pinch, a
  drag, a thumb swipe and the hand instruments do nothing.
- **Before the begin the hands do one thing, begin** (board #5531,
  `first_run::Live`). The hand menu does not come: the left palm does not
  bring it (`PalmPanel::hold`), a spoken "menu" answers "Pinch to begin
  first", and it is not drawn. Nothing is thrown, lifted or poured. A
  palm turned to the face during the wait is not read when the show
  begins: it brings the menu once it has turned away and back. Every way
  to begin opens all of it together, the 60 s and the knob as the pinch
  does.
- **A pinch from the wait is no gesture** (`first_run::HeldPinches`).
  The pinch that begins the show, and one that was closed when the show
  began another way, is held back until it opens. Its release is no tap,
  so it throws nothing, presses no menu row and changes no surface in the
  editor.
- **Still live before the begin.** The voice window opens with a thumb
  tap or the left fist, and what is said there applies, the menu aside.
  A thumb swipe is read and steps nothing. With Edit room turned on by
  voice the editor's beam and labels show, and its pinches do nothing.
- **Ways to begin** (`first_run::Begun`): the pinch with the card up; a
  spoken "begin" or "start" (`VOICE_DESIGN.md`); the wearer starting the
  music by voice; the card unanswered for 60 s
  (`WAIT_MAX_S`), so nobody is stuck in front of it; and
  `debug.fosfora.begin 1`. The log says which: `first run: the show
  begins (a pinch on the right hand)`, `(a spoken begin)`, `(the wearer
  started the music)`, `(nobody answered the card in 60 s)`,
  `(debug.fosfora.begin 1)`.
- **The start gesture is in one place**: `first_run::start_gesture` and
  the card's word `START_WORD`.
- **The music.** A launch used to start the music itself. It now starts
  at the begin. If the wearer's last press was stop, the show begins
  without music, as a launch did, and the room comes back after 1.5 s
  (`immerse::WAIT_RELEASE_S`).
- **The depth.** The wait's dim is scaled by the Immersion control like
  the rest: off leaves the room plain, black is black from the launch as
  it was. `WAIT_LEVEL` 0 would leave the room plain for everyone.
- **Hints wait their turn** (`first_run::Hints`). One at a time, in the
  order above, after 5 s of free label (`HINT_GAP_S`), never over another
  label. A hint's 20 s (`HINT_GIVE_UP_S`) count only while it is the
  label showing. A hint whose gesture can't work in this run (voice off,
  no microgestures) is passed over without being saved, and shows in a
  later run. No hint shows before a hand has been tracked, so a wearer
  still holding the controllers doesn't use them up.
- **Taught by doing.** A swipe that steps the theme, a voice window with
  something said in it or the palm bringing the menu, made before its
  hint, saves that hint as taught and it never shows. Talking counts
  during the wait too. The palm does not, since it brings nothing then.
- **The talk hint teaches the fist** (board #5531, `VOICE_DESIGN.md`, "The
  fist holds the window"): "Make a fist, say a command," over "then open
  your hand.", "Or tap thumb on index finger." and "Try: what can I say".
  Until then the hint taught only the thumb tap and nothing taught the
  fist. A fist or a tap that opened a window nobody spoke in does not
  answer it.
- **Saved** in `hand_menu.json` as `"taught":["swipe","talk","palm"]`,
  written once one is taught. A file from before has taught none, and a
  file with the old `tap` has not been taught the fist.
- **The caption** (`first_run::swipe_caption`): every thumb swipe that
  steps the theme names the new one for its reading time, where the voice
  label floats: "Aurora", as the menu's Theme control shows it.

### The wearer who can't

| Wearer | What happens |
|---|---|
| Voice off | The talk hint is passed over. The swipe and the palm are taught. |
| Microphone denied | The same card and hints. A fist or a tap still shows "The microphone isn't allowed" as before. |
| Controllers in hand | No hand is tracked, so after 4 s the card says to put them down. A pinch then begins. Left alone, the show begins by itself after 60 s, and the hints wait for a hand. |
| One hand | Either hand pinches, swipes, taps and makes the fist that talks. The palm hint asks for the left hand; a wearer without it lets it give up after 20 s, and "menu" by voice opens the menu with no palm. |
| Can't hold a fist | The thumb tap opens the voice window and needs no held pose. The talk hint says so. |
| Can't pinch | With a thumb tap or a fist the voice window opens and "begin" starts the show. With no gesture at all, the 60 s. |
| Can't pinch, at the flash question | "Limit flashing" or "full show" by voice. With no gesture at all, its 60 s: the limit stays on, and the question comes back at the next launch. |
| One hand, at the flash question | Either hand reaches both answers. |
| Controllers in hand, at the flash question | The fifth line says to put them down. Left alone, its 60 s, then the begin card as above. |

A spoken "begin" needs the voice window open, and the window still opens
with a thumb tap or a fist. The app does not listen by itself while
it waits, so the card names the pinch only.

### Checking it unworn

- `debug.fosfora.begin 1`: no card and no wait. `sweep.sh`, `soak.sh`,
  `gpu-stages.sh` and `worn-ab.sh` set it, so their runs start the show
  as they did. It is also polled while the show waits. It skips the flash
  question too and saves no answer, so those runs keep the limit on
  unless the file or `debug.fosfora.flashlimit` says otherwise. Set while
  the question is up, it ends the question unanswered.
- `debug.fosfora.say 'begin'` begins a waiting show, and is fed without
  waiting for the room's anchors. Under the flash question it begins the
  show and leaves the question unanswered.
- `debug.fosfora.say 'limit flashing'` or `'full show'` answers the flash
  question, and is fed without waiting for the room's anchors while the
  question is up.
- `debug.fosfora.flashask 1`: the flash question again this launch, on an
  install that has answered. An answer is saved over the old one. No
  answer leaves the file as it was. Read at launch.
- `debug.fosfora.flashlimit 0|1` overrides the saved setting and does not
  touch the question: a launch with nothing saved still asks.
- `debug.fosfora.hints 1`, `handshint 0`, `swipe` and `voicetap` do
  nothing to the question. With `handshint 0` its fifth line never reads
  "Put the controllers down."
- `debug.fosfora.hints 1`: the hints again this launch. The saved state
  is left out and the hints show with no hand tracked. A hint made or
  given up in such a launch is saved, and the file then holds only what
  that launch taught.
- `debug.fosfora.swipe right|left`: a thumb swipe from the right hand, for
  the caption and the swipe hint's "Got it". Right is the next theme, left
  the one before.
- `debug.fosfora.voicetap 1` (board #5453) is the thumb tap.

A capture of every step, with `ask 0` and `handshint 0` set: launch with
`hints 1` and no `begin` to see the card (the controllers line joins it
after 4 s unless `handshint 0`), then `say 'begin'`. The swipe hint shows
about 9 s later; `swipe right` gives the caption and "Got it". The tap
hint follows (it needs voice on and the microphone granted), then the
palm hint, which unworn can only give up.

The tests (`first_run.rs`):
`the_show_waits_until_the_wearer_pinches_with_the_card_up`,
`the_start_gesture_is_a_pinch_on_either_hand`,
`a_spoken_begin_the_music_and_the_knob_begin_without_the_card`,
`nobody_is_stuck_the_show_begins_by_itself_after_a_minute`,
`before_the_begin_the_hands_only_begin_and_every_begin_opens_everything`,
`the_pinch_that_begins_throws_nothing_and_its_release_is_no_tap`,
`the_card_names_the_pinch_and_the_controllers`,
`every_hint_is_two_lines_at_the_two_line_size`,
`the_hints_show_in_order_one_at_a_time_a_few_seconds_apart`,
`a_gesture_made_before_its_hint_teaches_it_without_showing`,
`a_hint_gives_up_after_its_time_in_view_and_is_taught`,
`got_it_waits_for_the_caption_and_is_dropped_behind_a_voice_window`,
`a_hint_whose_gesture_does_not_work_is_passed_over_untaught`,
`the_hints_wait_for_a_free_label_and_a_tracked_hand`,
`the_taught_hints_round_trip_through_their_names`,
`the_events_log_what_happened`,
`the_swipe_caption_names_the_new_theme` and
`the_swipe_hint_is_taught_by_a_swipe_that_changed_the_theme`. The flash
question's (`flash_choice.rs`):
`the_card_warns_asks_and_names_both_answers_at_the_cards_size`,
`the_setting_s_words_say_what_it_does_now`,
`a_hand_over_an_answer_reads_as_that_answer`,
`a_hand_at_rest_at_the_face_or_past_the_card_is_over_neither`,
`whichever_eye_the_wearer_looks_with_a_hand_on_one_answer_is_never_the_other`,
`the_question_is_asked_when_nothing_is_saved_and_never_with_the_begin_knob`,
`either_hand_alone_gives_either_answer_with_a_pinch_over_it`,
`a_pinch_answers_nothing_by_accident`,
`a_spoken_answer_answers_whether_the_card_shows_or_not`,
`nobody_answers_the_limit_stays_on_and_nothing_is_saved`,
`the_pinch_that_answers_does_not_begin_the_show_or_throw`,
`under_the_question_the_wait_has_no_card_and_no_start_gesture` and
`the_strobe_runs_only_with_the_limit_off_by_its_knob`; with them
`flash.rs`'s `the_knob_overrides_the_wearers_setting_and_the_line_says_which`,
`palm_panel.rs`'s `the_menu_file_saves_the_flash_limit_once_chosen_and_reads_no_choice_when_absent`,
`intent.rs`'s `limit_flashing_and_full_show` and `panel_grid.rs`'s menu
tests. Beside them:
`immerse.rs`'s `the_room_is_dim_and_still_while_the_show_waits_and_the_opening_runs_from_there`,
`palm_panel.rs`'s `the_menu_file_saves_the_hints_taught_and_reads_none_when_absent`
and `held_back_the_menu_does_not_come_and_nothing_from_the_hold_is_read_after`,
`voice.rs`'s `a_centered_label_sits_on_the_line_of_sight_and_follows_the_same_way`,
`intent.rs`'s `begin_the_show`, and the under-card rule in `label.rs`'s
controllers test.

### What only a worn check can tell

- Whether a room at 0.25 reads as dim and calm, and whether the step from
  the wait into the opening is smooth.
- Whether the card at the center of the view is comfortable seated and
  lying back, and whether it is in the way of the system's own dialogs.
- Whether a resting hand ever begins the show unasked.
- Whether the three hints' words are enough without a picture, above all
  which way "along your finger" is. A drawn hand would go beside the
  text in `label.rs`'s `draw`, which paints the label's texture.
- Whether "Got it" over the palm menu is readable with the menu up.
- Whether 5 s between hints and 20 s each feel right during a first song.

The flash question, none of it worn yet:

- Whether six lines at 64 px read comfortably at 1 m, seated and lying
  back, and whether the warning reads as a warning.
- Whether "Put a hand over one and pinch:" is enough for a wearer who has
  never used their hands in the headset.
- Whether a hand held over an answer lands on it: with the right eye
  alone, with the left, and with both.
- Whether the fifth line changing is noticed before the pinch.
- Whether 0.3 s over an answer feels instant, and whether a pinch made
  too early is tried again without confusion.
- Whether the card stays still enough under a raised hand, since it
  follows the head.
- Whether a fist made to talk, with the hand in view over an answer,
  reads as a pinch there.
- Whether a pinch right after the answer begins the show before "Pinch
  to begin" has been read.
- Whether "Limit flashing: off" from the menu, mid-song, is a jolt that
  wants a warning of its own.

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
  <uses-permission android:name="android.permission.READ_EXTERNAL_STORAGE"/>
  <uses-permission android:name="android.permission.READ_MEDIA_AUDIO"/>
  <uses-permission android:name="android.permission.INTERNET"/>
  <uses-permission android:name="org.khronos.openxr.permission.OPENXR"/>
  <uses-permission android:name="org.khronos.openxr.permission.OPENXR_SYSTEM"/>
  <queries>
    <provider android:authorities="org.khronos.openxr.runtime_broker;org.khronos.openxr.system_runtime_broker"/>
    <intent><action android:name="org.khronos.openxr.OpenXRRuntimeService"/></intent>
  </queries>
  <application android:hasCode="false" android:label="Fosfora XR">
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

- The package id is `dev.fosfora.xr`, on the project's own domain. It cannot
  change once a build has been uploaded.
- Release signing, the version policy and the upload: [RELEASE.md](RELEASE.md).
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
"Fosfora can't see the room. Allow spatial data in Settings, then restart
Fosfora." (`room scan: NotAllowed` in the log) instead of "Scanning the
room…", for 6 s (board #5461, "The labels a new wearer meets"; it held for
the whole wait before). `debug.fosfora.ask 0` skips the request for
unworn runs, where the dialog would sit over the view; the checks, the log and
the pickup stay. `scripts/xr/sweep.sh` and `soak.sh` set it for their runs
and clear it afterwards. Should the JNI layer fail, the app takes everything
as granted and runs as it did before it asked.

Board #5385 adds a third, the shared `Music/` folder's storage permission
(`READ_EXTERNAL_STORAGE` at `targetSdk` 32, see "Your music"). It is asked for
with the others whatever the audio source, and its log line reads like
theirs. A grant within the 120 s logs `permission READ_EXTERNAL_STORAGE
granted after N s: scanning Music/` and adds that folder's tracks without a
relaunch. Denied, the app's own music folder still plays, and the bundled
tracks while that folder is empty.

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

*The NOTICE (board #4198):* `assets/xr/NOTICE`, every part the APK ships
and its license, made by `scripts/xr/notice.py` and committed, is staged as
`NOTICE` beside `xr_manifest.txt`, `assets/assets/NOTICE` in the APK
([RELEASE.md](RELEASE.md), "The NOTICE").

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
