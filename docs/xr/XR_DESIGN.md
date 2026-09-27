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
      input.rs        hand tracking, pinch, (later) eye gaze
      scene.rs        passthrough, planes/mesh (S7)
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
   `XR_FB_spatial_entity_container`, `XR_EXT_eye_gaze_interaction`. S7's room
   geometry must use the `XR_EXT_spatial_plane_tracking` / `XR_META_*` routes.
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

1. `xrGetVulkanGraphicsRequirements2KHR`: check the Vulkan API version range.
2. Instance: build `VkInstanceCreateInfo` with the extensions wgpu-hal expects
   (**VERIFY** the hal helper for desired instance extensions in 27), then
   `xrCreateVulkanInstanceKHR`.
3. `xrGetVulkanGraphicsDevice2KHR` gives the `VkPhysicalDevice`.
4. Device: ask wgpu-hal which device extensions and features it needs for the
   requested wgpu features (**VERIFY**: the hal adapter exposes helpers for
   required device extensions and physical-device features), then call
   `xrCreateVulkanDeviceKHR` with them. Keep the queue family index; use queue 0.
5. Wrap: hal `Instance::from_raw` → `expose_adapter(physical_device)` → hal
   `device_from_raw` → `wgpu::Instance::from_hal`,
   `Instance::create_adapter_from_hal`, `Adapter::create_device_from_hal`
   (**VERIFY** exact names and signatures in 27; all are `unsafe`, so add
   `// SAFETY:` comments).
6. Session graphics binding `XrGraphicsBindingVulkan2KHR`: instance, physical
   device, device, queue family, queue index 0. It **must** be the same queue
   wgpu submits to.
7. Swapchains: format chosen from `xrEnumerateSwapchainFormats`. Prefer
   `R8G8B8A8_SRGB`; desktop also picks an sRGB surface format
   (`gpu/context.rs`). Size = the recommended per-eye image size. One swapchain
   per eye, or one two-layer array; log the choice. For each image: hal
   `texture_from_raw` with **no drop callback that frees it** (OpenXR owns the
   image), then `Device::create_texture_from_hal`. Usage: `RENDER_ATTACHMENT`,
   plus `COPY_DST` if needed.
8. **Image layout at release.** OpenXR expects the image in
   `COLOR_ATTACHMENT_OPTIMAL` at `xrReleaseSwapchainImage`. wgpu tracks layouts
   itself. Make the **last** use of each swapchain texture per frame a
   render-pass color-attachment write (a final blit or composite pass), never a
   copy or compute write. **VERIFY** with validation layers in S2.
9. Frame: `xrWaitFrame` → `xrBeginFrame` → `xrLocateViews` (predicted display
   time) → acquire, then wait (timeout), then render and submit → release →
   `xrEndFrame` with a `CompositionLayerProjection` (plus a passthrough layer
   first, in S7).

## Per-eye camera

From each `XrView`: pose → view matrix (inverse of the pose); fov
(angleLeft, angleRight, angleUp, angleDown, all asymmetric) → projection.
wgpu uses depth 0..1, so build the projection for that range; reverse-Z optional
(Fosfora uses Depth32Float). Use glam 0.29 to match core.

## World-space particles (S5)

The desktop scatter (`assets/shaders/builtin/compute_raster_scatter.wgsl`) does
`px = (pos.x * 0.5 + 0.5) * w` on screen-space `pos_life.xy`. The XR path adds a
**separate** scatter/draw shader variant that:
- reads world `xyz` (new test-sim layout; don't reinterpret existing sims' `z`);
- does `clip = view_proj[eye] * vec4(xyz, 1)`, discards if `clip.w <= near` or
  outside the frustum, then NDC → pixels as on desktop;
- sizes the footprint by depth: `radius_px = world_size * focal_px / clip.w`;
- is dispatched once per eye into that eye's raster target (the compute raster
  has no multiview; that's fine).

Resolve into the swapchain layer through a final render pass (layout rule above).
Additive vs alpha resolve follows the effect. Over passthrough (S7), output
premultiplied alpha.

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

## Glasses notes

- 70° × 66° field of view: keep the panel and key visuals inside the central
  region. Use the Meta VR CLI field-of-view simulation on the Quest 3 (see
  Meta's field-of-view doc) once there is UI.
- Eye gaze is only on the glasses; the Quest 3 falls back to head gaze. Design
  selection as "gaze ray (eye if available, else head) + pinch", one code path.
- Eye-tracked foveation has no public native route (Unity only), so it's not
  planned. Fixed foveation only helps fragment passes (see `PHASE0.md`).
