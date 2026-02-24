# Phosphor

Cross-platform particle and shader engine for live VJ performance. Built with raw winit + wgpu + egui (not Bevy).

## Project Status

**Phase 1 (Core Rendering MVP): COMPLETE** — committed as `fa187b5`
**Phase 2 (Multi-Pass Rendering): COMPLETE** — multi-pass rendering infrastructure
**GPU Particle System: COMPLETE** — compute shader particles with ping-pong buffers

### What's Built

#### Phase 1
- winit 0.30 window with wgpu 27 Vulkan rendering (fullscreen triangle technique)
- Shader hot-reload (notify file watcher, 100ms debounce, error recovery keeps old pipeline)
- Parameter system: ParamDef (Float/Color/Bool/Point2D), ParamStore, uniform packing
- Audio pipeline: cpal capture → lock-free ring buffer → dedicated thread → 2048-pt FFT (rustfft) → 12 spectral features → asymmetric EMA smoothing → crossbeam channel to main thread
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
- Backward compatible: single-shader .pfx files work unchanged
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
- Feedback + particles requires HDR clamp in background shader (`min(col, vec3f(1.5))`) to prevent runaway accumulation

### Known Issues
- ~30 compiler warnings (mostly unused items reserved for future phases)
- `compiler.rs` has an unused `compile_shader` function
- Fonts directory (`assets/fonts/`) is empty — Inter and JetBrains Mono not yet bundled
- Reduced motion detection (`ui/accessibility/motion.rs`) is stubbed for macOS/Windows
- Multi-pass hot-reload only recompiles the main (first) pass shader; changes to secondary pass shaders require re-loading the effect

### Architecture
```
Main Thread: winit event loop → drain audio/shader channels → update uniforms → PassExecutor (effect passes) → PostProcessChain (bloom/tonemap) → egui overlay → present
Audio Thread: cpal callback → ring buffer → FFT → smooth → send AudioFeatures
File Watcher Thread: notify → debounce → send changed paths
```

No mutexes in hot path. Three threads + cpal callback.

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
3. Beat detection (bass energy peak tracking)
4. MIDI input with MIDI learn
5. Preset save/load
6. Layer-based composition with blend modes
7. OSC input/output
