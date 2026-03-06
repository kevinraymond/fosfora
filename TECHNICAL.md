# Phosphor Technical Reference

Developer and contributor reference for the Phosphor engine internals.

## Architecture

Phosphor runs on multiple threads with no mutexes in the hot rendering path:

```
Main Thread        winit event loop → drain channels → update uniforms
                   → per-layer PassExecutor → Compositor → PostProcess → egui → present

Audio Thread       cpal callback → ring buffer → multi-res FFT → adaptive normalize
                   → beat detect → smooth → send AudioFeatures via crossbeam

MIDI Thread        midir callback → parse 3-byte MIDI → send MidiMessage (bounded 64)

OSC Thread         UdpSocket recv → rosc decode → send OscInMessage (bounded 64)

Web Accept Thread  TcpListener → HTTP serve or WebSocket upgrade → spawn client thread

Web Client Threads 50ms read timeout → parse JSON → WsInMessage (bounded 64)
                   drain outbound broadcast channel

File Watcher       notify → debounce 100ms → send changed shader paths
```

The main thread drains all input channels each frame in order: MIDI → OSC → Web (last-write-wins).

## Render Pipeline

```
For each enabled layer:
  Compute Dispatch (particle sim, if active)
                  ↓
  Effect Pass(es) → PingPong HDR Target [Rgba16Float]
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
egui Overlay → Surface → Present
```

## Source Layout

```
crates/phosphor-app/src/
├── main.rs              Entry point, wgpu/winit init
├── app.rs               Main App struct, event loop, channel draining
├── audio/               cpal capture, multi-res FFT, beat detection, smoothing
├── effect/              .pfx loader, effect registry, shader library prepend
├── gpu/
│   ├── mod.rs           RenderTarget, PingPongTarget, UniformBuffer
│   ├── layer.rs         Layer, LayerStack, LayerContent, BlendMode, Compositor
│   ├── compositor.rs    GPU blend pipeline (7 modes, ping-pong accumulator)
│   └── particle/
│       ├── system.rs    ParticleSystem (compute dispatch, render, ping-pong)
│       ├── types.rs     ParticleDef, EmitterDef, ParticleUniforms
│       ├── emitter.rs   Emitter shapes (point, ring, line, screen, image)
│       ├── sprite.rs    SpriteAtlas loader, dual blend pipelines
│       └── image_source.rs  Image decomposition (grid/threshold/random sampling)
├── media/               MediaLayer, GIF/WebP decoder, blit pipeline
├── midi/                midir integration, MIDI learn, config persistence
├── osc/                 rosc integration, OSC learn, TX broadcast
├── params/              ParamDef, ParamStore, uniform packing
├── preset/              PresetStore, save/load, layer snapshots
├── shader/              Hot-reload, PassExecutor, multi-pass orchestration
├── ui/
│   ├── panels/          egui panels (effects, params, layers, presets, MIDI, OSC, web)
│   ├── theme/           WCAG 2.2 AA dark/light themes
│   └── accessibility/   Reduced motion detection (stub)
└── web/                 WebSocket server, embedded HTML control surface, state sync
```

Shaders:
```
assets/shaders/
├── *.wgsl               Effect fragment shaders (aurora, drift, tunnel, etc.)
├── builtin/             Engine shaders (composite, blit, bloom, particle render/sim)
└── lib/                 WGSL library (noise.wgsl, palette.wgsl, sdf.wgsl, tonemap.wgsl)

assets/effects/
└── *.pfx                Effect definitions (JSON)
```

## Shader Authoring Guide

### Minimal Example

**`assets/shaders/my_effect.wgsl`:**
```wgsl
fn effect(uv: vec2f) -> vec4f {
    let t = u.time;
    let bass = u.bass;

    let color = 0.5 + 0.5 * cos(t + uv.xyx + vec3f(0.0, 2.0, 4.0));
    let brightness = mix(0.3, 1.0, bass);

    return vec4f(color * brightness, 1.0);
}
```

**`assets/effects/my_effect.pfx`:**
```json
{
    "name": "My Effect",
    "author": "You",
    "description": "A colorful thing",
    "shader": "my_effect.wgsl",
    "inputs": [
        { "type": "Float", "name": "speed", "default": 0.5, "min": 0.0, "max": 1.0 }
    ]
}
```

Run the app — the effect appears in the browser. Edit the `.wgsl` file and save; it hot-reloads instantly.

### Shader Template

Every effect shader must define an `effect(uv: vec2f) -> vec4f` function. The engine auto-prepends:
- The uniform block (access via `u.time`, `u.bass`, etc.)
- The shader library (`noise.wgsl`, `palette.wgsl`, `sdf.wgsl`, `tonemap.wgsl`)
- The `param(i)` helper function
- The `feedback(uv)` function (if feedback is enabled)

### Uniform Reference

All fields are accessible in WGSL as `u.field_name`:

**Core:**

| Field | Type | Description |
|-------|------|-------------|
| `time` | `f32` | Elapsed seconds |
| `resolution` | `vec2f` | Viewport size in pixels |
| `frame_index` | `u32` | Frame counter |
| `feedback_decay` | `f32` | Feedback blend factor |

**Audio (20 fields):**

| Field | Type | Range | Description |
|-------|------|-------|-------------|
| `sub_bass` | `f32` | 0-1 | 20-60 Hz |
| `bass` | `f32` | 0-1 | 60-250 Hz |
| `kick` | `f32` | 0-1 | Dedicated kick detection (30-120 Hz) |
| `low_mid` | `f32` | 0-1 | 250-500 Hz |
| `mid` | `f32` | 0-1 | 500-2000 Hz |
| `upper_mid` | `f32` | 0-1 | 2-4 kHz |
| `presence` | `f32` | 0-1 | 4-6 kHz |
| `brilliance` | `f32` | 0-1 | 6-20 kHz |
| `rms` | `f32` | 0-1 | Overall loudness |
| `onset` | `f32` | 0-1 | Transient detection |
| `centroid` | `f32` | 0-1 | Spectral brightness |
| `flux` | `f32` | 0-1 | Spectral change rate |
| `spread` | `f32` | 0-1 | Spectral width |
| `flatness` | `f32` | 0-1 | Noise vs. tone |
| `rolloff` | `f32` | 0-1 | High-frequency energy cutoff |
| `beat` | `f32` | 0/1 | Beat trigger (1.0 on beat frame) |
| `beat_phase` | `f32` | 0-1 | Sawtooth at detected tempo |
| `beat_strength` | `f32` | 0-1 | Beat confidence |
| `bpm` | `f32` | 0-1 | BPM / 300 (display: `bpm * 300.0`) |
| `flux_raw` | `f32` | 0-1 | Unsmoothed spectral flux |

**Parameters:**

Access via `param(0u)` through `param(15u)`. Up to 16 float params per effect, defined in the `.pfx` `inputs` array.

**Feedback:**

Call `feedback(uv)` in your shader to read the previous frame. Requires `"feedback": true` in the pass definition.

### Shader Library

Auto-prepended to all effect shaders:

| File | Functions |
|------|-----------|
| `noise.wgsl` | `hash`, `noise2d`, `noise3d`, `fbm2d`, `fbm3d`, `voronoi` |
| `palette.wgsl` | `palette(t, a, b, c, d)` — cosine palette generator |
| `sdf.wgsl` | `sd_circle`, `sd_box`, `sd_line`, `sd_ring`, and more |
| `tonemap.wgsl` | `aces_tonemap`, `gamma`, `linear_to_srgb` |

### Multi-Pass Effects

For effects needing multiple rendering passes (e.g., feedback background + foreground):

```json
{
    "name": "My Multi-Pass",
    "shader": "",
    "passes": [
        { "name": "background", "shader": "my_bg.wgsl", "feedback": true },
        { "name": "foreground", "shader": "my_fg.wgsl", "feedback": false }
    ],
    "inputs": [...]
}
```

When using `passes`, set `"shader": ""` at the top level. Each pass gets its own render target. The final pass output goes to the layer.

### Particle Effects

Add a `particles` block to the `.pfx` to spawn GPU compute particles on top of the fragment shader:

```json
{
    "particles": {
        "max_count": 3000,
        "compute_shader": "my_sim.wgsl",
        "emitter": { "shape": "ring", "radius": 0.3, "position": [0.0, 0.0] },
        "lifetime": 6.0,
        "initial_speed": 0.35,
        "initial_size": 0.012,
        "size_end": 0.004,
        "gravity": [0.0, 0.0],
        "drag": 0.995,
        "turbulence": 0.0,
        "attraction_strength": 0.4,
        "emit_rate": 50.0,
        "burst_on_beat": 60
    }
}
```

Emitter shapes: `point`, `ring`, `line`, `screen`, `image`.

Particle data uses SoA (Structure of Arrays) layout: 4 separate `vec4f` storage buffers (`pos_life`, `vel_size`, `color`, `flags`), 16 bytes each, 64 bytes total per particle. Position-only reads (spatial hash, sort keygen) load 16B instead of 64B — 4× bandwidth savings. Particles render as instanced quads with additive blending into the HDR target, so bloom and post-processing apply automatically.

Custom compute shaders receive the noise library but not the fragment uniform block. They have their own 128-byte `ParticleUniforms` with 10 audio fields: `sub_bass`, `bass`, `mid`, `rms`, `kick`, `onset`, `centroid`, `flux`, `beat`, `beat_phase`.

### Authoring Tips

- **`atan2` seam**: Avoid `atan2` in palette/color calculations — visible seam at ±pi (negative x-axis). Use radius, depth, or time instead. For angular patterns, use `sin(angle * N)` directly.
- **Feedback decay**: Use `mix()` not `max()` so dark areas can reclaim space. Keep decay ≤ 0.88. Clamp output (`min(result, vec3f(1.2))`) to prevent blowout.
- **Time * audio**: Never multiply `time * audio_varying_value` for position/speed — causes jittery oscillation. Use constant speed, apply audio to other properties (size, color, opacity).
- **Smooth Worley**: Use log-sum-exp (`-log(sum(exp(-k*d^2)))/k`) not `min()` for smooth Voronoi. `max(0.0, ...)` before `sqrt()` to prevent NaN.
- **Reserved words**: `target` is a WGSL reserved word — use `look_at` or similar instead.
- **Uniform alignment**: WGSL uniform arrays must be `array<vec4f, N>` (16-byte alignment), not `array<f32, N>`.
- **Additive particles + feedback**: Clamp feedback values in background shader (`min(col, vec3f(1.5))`) to prevent runaway accumulation. Keep particle brightness low (~0.12) since steady state = brightness / (1 - decay).

## .pfx Format Reference

All fields with types and defaults:

```
{
    "name": string,                    // Display name (required)
    "author": string,                  // Author name (required)
    "description": string,             // One-line description (required)
    "shader": string,                  // WGSL filename for single-pass effects
                                       // Empty string "" when using passes array

    "inputs": [                        // Parameter definitions (up to 16)
        {
            "type": "Float",           // Float | Color | Bool | Point2D
            "name": string,            // Param name (used in MIDI/OSC bindings)
            "default": number,
            "min": number,             // Float only
            "max": number              // Float only
        }
    ],

    "passes": [                        // Multi-pass pipeline (optional)
        {
            "name": string,            // Pass name
            "shader": string,          // WGSL filename
            "feedback": bool           // Enable feedback(uv) for this pass (default: false)
        }
    ],

    "particles": {                     // GPU particle system (optional)
        "max_count": int,              // Max particles (default: 1000)
        "compute_shader": string,      // Custom sim shader (optional, uses builtin if omitted)
        "emitter": {
            "shape": string,           // point | ring | line | screen | image
            "radius": float,           // Ring radius (default: 0.5)
            "position": [float, float],// Emitter center (default: [0,0])
            "image": string            // Image path (shape "image" only)
        },
        "lifetime": float,            // Seconds (default: 3.0)
        "initial_speed": float,        // (default: 0.5)
        "initial_size": float,         // (default: 0.02)
        "size_end": float,             // Size at death (default: 0.005)
        "gravity": [float, float],     // (default: [0, 0])
        "drag": float,                 // Velocity damping (default: 0.99)
        "turbulence": float,           // Noise force (default: 0.0)
        "attraction_strength": float,  // Center attraction (default: 0.0)
        "emit_rate": float,            // Particles per second (default: 100)
        "burst_on_beat": int,          // Extra particles on beat (default: 0)
        "sprite": {                    // Sprite texture (optional)
            "path": string,            // Atlas image path
            "cols": int,               // Atlas columns
            "rows": int,               // Atlas rows
            "fps": float               // Animation frame rate
        },
        "image_sample": {              // Image decomposition (optional)
            "path": string,            // Source image
            "mode": string,            // grid | threshold | random
            "count": int               // Sample count
        },
        "blend": string               // "additive" (default) | "alpha"
    },

    "postprocess": {                   // Post-processing overrides (optional)
        "enabled": bool,               // (default: true)
        "bloom_threshold": float,      // (default: 0.8)
        "bloom_intensity": float,      // (default: 0.3)
        "vignette": float              // (default: 0.3)
    }
}
```

## Audio Pipeline

**Capture:** cpal grabs audio from the default input device. Samples flow through a lock-free ring buffer to a dedicated audio thread.

**Multi-resolution FFT:** Three FFT sizes target different frequency ranges:
- 4096-pt: sub_bass (20-60 Hz), bass (60-250 Hz), kick (30-120 Hz)
- 1024-pt: low_mid (250-500 Hz), mid (500-2000 Hz), upper_mid (2-4 kHz)
- 512-pt: presence (4-6 kHz), brilliance (6-20 kHz)

Bass bands use linear RMS, mid/high bands use dB scaling (80 dB range).

**Adaptive normalization:** Per-feature running min/max with 0.005 decay replaces all fixed gain multipliers. Every audio feature auto-scales to the 0-1 range based on recent history.

**3-stage beat detection:**
1. **OnsetDetector** — log-magnitude multi-band spectral flux with adaptive threshold
2. **TempoEstimator** — FFT autocorrelation (Wiener-Khinchin), genre-aware log-Gaussian tempo prior (center 150 BPM), multi-ratio octave correction, cascading octave-up via local peak detection, Kalman filter in log2-BPM space
3. **BeatScheduler** — predictive state machine with phase correction, outputs `beat` trigger and `beat_phase` sawtooth

**Smoothing:** Asymmetric EMA (fast attack, slow release) on all features. `beat` and `beat_phase` are pass-through (no smoothing).

## GPU Particle System

- **Storage:** Two storage buffers (A and B) in ping-pong pattern — read from A, write to B, flip each frame. Avoids read-write hazards.
- **Emission:** Atomic counter. Dead particles (`life <= 0`) claim new emission slots via `atomicAdd`.
- **Rendering:** Vertex-pulling instanced draw — 6 vertices per particle expand to screen-space quads with aspect ratio correction. No vertex buffer.
- **Blending:** Additive (SrcAlpha + One) by default, or alpha (SrcAlpha + OneMinusSrcAlpha) via `blend` field.
- **Integration:** Particles render INTO the HDR target with `LoadOp::Load`, so bloom, feedback, and post-processing all apply automatically.
- **Particle SoA layout:** 4 separate `vec4f` storage buffers (`pos_life`, `vel_size`, `color`, `flags`), 16 bytes each, 64 bytes total per particle. `read_particle()`/`write_particle()` helpers abstract multi-buffer access. Compute bind group has 13 entries (ping-pong × 4 buffers + counters + uniforms + aux + sort); render bind group has 6 entries. Position-only reads (spatial hash, sort keygen) load 16B instead of 64B — 4× bandwidth savings.
- **ParticleAux buffer:** 16 bytes per particle (binding 4, read-only in compute) — `home.xy`, `home.z` = packed RGBA, `home.w` = sprite index. Used for image decomposition spring-reform.

## Layer System

- **LayerContent enum:** `Effect(EffectLayer)` or `Media(MediaLayer)`. Each Layer owns its own `PassExecutor`, `UniformBuffer`, `ParamStore`, render targets.
- **Compositor:** Ping-pong accumulator — blit first enabled layer, then `composite(accumulator, layer[i])` for each subsequent layer using the selected blend mode.
- **Single-layer fast path:** When only 1 layer is enabled, compositing is skipped entirely (zero overhead).
- **Lock:** Prevents all setting changes (blend, opacity, enable, params, effect loading). Locked layers are skipped during preset load. MIDI CC is blocked.
- **Pin:** Prevents drag reordering. Pinned layers hide the drag handle.
- **Media layers:** GPU blit with letterbox fit. Animated GIF/WebP playback with transport controls (speed, direction, loop). Frame upload only on change.

## Control Inputs

Main thread drains all input channels each frame:

1. **MIDI** — midir callback thread → crossbeam bounded(64). CC values scaled to param range. Rising-edge trigger detection (threshold 64). Auto-connect, hot-plug (2s poll).
2. **OSC** — rosc UDP receiver thread → crossbeam bounded(64). Float params, int blend modes, float triggers (>0.5). Layer-targeted addresses.
3. **Web** — tungstenite WebSocket, thread-per-client. JSON messages. Full state snapshot on connect, 10Hz audio broadcast. Same-port HTTP+WS via `ReplayStream`.

Drain order: MIDI → OSC → Web. Last write wins per frame.

## Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| wgpu | 27 | GPU rendering (Vulkan/Metal/DX12) |
| winit | 0.30 | Window management, input events |
| egui / egui-wgpu / egui-winit | 0.33 | Immediate-mode UI overlay |
| cpal | 0.17 | Cross-platform audio capture |
| rustfft | 6 | FFT for audio analysis |
| notify | 8 | Filesystem watching (shader hot-reload) |
| midir | 0.10 | MIDI input |
| rosc | 0.11 | OSC encode/decode |
| tungstenite | 0.28 | WebSocket server |
| serde / serde_json | 1 | Serialization (configs, presets, .pfx) |
| crossbeam-channel | 0.5 | Lock-free MPSC channels |
| bytemuck | 1 | GPU uniform packing (zero-copy cast) |
| image | 0.25 | PNG/JPEG/GIF/WebP loading |
| gif | 0.13 | Animated GIF frame decoding |
| image-webp | 0.2 | Animated WebP decoding |
| rfd | 0.15 | Native file dialogs |
| glam | 0.29 | Linear algebra |
| dirs | 6 | XDG config paths |
| anyhow / thiserror | 1 / 2 | Error handling |
