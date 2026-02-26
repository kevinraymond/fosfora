# Phosphor

**Real-time audio-reactive shader engine for live VJ performance.**

Phosphor turns your audio input into layered, beat-synced visuals using GPU shaders, particles, and compositing — all driven by WGSL and controlled via MIDI, OSC, or a phone browser. Built with raw wgpu, no game engine required.

<!-- TODO: hero image/gif -->

## Quick Start

**Prerequisites:** Rust 1.85+, an audio input device, a Vulkan-capable GPU.

```bash
git clone https://github.com/your-username/phosphor.git
cd phosphor
cargo run --release
```

On first launch: press **D** to show the UI, pick an effect from the browser, and press **F** for fullscreen. Audio reactivity works immediately from your default input device.

## Controls

### Keyboard

| Key | Action |
|-----|--------|
| `D` | Toggle UI overlay |
| `F` | Toggle fullscreen |
| `[` / `]` | Cycle active layer |
| `Tab` | Cycle UI widgets |
| `Esc` | Quit |

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

| Effect | Description | Features |
|--------|-------------|----------|
| **Aurora** | Horizontal flowing curtains driven by 7 frequency bands | |
| **Drift** | Triple domain-warped FBM fluid smoke | Feedback |
| **Tunnel** | Log-polar infinite cylindrical flythrough | |
| **Prism** | Kaleidoscopic N-fold mirror symmetry with FBM | |
| **Shards** | Animated Voronoi cells with stained-glass fill | |
| **Pulse** | Beat-synced concentric rings with trails | Feedback |
| **Iris** | Spinning dot with fading feedback trails | Feedback |
| **Swarm** | Orbital particle cloud with luminous webs | Feedback, Particles |
| **Storm** | Volumetric clouds with beat-triggered lightning | Feedback |

All effects are audio-reactive out of the box. Parameters are exposed as sliders in the UI and mappable to MIDI/OSC.

## Layers

- Up to **8 layers** composited on the GPU
- **7 blend modes**: Normal, Add, Multiply, Screen, Overlay, Soft Light, Difference
- Per-layer **opacity**, **enable/disable**, **lock** (freezes all settings), **pin** (prevents reorder)
- **Drag-and-drop** reordering in the layer panel
- **Media layers**: load PNG/JPEG/GIF/WebP as compositing layers with letterbox fit and animated playback
- **Post-processing**: bloom, chromatic aberration, ACES tonemapping, vignette, film grain (audio-reactive)

## Presets

- Save/load the entire layer stack (effects, params, blend modes, opacities) as named presets
- Stored in `~/.config/phosphor/presets/` as JSON
- Cycle presets via MIDI/OSC triggers or the preset panel
- Locked layers are preserved during preset load
- Bundled preset: **Crucible** (all 8 layers composited)

## Configuration

All config is stored in `~/.config/phosphor/`:

| File | Contents |
|------|----------|
| `midi.json` | MIDI port, CC/trigger bindings |
| `osc.json` | OSC ports, address bindings |
| `web.json` | Web control surface port, enable state |
| `presets/*.json` | Named presets |

## Writing Effects

Effects are WGSL fragment shaders paired with a JSON `.pfx` definition. Shaders have access to time, resolution, 20 audio features (7 frequency bands, beat detection, spectral shape), up to 16 parameters, feedback from the previous frame, and a built-in library (noise, palette, SDF, tonemap).

Edit a shader while running — it hot-reloads on save with error recovery.

See [TECHNICAL.md](TECHNICAL.md#shader-authoring-guide) for the full authoring guide, uniform reference, multi-pass pipelines, and particle system integration.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for build instructions, the effect creation walkthrough, and PR guidelines.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option.
