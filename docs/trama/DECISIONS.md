# trama — running decision log

One line per decision, with reasoning (handoff §15). Newest at the bottom.
Owner reads this instead of diffs when reviewing direction.

- **2026-08-07 — Shader ABI: adopt existing ABI v3, retire handoff §7.** (Owner
  ruling.) The single-group `PhosphorUniforms` contract already serves 139 shaders,
  the loader preamble, the in-app editor, and the off-thread compiler; per-node
  params fit as a uniform arena with static-offset bind groups, so §7's benefits
  were already absorbed. Cost accepted: 16 param slots v1 (batched v4 append is the
  escape), manifests use the typed `ParamDef` format.
- **2026-08-07 — New scene-level executor; `PassExecutor` untouched.** Its idioms
  (prev-frame parity flip, `@`-inputs, hard error on unknown names) are mined, not
  modified — retrofitting dynamic topology into the per-layer render path is the
  drive-by refactor §15 forbids.
- **2026-08-07 — Seam: `frame_graph::execute_and_composite` behind a per-scene
  `RenderMode`.** One edit covers live, dissolve, and headless; postprocess keeps
  tonemapping. Escape hatch: a `LayerContent::Graph` variant later, if wanted.
- **2026-08-07 — 32 bands from 64-mel adjacent pairs + dt-correct one-pole.**
  Mel-spaced rather than strictly log (VJ-grade fine). CPU-side only; the
  `AudioFeatures` ABI stays frozen; never touch `frame_receiver()` (signal/ owns it).
- **2026-08-07 — Trama modulation stays separate from `BindingBus` in v1.**
  The bus keeps routing audio/MIDI/OSC to layer params; unification is a real
  future question, deliberately deferred.
- **2026-08-07 — Gate passed: INTEGRATION.md §9 items 1–7 all approved by owner.**
  M0 authorized.
- **2026-08-07 — Names confirmed by owner: "trama", `.fio.json`.** No longer
  provisional; the handoff-header caveat is closed.
- **2026-08-07 — Phase 0 docs merge to main immediately; M0 code rides branch
  `trama-m0`.** Docs are inert and safe regardless of the spike's fate.
- **2026-08-07 — The handoff spec stays local-only** (untracked + gitignored),
  matching the owner's planning-file precedent.
- **2026-08-07 — egui-snarl pinned to `0.9`.** 0.9.0 declares egui ^0.33 (matches
  the repo's 0.33.3, verified on crates.io); 0.10/0.11 need egui 0.34/0.35 and
  Rust 1.92. The §9.5 canvas-less fallback stays authorized but should be unneeded.
- **2026-08-07 — Executor deviates from handoff §9.3 timing: pooled targets are
  assigned per plan-build, not acquired/released per frame.** Same pool, same
  `(w, h, format)` keying, same resize-only rebuild; per-plan assignment is what
  makes cached per-node bind groups sound and yields I8's zero-steady-state-alloc
  directly. Per-frame timing buys nothing until M2 previews/multi-resolution.
- **2026-08-07 — Canvas re-anchors the snarl viewport per frame (owner
  play-test finding).** egui-snarl persists its pan/zoom as a *screen-space*
  transform, so nodes ignored window drags; the viewer's `current_transform`
  hook now translates it by the canvas origin's frame-to-frame delta, keeping
  the window movable. Also from the play-test: noise_field became
  palette-colored — hue-rotating grayscale is a mathematical no-op, so the
  original demo chain showed nothing — and the canvas warns when Output is
  unwired in Trama mode instead of silently showing black.
- **2026-08-07 — M1: 32-band modulation smoother is attack 20 ms / release
  200 ms, uniform across bands; named-band sources (bass/mid/high, rms, onset…)
  are consumed as-is.** The mel column is the only raw source trama reads — the
  named features arrive adaptive-normalized and asymmetrically smoothed from the
  audio pipeline, so a second feature-level stage would double-smooth. The
  per-modulation `smoothing` knob remains the user's additional stage by choice.
- **2026-08-07 — M1: modulation lives on `NodeInstance` as a name-keyed slot
  list (one per param); runtime state (osc phase, S&H/Drift RNG, smoother) is
  embedded beside the config, never serialized, and seeded from node id + param
  name** — deterministic replay, no wall clock anywhere.
- **2026-08-07 — M1: resolution-rule semantics.** Oscillator signals are
  bipolar, audio signals unipolar; Add = `base + s·amount·span`; **Multiply is
  relative** (`base·(1 + s·amount)`) — the handoff's literal `base·m` with `m`
  pre-scaled into the span is dimensionally param-units² and pins the value to
  ~0 whenever the signal rests; Replace crossfades the base toward the
  range-mapped signal by `|amount|` (negative amounts invert). Clamp to
  [min,max], then a symmetric dt-correct one-pole with τ = smoothing² · 2 s
  (quadratic knob; 0 snaps, first resolve always snaps).
- **2026-08-07 — M1: param/modulation edits do not bump the graph version.**
  Only structural edits replan — a replan rebuilds every bind group and
  reassigns the texture pool, which would break I8 at slider-drag rates. The
  mutators hand out a `NodeParamsMut` projection so structural fields stay
  behind the versioned API.
- **2026-08-07 — M1: modulation resolves once per frame in `App::update`
  (`TramaSystem::update`, absorbing `set_frame_uniforms`); the executor only
  overlays the cached results.** The dissolve path executes the graph twice
  per frame — advancing state in `execute` would double-run oscillators. All
  nodes resolve, orphans included: phases stay warm across rewires, and
  `live_set()` would allocate per frame.
- **2026-08-07 — M1: modulation targets Float params only.** Bool needs
  thresholding/flicker semantics, Color a per-channel-vs-HSV policy, Point2D
  2D sources — deferred design debt serving no accept criterion. The
  inspector still gives Color/Bool/Point2D plain unmodulated editors.
- **2026-08-07 — M1: BeatSync derives phase from the continuous beat clock
  (`beat_index + beat_phase`) and freezes pre-tempo-lock; 4/4 assumed.** The
  interp PLL already holds phase through silence, and a fallback tempo would
  guarantee a visible snap the moment lock lands — Hz rates are the
  always-moving option. The inspector captions unlocked BeatSync in text.
- **2026-08-07 — M1: inspector is a right side column inside the trama
  window; the ghost indicator is a bright tick + triangle over the slider
  rail plus a monospace `→ value` readout** — luminance + shape + text,
  never hue alone (owner is colorblind). Mod-source state reads as a text
  glyph ("~ sine", "≈ bass").
- **2026-08-07 — M1 play-test finding: selection is ours, not egui-snarl's.**
  snarl 0.9 selects nodes only on shift/cmd-click or a background rect-drag —
  the owner clicked nodes and the inspector never showed content. A plain
  primary press on a node now selects it (via the `final_node_rect` hook,
  which hands the viewer each node's on-screen rect), a click on empty canvas
  deselects, and the selected node carries a bright outline ring
  (luminance-based). `CanvasState.selected` is the single source of truth;
  snarl's internal selection set is unused.
- **2026-08-07 — M1 play-test refinements (owner): inspector width is fixed
  at 315 px (house `exact_width` pattern) — greedy-width sliders inside a
  resizable panel inside an auto-sizing window ratcheted the window wider on
  every selection; each parameter is a card with its modulation sub-block
  indented beneath; every control row leads with the house `R` reset button
  and carries a tooltip.**
- **2026-08-09 — M2: Feedback's input wire is a non-edge in every cycle
  computation (`connect`/`topo_order`/`validate`) but a real edge in
  `live_set`** — the loop's upstream chain must stay live. A loop closed by
  a feedback node's *output* edge is still refused, and the delay edge dies
  with the node.
- **2026-08-09 — M2: feedback ping-pong pairs live OUTSIDE the plan
  (HashMap by node), with ONE executor-global parity — the pass_executor
  #1481 idiom.** A rewire elsewhere must not clear an unrelated echo, and
  pool identity isn't preserved across replans. Parity (and the preview
  cursor) advance only in `begin_frame`, driven from `TramaSystem::update`,
  because the dissolve path executes the graph twice per frame — same rule
  that keeps modulation single-advance. Steps carry `[BindGroup; 2]`
  (identical Arc-clones when no feedback input) indexed by parity.
- **2026-08-09 — M2: feedback copy steps are 1-input "effect-shaped"
  passthrough pipelines appended after all effect passes.** Consumers read
  the *read* buffer, so end-of-frame copies are universally correct
  (chained feedbacks included); the effect-shaped layout reuses the entire
  bind-group builder. A Feedback node feeding Output gets an explicit
  read→Output blit — it has no effect pass to target the output.
- **2026-08-09 — M2: bypassed Feedback = placeholder, never wire-through;
  unwired input = no copy step, consumers read black, pair retained.**
  Bypass aliasing across the delay edge would re-close the cycle
  combinationally (a node sampling its own target in its own pass); an
  un-copied pair under a flipping parity would strobe two stale frames.
  Retaining the pair means rewiring resumes from the stale echo.
- **2026-08-09 — M2: preview targets are `Rgba8Unorm`, NOT `-srgb`
  (deviation from handoff §9.6); the blit shader encodes linear→sRGB
  itself.** The egui renderer paints the sRGB surface via its
  linear-framebuffer entry point, which treats sampled *user* textures as
  gamma-encoded — an `-srgb` view would hardware-decode and then get
  gamma-converted a second time (thumbnails too dark). First
  `register_native_texture` use in the repo; texture identity is stable per
  node so registration is once-per-texture, freed via a dead-list when the
  node goes.
- **2026-08-09 — M2: the plan is keyed `(graph version, canvas_open)`;
  orphans execute only while the canvas is open (handoff §9.1), and
  Layers mode runs a preview-only execute when it is.** Toggle-replan is a
  human-speed event; always-running orphans would burn GPU rendering
  invisible content. The I4 failure path stamps BOTH keys or a failed
  build would retry every frame. Transform ships 4 Float params (not
  Point2D) because M1 modulation targets Floats only and a modulated
  transform is the entire point of the node.
- **2026-08-09 — M2: I8 is now *verified*, with this interpretation: trama-
  owned per-frame CPU work allocates a hard ZERO (thread-local counting
  allocator, test-only; guard proven by injecting an allocation); around
  the GPU boundary the assertion is that the per-frame allocation FLOOR
  never rises (wgpu's own encoding allocates ~93/frame with sporadic
  spikes — exact constancy is untestable there), plus the existing frozen
  resource identity (pool stats, feedback generation, plan version).**
- **2026-08-09 — M2: first wgpu-profiler scopes, via a cfg-free zero-sized
  `ProfilerHandle` threaded `execute_and_composite → TramaSystem →
  TramaExecutor`** — "layers", "trama" + per-node children by effect id,
  "feedback-copy", "preview-blit". The profiler window had shipped
  data-less since the feature landed; this is also the measuring stick for
  the previews-<1 ms acceptance.
- **2026-08-09 — M2: .pfx-wrap spike verdict: GO for M4 wrapped Sources.**
  Aurora built via `layer_builder` feeds a trama input validation-clean
  (probe `trama_spike_pfx_layer_feeds_trama_input`): formats/usages match,
  ParamStore packing is shared, `layer.flip()` maps 1:1 onto
  `begin_frame` (both parities advance in lockstep → consumers use the
  existing `[BindGroup; 2]` idiom). Costs accepted going in: a full-res
  ping-pong pair per wrapped pass (a few sources fine, tens not — profile
  it), particle dispatch runs 2× during a dissolve (inherited from the
  Layers path), and the real M4 work is registry/hot-reload bridging, not
  rendering.
