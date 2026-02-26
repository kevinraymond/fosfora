# Phosphor

Cross-platform particle and shader engine for live VJ performance. Built with raw winit + wgpu + egui (not Bevy).

## Project Status

**Phase 1 (Core Rendering MVP): COMPLETE** — committed as `fa187b5`
**Phase 2 (Multi-Pass Rendering): COMPLETE** — multi-pass rendering infrastructure
**GPU Particle System: COMPLETE** — compute shader particles with ping-pong buffers
**Audio Upgrade + Beat Detection: COMPLETE** — multi-resolution FFT, adaptive normalization, 3-stage beat detector
**BPM Detection Rewrite: COMPLETE** — FFT autocorrelation, Kalman filter, octave disambiguation
**MIDI Input: COMPLETE** — midir integration, MIDI learn, auto-connect, hot-plug, config persistence
**Preset Save/Load: COMPLETE** — multi-layer presets with effect + params + postprocess, MIDI next/prev
**Layer Composition: COMPLETE** — up to 8 layers with blend modes, opacity, lock/pin, drag-and-drop reorder, GPU compositing
**OSC Input/Output: COMPLETE** — rosc UDP, RX on port 9000, TX opt-in on 9001, OSC learn, config persistence
**Web Control Surface: COMPLETE** — tungstenite WebSocket server, embedded HTML touch UI, bidirectional JSON state sync, multi-client
**Media Layers: COMPLETE** — PNG/JPEG/GIF as compositing layers, GPU blit with letterbox, animated GIF playback, transport controls, preset save/load
**Advanced Particles: COMPLETE** — sprite atlas textures, dual blend pipelines, image decomposition with spring-reform compute shader
**Video Playback: COMPLETE** — feature-gated ffmpeg pre-decode to RAM, instant scrub, 60s max
**NDI Output: COMPLETE** — feature-gated runtime-loaded NDI SDK, GPU capture with double-buffered staging, sender thread, UI panel
**Per-Effect Alpha: COMPLETE** — effects write meaningful alpha, preserved through post-processing, delivered to NDI for downstream compositing

### What's Built

#### Phase 1
- winit 0.30 window with wgpu 27 Vulkan rendering (fullscreen triangle technique)
- Shader hot-reload (notify file watcher, 100ms debounce, error recovery keeps old pipeline)
- Parameter system: ParamDef (Float/Color/Bool/Point2D), ParamStore, uniform packing
- Audio pipeline: cpal capture → lock-free ring buffer → dedicated thread → multi-resolution FFT (4096/1024/512-pt) → 20 features (7 bands + aggregates + spectral + beat) → adaptive normalization → 3-stage beat detection → asymmetric EMA smoothing → crossbeam channel to main thread
- egui overlay (D key toggle): WCAG 2.2 AA dark/light themes, audio spectrum bars, auto-generated param controls, effect browser, status bar
- .pfx JSON effect format with WGSL shader library (noise, palette, sdf, tonemap) auto-prepended
- 9 curated effects (see Effect Set below)

#### Phase 2
- Off-screen Rgba16Float HDR render targets (`RenderTarget`, `PingPongTarget`)
- Ping-pong feedback system: shaders access previous frame via `feedback(uv)` function
- Post-processing chain: bloom extract → separable 9-tap Gaussian blur → composite (chromatic aberration, bloom, ACES tonemap, vignette, film grain)
- Audio-reactive post-processing: RMS modulates bloom threshold/intensity, onset drives chromatic aberration, flatness drives film grain
- `PostProcessChain` with enable/disable toggle (UI checkbox in Parameters panel)
- `PassExecutor` for multi-pass effect execution with per-pass feedback support
- Extended .pfx format: `passes` array for multi-pass pipelines, `postprocess` overrides
- Backward compatible: single-shader .pfx files work unchanged via `normalized_passes()`
- Per-effect `postprocess` overrides (bloom threshold/intensity, vignette) applied at runtime
- Multi-pass shader hot-reload: all passes recompile on file change, not just pass 0
- New uniforms: `feedback_decay`, `frame_index` (256-byte uniform struct maintained)

#### GPU Particle System
- GPU compute particle simulation with ping-pong storage buffers (avoids read-write hazards)
- Atomic emission counter — dead particles claim new emission slots via `atomicAdd`
- Vertex-pulling instanced rendering: 6 vertices per particle, no vertex buffer needed
- Additive blending (SrcAlpha + One) — particles glow and stack
- Particles render INTO the HDR target with `LoadOp::Load` — bloom, feedback, post-processing all apply automatically
- Configurable emitter shapes: point, ring, line, screen
- Audio-reactive: beat burst emission, RMS/centroid-driven color and size
- Custom compute shaders per effect via `compute_shader` field in .pfx
- Compute shader hot-reload: edit simulation code while running (content-change detection prevents spam)
- 128-byte `ParticleUniforms` (includes resolution for aspect ratio correction) separate from main 256-byte `ShaderUniforms`
- Aspect-ratio-corrected orbital physics: all distance/force calculations in screen space
- Particle count shown in status bar when active
- Feedback + particles requires HDR clamp in background shader (`min(col, vec3f(1.5))`) to prevent runaway accumulation
- Advanced particles: sprite atlas textures (dual render pipelines: additive/alpha), image decomposition (grid/threshold/random sampling), ParticleAux buffer (binding 4, per-particle home positions + packed RGBA)

#### Media Layers
- Load PNG/JPEG/GIF/WebP as compositing layers via `rfd::FileDialog` ("+ Media" button in layer panel)
- `MediaLayer` struct: owns GPU frame texture (Rgba8UnormSrgb), HDR output RenderTarget, blit pipeline
- Blit shader (`media_blit.wgsl`): letterbox UV transform with aspect-ratio-correct fit mode, transparent black outside
- Animated GIF/WebP playback: forward/reverse/ping-pong direction, speed control (0.1–4.0x), loop toggle
- Frame upload via `queue.write_texture()` only on frame change (no per-frame upload for static images)
- Transport controls UI in right sidebar when active layer is media (replaces Parameters panel)
- Media layers composite through existing Compositor (all 10 blend modes + opacity work)
- Preset save/load: `media_path` (absolute), `media_speed`, `media_looping` in `LayerPreset` (serde defaults for backward compat)
- Loading an effect on a media layer converts it back to Effect (creates fresh UniformBuffer + PassExecutor)
- Layer panel: truncated names with hover tooltip, media file name displayed, "IMG"/"GIF" type indication
- Dependencies: `gif = "0.13"` for direct GIF frame decoding
- **Video playback** (feature-gated `video`): MP4/MOV/AVI/MKV/WebM/M4V/FLV via ffmpeg subprocess
- Pre-decode all frames to RAM (`decode_all_frames`) → `MediaSource::Animated` with `from_video` flag — instant random access for scrubbing/audio reactivity
- ffprobe probes metadata (dimensions, fps, duration); ffmpeg decodes raw RGBA to stdout; all subprocess stdin detached (`Stdio::null`) to prevent terminal corruption
- 60s max duration (`MAX_PREDECODE_SECS`); ~3.7MB/frame at 1280x720
- Seek slider with real-time scrub (`seek_to_secs()` / `seek_to_frame()`), mm:ss time display
- Video filter group in file dialog only when `ffmpeg_available()` (cached via `OnceLock`)
- Future: `ffmpeg-next` crate for long video support without RAM cost

#### NDI Output
- Feature-gated `ndi`: `cargo run --features ndi`
- Runtime dynamic loading via `libloading` — no build-time NDI SDK dependency
- `NdiCapture`: GPU capture texture (surface format, RENDER_ATTACHMENT | COPY_SRC) + double-buffered staging buffers with padded row alignment
- Capture pipeline: `PostProcessChain::render_composite_to()` renders final composite to capture texture (reuses existing bloom results), `copy_texture_to_buffer` → staging, async map → 1-frame latency readback
- `NdiSender` (FFI wrapper): loads libndi.so/dylib/dll at runtime, initializes NDI SDK, creates sender with `clock_video=true`, sends BGRA frames via `NDIlib_send_send_video_v2`
- Sender thread: crossbeam bounded(2) channel, `try_send` drops frames if NDI thread is behind (VJ performance priority)
- `NdiSystem`: config (source name, resolution), start/stop/restart, resize on window change
- UI: "Outputs" section in left sidebar (enable checkbox, source name, resolution dropdown, frame counter, activity dot), NDI status dot in status bar
- Config persists to `~/.config/phosphor/ndi.json`
- `OutputResolution`: Match Window / 720p / 1080p / 4K
- `ndi_available()` cached runtime check (OnceLock), graceful "NDI not found" message in UI
- NDI source name default: "Phosphor"

#### Audio Upgrade + Beat Detection
- Multi-resolution FFT: 4096-pt (sub_bass, bass, kick), 1024-pt (low_mid, mid, upper_mid), 512-pt (presence, brilliance)
- 20-field AudioFeatures: 7 frequency bands, 2 aggregates (rms, kick), 6 spectral shape, 5 beat detection
- Adaptive normalization: per-feature running min/max replaces all fixed gain multipliers
- 3-stage beat detection: OnsetDetector (log-magnitude multi-band spectral flux + adaptive threshold) → TempoEstimator (FFT autocorrelation + Kalman filter) → BeatScheduler (predictive state machine with phase correction)
- TempoEstimator: 8s onset buffer, FFT-based Wiener-Khinchin autocorrelation (|FFT|², mean-subtracted), genre-aware log-Gaussian tempo prior (center 150 BPM, σ=1.5), multi-ratio octave correction (9 ratios incl. 1:3, 1:4), cascading octave-up correction via local peak detection
- KalmanBpm: log₂-BPM space filter with octave snap (2:1/1:2 within 5%, 50-frame escape), adaptive Q/R, confidence gating (< 0.15 skipped), divergence reset after 15 frames
- Dedicated kick detection: half-wave rectified spectral flux in 30-120 Hz from 4096-pt FFT
- Bass bands use linear RMS, mid/high bands use dB scaling (80dB range)
- Beat trigger (`beat` field) replaces onset-based beat proxy (`onset > 0.5`)
- `beat_phase` (0-1 sawtooth at detected tempo), `bpm` (normalized /300), `beat_strength`
- BPM shown in status bar with beat flash indicator
- 7-band frequency visualization in audio panel
- `treble` → `presence` + `brilliance`, `phase` → `beat_phase` in all shaders
- ShaderUniforms: 256 bytes with 20 audio fields + params + feedback uniforms
- ParticleUniforms: 128 bytes with 10 most useful audio fields (sub_bass, bass, mid, rms, kick, onset, centroid, flux, beat, beat_phase)

#### MIDI Input
- midir 0.10 integration: callback thread → crossbeam bounded(64) channel → main thread drain in `App::update()`
- Auto-connect to saved port on startup, fallback to first available
- Hot-plug detection: polls `list_ports()` every 2s, auto-disconnect on removal, auto-reconnect on reappear
- MIDI learn: click "M" on any param or trigger, move a knob/press a button to bind
- Params: Float and Bool mappable via CC (raw 0-127 scaled to param range, no smoothing)
- Triggers: NextEffect, PrevEffect, TogglePostProcess, ToggleOverlay, NextPreset, PrevPreset, NextLayer, PrevLayer with rising-edge detection
- Config persists to `~/.config/phosphor/midi.json` (JSON via `dirs` crate)
- Channel 0 = omni (respond to all channels)
- UI: port dropdown + activity dot + learn prompt in left panel, per-param MIDI badges + trigger learn in right panel, MIDI status in status bar

#### OSC Input/Output
- rosc 0.11 integration: UDP receiver thread → crossbeam bounded(64) channel → main thread drain in `App::update()`
- RX default port 9000 (0.0.0.0), TX default port 9001 (127.0.0.1), configurable via UI
- RX on by default, TX off by default (user opts in via UI)
- Address scheme: `/phosphor/param/{name}`, `/phosphor/layer/{n}/param/{name}`, `/phosphor/trigger/{action}`, `/phosphor/layer/{n}/opacity|blend|enabled`, `/phosphor/postprocess/enabled`
- TX broadcasts at 30Hz (configurable): 13 audio messages (7 bands + aggregates/beat) + 2 state messages (active layer, effect name)
- OSC learn: click "O" on any param or trigger, send any OSC message to bind address
- Coexistence with MIDI: last-write-wins per frame (OSC runs after MIDI so it takes priority)
- Layer-targeted messages: set params, opacity, blend mode, enable on specific layers by index
- Config persists to `~/.config/phosphor/osc.json` (JSON via `dirs` crate)
- UI: OSC panel in left sidebar (enable toggle, RX/TX port config, activity dot, trigger learn), per-param OSC badges in right panel, OSC status dot in status bar

#### Web Control Surface
- tungstenite 0.28 sync WebSocket server on configurable port (default 9002)
- Same-port HTTP+WS: TcpListener accept loop, detect `Upgrade: websocket` header, serve HTML or upgrade
- Embedded single-file HTML/CSS/JS touch control surface via `include_str!` (debug mode reads from filesystem for hot-reload)
- Thread-per-client architecture: accept thread + per-client read/write threads (50ms read timeout for interleaved I/O)
- Bidirectional JSON state sync: full state snapshot on connect, params/layers/effects/presets
- Audio features broadcast at 10Hz to all connected clients
- Multiple simultaneous clients supported (Vec<Sender<String>> with pruning)
- Client → Server: set_param, set_layer_param, load_effect, select_layer, set_layer_opacity/blend/enabled, trigger, load_preset, set_postprocess_enabled
- Server → Client: state (full snapshot), audio (10Hz), param_changed, layer_changed, active_layer, effect_loaded, presets
- Mobile-first touch UI: dark theme, 48px min touch targets, 7-band audio bars, effect grid, auto-generated param sliders, layer cards, preset list, trigger buttons
- Auto-reconnect with exponential backoff (1/2/4/8s), throttled sends (~30Hz)
- egui panel: enable toggle, port config, URL display (localhost + LAN IP), client count, activity dot
- Config persists to `~/.config/phosphor/web.json`
- Coexists with MIDI/OSC: last-write-wins (web drain runs after OSC)
- Web status dot in status bar (blue when clients connected)

#### Preset Save/Load
- `PresetStore`: scan/save/load/delete presets from `~/.config/phosphor/presets/*.json`
- `Preset` struct: version, layers (Vec<LayerPreset>), active_layer, postprocess (PostProcessDef)
- `LayerPreset`: effect_name, params, blend_mode, opacity, enabled, locked, pinned (serde defaults for backward compat)
- Preset panel in left sidebar: text input + Save button, selectable list with delete buttons
- Save captures all layers (effect + params + blend + opacity + enabled + locked + pinned) + active_layer + postprocess
- Load adjusts layer count to match preset, loads each layer (skips locked layers), restores active_layer + postprocess
- Overwrite on save (standard VJ workflow), graceful mismatch handling
- Name sanitization: strip `/ \ .`, trim whitespace, max 64 chars
- MIDI triggers: NextPreset/PrevPreset cycle through preset list
- Persists to disk as JSON, survives app restart

#### Layer Composition
- Up to 8 layers, each with own `PassExecutor`, `UniformBuffer`, `ParamStore`, `ShaderUniforms`
- 10 blend modes: Normal, Add, Screen, ColorDodge, Multiply, Overlay, HardLight, Difference, Exclusion, Subtract
- Per-layer opacity (0-1), enable/disable toggle
- Per-layer lock (prevents all setting changes, blocks MIDI CC params, blocks effect loading, skipped during preset load)
- Per-layer pin (prevents drag reordering, hides drag handle)
- Drag-and-drop reordering via egui DnD (manual `DragAndDrop::set_payload` on handle only, not whole card)
- GPU `Compositor`: ping-pong accumulator blits first layer, composites subsequent layers with blend shader
- Single-layer fast path: skip compositing entirely when only 1 layer enabled (zero overhead, backward compatible)
- `LayerStack` manages ordered layer vec + active_layer index
- `LayerInfo` snapshot struct passed to UI to avoid borrow conflicts
- Layer panel in left sidebar: drag handle, enable checkbox, lock/pin icons, layer name/effect, delete, blend mode dropdown, opacity slider
- Effects panel loads effects onto active layer
- Shader hot-reload iterates ALL layers (each tracks own shader_sources)
- Particle systems per-layer (each layer can have particles independently)
- MIDI triggers: NextLayer/PrevLayer cycle active layer
- Keyboard: `[`/`]` cycle active layer
- Presets save/load all layers (locked layers skipped during load)
- `MidiSystem::update_triggers_only()`: drains MIDI but skips CC→param when active layer is locked

#### Effect Set
9 curated audio-reactive effects designed for compositing across layers:

1. **Aurora** (`aurora.wgsl`) — 7 frequency bands as horizontal flowing northern light curtains. Params: curtain_speed, band_spread, glow_width. No feedback.
2. **Drift** (`drift.wgsl`) — Triple domain-warped FBM fluid smoke with advected feedback. Params: warp_intensity, flow_speed, color_mode, density. Uses `mix()` feedback blend (not `max()`) so darks reclaim space.
3. **Tunnel** (`tunnel.wgsl`) — Log-polar infinite cylindrical flythrough with wall panels, checkerboard shading, and twist rotation. Params: twist_amount (centered at 0.5 = no twist), speed, tunnel_radius, segments. No feedback. IMPORTANT: speed must NOT be multiplied by audio (`t * varying_value` causes back-and-forth jitter).
4. **Prism** (`prism.wgsl`) — Kaleidoscopic N-fold mirror symmetry over FBM + geometric patterns. Params: fold_count, rotation_speed, zoom, complexity + Bool toggles: sparkle, bass_pulse, beat_flash. No feedback.
5. **Shards** (`shards.wgsl`) — Animated Voronoi cells with stained-glass fill and glowing fracture edges. Params: cell_scale, edge_glow, fill_amount, saturation (0=gray, 0.5=normal, 1.0=vivid). No feedback.
6. **Pulse** (`pulse.wgsl`) — Beat-synced concentric rings expanding from center with feedback trails. Params: ring_count, expansion_speed, ring_width. Uses feedback.
7. **Iris** (`feedback_test.wgsl`) — Spinning dot with fading feedback trails. Params: trail_length. Uses feedback.
8. **Swarm** (`spectral_eye_bg.wgsl` + `spectral_eye_sim.wgsl`) — Orbital particle cloud with custom compute shader. Params: orbit_speed, trail_decay. Uses feedback + particles.
9. **Storm** (`storm.wgsl`) — Volumetric dark clouds with beat-triggered internal lightning. FBM-Worley density (smooth log-sum-exp Worley for puffy billow shapes), Beer-Lambert 4-step light march for self-shadowing, silver lining at cloud edges. Params: turbulence, flow_speed, flash_power, flash_spread. Uses feedback.

**Bundled preset**: "Crucible" (`~/.config/phosphor/presets/Crucible.json`) — all 8 layers composited with tuned blend modes, opacities, and params.

**Shader authoring notes**:
- 16 params per effect (`array<vec4f, 4>`), accessed via `param(0u)` through `param(15u)`
- Avoid `atan2` in palette/color calculations — causes visible seam at ±π (negative x-axis). Use radius, depth, or time instead.
- For seamless angular patterns, use `sin(angle * N)` directly (wraps cleanly) or embed angle via `cos(a), sin(a)` for noise lookups.
- For feedback effects: use `mix()` not `max()` to allow dark areas to reclaim; clamp output (`min(result, vec3f(1.2))`) to prevent blowout; keep decay ≤ 0.88.
- Never multiply `time * audio_varying_value` for position/speed — causes oscillation. Use constant speed, apply audio to other properties.
- Smooth Worley noise: use log-sum-exp (`-log(sum(exp(-k*d²)))/k`) not `min()` — standard min creates hard gradient discontinuities at cell boundaries. Clamp with `max(0.0, ...)` before `sqrt()` to prevent NaN at cell centers where sum > 1.
- Beer-Lambert light march: 4-step march toward light direction, `transmittance *= exp(-density * extinction * step)`. Use fewer FBM octaves (LOD) in march steps for performance.
- **Alpha for compositing**: effects that have empty space should write `alpha < 1.0`. Use `brightness_alpha` pattern: `clamp(max(r, max(g, b)) * 2.0, 0.0, 1.0)`. For feedback effects, track alpha decay alongside RGB: `result_alpha = max(new_alpha, prev.a * decay)`. Full-screen effects (aurora, drift, storm, shards, prism) keep `alpha = 1.0`.

### Known Issues
- ~33 compiler warnings (mostly unused items reserved for future phases)
- Reduced motion detection (`ui/accessibility/motion.rs`) is stubbed for macOS/Windows

### Architecture
```
Main Thread: winit event loop → drain audio/midi/osc/shader channels → update per-layer uniforms → per-layer PassExecutor → Compositor (multi-layer blend) → PostProcessChain (bloom/tonemap) → egui overlay → present
Audio Thread: cpal callback → ring buffer → multi-res FFT → adaptive normalize → beat detect → smooth → send AudioFeatures
MIDI Thread: midir callback → parse 3-byte MIDI → send MidiMessage via crossbeam bounded(64)
OSC Thread: UdpSocket recv → rosc decode → send OscInMessage via crossbeam bounded(64)
Web Accept Thread: TcpListener → HTTP serve or WS upgrade → spawn client thread
Web Client Thread(s): 50ms read timeout → parse JSON → WsInMessage via crossbeam bounded(64); drain outbound broadcast channel
NDI Sender Thread: recv NdiFrame via crossbeam bounded(2) → NDIlib_send_send_video_v2 (clock_video paced)
File Watcher Thread: notify → debounce → send changed paths
```

No mutexes in hot path (web uses Arc<Mutex> only for client list + latest state, not per-frame). Six+ threads + cpal callback + midir callback.

### Render Pipeline
```
For each enabled layer:
  Compute Dispatch (particle sim, if active)
                        ↓
  Effect Pass(es) → PingPong HDR Target(s) [Rgba16Float]
                        ↓
  Particle Render Pass (instanced quads, additive blend, LoadOp::Load)

Single layer (fast path):
  Layer output → PostProcessChain → Surface [sRGB]
Multiple layers:
  Layer outputs → Compositor (blit first, blend subsequent) → Accumulator HDR
                        ↓
PostProcessChain (if enabled):
  Bloom Extract (quarter-res) → Blur H → Blur V → Composite → Surface [sRGB]
PostProcessChain (if disabled):
  Simple Blit → Surface [sRGB]
                        ↓
NDI Capture (if enabled, feature-gated):
  render_composite_to() → Capture Texture → copy_texture_to_buffer → Staging[N]
  map_async(Staging[N]) → next frame: read Staging[N-1] → NDI sender thread
                        ↓
egui Overlay → Surface
```

### Key Design Decisions
- WGSL uniform arrays must be `array<vec4f, N>` not `array<f32, N>` (16-byte alignment requirement). Params accessed via `param(i)` helper function in shaders.
- `target` is a WGSL reserved word — use `look_at` instead in shaders.
- wgpu 27 (not 28) because Rust 1.90 doesn't support wgpu 28's MSRV of 1.92.
- egui 0.33: `CornerRadius` not `Rounding`, `corner_radius` not `rounding` field, `Renderer::new` takes `RendererOptions` struct, `RenderPass` needs `.forget_lifetime()` for egui's `'static` requirement.
- cpal 0.17: `SampleRate` is `u32` (not tuple struct), `description()` returns `Result<DeviceDescription>`, field access via `.name()` method.
- Feedback bind group uses 3 entries: uniform buffer (binding 0), prev_frame texture (binding 1), prev_sampler (binding 2). Effects that don't use feedback get a 1x1 placeholder texture bound.
- Bloom operates at quarter resolution for performance. Post-processing uses separate uniform buffers per stage.
- Particle storage buffers use ping-pong pattern (read from A, write to B, flip). Compute bind groups pre-created for both states.
- Particle Struct is 64 bytes (4 x vec4f): pos_life, vel_size, color, flags. Size chosen for GPU cache-line friendliness.
- Particle render uses vertex-pulling (no vertex buffer) — 6 vertices per instance expand to screen-space quads with aspect ratio correction.
- midir 0.10: `MidiInputConnection<()>` is RAII — drop closes the port. No explicit close needed.
- Presets stored at `~/.config/phosphor/presets/{name}.json`. `PresetStore` re-scans after every save/delete.
- Layer system: each Layer owns its own `PassExecutor` + `UniformBuffer` + `ParamStore` + `ShaderUniforms`. Compositor is separate App field (not inside LayerStack) to avoid borrow conflicts when passing layer render targets to compositor.
- `LayerInfo` snapshot struct: collected before mutable UI borrow to avoid simultaneous mutable+immutable borrow of layers vec.
- Compositor uses ping-pong accumulator: blit first layer, then composite(accumulator.read, layer[i]) → accumulator.write for each subsequent layer, tracking read/write indices manually without flipping.
- rosc 0.11: pure Rust OSC. `decode_udp` returns `(usize, OscPacket)`. Bundles recursively flattened. `UdpSocket` with 100ms read timeout for clean shutdown.
- OSC coexists with MIDI: last-write-wins per frame. OSC drain runs after MIDI drain in `App::update()`.
- Web coexists with MIDI/OSC: last-write-wins. Web drain runs after OSC drain. Order: MIDI → OSC → Web.
- tungstenite 0.28: `Message::text()` constructor (not `Message::Text(String)`), text payload is `Utf8Bytes` not `String`. Client handler is generic over `S: Read + Write` for `ReplayStream` wrapper.
- Web same-port HTTP+WS: `ReplayStream` wrapper replays already-read request bytes then delegates to `TcpStream`. Avoids needing separate HTTP server.
- OSC TX uses fire-and-forget nonblocking UDP. Rate limited by `Instant` comparison (default 30Hz). Sends ~15 messages per frame.
- Media layers use single RenderTarget (no ping-pong) — no feedback for media. Frame texture is Rgba8UnormSrgb (GPU auto-converts sRGB→linear on sample).
- Media bind group created once in `new()`, rebuilt only on `resize()` — texture object unchanged, only data written via `write_texture()`.
- Media frame upload in `update()` (mutable phase), blit in `execute()` (immutable) — matches existing Effect pattern.
- Media resize needs `&Queue` (for uniform upload) — separate `resize_media()` method on Layer since normal `resize()` only takes `&Device`.
- Loading an effect on a media layer: converts back by creating fresh UniformBuffer + EffectLayer, then proceeds with normal effect load.
- `gif = "0.13"` as direct dep (decoder.rs uses gif crate API for frame-by-frame compositing with canvas accumulator).
- UI panels widened to 270px (from 240px). Layer names use egui `Label::truncate()` with full name on hover.
- Video pre-decode: ffmpeg decodes all frames to `MediaSource::Animated` (same as GIF). Instant random access, no streaming complexity. RAM cost acceptable for VJ clips (≤60s). `from_video` flag on `Animated` variant (cfg-gated) controls UI differences (seek slider vs frame counter).
- All ffmpeg/ffprobe subprocess spawns use `.stdin(Stdio::null())` to prevent terminal corruption (ffmpeg inherits stdin and can switch to raw mode).
- NDI output uses runtime dynamic loading (`libloading`) instead of build-time SDK binding. No `grafton-ndi` dep needed — raw FFI with `NDIlib_*` symbols loaded from libndi.so at runtime. `ndi_available()` cached via `OnceLock`.
- NDI capture reuses existing `PostProcessChain` composite pipeline via `render_composite_to()`. Capture texture uses same format as surface (typically `Bgra8UnormSrgb`) so readback data is BGRA — matches NDI's `NDIlib_FourCC_type_BGRA` natively. Alpha is preserved end-to-end: effects → compositor → post-processing → NDI output.
- NDI staging buffers use `align_to(width*4, COPY_BYTES_PER_ROW_ALIGNMENT)` for wgpu row padding, stripped on readback.
- NDI frame channel is bounded(2) with `try_send` — drops frames if sender thread is behind (VJ performance > NDI latency).
- NDI state passes through egui temp data (NdiInfo snapshot struct) to avoid `&mut NdiSystem` in `draw_panels` signature (feature-gated types can't be conditional function params).

### Controls
- `D` — Toggle egui overlay
- `F` — Toggle fullscreen
- `Esc` — Quit
- `[` / `]` — Cycle active layer
- `Tab` — Cycle widgets (when overlay visible)
- Sliders have +/- buttons for WCAG 2.5.7 compliance

### Build & Run
```bash
cargo run                          # debug build
cargo run --release                # release build (much faster shaders)
cargo run --features video         # with video playback (requires ffmpeg on PATH)
cargo run --features ndi           # with NDI output (requires NDI runtime from ndi.video)
cargo run --features "video,ndi"   # with both
RUST_LOG=phosphor_app=debug cargo run  # verbose logging
```

### Test Coverage
~241 unit tests covering pure logic, parsing, serde, and data transforms across 16 modules:
- **Audio**: beat detection pipeline (onset → tempo → scheduler), adaptive normalization, BPM convergence
- **Params**: ParamDef/ParamValue types, ParamStore CRUD, pack_to_buffer with all types
- **Effect loader**: `is_builtin`, `prepend_library` (with/without existing uniforms)
- **GPU layer**: BlendMode serde/display, `adjusted_active_after_remove/move` edge cases
- **Preset**: sanitize_name, LayerPreset serde defaults, Preset roundtrip, PresetStore API
- **MIDI**: MidiMapping scale (inverted, custom range, zero range), matches (channel, omni, type), MidiConfig
- **OSC**: parse all message types (param, layer, trigger, blend, enabled, raw), msg_address, OscConfig/OscLearnTarget
- **Web**: parse_client_message (all types + edge cases), build_params (all ParamDef types), state builders
- **Settings/Theme**: ThemeMode serde, display names, toggle, ThemeColors constructors (all 6 themes)
- **NDI**: OutputResolution serde/display/dimensions, NdiConfig defaults/partial JSON

**Not unit-tested** (requires hardware/runtime):
- GPU rendering (wgpu Device/Queue, shader compilation, render passes, compositor)
- UI panels (egui Context, draw calls, layout)
- I/O threads (cpal audio capture, midir MIDI, UDP sockets, WebSocket server, NDI sender)
- App orchestration (winit event loop, inter-thread coordination)
- File I/O in config load/save (tested implicitly via serde roundtrips)

```bash
cargo test                         # run all tests (~241)
cargo test --features ndi          # include NDI tests
cargo tarpaulin --features ndi     # line coverage report
```

### Reference Projects (for porting)
- `~/ai/audio/spectral-senses/` — C++ audio analysis (12 features, EMA smoothing)
- `~/ai/audio/spectral-senses-old/` — GLSL shader library (SDF, noise, palette, tonemap) + scene shaders
- `~/ai/audio/easey-glyph/` — Python adaptive normalization, beat detection

### Full Plan
The complete 28-week, 4-phase plan is at `~/ai/audio/phosphor-internal/cross-platform particle and shader engine.md`. Phases 1-2 are done. Phases 3-4 cover: particle system, performance profiling, preset management, plugin architecture.

### Remaining Roadmap
1. ~~Multi-pass rendering~~ ✓
2. ~~GPU compute particle system~~ ✓
3. ~~Beat detection~~ ✓ (3-stage: onset → tempo → scheduler)
4. ~~MIDI input with MIDI learn~~ ✓
5. ~~Preset save/load~~ ✓
6. ~~Layer-based composition with blend modes~~ ✓
7. ~~OSC input/output~~ ✓
8. ~~Web control surface~~ ✓ (WebSocket server + embedded touch UI)
9. ~~Media layers~~ ✓ (PNG/JPEG/GIF with letterbox, GIF playback, transport controls)
10. ~~Advanced particles~~ ✓ (sprite textures, image decomposition, aux buffer)
11. AI shader assistant (planned: local LLM via llama.cpp/Ollama, naga validation)
12. ~~Video playback~~ ✓ (feature-gated, ffmpeg pre-decode to RAM; future: ffmpeg-next for long videos)
13. 3D Gaussian Splatting (deferred: blocked on wgpu 28 / egui-wgpu update)
14. ~~NDI output~~ ✓ (feature-gated, runtime-loaded libndi, GPU capture + sender thread)
15. Spout/Syphon output (deferred: no mature Rust crates)
