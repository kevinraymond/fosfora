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
