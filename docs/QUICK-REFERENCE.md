# Fosfora Quick Reference

Three workspaces, picked in the top bar: **Perform** to play, **Build** to make presets, **Setup** for
devices and settings.

```
Perform                                 Build
+-----------+----------------------+    +-----------+-----------------+-----------+
| Presets   |                      |    | Stack     | Inspector       | Output    |
| Layers    |       Output         |    | Presets   | (the selected   | Audio     |
|           |                      |    |           |  layer, or      |           |
|           |                      |    |           |  Master)        |           |
+-----------+----------------------+    +-----------+-----------------+-----------+
| Scenes (drawer)                  |    | Catalog / Scenes (drawer)                 |
+----------------------------------+    +-------------------------------------------+

Setup
+-----------+---------------------------------+-----------+
| Pages     | One page at a time              | Output    |
+-----------+---------------------------------+-----------+
```

Press **D** to hide or show the interface. Press **F** for fullscreen.

---

## Keyboard Shortcuts

| Key              | Action                                                      |
|------------------|-------------------------------------------------------------|
| D                | Hide / show the interface                                   |
| F                | Fullscreen                                                  |
| B                | Binding matrix                                              |
| C                | trama chain editor                                          |
| [ / ]            | Previous / next layer                                       |
| Space            | Next cue (when the scene has cues)                          |
| T                | Play / pause the scene (when it has cues)                   |
| Esc              | Step back: cancel a half-made binding, close the binding matrix, show a hidden interface, close the chain editor, close the second output window. With nothing left to close, quit |
| Tab / Shift+Tab  | Next / previous widget                                      |
| Arrow keys       | Adjust slider (1% step)                                     |
| Shift+Arrow keys | Adjust slider (10% step)                                    |
| Home / End       | Slider min / max                                            |

---

## Perform

### Presets
Save/load named presets. Dirty indicator shows unsaved changes. Cycle via MIDI/OSC triggers (NextPreset / PrevPreset).

**Switch** (top row of the Presets panel, on the left of Perform and in Build's left column): how switching presets changes the picture, Cut, Dissolve or Morph, plus a length in seconds. Applies to clicks, NextPreset / PrevPreset and the web remote; scene cues keep their own transitions. **Keep moving** (shown with Dissolve, on by default) keeps the outgoing preset animating through every Dissolve, cues included; turn it off if two presets at once is too much for your GPU.

### Layers
Up to **8 layers** (0-7), composited bottom-to-top. Each layer has:
- Enable (eye), Lock (padlock), Pin (pin) toggles
- Opacity slider (0-1) and blend mode selector
- Drag handle for reorder
- Type label: **FX** (effect), **MD** (media), **WC** (webcam)

### Scenes
The drawer along the bottom. Each cue is a card (its preset's picture, its name, how long it holds), and each transition is the joint between two cards. Advance by hand, on a timer (after the hold), or every N beats. Loop toggle. Build's drawer has the same Scenes tab.

---

## Build

### Stack (left)
Master on top, then each layer, each row with a picture of the stack so far. Lock and Pin per layer. **+ Effect layer**, **+ Media layer** and **+ Camera layer** add one. Select a row to show it in the inspector. The Presets panel sits under the stack, closed to start.

### Inspector (middle)
What the selected layer, or Master, controls.

**Blend** (every layer): blend mode, visibility and opacity.

**Parameters** (effect layers): sliders with **M** (MIDI) and **O** (OSC) learn badges. Color pickers, Point2D controls. Click a badge to enter learn mode (blinking orange), then move the target control to bind. Under them, how many bindings drive the layer and the way into the binding matrix.

**Media** (media layers): file info, video playback controls (play/pause/seek).

**Camera** (camera layers): device selector (cameras, and the network streams set up in Setup ▸ General ▸ Cameras), mirror toggle, disconnect.

**Particles** (particle effects): alive/max count, quality level, image source selector, morph target controls.

**Obstacle** (particle effects): enable toggle, source tabs (image/model/depth/webcam), threshold, elasticity, collision mode. Model loads a rotating 3D `.glb`/`.gltf` mesh or `.ply`/`.splat` cloud. Depth model downloads on first use.

**Lattice** and **Helix** (those effects): their own controls.

**Audio Reactivity** (effect layers): map audio bands or dynamics to any parameter. Shows mapping count badge.

**Post-Processing** (Master): four toggleable effects, saved with the preset:

| Effect               | Default | Range |
|----------------------|---------|-------|
| Bloom threshold      | 0.8     | 0-1   |
| Bloom intensity      | 0.35    | 0-1   |
| Chromatic aberration  | 0.5     | 0-1   |
| Vignette strength    | 0.3     | 0-1   |
| Film grain intensity | 0.5     | 0-1   |
| Film grain rate (Hz) | 24      | 0-60  |

**Volumetric** (Master): renders the particle layer as fog or nebula instead of dots.

### Output and Audio (right)
The output, and under it the audio: 7-band spectrum analyzer, dynamics display (RMS, kick, onset, flux, centroid, flatness, rolloff), 13 MFCC coefficients, 12 chroma pitch classes, BPM ring.

### Catalog (drawer)
Every effect as a picture, in families, searchable, with favorites. Click a picture to load it into the selected layer. Drag it onto the stack: onto a row to replace that layer's effect, onto the edge between rows to add a layer there. **+ New effect** starts one.

---

## Setup

A list of pages on the left, one page at a time.

| Page                | What is on it                                                        |
|---------------------|----------------------------------------------------------------------|
| Audio               | Input device, level meter and trim, what happens if it goes quiet, band scale |
| Control             | MIDI, OSC, the web remote, triggers                                  |
| Outputs and streams | What black becomes, second window, recording, NDI, virtual camera (Spout and Syphon where available) |
| Sync                | Ableton Link                                                         |
| Appearance          | Theme, interface scale                                               |
| Tutorials           | The guided tours                                                     |
| General             | Particle quality, flash limiter, cameras and network streams         |

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
appears beside Opacity to set the strength (OSC `/fosfora/layer/{n}/displace`,
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

Full glossary of all 85 audio features, in plain English: [AUDIO-FEATURES.md](AUDIO-FEATURES.md).

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
| `/fosfora/param/{name}`            | float | Set param on active layer    |
| `/fosfora/layer/{n}/param/{name}`  | float | Set param on layer n         |
| `/fosfora/layer/{n}/opacity`       | float | Layer opacity (0-1)          |
| `/fosfora/layer/{n}/blend`         | int   | Blend mode (0-12)            |
| `/fosfora/layer/{n}/displace`      | float | Warp strength, modes 10-12   |
| `/fosfora/layer/{n}/enabled`       | bool  | Layer enabled state          |
| `/fosfora/trigger/{action}`        | float | Fire trigger action          |
| `/fosfora/postprocess/enabled`     | bool  | Toggle post-processing       |
| `/fosfora/overlay/visible`         | bool  | Show / hide the UI (a set)   |
| `/fosfora/scene/goto_cue`          | int   | Jump to cue index            |
| `/fosfora/scene/load`              | int/s | Load scene by index or name  |
| `/fosfora/scene/loop_mode`         | bool  | Set loop mode                |
| `/fosfora/scene/advance_mode`      | int   | Manual(0)/Timer(1)/Beat(2)   |
| `/fosfora/preset/transition`       | int   | Cut(0)/Dissolve(1)/Morph(2)  |
| `/fosfora/preset/transition_secs`  | float | Switch length, 0.1-30 s      |

### Transmit (audio data at 30 Hz)

`/fosfora/audio/bands/{sub_bass,bass,low_mid,mid,upper_mid,presence,brilliance}`

Everything under `/fosfora/audio/` is a float.

**Beats, downbeats and drops.** Each fires as a 1-frame pulse *and* as a running total:

| Address                          | Description                                |
|----------------------------------|--------------------------------------------|
| `/fosfora/audio/beat`           | 1.0 on the frame a beat fires, else 0.0    |
| `/fosfora/audio/beat_count`     | Beats since startup                        |
| `/fosfora/audio/downbeat`       | 1.0 on the bar's "one"                     |
| `/fosfora/audio/downbeat_count` | Downbeats since startup                    |
| `/fosfora/audio/drop`           | 1.0 on a detected drop                     |
| `/fosfora/audio/drop_count`     | Drops since startup                        |

**Bind the `_count` addresses, not the pulses.** A pulse lasts one render frame (60+ fps)
while transmission is rate-limited to 30 Hz, so most pulses never reach the wire — trigger
on the count *changing* instead and you catch every event. Watch for a change rather than an
increase: the count restarts at 0 when you switch audio device.

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

All under `~/.config/fosfora/`:

| File/Dir       | Contents                              |
|----------------|---------------------------------------|
| settings.json  | Theme, audio device, particle quality |
| midi.json      | MIDI port, mappings, enabled state    |
| osc.json       | RX/TX ports, hosts, enabled state     |
| web.json       | Web server config                     |
| presets/       | User preset files (.json)             |
| scenes/        | Scene files (.json)                   |
| models/        | ML models (MiDaS depth)              |
