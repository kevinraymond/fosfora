# Fosfora Quick Reference

```
+--[ Left Panel (315px) ]--+-----[ Canvas ]-----+--[ Right Panel (315px) ]-+
|  Audio                   |                     |  Parameters              |
|  Effects                 |   Live visual       |  Particles / Media / Cam |
|  Layers                  |   output            |  Obstacle                |
|  Presets                 |                     |  Splat Scene / Lattice   |
|  Scenes                  |                     |  Audio Reactivity        |
|  Settings                |                     |  Volumetric              |
|                          |                     |  Post-Processing         |
+--------------------------+---------------------+--------------------------+
|                          Status Bar                                       |
+---------------------------------------------------------------------------+
```

Press **D** to toggle all UI panels. Press **F** for fullscreen.

---

## Keyboard Shortcuts

| Key              | Action                      |
|------------------|-----------------------------|
| D                | Toggle UI overlay           |
| F                | Fullscreen                  |
| Esc              | Quit                        |
| Tab / Shift+Tab  | Next / previous widget      |
| F6               | Cycle panels                |
| Arrow keys       | Adjust slider (1% step)     |
| Shift+Arrow keys | Adjust slider (10% step)    |
| Home / End       | Slider min / max            |

---

## Left Panel

### Audio
Device selector dropdown, 7-band spectrum analyzer, dynamics display (RMS, kick, onset, flux, centroid, flatness, rolloff), 13 MFCC coefficients, 12 chroma pitch classes, BPM ring.

### Effects
Grid browser (3 columns) with type badges:
- **SH** (purple) — Shader effect
- **PS** (orange) — Particle system
- **FB** (teal) — Feedback effect

Copy, edit, or create new effects from the browser.

### Layers
Up to **8 layers** (0-7), composited bottom-to-top. Each layer has:
- Enable (eye), Lock (padlock), Pin (pin) toggles
- Opacity slider (0-1) and blend mode selector
- Drag handle for reorder
- Type label: **FX** (effect), **MD** (media), **WC** (webcam)

### Presets
Save/load named presets. Dirty indicator shows unsaved changes. Cycle via MIDI/OSC triggers (NextPreset / PrevPreset).

### Scenes
Cue timeline with per-cue preset, transition type, and duration. Advance modes: Manual, Timer (auto-advance after hold), BeatSync (advance every N beats). Loop toggle.

### Settings
Status dots show connection state (MIDI / OSC / WEB / NDI / AUD). Subsections: MIDI, OSC, Web,
Outputs (NDI and video recording), Global (theme, particle quality, band scale, auto-reconnect).

---

## Right Panel

Contextual — shows controls for the active layer type.

### Parameters (effect layers)
Sliders with **M** (MIDI) and **O** (OSC) learn badges. Color pickers, Point2D controls. Click a badge to enter learn mode (blinking orange), then move the target control to bind.

### Media (media layers)
File info, video playback controls (play/pause/seek).

### Webcam (webcam layers)
Device selector, mirror toggle, disconnect.

### Particles (effect layers)
Alive/max count, quality level, image source selector, morph target controls.

### Obstacle (effect layers)
Enable toggle, source tabs (image/model/depth/webcam), threshold, elasticity, collision mode. Model loads a rotating 3D `.glb`/`.gltf` mesh or `.ply`/`.splat` cloud. Depth model downloads on first use.

### Audio Reactivity (effect layers)
Map audio bands or dynamics to any parameter. Shows mapping count badge.

### Post-Processing
Four toggleable effects (per-effect overridable):

| Effect               | Default | Range |
|----------------------|---------|-------|
| Bloom threshold      | 0.8     | 0-1   |
| Bloom intensity      | 0.35    | 0-1   |
| Chromatic aberration  | 0.5     | 0-1   |
| Vignette strength    | 0.3     | 0-1   |
| Film grain intensity | 0.5     | 0-1   |

---

## Blend Modes

| # | Mode         | Description                              |
|---|--------------|------------------------------------------|
| 0 | Normal       | Replace background with foreground       |
| 1 | Add          | Brightens — glow, fire                   |
| 2 | Screen       | Lightens — like two projected slides     |
| 3 | Color Dodge  | Intense brighten — burns to white        |
| 4 | Multiply     | Darkens — stacked transparencies         |
| 5 | Overlay      | Contrast — darks darker, lights lighter  |
| 6 | Hard Light   | Strong contrast — Overlay from other side|
| 7 | Difference   | Inverts where bright — psychedelic       |
| 8 | Exclusion    | Softer Difference — grays similar colors |
| 9 | Subtract     | Darkens — removes foreground color       |
| 10| Displace     | Edges shove what's beneath — shockwaves  |
| 11| Refract      | Bright areas bend like glass, split color|
| 12| Lens         | Bright areas magnify what's beneath      |

Modes 10–12 are the **displacement family**: the layer is read as a warp field
rather than an image, so it draws none of its own color. A **Displace** slider
appears beside Opacity to set the strength (OSC `/phosphor/layer/{n}/displace`,
bindable as `layer.{n}.displace`). They need something below them to warp — on
the bottom layer they do nothing.

Note that `layer.{n}.blend` driven from the binding bus sweeps modes 0–9 only;
pick a displacement mode from the UI, OSC or a preset.

---

## Audio Bands

| # | Band       | Abbr | Range         | Character                  |
|---|------------|------|---------------|----------------------------|
| 0 | Sub Bass   | SB   | 20-60 Hz      | Kick drums, deep rumble    |
| 1 | Bass       | BS   | 60-250 Hz     | Basslines, low-end warmth  |
| 2 | Low Mid    | LM   | 250-500 Hz    | Body, fullness             |
| 3 | Mid        | MD   | 500 Hz-2 kHz  | Vocals, instruments        |
| 4 | Upper Mid  | UM   | 2-4 kHz       | Presence, clarity          |
| 5 | Presence   | PR   | 4-6 kHz       | Definition, edge           |
| 6 | Brilliance | BR   | 6-20 kHz      | Air, sparkle, cymbals      |

Full glossary of all 74 audio features, in plain English: [AUDIO-FEATURES.md](AUDIO-FEATURES.md).

---

## MIDI

**Learn workflow**: Click **M** badge on any parameter or trigger > badge blinks orange > move a knob/press a button on your controller > mapping saved. Cancel with the **...** button.

Mappings support: CC or Note messages, per-channel or omni, custom min/max range, invert.

**Trigger actions** (bindable via MIDI or OSC):

| Action              | Description                |
|---------------------|----------------------------|
| NextEffect          | Switch to next effect      |
| PrevEffect          | Switch to previous effect  |
| NextPreset          | Load next preset           |
| PrevPreset          | Load previous preset       |
| NextLayer           | Select next layer          |
| PrevLayer           | Select previous layer      |
| TogglePostProcess   | Toggle post-processing     |
| ToggleOverlay       | Toggle UI overlay          |
| SceneGoNext         | Advance to next cue        |
| SceneGoPrev         | Go to previous cue         |
| ToggleTimeline      | Toggle timeline playback   |

---

## OSC Addresses

Default ports: **RX 9000**, **TX 9001**

### Receive (control Fosfora)

| Address                             | Type  | Description                  |
|-------------------------------------|-------|------------------------------|
| `/phosphor/param/{name}`            | float | Set param on active layer    |
| `/phosphor/layer/{n}/param/{name}`  | float | Set param on layer n         |
| `/phosphor/layer/{n}/opacity`       | float | Layer opacity (0-1)          |
| `/phosphor/layer/{n}/blend`         | int   | Blend mode (0-12)            |
| `/phosphor/layer/{n}/displace`      | float | Warp strength, modes 10-12   |
| `/phosphor/layer/{n}/enabled`       | bool  | Layer enabled state          |
| `/phosphor/trigger/{action}`        | float | Fire trigger action          |
| `/phosphor/postprocess/enabled`     | bool  | Toggle post-processing       |
| `/phosphor/scene/goto_cue`          | int   | Jump to cue index            |
| `/phosphor/scene/load`              | int/s | Load scene by index or name  |
| `/phosphor/scene/loop_mode`         | bool  | Set loop mode                |
| `/phosphor/scene/advance_mode`      | int   | Manual(0)/Timer(1)/Beat(2)   |

### Transmit (audio data at 30 Hz)

`/phosphor/audio/bands/{sub_bass,bass,low_mid,mid,upper_mid,presence,brilliance}`

---

## Scene Transitions

| Type     | Description                              |
|----------|------------------------------------------|
| Cut      | Instant switch                           |
| Dissolve | GPU crossfade between layers             |
| Morph    | Per-frame parameter interpolation        |

---

## Particle Quality

| Level  | Multiplier |
|--------|------------|
| Low    | 0.25x      |
| Medium | 0.5x       |
| High   | 1.0x (default) |
| Ultra  | 2.0x       |
| Max    | 4.0x       |

---

## Config Files

All under `~/.config/phosphor/`:

| File/Dir       | Contents                              |
|----------------|---------------------------------------|
| settings.json  | Theme, audio device, particle quality |
| midi.json      | MIDI port, mappings, enabled state    |
| osc.json       | RX/TX ports, hosts, enabled state     |
| web.json       | Web server config                     |
| presets/       | User preset files (.json)             |
| scenes/        | Scene files (.json)                   |
| models/        | ML models (MiDaS depth)              |
