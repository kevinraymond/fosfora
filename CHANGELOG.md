# Changelog

<!-- Release workflow extracts notes between ## vX.Y.Z headers via awk. -->
<!-- Keep the "## vX.Y.Z — date" format for automatic release notes. -->

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
