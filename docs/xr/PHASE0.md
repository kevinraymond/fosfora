# Phase 0: findings (Sep 27, 2026)

The research behind the build plan. Repo facts are from commit `3f7cce0`
(Sep 22, 2026, v1.39.0); recheck them if main has moved.

## Decisions already made (by Kevin)

| Decision | Choice |
|---|---|
| Engine path | Native: Rust + OpenXR + wgpu (Vulkan). Spike for one week, pivot if needed |
| Pivot target | Unity shell for XR, input and UI, with Fosfora's Rust core as a native library |
| Devices | Quest 3 now; Meta VR Glasses (Spring 2027) as a second target |
| Repo strategy | No fork. Same repo, `xr` branch, new `crates/fosfora-xr` |

## Product requirements that shape the build

- **Hands-first:** fully usable with hands end to end, no controller ever needed.
- **Session shape:** seated (2 ft radius), a complete experience in 10 minutes or less,
  fast cold start, clean pause and resume.
- **Quality bar:** purposeful passthrough, field-of-view-aware layout, hand tracking,
  gaze, scene understanding and anchors, 60 fps minimum, and it must hold up in rooms
  the developer never tested.
- **Constraints:** only publicly released Meta tools and SDKs. No third-party IP and
  no identifiable people in bundled content.

## Existing apps

| App | What it is | Gap |
|---|---|---|
| Evryway Visualiser (Unity, $4.99) | Surround VR visualizer, local files and DLNA, hand tracking | Reviews: "feels like a Windows screen saver… wish it melded with the music more". No room use |
| Effex (XOCUS) | Mixed-reality visualizer, 9 effects, passthrough dimmer | Thin: effects float in front of you, no room geometry, weak audio sourcing |

**Opening:** nothing follows musical structure (beat, phrase, build, drop), and
nothing maps visuals onto the real room. Both are things Fosfora can do.

**Audio source:** Quest supports capturing other apps' audio (MediaProjection +
AudioPlaybackCapture), but Meta's policy restricts that API to casting,
streaming and screen sharing. Plan for **local files + the headset mic**, and
bundle original or licensed demo tracks.

## The finding that shapes the port: Fosfora is 2D

- Particles store position in `pos_life.xy` in screen space (−1 to 1).
  `compute_raster_scatter.wgsl` maps straight to pixels:
  `px = (pl.x * 0.5 + 0.5) * w`.
- `pos_life.z` is unused in most sims and **reused as the starting size** in
  some (e.g. `murmur_sim.wgsl`, `cymatics_sim.wgsl`). Audit each sim before
  giving `z` a meaning.
- Shader effects are fullscreen passes. Splat and the model sources use an
  orthographic camera.
- **Consequence:** a straight port is a flat screen floating in space. Real VR
  needs world-space particles (`xyz` plus a per-eye view-projection in the scatter
  pass). Shader effects become textures placed on room surfaces or panels.
- **Foveation:** the compute rasterizer writes storage buffers, so fixed
  foveation does nothing for particles. It only helps fragment-shaded passes.
  For particles, the levers are particle count and raster resolution.

## Why the port is cheaper than 130K lines suggests

- The audio analysis has zero GPU or windowing imports and runs on its own thread
  (`ARCHITECTURE-NOTES.md`, "Threading & headless viability").
- `headless/` already builds a device with no surface (`headless/gpu.rs`) and
  renders scenes to textures (`headless/scene_renderer.rs`). The XR path is the
  same shape, with OpenXR supplying the device and target images.
- Core modules (`gpu`, `audio`, `effect`, `bindings`, `signal`, `scene`,
  `preset`, `params`, `shader`) never import `ui` or `app`.
- wgpu device requirements: no mandatory features (pipeline cache and timestamps
  are optional). Limits: 16 storage buffers per shader stage, 5 bind groups.
  Formats used: Rgba16Float (heavily), Rgba8Unorm(Srgb), R32Float, Depth32Float.

## Repo shape (v1.39.0)

About 130,700 lines of Rust in one binary crate, `crates/fosfora-app` (plus
`xtask`). wgpu 27, naga 27, egui 0.33, winit 0.30, cpal 0.17.3, glam 0.29. 58
effects (`assets/effects/*.pfx`), 167 WGSL files, 27 particle sims.

| Module | Lines | XR verdict |
|---|---|---|
| `gpu/` (particles 20.3K) | 36.6K | Adapt; particles heavily |
| `ui/` | 16.9K | Rebuild small (one egui panel rendered to a texture) |
| `audio/` | 15.9K | Keep; adapt `capture.rs` only |
| `trama/` | 10.1K | Keep the runtime pieces core needs; cut the editor UI |
| `effect/`, `bindings/`, `signal/`, `scene/`, `preset/`, `params/`, `shader/` | 17.9K | Keep (gate MIDI/OSC in `bindings`, hot-reload in `shader`) |
| `media/` | 2.0K | Images yes; ffmpeg video and webcam no |
| `depth/` | 0.7K | Replace with the Quest depth API and room mesh |
| `ndi`, `spout`, `syphon`, `v4l2`, `link`, `recording`, `analyze`, `web`, `midi`, `osc`, `output` | about 11K | Cut from XR (desktop feature) |

**Core → desktop-only couplings to gate** (found by grep; recheck):
- `bindings/{sources,bus,migration}.rs` → `midi`, `osc`
- `signal/sink.rs` → `osc`
- `gpu/particle/{splat_source,source_loader}.rs` → `rfd` / `ureq`
- `trama/persist.rs` → `rfd` / `ureq` or similar
- `media/{webcam,video,webcam_ffmpeg,mod}.rs` → `nokhwa`, ffmpeg subprocess
- `shader/hot_reload.rs` → `notify`
- `gpu/frame_graph.rs`, `trama/exec/executor.rs` → `std::process`
- `gpu/context.rs`, `gpu/profiler.rs`, `gpu/frame_graph.rs`,
  `gpu/particle/source_loader.rs` → `winit` / `egui`

## The concept (for later phases, not the spike)

"Your room is the venue." Visuals land on your walls, flow around your
furniture, react to your hands, and follow the song's structure.

- **Room as canvas:** scene planes become surfaces for shader and overlay effects.
  This is the projector-previz roadmap idea, aimed at the real room.
- **Room as obstacle:** the room mesh and hand joints feed the existing
  Flow Around, Contain and Bounce modes.
- **Structure drives the show:** builds tighten, drops release, phrase
  boundaries change scenes.
- **Lead effects:** 6–8 polished ones, not 58. Candidates: Murmur, Symbiosis,
  Flux, Cymatics, Tide, and the overlay set (Tessera, Fenestra, Reticle,
  Astrolabe, Bezel), which already has real alpha.
- **Glasses:** 70° × 66° field of view (Quest 3 is 110° × 96°). Keep key content
  in the central region; room-anchored content helps.

## Device facts (Meta compare-devices page, Sep 18, 2026)

| | Quest 3 | Meta VR Glasses |
|---|---|---|
| Chip / RAM | XR2 Gen 2 / 8 GB | XR2 Gen 3 / 12 GB |
| Per-eye resolution | 2064 × 2208, 25 pixels per degree | 2412 × 2288, 37 pixels per degree, micro-OLED |
| Field of view | 110° × 96° | 70° × 66° |
| Tracking | Head, hands | Head, hands, **eyes** |
| Primary input | Controllers + hands | Eyes + hands (look and pinch) |
| Passthrough | Color stereo, depth sensor | Color stereo, autofocus, depth sensing |
| OS | Horizon OS, Android 14 | Same |
