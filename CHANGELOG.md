# Changelog

<!-- Release workflow extracts notes between ## vX.Y.Z headers via awk. -->
<!-- Keep the "## vX.Y.Z — date" format for automatic release notes. -->

## v1.2.1 — 2026-03-05

### Audio
- Add WASAPI loopback capture for Windows — auto-captures desktop audio (what's playing through speakers) without requiring Stereo Mix
- WASAPI backend uses `windows` crate COM APIs: `IMMDeviceEnumerator` → `IAudioClient` (loopback) → `IAudioCaptureClient`
- Same fallback pattern as Linux: try WASAPI loopback first, fall back to cpal input devices
- Supports float32, int16, and int24 formats with stereo→mono downmix
- Device friendly name shown in status bar (e.g. "Speakers (Realtek Audio)")

### Bug Fixes
- Fix webcam "Failed to fulfill requested format" on Windows: cameras that only support raw formats (YUYV/NV12) now work via automatic fallback from MJPEG to any supported format

### CI
- Upload debug build artifacts on PRs for all platforms

## v1.2.0 — 2026-02-28

### GPU Particle System
- GPU-driven pipeline: counter buffer, indirect draw, alive/dead index lists — zero dead particle processing
- `particle_lib.wgsl` auto-prepended to all compute shaders: shared structs, bindings, hash/rand, emit/alive helpers
- Alive/dead protocol: atomic emission claims, compact alive index rendering
- 3D curl noise flow field: 64x64x64 baked texture at `@group(1)`, `sample_flow_field()` helper
- Trail rendering: per-particle ring buffer, ribbon triangle strips with tapering width/alpha, separate indirect draw
- Spatial hash grid: 40x40 GPU 3-pass pipeline (count → prefix sum → scatter), neighbor query at `@group(3)`
- ParticleUniforms extended from 128 to 192 bytes (flow field + trail params)
- ParticleRenderUniforms extended from 32 to 48 bytes (frame_index + trail params)
- Particle UI panel: alive/max count with utilization bar, emit rate, burst, lifetime, speed, size, drag sliders, feature badges

### New Effects
- **Flux**: 30K particles following curl noise flow field, audio-reactive flow strength and speed
- **Ribbons**: 8K particles with flow field + 16-point trail ribbons, audio-reactive width/opacity
- **Chaos**: 50K particles tracing Lorenz/Rossler strange attractors with RK4 integration, 3D perspective projection, audio-reactive bifurcation parameters
- **Helix**: 20K charged particles spiraling via Lorentz force F=q(E+v×B), positive/negative charges, audio-reactive B_z and E fields
- **Murmuration**: 80K flocking particles with Vicsek model, spatial hash neighbor query, audio-reactive order↔disorder phase transition
- **Cymatics**: 40K particles forming Chladni nodal line patterns via gradient descent, audio frequency bands select mode numbers
- **Coral**: 50K particles tracing Turing-like organic growth patterns, hexagonal spots morphing to labyrinthine stripes

## v1.1.0 — 2026-02-28

### Scene System
- Scene = ordered cue list referencing presets with transitions (Cut, Dissolve, ParamMorph)
- SceneStore: save/load/delete scenes to `~/.config/phosphor/scenes/*.json`
- Timeline state machine: Idle → Holding → Transitioning with auto-advance and beat sync
- ParamMorph transitions: smooth interpolation of all params and layer opacities
- Dissolve transitions: GPU crossfade via fullscreen shader with snapshot capture
- Scene panel in left sidebar: cue list management, transport controls, save/load/delete
- Per-cue transition type editing: click to cycle Cut → Dissolve → Morph
- Per-cue transition duration editing via DragValue
- Advance mode selector: Manual / Timer / Beat Sync with per-cue hold times and beats-per-cue
- Auto-save: cue and timeline changes persist to disk immediately
- Timeline bar above status bar: cue blocks, playhead during hold and transitions, click-to-jump
- Scene status indicator (SCN) in status bar with cue counter
- Keyboard: Space (next cue), T (toggle timeline)
- MIDI triggers: SceneGoNext, SceneGoPrev, ToggleTimeline
- OSC: `/phosphor/trigger/scene_go_next`, `scene_go_prev`, `toggle_timeline`
- Web control: Prev Cue / Next Cue / Timeline trigger buttons
- MIDI Clock sync: parse 0xF8/FA/FB/FC system realtime, derive external BPM, beat-synced advance
- MIDI Clock → Timeline: auto-follow transport (play/stop), MIDI clock beats drive BeatSync mode with audio fallback
- OSC scene control: `/phosphor/scene/goto_cue`, `/scene/load` (int or string), `/scene/loop_mode`, `/scene/advance_mode`
- OSC outbound timeline state: `/phosphor/state/timeline/active`, `cue_index`, `cue_count`, `transition_progress` at TX rate

### UI
- Redesign scene panel: card-framed scene list, section headers (TRANSPORT / CUE LIST), sized transport buttons (PLAY ghost-border, STOP|PREV|GO), transition type badges with color per type, ghost-border "+ Cue" button
- Widen side panels from 270px to 315px

## v1.0.1 — 2026-02-27

### UI
- Shader compilation errors in status bar now have a dismiss (×) button

### Documentation
- Add comprehensive TUTORIALS.md covering all features
- Quick Start now leads with binary downloads, build-from-source in collapsible details

## v1.0.0 — 2026-02-27

### Rendering
- HDR multi-pass pipeline with Rgba16Float render targets and ping-pong feedback
- Post-processing chain: bloom extract, separable Gaussian blur, chromatic aberration, ACES tonemapping, vignette, film grain
- Audio-reactive post-processing (RMS → bloom, onset → chromatic aberration, flatness → film grain)
- Shader hot-reload via file watcher with 100ms debounce and error recovery

### Effects
- 12 curated audio-reactive effects: Aurora, Drift, Tunnel, Prism, Shards, Pulse, Iris, Swarm, Storm, Veil, Nova, Vortex
- .pfx JSON effect format with multi-pass pipeline support
- WGSL shader library auto-prepended (noise, palette, SDF, tonemap)
- In-app WGSL shader editor with live hot-reload, built-in/user sections, copy-shader

### GPU Particles
- Compute shader particle simulation with ping-pong storage buffers and atomic emission
- Vertex-pulling instanced rendering with additive and alpha-blend pipelines
- Sprite atlas textures, image decomposition (grid/threshold/random sampling)
- Per-particle aux buffer for home positions and packed RGBA
- Configurable emitters: point, ring, line, screen

### Audio
- Multi-resolution FFT (4096/1024/512-pt) with 20 audio features across 7 frequency bands
- Adaptive normalization with per-feature running min/max
- 3-stage beat detection: onset detector → FFT autocorrelation tempo estimator → predictive beat scheduler
- Kalman filter BPM tracking with octave disambiguation
- Audio input device selector with runtime switching and persistence
- PulseAudio capture on Linux with dlopen (no libpulse build dependency), cpal/ALSA fallback
- `--audio-test` CLI flag for standalone audio diagnostics (no GPU required)

### Layer Composition
- Up to 8 compositing layers, each with independent effect/params/particles
- 10 blend modes: Normal, Add, Screen, ColorDodge, Multiply, Overlay, HardLight, Difference, Exclusion, Subtract
- Per-layer opacity, enable/disable, lock (prevent changes), pin (prevent reorder)
- Drag-and-drop layer reordering
- GPU compositor with single-layer fast path (zero overhead for one layer)

### MIDI + OSC
- MIDI input via midir: MIDI learn, auto-connect, hot-plug detection, CC mapping for params and triggers
- OSC input/output via rosc: RX on port 9000, TX on port 9001, OSC learn, layer-targeted messages
- Config persistence for both MIDI and OSC bindings

### Web Control Surface
- Same-port HTTP + WebSocket server with embedded single-file touch UI
- Bidirectional JSON state sync with full snapshot on connect
- Multi-client support, audio broadcast at 10Hz, mobile-first dark theme
- Auto-reconnect with exponential backoff

### Media Layers
- PNG/JPEG/GIF/WebP as compositing layers with aspect-ratio-correct letterbox
- Animated GIF/WebP playback with forward/reverse/ping-pong, speed control, loop toggle
- Video playback (feature-gated `video`): MP4/MOV/AVI/MKV/WebM via ffmpeg pre-decode, seek slider, 60s max

### Webcam Input
- Live camera feed as compositing layer (feature-gated `webcam`) via nokhwa
- Cross-platform: v4l2, AVFoundation, MediaFoundation
- Capture thread with frame drain, device controls, mirror, preset save/load with device reconnection
- Corrupted frame recovery, dead thread detection, panic hardening

### NDI Output
- Runtime-loaded NDI SDK via libloading (feature-gated `ndi`), no build-time dependency
- GPU capture with double-buffered staging and 1-frame latency readback
- Per-effect alpha preserved end-to-end through compositor and post-processing
- Luma-to-alpha toggle, configurable source name and resolution
- Sender thread with bounded channel (drops frames to maintain VJ performance)

### Preset System
- Save/load multi-layer compositions with effect params, blend modes, opacity, lock/pin state
- Async preset loading for media/video layers with background decoding thread
- Generation-based cancellation for rapid MIDI preset cycling
- MIDI triggers for next/prev preset navigation
- Bundled "Crucible" preset showcasing all 8 layers

### UI & Accessibility
- egui overlay (D key toggle) with WCAG 2.2 AA dark/light themes plus Midnight, Ember, Neon VJ themes
- Auto-generated parameter controls, 7-band audio spectrum, BPM display with beat flash
- Status bar with keyboard hints, particle count, audio health, NDI/MIDI/OSC/Web status dots
- macOS app icon and polished DMG installer

### Testing
- ~251 unit tests across 17 modules covering audio, params, effects, layers, presets, MIDI, OSC, web, NDI, themes

## v0.2.0

Initial development release.

---
NDI® is a registered trademark of Vizrt NDI AB.
