# Fosfora

![CI](https://github.com/kevinraymond/fosfora/actions/workflows/ci.yml/badge.svg)

**Cross-platform real-time particle and shader engine for live performance. Welcome to the light.**

<p align="center">
  <img src="assets/fosfora-teaser.gif" alt="Fosfora reacting to music" width="100%" />
</p>

<!-- Placeholder hero clip. Replace with a fresh 10–15s screen capture of a few effects reacting to music — the single biggest thing that shows what Fosfora does. -->

## What is this?

Fosfora is a **free, open-source app that turns whatever audio your computer is playing into live, beat-synced visuals** — a music visualizer built for actually performing with. There's nothing to code, no account to create, and no setup: open it, play music, and it reacts.

At a glance:

| | |
|---|---|
| **Rendering** | Native GPU app (Vulkan/Metal via wgpu) · Compute rasterizer for million-particle effects · Shader hot-reload (edit WGSL live) |
| **Audio** | BPM detection · 7-band spectrum · 13 MFCC timbral features · 12 chroma pitch classes · Beat sync |
| **Effects** | 24 built-in effects (particle sims, feedback shaders, reaction-diffusion, flocking, strange attractors, morphing) |
| **Compositing** | 8-layer stack · 10 blend modes · Media layers (GIF/PNG/MP4) · Webcam layers · Monocular depth (MiDaS) |
| **Control** | Binding matrix (flow editor) · MIDI learn + auto-connect · OSC in/out with learn · Web touch surface (phone/tablet) |
| **Output** | NDI® (built in, runtime-loaded) · Presets · Scene cues with timeline morphing |

## Note from Dev

Thanks for checking this project out! I've gone through a couple experimental projects recently - [EASE](https://github.com/kevinraymond/ease) and [EASEy-GLYPH](https://github.com/kevinraymond/easey-glyph) - and with Fosfora I'm trying to put it all together.

I hope this project is genuinely useful. I've tried to make it as easy to use as possible, but I'm no kind of performer so I'm open to suggestions for improvements (drop an issue)!

Everything below here is AI-assisted, like this entire project, except for this first section. I started writing software during the time of C-64 and TRaSh-80 ... even so, I never imagined it would be at this point. Wild times.

Give it a try! Let me know how I can make it better for you.

---

Under the hood it's a GPU engine — WGSL shaders, compute particles, and layer compositing — that you drive from the panels below, or from MIDI, OSC, or a phone browser:

<p align="center">
  <img width="1518" height="1222" alt="Fosfora UI" src="https://github.com/user-attachments/assets/3072dd58-17d4-4ce4-8166-429bee57f780" />
</p>


## Quick Start

Three steps, and you can't break anything — every setting saves to its own file and can be reset.

**1. Download it** for your platform from [**GitHub Releases**](https://github.com/kevinraymond/fosfora/releases/latest):

- **macOS** — download the `.dmg`, open it, drag Fosfora to Applications (signed & notarized)
- **Windows** — download the `.zip`, extract, run `fosfora.exe`
- **Linux** — download the `.tar.gz`, extract, run `./fosfora`

**2. Open it.** A visual is already running when the window appears. The control UI fades in after a second or two — or press **D** to show it right away.

**3. Play some music** — anything your computer can hear. The visuals react immediately from your default input device.

**What to expect in the first 10 seconds:** the window opens with a visual already moving → the panels fade in → press **D** to toggle them → click an effect on the left → press **F** for fullscreen. That's the whole loop.

NDI® output is built into the official downloads — to use it, install the [NDI® runtime](https://ndi.video) (the only extra step needed).

<details>
<summary><strong>Build from source</strong></summary>

**Prerequisites:** Rust 1.97+ (pinned via `rust-toolchain.toml`), a Vulkan-capable GPU.

```bash
git clone https://github.com/kevinraymond/fosfora.git
cd fosfora
cargo run --release                    # no extra deps
cargo run --release --features video   # requires: ffmpeg on PATH
cargo run --release --features webcam  # requires: libclang-dev, v4l-utils (Linux)
cargo run --release --features ndi     # requires: NDI SDK (runtime-loaded)
cargo run --release --features depth   # requires: libssl-dev, libclang-dev (includes webcam), ONNX Runtime (runtime, auto-downloaded)
```

</details>

**New to Fosfora?** Check out the [Tutorials](TUTORIALS.md) for in-depth guides on every feature — effects, audio, layers, MIDI, OSC, and more.

## Make it yours

A few ideas, in plain terms:

- **Effects** are like TV channels — flip through them in the left panel until one grabs you.
- **Layers** are stacked transparent sheets (think Photoshop or OBS): stack one effect over another, set each sheet's opacity and blend mode, and drag to reorder.
- **Presets** are saved looks — build a layer stack you love, save it, and recall it instantly later or from a MIDI/OSC trigger.
- The **binding matrix** is a patch bay: draw a line from any source (a MIDI knob, an OSC message, an audio feature, your phone, even hand tracking) to any target (a slider, layer opacity, a particle setting) to make it move with the music or your hands.

## Controls

### Keyboard

| Key | Action |
|-----|--------|
| `D` | Toggle the UI overlay |
| `F` | Toggle fullscreen |
| `B` | Open the binding matrix |
| `[` / `]` | Previous / next layer (when you have more than one) |
| `Space` | Go to the next scene cue (when a timeline has cues) |
| `T` | Play / pause the scene timeline (when a timeline has cues) |
| `Esc` | Quit — or close the binding matrix first if it's open |

`Tab` and the arrow keys navigate the UI itself (widget focus, slider nudges) — see [QUICK-REFERENCE.md](QUICK-REFERENCE.md) for the full accessibility shortcuts.

### Binding Matrix

Press **B** (or click "Matrix" in the left panel) to open the binding matrix — a full-screen flow editor for connecting any source to any target. Three columns show sources (left), active bindings (center), and targets (right) with animated Bezier connection lines. Sources include MIDI CCs, OSC addresses, audio features, and WebSocket bridges (e.g. MediaPipe hand/pose/face tracking, game controllers). Targets include all effect parameters, layer opacity/blend, particle settings, and post-processing controls.

### MIDI

Any MIDI controller works. Plug in and use MIDI learn:

1. Click the **M** button next to any parameter
2. Move a knob or press a button on your controller
3. The binding saves automatically to `~/.config/phosphor/midi.json`

Trigger actions (next/prev effect, layer, preset, toggle post-processing) can also be MIDI-learned.

### OSC

| Port | Direction | Default |
|------|-----------|---------|
| 9000 | Receive | On |
| 9001 | Transmit | Off (enable in UI) |

Addresses: `/phosphor/param/{name}`, `/phosphor/trigger/{action}`, `/phosphor/layer/{n}/opacity`, `/phosphor/layer/{n}/blend`, `/phosphor/layer/{n}/enabled`, `/phosphor/postprocess/enabled`.

OSC learn works the same as MIDI learn — click the **O** button, send any message to bind.

### Web Touch Control

When enabled in the UI, a touch-friendly control surface is served at:

```
http://localhost:9002
```

Open it on a phone or tablet on the same network. Supports multiple simultaneous clients with live audio visualization, effect selection, param sliders, and layer management.

## Effects

**24 built-in effects** — 22 you can browse and cycle through, plus 2 hidden ones (the signature **Phosphor** intro visual and a rasterizer stress test). All are audio-reactive out of the box, and every parameter is a slider in the UI that you can map to MIDI/OSC.

**Shader effects** (pure GPU shaders):

| Effect | Description |
|--------|-------------|
| **Aurora** | Flowing curtain bands driven by 7 frequency bands |
| **Drift** | Triple domain-warped FBM fluid smoke with advected feedback |
| **Iris** | Spinning dot with fading feedback trails |
| **Prism** | Kaleidoscopic N-fold mirror symmetry over FBM patterns |
| **Pulse** | Beat-synced concentric rings with feedback trails |
| **Shards** | Animated Voronoi cells with glowing fracture edges |
| **Storm** | Billowing dark clouds lit from within by lightning |
| **Tunnel** | Raymarched infinite cylindrical flythrough with twist and glow |

**Particle effects** (GPU compute simulations):

| Effect | Description |
|--------|-------------|
| **Accretion** | Gravitational N-body — audio seeds attract swarms into discs and orbits |
| **Array** | Toroidal per-band speaker emitters firing rings of particles outward |
| **Cascade** | Screen edges emit audio-segmented particle streams that interfere |
| **Chaos** | Strange-attractor system (Lorenz, Rössler, Chen…) with feedback trails |
| **Cymatics** | Chladni standing-wave nodal patterns synced to frequency bands |
| **Flux** | Organic smoke following a 3D curl-noise flow field |
| **Genesis** | Multi-species Particle Lenia self-organizing into predator/prey |
| **Morph** | Particles spring between images and geometry on beat drops |
| **Murmur** | Starling murmuration with topological K=7 flocking |
| **Mycelium** | Branching tendrils that grow at the tips and decay at the roots |
| **Raster** | Video wall — particles map to image pixels with audio displacement |
| **Symbiosis** | Particle Life multi-species ecosystems from a force matrix |
| **Tesla** | Charged particles spiraling through magnetic dipole fields |
| **Turing** | Reaction-diffusion (Gray-Scott) sculpting particles into organic patterns |

## Layers

- Up to **8 layers** composited on the GPU
- **10 blend modes**: Normal, Add, Screen, Color Dodge, Multiply, Overlay, Hard Light, Difference, Exclusion, Subtract
- Per-layer **opacity**, **enable/disable**, **lock** (freezes all settings), **pin** (prevents reorder)
- **Drag-and-drop** reordering in the layer panel
- **Media layers**: load PNG/JPEG/GIF/WebP as compositing layers with letterbox fit and animated playback
- **Video layers** (with `--features video`): MP4/MOV/AVI/MKV/WebM pre-decoded to RAM for instant scrub and seek (requires ffmpeg, 60s max)
- **Post-processing**: bloom, chromatic aberration, ACES tonemapping, vignette, film grain (audio-reactive)

## Presets

- Save/load the entire layer stack (effects, params, blend modes, opacities) as named presets
- Stored in `~/.config/phosphor/presets/` as JSON
- Cycle presets via MIDI/OSC triggers or the preset panel
- Locked layers are preserved during preset load
- Bundled presets: **Crucible** and **Spectral Eye** — full multi-layer compositions to start from

## Configuration

All config is stored in `~/.config/phosphor/`:

| File | Contents |
|------|----------|
| `midi.json` | MIDI port, CC/trigger bindings |
| `osc.json` | OSC ports, address bindings |
| `web.json` | Web control surface port, enable state |
| `presets/*.json` | Named presets |

## FAQ / Troubleshooting

**The visuals aren't reacting to my music.**
Fosfora listens to an *input* device. Open the **Audio** panel (right sidebar) and pick the right source:
- **Linux** — choose the **Monitor** of your output device (that's your system audio via PulseAudio/PipeWire); a plain microphone only hears the room.
- **Windows** — pick the WASAPI **loopback** device for system audio, or a mic to react to the room.
- **macOS** — the default input is your mic; to visualize system audio, route it with a loopback tool (e.g. BlackHole).

**I just get a black screen / it won't start.**
Fosfora needs a Vulkan-capable GPU (Vulkan on Linux/Windows, Metal on macOS). Update your graphics drivers, and if you built from source make sure the release build ran cleanly.

**macOS says it can't verify the app / it's from an unidentified developer.**
The official `.dmg` is signed and notarized, so a normal download-and-drag should just work. If macOS still blocks it, right-click the app → **Open** the first time, or allow it under **System Settings → Privacy & Security**.

**Where do my settings and presets live?**
In a per-user config folder — see [Configuration](#configuration) for the exact files. Deleting them resets Fosfora to defaults; nothing else on your system is touched.

## Writing Effects

Effects are WGSL fragment shaders paired with a JSON `.pfx` definition. Shaders have access to time, resolution, 74 audio features (7 frequency bands, beat detection, spectral shape, 13 MFCC timbral coefficients, 12 chroma pitch classes, plus loudness, key, downbeat, stereo, structure, harmonic/percussive, pitch and spectral-contrast detectors), three live audio textures (`waveform`/`spectrum`/`spectrogram` — for oscilloscopes, spectrum bars and waterfalls), up to 16 parameters, feedback from the previous frame, and a built-in library (noise, palette, SDF, tonemap).

Edit a shader while running — it hot-reloads on save with error recovery.

See [TECHNICAL.md](TECHNICAL.md#shader-authoring-guide) for the full authoring guide, uniform reference, multi-pass pipelines, and particle system integration.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for build instructions, the effect creation walkthrough, and PR guidelines.

## Acknowledgments

**Rendering & GPU**
- [wgpu](https://github.com/gfx-rs/wgpu) — WebGPU implementation (Vulkan/Metal/DX12)
- [egui](https://github.com/emilk/egui) — Immediate-mode GUI
- [naga](https://github.com/gfx-rs/wgpu/tree/trunk/naga) — WGSL shader validation
- [glam](https://github.com/bitshifter/glam-rs) — Linear algebra

**Audio**
- [cpal](https://github.com/RustAudio/cpal) — Cross-platform audio I/O
- [rustfft](https://github.com/ejmahler/RustFFT) — FFT for spectral analysis and beat detection
- [midir](https://github.com/Boddlnagg/midir) — Cross-platform MIDI I/O

**Networking & Control**
- [rosc](https://github.com/klingtnet/rosc) — Open Sound Control protocol
- [tungstenite](https://github.com/snapview/tungstenite-rs) — WebSocket server
- [NDI](https://ndi.video) — Network Device Interface (runtime-loaded)

**Depth Estimation**
- [MiDaS](https://github.com/isl-org/MiDaS) (Intel ISL) — Monocular depth estimation model
- [ONNX Runtime](https://onnxruntime.ai) via [ort](https://github.com/pykeio/ort) — ML inference

**Algorithms & Techniques**
- [Reynolds Boids](https://www.red3d.com/cwr/boids/) (Craig Reynolds) — Flocking behavior baseline for Murmur effect
- Vicsek model — Noise-driven order-chaos phase transitions in Murmur
- Topological interaction (K=7 nearest neighbors) — Scale-free correlations in Murmur
- [Inigo Quilez](https://iquilezles.org/) — Smooth Worley noise (log-sum-exp) in Storm
- Beer-Lambert law — Volumetric light absorption in Storm
- Curl noise — Divergence-free particle advection in Flux
- MFCC / Chroma — Mel-frequency cepstral coefficients and pitch-class energy features
- Beat detection pipeline ported from [EASEy-GLYPH](https://github.com/kevinraymond/easey-glyph)

**Fonts (SIL Open Font License 1.1)**
- [Inter](https://github.com/rsms/inter) — Rasmus Andersson
- [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono) — JetBrains

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option.
