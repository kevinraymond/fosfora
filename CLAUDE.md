# Phosphor

Cross-platform particle and shader engine for live VJ performance. Built with raw winit + wgpu + egui (not Bevy).

## Project Status

**Phase 1 (Core Rendering MVP): COMPLETE** — committed as `fa187b5`
**Phase 2 (Multi-Pass Rendering): COMPLETE** — multi-pass rendering infrastructure
**GPU Particle System: COMPLETE** — compute shader particles with ping-pong buffers
**Audio Upgrade + Beat Detection: COMPLETE** — multi-resolution FFT, adaptive normalization, 3-stage beat detector
**BPM Detection Rewrite: COMPLETE** — FFT autocorrelation, Kalman filter, octave disambiguation
**MIDI Input: COMPLETE** — midir integration, MIDI learn, auto-connect, hot-plug, config persistence
**Preset Save/Load: COMPLETE** — global presets with effect + params + postprocess, MIDI next/prev

### What's Built

#### Phase 1
- winit 0.30 window with wgpu 27 Vulkan rendering (fullscreen triangle technique)
- Shader hot-reload (notify file watcher, 100ms debounce, error recovery keeps old pipeline)
- Parameter system: ParamDef (Float/Color/Bool/Point2D), ParamStore, uniform packing
- Audio pipeline: cpal capture → lock-free ring buffer → dedicated thread → multi-resolution FFT (4096/1024/512-pt) → 20 features (7 bands + aggregates + spectral + beat) → adaptive normalization → 3-stage beat detection → asymmetric EMA smoothing → crossbeam channel to main thread
- egui overlay (D key toggle): WCAG 2.2 AA dark/light themes, audio spectrum bars, auto-generated param controls, effect browser, status bar
- .pfx JSON effect format with WGSL shader library (noise, palette, sdf, tonemap) auto-prepended
- 3 demo effects: plasma_wave, singularity (SDF raymarched), membrane (ocean surface)

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
- 2 new effects: feedback_test (spinning dot with trails), singularity_feedback (multi-pass demo)
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
- 2 new effects: particle_test (fountain), spectral_eye (orbital light trails with feedback)
- 1 combo effect: orbital_trails (feedback_test + spectral_eye layered)
- Feedback + particles requires HDR clamp in background shader (`min(col, vec3f(1.5))`) to prevent runaway accumulation

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
- Triggers: NextEffect, PrevEffect, TogglePostProcess, ToggleOverlay, NextPreset, PrevPreset with rising-edge detection
- Config persists to `~/.config/phosphor/midi.json` (JSON via `dirs` crate)
- Channel 0 = omni (respond to all channels)
- UI: port dropdown + activity dot + learn prompt in left panel, per-param MIDI badges + trigger learn in right panel, MIDI status in status bar

#### Preset Save/Load
- `PresetStore`: scan/save/load/delete presets from `~/.config/phosphor/presets/*.json`
- `Preset` struct: version, effect_name, params (HashMap<String, ParamValue>), postprocess (PostProcessDef)
- Preset panel in left sidebar: text input + Save button, selectable list with delete buttons
- Save captures current effect + tweaked params + postprocess overrides
- Load switches effect, applies saved params (unknown params skipped with warning), applies postprocess
- Overwrite on save (standard VJ workflow), graceful mismatch handling
- Name sanitization: strip `/ \ .`, trim whitespace, max 64 chars
- MIDI triggers: NextPreset/PrevPreset cycle through preset list
- Persists to disk as JSON, survives app restart

### Known Issues
- ~29 compiler warnings (mostly unused items reserved for future phases)
- Fonts directory (`assets/fonts/`) is empty — Inter and JetBrains Mono not yet bundled
- Reduced motion detection (`ui/accessibility/motion.rs`) is stubbed for macOS/Windows

### Architecture
```
Main Thread: winit event loop → drain audio/midi/shader channels → update uniforms → PassExecutor (effect passes) → PostProcessChain (bloom/tonemap) → egui overlay → present
Audio Thread: cpal callback → ring buffer → multi-res FFT → adaptive normalize → beat detect → smooth → send AudioFeatures
MIDI Thread: midir callback → parse 3-byte MIDI → send MidiMessage via crossbeam bounded(64)
File Watcher Thread: notify → debounce → send changed paths
```

No mutexes in hot path. Three threads + cpal callback + midir callback.

### Render Pipeline
```
Compute Dispatch (particle sim, if active)
                      ↓
Effect Pass(es) → PingPong HDR Target(s) [Rgba16Float]
                      ↓
Particle Render Pass (instanced quads, additive blend, LoadOp::Load)
                      ↓
PostProcessChain (if enabled):
  Bloom Extract (quarter-res) → Blur H → Blur V → Composite → Surface [sRGB]
PostProcessChain (if disabled):
  Simple Blit → Surface [sRGB]
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

### Controls
- `D` — Toggle egui overlay
- `F` — Toggle fullscreen
- `Esc` — Quit
- `Tab` — Cycle widgets (when overlay visible)
- Sliders have +/- buttons for WCAG 2.5.7 compliance

### Build & Run
```bash
cargo run                          # debug build
cargo run --release                # release build (much faster shaders)
RUST_LOG=phosphor_app=debug cargo run  # verbose logging
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
6. Layer-based composition with blend modes
7. OSC input/output
