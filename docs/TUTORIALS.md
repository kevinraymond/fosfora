# Fosfora Tutorials

A comprehensive guide to using Fosfora — a real-time particle and shader engine for live VJ performance.

---

## Table of Contents

**New here? Start with [Your First 5 Minutes](#your-first-5-minutes).**

1. [Effects](#effects)
2. [Audio](#audio)
3. [Audio Reactivity](#audio-reactivity)
4. [Parameters](#parameters)
5. [Layers](#layers)
6. [Presets](#presets)
7. [Scenes](#scenes)
8. [Obstacles](#obstacles)
9. [Volumetric](#volumetric)
10. [Binding Matrix](#binding-matrix)
11. [Post-Processing](#post-processing)
12. [MIDI](#midi)
13. [OSC](#osc)
14. [Web Control Surface](#web-control-surface)
15. [Outputs](#outputs)
16. [Global](#global)

---

## Your First 5 Minutes

**Goal: go from a fresh launch to music-reactive visuals on your screen.** No setup, no accounts, and you can't break anything.

1. **Open Fosfora.** A visual starts running right away. After a couple of seconds the control UI fades in on its own — or press **D** any time to show/hide it.
2. **Play some music** — anything your computer can hear. The visuals start reacting immediately using your default input device. (Hearing nothing react? See [Audio → Choosing an Input](#audio).)
3. **Pick a look.** In the **Effects** panel on the left, click any effect to load it onto the active layer. Try Aurora, Storm, or Tesla to feel the range.
4. **Go big.** Press **F** for borderless fullscreen. Press **F** again (or **Esc**) to come back.
5. **Make it yours.** Drag the sliders in the right panel to reshape the effect — every one is audio-mappable later. When something looks great, save it as a preset.

That's the whole loop: **open → play music → pick an effect → fullscreen**. Everything below goes deeper on each piece.

---

## Effects

Effects are the core visual building blocks of Fosfora. Each effect is a WGSL shader (or set of shaders) that generates audio-reactive visuals in real time.

### Quick Start

1. The UI fades in automatically after a couple seconds (or press **D** to toggle it)
2. The **Effects** panel on the left lists all available effects
3. Click any effect name to load it onto the active layer
4. The visuals update immediately — no restart needed

### Built-In Effects

Fosfora ships with **38 built-in effects**, plus 2 hidden ones (the signature **Phosphor**
intro visual you see at startup, and a rasterizer stress test).

**Shaders** (11) — pure fragment shaders, no particles:
Aurora · Beam · Drift · Frost · Iris · Prism · Pulse · Shards · Storm · Strata · Tunnel

**Particle simulations** (19) — GPU compute, from a few thousand particles to two million:
Accretion · Array · Cascade · Chaos · Cleave · Cymatics · Flux · Genesis · Morph · Murmur ·
Mycelium · Polycephalum · Raster · Splat · Symbiosis · Tesla · Tide · Turing · Vessel

**Lattice** (8) — one 3D cellular-automata engine, eight rules, ray-marched as a volume:
445 · Brain · Builder · Chunky · Clouds · Pulse · Pyroclastic · Shells

The browser groups these into **Built-in** and **User** sections, with a search box and type
filters, and a ★ Favorites row at the top. Badges mark each effect **SH** (shader), **PS**
(particle) or **FB** (feedback).

See the **[Effect Gallery](GALLERY.md)** for a clip of every one at default settings.

### Creating Your Own Effects

Effects are defined by `.pfx` files — JSON manifests that reference WGSL shaders.

**Create from scratch:**
1. In the Effects panel, click the **+ New** button
2. Enter a name for your effect
3. Fosfora creates a `.pfx` file and starter `.wgsl` shader in the `assets/` folder next to
   the app (`assets/effects/` and `assets/shaders/`)
4. The shader editor opens automatically

**Copy a built-in effect:**
1. Select a built-in effect
2. Click **Copy Shader** in the Effects panel
3. Enter a name — Fosfora copies the shader files to your user effects directory
4. Edit the copy freely without affecting the original

### The .pfx Format

A `.pfx` file is JSON describing an effect:

```json
{
  "name": "My Effect",
  "author": "Your Name",
  "description": "What this effect does",
  "shader": "my_effect.wgsl",
  "inputs": [
    { "type": "Float", "name": "speed", "default": 0.5, "min": 0.0, "max": 1.0 },
    { "type": "Bool", "name": "sparkle", "default": true },
    { "type": "Color", "name": "tint", "default": [1.0, 0.5, 0.0, 1.0] },
    { "type": "Point2D", "name": "center", "default": [0.5, 0.5], "min": [0.0, 0.0], "max": [1.0, 1.0] }
  ],
  "postprocess": {
    "bloom_threshold": 0.6,
    "bloom_intensity": 0.5,
    "vignette": 0.3
  }
}
```

**Multi-pass effects** use a `passes` array instead of a single `shader`:

```json
{
  "name": "Multi-Pass Effect",
  "passes": [
    { "name": "bg", "shader": "background.wgsl", "feedback": true },
    { "name": "detail", "shader": "detail.wgsl", "inputs": ["bg"] }
  ]
}
```

**Particle effects** add a `particles` section:

```json
{
  "particles": {
    "max_count": 10000,
    "compute_shader": "my_sim.wgsl",
    "emitter": { "shape": "ring", "radius": 0.3 },
    "lifetime": 3.0,
    "initial_speed": 0.2,
    "gravity": [0.0, -0.3]
  }
}
```

### Shader Editor

Fosfora includes a built-in WGSL shader editor with live hot-reload:

1. Click the **Edit** button next to the active effect name (only available for user effects)
2. The editor opens as a full-screen overlay
3. Edit the WGSL code directly
4. Press **Ctrl+S** to save — the shader recompiles instantly
5. If there's an error, it appears in the status bar with a dismiss button
6. Press **Esc** to close the editor

The editor supports syntax highlighting and shows compilation errors inline.

### Shader Authoring

Fosfora auto-prepends a WGSL shader library to every effect. You can use these functions without any imports:

**Noise:**
- `phosphor_noise2(p)` / `phosphor_noise3(p)` — Perlin gradient noise (0–1)
- `phosphor_hash2(p)` / `phosphor_hash3(p)` — Fast hash without sin (0–1)

**Color:**
- `phosphor_palette(t, a, b, c, d)` — IQ cosine palette
- `phosphor_audio_palette(t, centroid, phase)` — Warm-to-cool audio palette
- `phosphor_hue_shift(color, amount)` — Hue rotation

**SDF (Signed Distance Functions):**
- `phosphor_sd_sphere(p, r)`, `phosphor_sd_box(p, b)`, `phosphor_sd_torus(p, t)`, `phosphor_sd_cylinder(p, h, r)`
- `phosphor_op_union`, `phosphor_op_subtract`, `phosphor_op_intersect` — Boolean operations
- `phosphor_smin(a, b, k)`, `phosphor_smax(a, b, k)` — Smooth min/max

**Tonemapping:**
- `phosphor_aces_tonemap(color)` — ACES filmic HDR→SDR
- `phosphor_linear_to_srgb(color)` — Linear to sRGB gamma

**Parameter access in shaders:**
- Use `param(0u)` through `param(15u)` to read your effect's parameters
- Parameters are packed as `array<vec4f, 4>` (16-byte aligned)

**Feedback:**
- Call `feedback(uv)` to sample the previous frame (when feedback is enabled in the .pfx)

**Tips:**
- Avoid `atan2` in palettes — it creates a visible seam at ±π. Use `sin(angle * N)` instead.
- Never multiply `time * audio_value` for position — it causes jitter. Use constant speed and apply audio to other properties.
- For feedback effects, use `mix()` not `max()` for blending, and clamp output to prevent blowout.

---

## Audio

Fosfora analyzes your system's audio input in real time and passes the results to every shader as uniform values.

### Quick Start

1. Make sure audio is playing on your system (music, microphone, etc.)
2. Fosfora automatically captures from the default audio device
3. The **Audio** panel in the UI shows a 7-band frequency spectrum
4. BPM and beat detection appear in the status bar

### Audio Device Selection

To change the audio input device:

1. Open the **Audio** panel in the UI (right sidebar)
2. Select a different device from the dropdown
3. The change takes effect immediately
4. Your selection is saved to `~/.config/phosphor/settings.json`

On Linux, Fosfora uses PulseAudio/PipeWire for monitor capture (loopback of system audio). Run `cargo run -- --audio-test` for standalone audio diagnostics.

### What Gets Detected

Fosfora extracts **74 audio features** from multi-resolution FFT analysis. The list below is a quick index — for what each feature *means* musically, what to hook it to, and the research behind it, see [AUDIO-FEATURES.md](AUDIO-FEATURES.md).

The core set below is joined by eight detector groups — loudness, key, downbeat, stereo, structure, harmonic/percussive split, pitch, and spectral contrast — that fill the shader ABI's reserved tail (see [Detector features](#reserved-features)):

**7 Frequency Bands** (normalized 0–1):
| Band | Range | Typical Content |
|------|-------|----------------|
| sub_bass | 20–60 Hz | Sub-bass rumble, kick drum fundamental |
| bass | 60–250 Hz | Bass guitar, kick body |
| low_mid | 250–500 Hz | Low vocals, warmth |
| mid | 500–2000 Hz | Vocals, guitars, snare |
| upper_mid | 2000–4000 Hz | Vocal clarity, guitar bite |
| presence | 4000–6000 Hz | Hi-hats, cymbal shimmer |
| brilliance | 6000+ Hz | Air, sparkle |

**Aggregates:**
- **rms** — Overall energy level
- **kick** — Dedicated 30–120 Hz spectral flux for beat-driving

**Spectral Shape:**
- **centroid** — Brightness (0=dark/bassy, 1=bright/trebly)
- **flux** — Rate of spectral change
- **flatness** — Tonal vs. noisy (0=tonal peaks, 1=flat noise)
- **rolloff** — Frequency below which 85% of energy lies
- **bandwidth** — Spectral spread
- **zcr** — Zero-crossing rate

**Beat Detection (3-stage pipeline):**
- **onset** — Transient attacks (0–1)
- **beat** — Beat trigger (0 or 1 on each beat)
- **beat_phase** — Sawtooth wave 0→1 at detected tempo
- **bpm** — Detected BPM (normalized, multiply by 300 for actual BPM)
- **beat_strength** — Detection confidence (0–1)

<a name="reserved-features"></a>
**Detector features:** Eight detector groups fill the reserved tail of the shader ABI. The first five are live as of the A13 stereo detector (they read `0.0` in earlier builds while each detector was still in progress):
- **loudness_m / loudness_s / loudness_trend** — perceptual loudness envelope
- **key_class / key_is_minor / key_confidence** — musical key estimate
- **downbeat / bar_phase / beat_in_bar** — bar-level clock
- **pan / stereo_width / stereo_corr** — stereo field
- **section_novelty / buildup / drop** — song-structure cues

The remaining three groups fill the shader ABI v3 tail. All three are live as of the A14, A15 and A16 detectors:
- **percussive_energy / harmonic_energy / harmonic_ratio** — harmonic/percussive split (A14): drum-vs-tone energy and their 0–1 balance, for routing transients to strobes and sustained tones to color washes
- **pitch / pitch_confidence** — monophonic pitch estimate (A15): YIN fundamental frequency over five octaves, plus a voiced/unvoiced confidence to gate it
- **contrast_0 … contrast_5 / contrast_mean / timbre_flux** — spectral contrast + timbre dynamics (A16): per-octave peak-vs-valley tonality, plus a volume-independent measure of timbre change

Alongside these, three live audio *textures* let effects read the signal directly, for oscilloscopes, spectrum bars and waterfalls — sample them with the built-in helpers:
- **`waveform(x)`** → `vec2f` (min, max) of the raw PCM at horizontal position `x` — a min/max-decimated, zero-crossing-triggered scope trace.
- **`spectrum(x)`** → `f32` log-frequency magnitude (0–1) at `x` — spectrum-bar heights.
- **`spectrogram(uv)`** → `f32` mel energy (0–1); `uv.x` is time (0 = oldest, 1 = newest), `uv.y` is frequency (mel) — a scrolling waterfall.

### Adaptive Normalization

Features are auto-leveled so you never touch a gain knob. Energy-like features (the seven bands, `rms`, `flux`) use gated percentile ranging: the 5th and 95th percentile of the last few seconds are stretched to fill 0–1, with a soft knee above the 95th so an unusually big hit still reads as bigger instead of clipping. This means:
- Quiet music still produces full 0–1 range features
- No fixed gain knobs to adjust manually
- The system adapts over a few seconds to changing input levels
- One loud spike can't flatten everything after it

Not every feature is auto-leveled — spectral shape features are already on a meaningful scale, MFCCs are centered on their own average, and detector outputs like key, pitch and the beat group are passed through untouched. See [How the Numbers Are Tamed](AUDIO-FEATURES.md#how-the-numbers-are-tamed) for the full picture.

---

## Audio Reactivity

This is where the magic happens — audio features drive every aspect of the visuals.

### How It Works

Every frame, Fosfora packs all **74 audio features** into the shader uniform buffer. Your shaders read these values and use them to modulate anything: color, position, size, speed, distortion, brightness.

### Available Uniforms in Shaders

All effect shaders have access to these uniforms:

```wgsl
// Time
time          // Seconds since app start
delta_time    // Frame delta
resolution    // vec2f: window width, height

// Audio bands (0.0–1.0)
sub_bass, bass, low_mid, mid, upper_mid, presence, brilliance

// Audio aggregates
rms           // Overall energy
kick          // Dedicated kick detection

// Spectral shape
centroid      // Brightness (low=dark, high=bright)
flux          // Rate of change
flatness      // Tonal vs. noisy
rolloff       // High-frequency cutoff
bandwidth     // Spectral width
zcr           // Zero-crossing rate

// Beat detection
onset         // Transient attacks
beat          // Beat trigger (0 or 1)
beat_phase    // 0→1 sawtooth at detected tempo
bpm           // Detected BPM / 300
beat_strength // Detection confidence

// Pitch / timbre (accessed via helper functions)
dominant_chroma       // Strongest pitch class, normalized 0–1
mfcc(i)               // 13 MFCC timbral coefficients, i = 0..12
chroma_val(i)         // 12 chroma pitch-class energies, i = 0..11 (C, C#, D … B)

// Detector features — the reserved tail (first five groups live)
loudness_m, loudness_s, loudness_trend        // perceptual loudness
key_class, key_is_minor, key_confidence       // musical key estimate
downbeat, bar_phase, beat_in_bar              // bar-level clock
pan, stereo_width, stereo_corr                // stereo field
section_novelty, buildup, drop                // song-structure cues

// Shader ABI v3 tail
percussive_energy, harmonic_energy, harmonic_ratio  // harmonic/percussive split (A14)
pitch, pitch_confidence                             // monophonic pitch (A15)
contrast_0, contrast_1, contrast_2, contrast_3,     // spectral contrast (A16)
contrast_4, contrast_5, contrast_mean, timbre_flux

// Audio textures — read the signal directly
waveform(x)           // vec2f min/max of the PCM waveform at x = 0..1
spectrum(x)           // magnitude at log-frequency x = 0..1
spectrogram(uv)       // scrolling mel-band history
```

The 20 scalar fields above plus `dominant_chroma`, the 13 MFCCs, the 12 chroma values, and the 28 detector scalars (listed above) are the full set of **74 audio features** — all available in every effect shader. MFCC and chroma are packed as `array<vec4f>` internally, so read them through the `mfcc(i)` / `chroma_val(i)` helpers rather than by field name.

Not sure what one of these means, or which to reach for? Every field is explained in plain English in [AUDIO-FEATURES.md](AUDIO-FEATURES.md), including a [pick-by-what-you-want table](AUDIO-FEATURES.md#pick-a-feature-by-what-you-want).

### Common Patterns

**Pulse on beat:**
```wgsl
let flash = beat * 0.5; // bright flash on each beat
```

**Smooth sway with bass:**
```wgsl
let offset = sin(time * 2.0) * bass * 0.3;
```

**Color from spectral centroid:**
```wgsl
let color = phosphor_audio_palette(time * 0.1, centroid, beat_phase);
```

**Size from RMS energy:**
```wgsl
let radius = 0.1 + rms * 0.5;
```

**Beat-synced animation:**
```wgsl
let phase = beat_phase; // 0→1 sawtooth at BPM
let bounce = 1.0 - phase * phase; // decaying bounce per beat
```

### Post-Processing Reactivity

Post-processing effects are also audio-reactive (automatically):
- **Bloom** intensity increases with RMS (louder = more glow)
- **Chromatic aberration** spikes on onset (transients cause RGB split)
- **Film grain** increases with flatness (noisy audio = visual noise)

---

## Parameters

Each effect defines up to 16 parameters that you can tweak in real time.

### Quick Start

1. Load an effect
2. The **Parameters** panel on the right shows all available sliders and controls
3. Drag sliders, toggle checkboxes, pick colors — changes are instant
4. Parameters are saved in presets

### Parameter Types

| Type | UI Control | Shader Access |
|------|-----------|---------------|
| **Float** | Slider with +/- buttons | `param(N)` returns f32 |
| **Bool** | Checkbox | `param(N)` returns 0.0 or 1.0 |
| **Color** | Color picker (RGBA) | `param(N)` through `param(N+3)` for R, G, B, A |
| **Point2D** | XY picker | `param(N)` and `param(N+1)` for X, Y |

### MIDI/OSC Control

Parameters can be mapped to external controllers:
- Click the **M** button next to any parameter to enter MIDI learn mode
- Click the **O** button for OSC learn mode
- Move a knob or send an OSC message to bind it
- A badge appears showing the binding (e.g., "CC 14")
- See the [MIDI](#midi) and [OSC](#osc) sections for details

---

## Layers

Fosfora supports up to 8 layers, each running its own effect (or media), composited together with blend modes.

### Quick Start

1. You start with 1 layer
2. Click **+ Layer** in the Layer panel (left sidebar) to add an effect layer
3. Click **+ Media** to add an image/GIF/video layer
4. Each layer can run a different effect independently
5. Select a layer by clicking it in the Layer panel
6. The Parameters panel shows the selected layer's controls

### Layer Controls

Each layer card shows:
- **Drag handle** (≡) — Reorder layers by dragging (top layer renders last/on top)
- **Enable checkbox** — Toggle layer visibility
- **Lock icon** (🔒) — Prevent all changes (params, effects, preset loading)
- **Pin icon** (📌) — Prevent drag reordering
- **Layer name** — Click to select, double-click to rename
- **Delete button** (×) — Remove the layer

Below the layer list:
- **Blend mode** dropdown — How this layer combines with layers below
- **Opacity** slider — Layer transparency (0–1)

### Blend Modes

| Mode | Description |
|------|-------------|
| **Normal** | Replaces background with foreground |
| **Add** | Brightens — adds colors together (great for glow, fire) |
| **Screen** | Lightens — like projecting two slides together |
| **Color Dodge** | Intense brighten — burns through to white |
| **Multiply** | Darkens — like stacking two transparencies |
| **Overlay** | Contrast boost — darks darker, lights lighter |
| **Hard Light** | Strong contrast — like Overlay from the other side |
| **Difference** | Inverts where bright — psychedelic color shifts |
| **Exclusion** | Softer Difference — grays out similar colors |
| **Subtract** | Darkens — removes foreground color from background |

### Media Layers

You can load images, GIFs, and videos as layers:

**Supported formats:** PNG, JPEG, GIF, WebP, BMP

**Video** (requires `--features video` and ffmpeg on PATH): MP4, MOV, AVI, MKV, WebM, M4V, FLV

Media layers support:
- Letterbox scaling (maintains aspect ratio, transparent outside)
- All 10 blend modes + opacity
- Animated GIF/WebP playback with transport controls:
  - Play/pause, loop toggle
  - Speed control (0.1x–4.0x)
  - Direction: forward, reverse, ping-pong
- Video playback with seek slider and time display (max 60s pre-decoded)

**Tip:** Loading an effect onto a media layer converts it back to an effect layer.

### Keyboard Shortcuts

- **[** — Select previous layer
- **]** — Select next layer

---

## Presets

Presets save and restore your entire visual setup — all layers, effects, parameters, blend modes, and post-processing settings.

### Quick Start

1. Set up your layers and effects how you like them
2. In the **Presets** panel (left sidebar), type a name
3. Click **Save**
4. To recall, click any preset in the list
5. Saving with an existing name overwrites it (standard VJ workflow)

### What Gets Saved

A preset captures:
- All layers: effect, parameters, blend mode, opacity, enabled, locked, pinned
- Active layer selection
- Post-processing settings (bloom, vignette, chromatic aberration, film grain)
- Media layer paths (images, GIFs, videos)

### What Doesn't Get Saved

- Audio device selection (global setting)
- MIDI/OSC/Web configuration (global settings)
- Window size and position

### Preset Management

- **Save** — Creates or overwrites a preset
- **Delete** — Click the × next to a preset name
- **Copy** — Right-click a preset to duplicate it
- **MIDI cycling** — Map NextPreset/PrevPreset triggers to MIDI buttons
- **Dirty indicator** — An asterisk (*) appears when the current preset has unsaved changes

### Locked Layers

Locked layers (🔒) are skipped during preset loading. This lets you "freeze" a layer while cycling through presets — useful for keeping a background layer constant while swapping foreground effects.

### Storage

Presets are stored as JSON files in `~/.config/phosphor/presets/`. You can share presets by copying these files.

---

## Scenes

Scenes let you sequence presets into a cue list with timed or beat-synced transitions — turning a collection of presets into an automated show.

### Quick Start

1. Open the **Scenes** panel in the left sidebar
2. Click **+ New Scene** and enter a name
3. Add cues by clicking **+ Cue** — each cue references a saved preset
4. Set transition type and duration for each cue
5. Press **Space** or click the play button to start the timeline
6. Press **T** to toggle the timeline on/off

### Cue List

Each cue in a scene references a preset and defines how to transition to it:

- **Preset** — Which saved preset to load (selected from your preset list)
- **Transition** — How to get there: Cut, Dissolve, or Morph
- **Transition duration** — How long the transition takes (in seconds, ignored for Cut)
- **Hold time** — How long to stay on this cue before advancing (used in Timer mode)
- **Label** — Optional display name override

Cues can be reordered, edited, and deleted from the scene panel. Changes are auto-saved.

### Transitions

| Type | Description |
|------|-------------|
| **Cut** | Instant switch — no transition, immediately loads the next preset |
| **Dissolve** | GPU crossfade between outgoing and incoming visuals over the transition duration |
| **Morph** | Interpolates all parameters and layer opacities smoothly over the transition duration |

**Dissolve** creates a true visual crossfade — both the old and new states render simultaneously and blend together. **Morph** keeps the current effects running and smoothly slides their parameters toward the target preset's values, which works best when consecutive cues use the same effects with different parameter settings.

### Advance Modes

| Mode | Behavior |
|------|----------|
| **Manual** | Cues advance only when you press Space, a MIDI trigger, or an OSC message |
| **Timer** | Automatically advances after each cue's hold time elapses |
| **Beat Sync** | Advances every N beats, using MIDI clock when available or the audio beat detector as fallback |

Set the advance mode in the scene panel. In Beat Sync mode, you can configure the number of beats per cue.

### MIDI Clock Sync

When a MIDI controller or DAW sends MIDI clock, Fosfora follows the external transport automatically:

- **Start/Continue** (MIDI 0xFA/0xFB) — starts the timeline if it has cues but is idle
- **Stop** (MIDI 0xFC) — stops the timeline if it is active
- **Timing ticks** (MIDI 0xF8, 24 per quarter note) — used for BPM and beat-phase tracking

In **Beat Sync** advance mode, MIDI clock beats take priority over the internal audio beat detector. If MIDI clock is not playing, Beat Sync falls back to audio-detected beats.

### OSC Scene Control

Scenes can be controlled via OSC (default RX port 9000):

**Scene-specific addresses:**

| Address | Arg | Description |
|---------|-----|-------------|
| `/phosphor/scene/goto_cue` | int | Jump directly to a cue by index (0-based) |
| `/phosphor/scene/load` | string | Load a scene by name |
| `/phosphor/scene/load` | int | Load a scene by index (0-based) |
| `/phosphor/scene/loop_mode` | float | Set loop mode (> 0.5 = on) |
| `/phosphor/scene/advance_mode` | int | 0 = Manual, 1 = Timer, 2 = Beat Sync |

**Trigger actions** (via `/phosphor/trigger/{action}`):
- `scene_go_next` — advance to the next cue
- `scene_go_prev` — go to the previous cue
- `toggle_timeline` — start/stop the timeline

**Outbound timeline state** (TX, sent at the configured rate when TX is enabled):

| Address | Type | Description |
|---------|------|-------------|
| `/phosphor/state/timeline/active` | int (0/1) | Whether the timeline is playing |
| `/phosphor/state/timeline/cue_index` | int | Current cue index (0-based) |
| `/phosphor/state/timeline/cue_count` | int | Total number of cues |
| `/phosphor/state/timeline/transition_progress` | float (0–1) | Transition progress (0.0 when idle) |

### Timeline Bar

When the timeline is active, a visual timeline bar appears showing all cues as equal-width blocks:

- The **current cue** is highlighted
- A **playhead** line shows the current position
- During transitions, a **progress overlay** fills the target cue block
- **Dissolve** transitions show in the accent color; **Morph** shows in green
- A label displays the transition type and progress percentage (e.g., "Dissolve 47%")
- **Click any cue block** to jump directly to that cue

### Storage

Scenes are stored as JSON files in `~/.config/phosphor/scenes/`. You can share scenes by copying these files. Scene names follow the same sanitization rules as presets (no `/\\.`, max 64 chars).

---

## Obstacles

Particles can collide with a shape you supply — a photo, a video, or a live webcam silhouette. This is what makes water part around a body, or a crowd of particles pile up on someone's shoulders.

### Quick Start

1. Select a **particle** layer (obstacles do nothing on a pure shader effect — see the list below)
2. Open the **Obstacle** section of the Parameters panel
3. Click **Image…** and pick a picture with a clear bright subject on a dark background
4. Turn **Enabled** on

The shape is read from the image's alpha channel. If the image has no alpha — most photos don't — brightness is used instead, so a light subject on a dark background works out of the box.

### Sources

| Source | Needs | Notes |
|--------|-------|-------|
| **Image** | — | PNG, JPEG, WebP. The shipped `assets/images/` pictures all work |
| **Video** | `video` feature, ffmpeg | The shape animates with the footage |
| **Webcam** | `webcam` feature | Live silhouette, thresholded by brightness |
| **Depth** | `depth` feature | Monocular depth estimate from the webcam — near surfaces block, far ones don't |

### Controls

- **Threshold** (0–1) — how bright a pixel must be to count as solid. Raise it if background texture is catching particles; lower it if the shape has holes.
- **Elasticity** (0–1) — how much speed survives a bounce. 0 is a dead stop, 1 is a perfect rebound.
- **Fit** — how the image is mapped onto a 16:9 canvas. **Fill** (default) crops to cover; **Fit** letterboxes the whole image; **Stretch** distorts it.
- **Mode** — what happens on contact:

| Mode | Behaviour |
|------|-----------|
| **Bounce** | Reflects off the surface |
| **Stick** | Stops dead where it lands, building up a crust |
| **Flow** | Slides along the surface instead of stopping — best for water |
| **Contain** | Traps particles *inside* the shape instead of outside |

### Which effects support it

Accretion, Array, Cascade, Chaos, Cleave, Cymatics, Flux, Genesis, Morph, Murmur, Mycelium, Raster, Splat, Symbiosis, Tesla, Tide, Turing and Vessel. **Tide** and **Vessel** were built around it — Tide parts and pools, Vessel fills the shape and bursts on the drop. Splat is the odd one out: it carves the obstacle out of the splat cloud rather than bouncing anything off it.

### Automation

Per-layer OSC:

| Address | Type | Description |
|---------|------|-------------|
| `/phosphor/layer/{n}/obstacle/enabled` | float | > 0.5 turns it on |
| `/phosphor/layer/{n}/obstacle/mode` | float | 0 = Bounce, 1 = Stick, 2 = Flow, 3 = Contain |
| `/phosphor/layer/{n}/obstacle/threshold` | float (0–1) | |
| `/phosphor/layer/{n}/obstacle/elasticity` | float (0–1) | |

In the [binding matrix](#binding-matrix) the targets are `particle.obstacle_enabled`, `_mode`, `_threshold` and `_elasticity` — note these apply to **all** layers at once, and take a normalized 0–1 value. Breathing the threshold on `audio.rms` makes the silhouette seem to inhale.

The image path is saved in the preset, so a whole obstacle setup recalls with everything else.

---

## Volumetric

Any particle layer can be rendered as ray-marched fog instead of discrete points: the same simulation, made of smoke. Turn it on in the **Volumetric** section of the Parameters panel — it applies to the **selected layer**.

### What it does

Particles are deposited into a 3D voxel grid, which is then ray-marched with a camera you control. Depth is synthesized per particle, so a flat 2D simulation gains a stable thickness rather than staying a sheet.

**This shapes what looks good.** An effect whose particles fill the frame evenly becomes a featureless glowing ball, because a uniformly full volume has no internal structure to see. Effects with a *sparse, structured* footprint — Chaos's strange attractors, Mycelium's tendrils, Polycephalum's networks — keep their shape as fog. If you get a blob, lower **Density gain** until it turns translucent, and try a cube envelope instead of a sphere.

### Controls

- **March steps** (16–160) — samples per ray. More is smoother and slower.
- **Absorption** — how fast the fog swallows light. Higher is denser and more contrasty.
- **Density gain** (0.02–1) — saturation. This is the first knob to reach for when everything reads as a solid mass.
- **Volume depth** — how far the synthesized depth spreads. Low values give a glowing sheet, high values a thick cloud.
- **Detail scale / strength** — noise breaking up the fog.
- **Camera** — yaw, pitch, distance, orbit speed and field of view.
- **Envelope** — cube (edge fade) or sphere. The sphere fades to a ball at the edges, which is flattering on structured content and merciless on full ones.
- **Palette hue**, **Emission gain** — colour and glow.

### Automation

Every control has an OSC address, so a camera move is one message:

```bash
oscsend localhost 9000 /phosphor/volumetric/enabled f 1.0
oscsend localhost 9000 /phosphor/volumetric/cam_yaw f 2.4
oscsend localhost 9000 /phosphor/volumetric/cam_orbit_speed f 0.3
oscsend localhost 9000 /phosphor/volumetric/density_gain f 0.09
```

Unlike `/phosphor/param/*`, these take **raw** values in the control's own range, not 0–1. The full set: `march_steps`, `absorption`, `detail_scale`, `detail_strength`, `density_threshold`, `volume_depth`, `density_scale`, `density_gain`, `cam_yaw`, `cam_pitch`, `cam_distance`, `cam_orbit_speed`, `fov`, `palette_hue`, `emission_gain`, `env_shape`, `jitter`, `age_influence`.

Volumetric state is saved with the preset.

---

## Binding Matrix

Press **B** for a full-screen patch bay: drag a line from any source to any target and it moves with the music, with your hands, or with a knob.

### Quick Start

1. Press **B**
2. Pick a source on the left — start with **Audio · Bands → Bass**
3. Pick a target on the right — any parameter of any layer
4. Play something. The cable animates when signal is flowing.

The **Templates** dropdown wires up a whole set at once against the layer you have selected. **Audio Reactive** maps kick, level, brightness and beat phase onto whichever of the current effect's parameters best match; **Spectral Bands** puts the seven frequency bands on the first seven parameters.

### Sources

- **Audio** — all 74 detected features, grouped: Bands, Loudness, Features, Timbre, Beat, Structure, Harmonic, Stereo, Pitch, Key, Chroma, plus per-bin MFCC, Mel and ΔMFCC. See [Audio Features](AUDIO-FEATURES.md) for what each one means. The long groups start collapsed.
- **MIDI** — any CC on any channel. The **Learn** button captures the next knob you touch.
- **OSC** — any address the app receives, whether or not it is one of Fosfora's own.
- **Bridges** — hand, face and body tracking over WebSocket. See [bridges/README.md](../bridges/README.md).

### Targets

Effect parameters (per layer), layer opacity / blend / enabled, master opacity, the post-processing controls, particle settings including the obstacle controls, shader uniforms, and scene transport (next / previous / stop cue).

### Transforms

Each binding runs an ordered chain, so a raw feature can be shaped into something musical:

| Transform | Use |
|-----------|-----|
| **Remap** | Rescale an input range onto an output range |
| **Smooth** | Exponential smoothing — the difference between a twitch and a swell |
| **Curve** | `linear`, `ease_in`, `ease_out`, `ease_in_out`, `log`, `exp` |
| **Gate** | Everything above a threshold becomes 1, below becomes 0 |
| **Deadzone** | Ignore the middle, rescale the edges |
| **Quantize** | Snap to N steps |
| **Invert**, **Scale**, **Offset**, **Clamp** | The arithmetic |

Order matters. A one-frame trigger like `audio.drop` is invisible bound raw — smooth it heavily, scale it up, then clamp, and it becomes a flare with a tail.

### Scope and storage

- **Effect** scope saves beside the preset, in `~/.config/phosphor/presets/{name}.bindings.json`, and loads and unloads with it.
- **Global** scope lives in `~/.config/phosphor/global-bindings.json` and is always active.

Both are plain JSON you can edit or share.

---

## Post-Processing

Post-processing applies screen-space effects after all layers are composited.

### Quick Start

1. Post-processing is enabled by default
2. Toggle it with the checkbox in the **Post-Processing** section of the Parameters panel
3. Adjust individual effects with their sliders

### Effects

**Bloom** — Extracts bright areas and adds a soft glow
- *Threshold* (0.0–1.5): Brightness cutoff. Lower = more glow
- *Intensity* (0.0–2.0): Glow strength

**Vignette** — Darkens the screen edges for a cinematic look
- *Amount* (0.0–1.0): Edge darkness

**Chromatic Aberration** — Shifts RGB channels apart for a lens distortion look
- *Intensity* (0.0–1.0): Channel separation amount

**Film Grain** — Adds animated noise texture for a filmic feel
- *Intensity* (0.0–1.0): Noise strength

### Audio-Reactive Post-Processing

Post-processing is automatically audio-reactive:
- **RMS** (overall loudness) modulates bloom threshold and intensity
- **Onset** (transient attacks) drives chromatic aberration spikes
- **Flatness** (spectral shape) drives film grain intensity

### Per-Effect Overrides

Each `.pfx` effect can specify its own post-processing defaults in its `postprocess` section. These are applied when the effect loads, so different effects can have different bloom/vignette settings tuned to look their best.

### Performance

Bloom operates at quarter resolution for performance. Disabling post-processing entirely (uncheck the master toggle) removes all overhead.

---

## MIDI

Connect hardware MIDI controllers for hands-on control of parameters and triggers.

### Quick Start

1. Connect a MIDI controller to your computer
2. Open the **MIDI** panel in the left sidebar
3. Select your controller from the port dropdown
4. The activity dot flashes green when MIDI messages are received

### MIDI Learn

To map a MIDI control to a parameter:

1. Click the **M** button next to any parameter slider or trigger
2. The button highlights, showing "learning..."
3. Move a knob or press a button on your MIDI controller
4. The binding is created — a badge shows the CC number (e.g., "CC 14")
5. Your MIDI mappings are saved to `~/.config/phosphor/midi.json`

To remove a binding, click the badge.

### Parameter Mapping

- **Float parameters**: CC value 0–127 is scaled to the parameter's min–max range
- **Bool parameters**: CC ≥ 64 = true, CC < 64 = false
- **Channel**: Channel 0 means "omni" — responds to all MIDI channels

### Trigger Actions

Map MIDI buttons to these actions:

| Trigger | Description |
|---------|-------------|
| **Next Effect** | Load the next effect |
| **Prev Effect** | Load the previous effect |
| **Next Preset** | Cycle to the next preset |
| **Prev Preset** | Cycle to the previous preset |
| **Next Layer** | Select the next layer |
| **Prev Layer** | Select the previous layer |
| **Toggle Post-Process** | Enable/disable post-processing |
| **Toggle Overlay** | Show/hide the UI |
| **Scene Next** | Advance to the next scene cue |
| **Scene Prev** | Go to the previous scene cue |
| **Toggle Timeline** | Start/stop the scene timeline |

Triggers use rising-edge detection (CC crosses from < 64 to ≥ 64) to fire once per press.

### Hot-Plug

Fosfora polls for MIDI devices every 2 seconds:
- Disconnected controllers are detected automatically
- Reconnected controllers re-bind automatically
- Your saved port preference is restored when the device reappears

---

## OSC

Open Sound Control (OSC) enables communication with other software — DAWs, lighting controllers, TouchDesigner, and more.

### Quick Start

1. Open the **OSC** panel in the left sidebar
2. OSC receive (RX) is on by default on port **9000**
3. OSC transmit (TX) is off by default — enable it and set port **9001** if needed
4. Send OSC messages to control Fosfora from external software

### Receiving OSC (RX)

Default: **port 9000** on all interfaces (0.0.0.0)

**Address patterns:**

| Address | Type | Description |
|---------|------|-------------|
| `/phosphor/param/{name}` | float | Set parameter on active layer |
| `/phosphor/layer/{n}/param/{name}` | float | Set parameter on layer N |
| `/phosphor/layer/{n}/opacity` | float | Layer opacity (0–1) |
| `/phosphor/layer/{n}/blend` | int | Blend mode (0–9) |
| `/phosphor/layer/{n}/enabled` | int | Layer on/off (0 or 1) |
| `/phosphor/postprocess/enabled` | int | Post-processing toggle |
| `/phosphor/trigger/{action}` | float | Fire a trigger action |

Trigger action names: `next_effect`, `prev_effect`, `toggle_postprocess`, `toggle_overlay`, `next_preset`, `prev_preset`, `next_layer`, `prev_layer`, `scene_go_next`, `scene_go_prev`, `toggle_timeline`

**Scene control addresses:**

| Address | Arg | Description |
|---------|-----|-------------|
| `/phosphor/scene/goto_cue` | int | Jump to cue by index (0-based) |
| `/phosphor/scene/load` | string/int | Load scene by name or index |
| `/phosphor/scene/loop_mode` | float | Set loop mode (> 0.5 = on) |
| `/phosphor/scene/advance_mode` | int | 0 = Manual, 1 = Timer, 2 = Beat Sync |

### OSC Learn

Similar to MIDI learn:
1. Click the **O** button next to any parameter or trigger
2. Send any OSC message from your controller
3. Fosfora binds that address to the parameter
4. Mappings are saved to `~/.config/phosphor/osc.json`

### Sending OSC (TX)

When TX is enabled, Fosfora broadcasts at 30 Hz (configurable):
- Audio features: all 7 bands, RMS, kick, onset, beat, etc.
- State: active layer index, current effect name
- Timeline state (when a scene is active):

| Address | Type | Description |
|---------|------|-------------|
| `/phosphor/state/timeline/active` | int (0/1) | Whether the timeline is playing |
| `/phosphor/state/timeline/cue_index` | int | Current cue index (0-based) |
| `/phosphor/state/timeline/cue_count` | int | Total number of cues |
| `/phosphor/state/timeline/transition_progress` | float (0–1) | Transition progress (0.0 when idle) |

This is useful for driving other software (lighting, video) from Fosfora's audio analysis and timeline state.

### Testing with Command Line

Install `liblo-tools` (Linux: `apt install liblo-tools`) for quick testing:

```bash
# Set a parameter
oscsend localhost 9000 /phosphor/param/warp_intensity f 0.8

# Fire a trigger
oscsend localhost 9000 /phosphor/trigger/next_effect f 1.0

# Set layer opacity
oscsend localhost 9000 /phosphor/layer/0/opacity f 0.5

# Monitor Fosfora's outbound OSC
oscdump 9001
```

---

## Web Control Surface

Fosfora includes a built-in web-based touch control surface — perfect for controlling visuals from a phone or tablet.

### Quick Start

1. Open the **Web** panel in the left sidebar
2. Enable the WebSocket server (default port: **9002**)
3. The panel shows two URLs:
   - **localhost** — for the same machine
   - **LAN IP** — for other devices on your network
4. Open the URL in any web browser on your phone/tablet
5. The touch UI connects automatically

### Features

The web control surface provides:
- **Audio spectrum** — Real-time 7-band frequency display
- **Effect grid** — Tap any effect to load it
- **Parameter sliders** — All active effect parameters
- **Layer cards** — Select layers, adjust opacity and blend mode
- **Preset list** — Tap to load presets
- **Trigger buttons** — Next/prev effect, preset, layer, etc.

### Multi-Client

Multiple devices can connect simultaneously. All clients receive real-time state updates — great for collaborative VJ sessions or letting the audience interact.

### Technical Details

- Same-port HTTP and WebSocket on port 9002 (configurable)
- Mobile-first touch UI with 48px min touch targets
- Auto-reconnect with exponential backoff (1/2/4/8s)
- Audio features broadcast at 10 Hz to all clients
- Configuration saved to `~/.config/phosphor/web.json`

---

## Outputs

### NDI Output

NDI (Network Device Interface) lets you send Fosfora's output to other software over the network — OBS, vMix, Resolume, TouchDesigner, and any NDI-compatible receiver.

**Requirements:**
- **Official release downloads** (macOS/Windows/Linux): NDI is already built in — you only need the NDI runtime.
- **Building from source:** add `--features ndi` (e.g. `cargo run --release --features ndi`).
- Install the NDI runtime from [ndi.video](https://ndi.video). Fosfora loads it dynamically at startup; if it's missing, the NDI panel lists the locations it searched and a download link.

**Setup:**
1. Open the **Outputs** section in the left sidebar
2. Enable NDI output
3. Set a source name (default: "Fosfora")
4. Choose output resolution: Match Window, 720p, 1080p, or 4K
5. In your NDI receiver, look for the source name you configured

**Alpha channel:** Effects that write meaningful alpha (particles, transparent backgrounds) preserve it through post-processing and deliver it to NDI for downstream compositing. Enable "Alpha from Luma" if you want brightness-based alpha instead.

**Performance:** NDI capture runs on a separate thread with GPU readback. Frames are dropped gracefully if the sender falls behind — VJ performance always takes priority over NDI output.

---

## Global

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| **D** | Toggle UI overlay |
| **F** | Toggle fullscreen |
| **Esc** | Quit (with confirmation dialog) |
| **[** | Previous layer |
| **]** | Next layer |
| **Space** | Next cue (when timeline has cues) |
| **T** | Toggle timeline play/stop |
| **Tab** | Cycle UI widgets |

### Themes

Fosfora supports multiple UI themes. Change the theme in the settings area of the UI. Available themes follow WCAG 2.2 AA contrast standards for accessibility.

### Configuration Files

All configuration is stored in `~/.config/phosphor/`:

| File | Contents |
|------|----------|
| `settings.json` | Theme, audio device |
| `midi.json` | MIDI port, CC mappings, trigger bindings |
| `osc.json` | OSC ports, address mappings, TX rate |
| `web.json` | WebSocket port, enabled flag |
| `ndi.json` | NDI source name, resolution, enabled |
| `presets/*.json` | Saved presets |
| `scenes/*.json` | Saved scenes |
| `effects/*.pfx` | User-created effects |
| `effects/*.wgsl` | User-created shaders |

### Build Variants

```bash
cargo run                          # Standard build
cargo run --release                # Release build (faster shaders)
cargo run --features video         # Video playback (requires ffmpeg)
cargo run --features ndi           # NDI output (requires NDI runtime)
cargo run --features "video,ndi"   # Both features
cargo run --features webcam        # Webcam input
```

### Status Bar

The bottom status bar shows at a glance:
- **Shader errors** (with dismiss button) or keyboard hints
- **BPM** with beat flash indicator
- **SCN** — Scene indicator with cue counter (e.g., "2/5") when a scene is active
- **PTL** — Particle count (when active)
- **MIDI** — Green dot when receiving
- **OSC** — Green dot when receiving
- **WEB** — Blue dot when clients connected
- **NDI** — Green dot when streaming
- **FPS** — Smoothed frame rate

### Priority Order

When multiple controllers send conflicting values in the same frame, the last-write-wins rule applies in this order:

1. MIDI (processed first)
2. OSC (processed second, overrides MIDI)
3. Web (processed last, overrides both)
