# Fosfora — Feature Program: Code Session Starter

You are working in the Fosfora repo (Rust + wgpu + egui + cpal + rustfft music visualizer,
55 effects, 74 audio features @ 86 Hz, OSC/MIDI, NDI/Spout/Syphon/v4l2 out, phone web
control surface, binding matrix, presets + scene cue lists, live-editable WGSL effects).

**This document was written from the README and UI design mockups only — I have not seen
the Rust source. Verify every assumption in Phase 0 before writing code.** Design mockups
for the six main panels exist as React JSX (phosphor-audio/effects/presets/scenes/layers/
settings-panel.jsx) — they are visual/interaction references for the egui UI, not code to
port literally.

Prioritized workstreams below. Do Phase 0, then propose a concrete plan per workstream
before large diffs. Keep everything feature-flagged; don't break existing behavior.

---

## Phase 0 — Repo recon (do first, no code changes)

1. Map the codebase: audio capture → feature extraction pipeline (hop size, windowing,
   where the 74 features and the existing build/drop + drum/melody separation live),
   render/compositor, OSC/MIDI I/O, binding matrix, egui panel code, output modules,
   config layer (`~/.config/phosphor/`).
2. Produce a short ARCHITECTURE-NOTES.md: module map + the exact integration points for
   each workstream below.
3. Note current threading model and per-frame budget (who owns the audio thread, is
   analysis on its own thread, can it run without a window/GPU surface at all?).
4. Flag the phosphor/fosfora naming split (config dir, OSC prefix) — decide one prefix
   for the public OSC schema before it ships.

---

## Workstream A — "Signal": headless analysis-broadcast mode  ★ flagship

A `--headless` (or `--signal`) mode: no window, no GPU render path, just the analysis
engine broadcasting structured signals. Fosfora as the analysis brain of a rig feeding
TouchDesigner / Resolume / VDMX / QLC+ / grandMA / custom OSC consumers.

**OSC schema (versioned, documented, stable — this is the product):**

```
/fosfora/v1/beat            i (beat count)         on every beat
/fosfora/v1/downbeat        i (bar count)          on every downbeat
/fosfora/v1/bpm             f
/fosfora/v1/bar_phase       f 0..1                 continuous, TX rate
/fosfora/v1/key             s ("Am", "F#")         on change
/fosfora/v1/chord           s                      on change
/fosfora/v1/onset           f (strength)           event
/fosfora/v1/stem/drums/energy    f 0..1            continuous
/fosfora/v1/stem/bass/energy     f 0..1            continuous
/fosfora/v1/stem/melody/energy   f 0..1            continuous
/fosfora/v1/stem/*/onset         f                 event
/fosfora/v1/section         s enum: intro|build|drop|break|outro|steady   on change
/fosfora/v1/build           f 0..1 (ramp)          continuous
/fosfora/v1/drop            i (trigger)            event
/fosfora/v1/energy          f 0..1                 continuous
```

Plus: raw feature bus (all 74, opt-in, namespaced `/fosfora/v1/feat/<name>`), MIDI clock
out, MIDI note/CC emit mapped from events. TX rate configurable (default = existing 30 Hz
OSC TX; events fire immediately).

**Section state machine:** derive from existing build-to-drop detection + energy/flux
features. Explicit hysteresis, per-state minimum dwell (musical time, in bars, not
seconds), confidence output alongside the label. Design it so an ONNX causal model can
replace/augment the heuristic later without schema change (ML infra is staged separately;
leave a clean trait boundary: `trait SectionEstimator`).

**Deliverables:** headless mode, schema doc (markdown, shipped in docs/), example
receiver patches (a TouchDesigner .tox note, a QLC+ mapping note — even just docs),
config in the existing settings system, and a "Signal" focused UI panel later (see E).

Target footprint: should run on a Pi-class/spare machine — measure CPU in headless mode
as part of D.

---

## Workstream B — Ableton Link (small, table stakes)

Integrate a Rust Link binding (evaluate `rusty_link`). Link as both input (chase DJ/
producer tempo + phase) and output. Surface toggle in settings panel; feed Link phase
into the beat-driven scene advance and into the Signal schema (`/fosfora/v1/link/...`).
Fallback stays MIDI clock + internal tracker.

---

## Workstream C — Benchmark harness: real tested results

Goal: honest numbers for where Fosfora's analysis is strong/weak vs published systems,
**measured in causal streaming mode** (chunked at the real hop size, no lookahead) —
because that's the product claim. Batch numbers from papers (madmom, BeatNet, allin1)
are the comparison targets, clearly labeled offline-vs-online.

**Harness design:**
- Offline runner that streams audio files through the *actual* real-time engine
  (same code path, simulated real-time chunks), dumps timestamped event/feature logs
  (JSONL), scores against annotations.
- Metrics per mir_eval conventions:
  - Beat: F-measure (±70 ms), CMLt, AMLt
  - Downbeat: F-measure
  - Tempo: Acc1 / Acc2
  - Key: MIREX weighted score
  - Structure: boundary F1 @ 0.5 s and @ 3 s; pairwise clustering F1; for the
    build/drop detector specifically: drop-onset hit rate within ±1 bar, false-drop rate
  - Stems (energy proxy): correlation of per-stem energy vs ground-truth stem energy
    from MUSDB18-HQ
- Report card generated as markdown table: Fosfora (causal) vs BeatNet (causal) vs
  madmom/allin1 (offline), per dataset.

**Datasets:**

| Dataset | What | Notes |
|---|---|---|
| Harmonix Set (~912 tracks) | beats, downbeats, functional segments | annotations on GitHub; audio is YouTube-sourced — needs fetch/align step; allin1's SOTA benchmark |
| GiantSteps Tempo + Key | EDM tempo & key | most genre-relevant; audio via Beatport previews |
| Ballroom / SMC | beat tracking (SMC = hard cases) | classic MIREX sets |
| SALAMI | structure | genre diversity check |
| MUSDB18-HQ | stems | for per-stem energy validation |

Build dataset download/prep scripts with checksums; keep audio out of the repo. CI runs
a tiny fixture subset; full runs are a local `cargo run -p bench-harness` (or xtask).

---

## Workstream D — Progressive real-time performance tiers

Figure out what runs where, and degrade gracefully.

1. **Cost audit:** instrument the feature pipeline (criterion micro-benches + a
   whole-pipeline bench); produce a per-feature cost table (µs/frame) on: Pi-class ARM,
   mid laptop CPU, desktop.
2. **Tier the 74 features:**
   - T0 core: RMS, bands, onset, beat/BPM — always on, must fit Pi headless
   - T1 standard: chroma/key, MFCC, flux/centroid/etc.
   - T2 heavy: stem separation, chord tracking, section estimation
   - T3 ML (future): ONNX causal models
3. **Profiles:** `signal-lite` (T0–T1, headless, Pi target), `full` (all, desktop),
   `custom`. Profile selection in config + CLI flag.
4. **Runtime governor:** watch analysis-thread deadline misses; auto-shed T2 features
   with a visible warning rather than glitching; expose current tier over
   `/fosfora/v1/status`.
5. **Latency budget doc:** capture → hop → feature → OSC emit, per profile, measured
   not estimated. This number goes in the README when Signal ships.

C and D feed each other: the harness measures accuracy per tier (what does key detection
cost in accuracy at a cheaper config?).

---

## Workstream E — UI review + redesign

Current state: main functionality in left/right column panels; cramped. The six JSX
mockups show the intended density/visual language (JetBrains Mono, dark #0d0e11,
3px type strips, collapsible sections, 26px control rows).

1. **Audit:** inventory every egui panel, its controls, and pain points (what overflows,
   what's buried, what's used constantly vs rarely). Screenshot-driven if possible.
2. **IA proposal:** evaluate workspace/mode-based layouts before adding more panels:
   - **Perform** — layers, presets, scenes, transport front and center
   - **Design** — effect browser, parameters, shader editor, binding matrix
   - **Signal** — the analysis-brain view: feature meters, section state, OSC monitor,
     tier/profile status (this is the new focused UI mentioned for Workstream A)
   Also evaluate: dockable/detachable panels vs tabs, a bottom strip for transport,
   collapsible-by-default sections, and whether the 280px column width should flex.
3. Propose first, then implement incrementally behind a layout setting so the current
   layout stays available.

---

## Workstream F — FFGL/ISF export (after A–E are moving)

Evaluate `ffgl-rs` / `ffgl-core` (edeetee) to ship 5–10 flagship shader effects as FFGL
plugins for Resolume/VDMX. Spike first: WGSL→GLSL transpilation fidelity (naga can emit
GLSL — test it on the simplest shader effect). If fidelity is poor, fall back to the
interop story (Syphon/Spout/NDI alongside Resolume) and document it. Reuse existing
effect parameter definitions as FFGL parameters.

## Workstream G — Art-Net/sACN output + minimal pixel map

Output module: sample a chosen layer/region into an N×M pixel grid → Art-Net + sACN
(E1.31) universes. Fixture map via CSV/JSON import; preview overlay in UI. Evaluate
`artnet_protocol` / `sacn` crates. Pairs with A: analysis + visuals both drive a rig.

## Deferred (do not build yet)

- Preset/shader sharing gallery — define the portable preset bundle format only
  (layer stack + WGSL + metadata + preview image, self-contained) so future work
  doesn't require a format break.
- Multi-operator role-scoped phone surface (VJ/LD/booth views) — note integration
  points in the web surface code during Phase 0; implement later.

---

## Suggested sequencing

1. Phase 0 recon → ARCHITECTURE-NOTES.md
2. A (Signal) + C (harness) in parallel — the harness validates A's section/drop
   detectors before the schema is declared stable
3. B (Link) — small, slot in early
4. D (tiers/governor) — informed by C's profiling
5. E (UI) — Signal workspace depends on A existing
6. F, G

## Ground rules

- Explore before editing; plan per workstream before large diffs
- Feature flags for everything new; existing behavior untouched by default
- Tests + criterion benches for the analysis path; harness fixtures in CI
- OSC schema is versioned (`/v1/`) from day one; breaking changes bump the version
- Commit granularity: one workstream = one branch; small reviewable commits
