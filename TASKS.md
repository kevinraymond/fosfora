# Fosfora feature program — task tracker

Working checklist for the multi-session program defined in
`2026-08-06_SESSION_STARTER.md` + `2026-08-06_SESSION_STARTER_ADDENDUM.md`.
Recon facts and integration points live in `ARCHITECTURE-NOTES.md`.
Update this file at the end of every session (status board + session log).

## Status board

| Workstream | State |
|---|---|
| Phase 0 recon + docs | **done** (2026-08-06) |
| R — phosphor→fosfora rename | **done + merged to main** (2026-08-06) |
| A — Signal v1 (headless broadcast) | **done + merged to main** (2026-08-06), incl. live smoke test (A10) |
| B — Ableton Link | later (small; slot in after A) |
| C — Benchmark harness | later (validates A's detectors; JSONL contract already fixed by A) |
| D — Perf tiers + governor | later (needs C's cost data; no bench infra exists yet) |
| E — UI workspaces | later (Signal panel depends on A) |
| F — FFGL/ISF export | later (naga WGSL→GLSL spike first) |
| G — Art-Net/sACN output | later (follows `FrameSink` pattern) |
| H — Virtual venue / projector previz | later (added 2026-08-06; see block below) |

Decisions locked (2026-08-06): full rename; config dir auto-move; OSC RX accepts both
prefixes + TX prefix toggle (default `fosfora`); stem addresses ship as documented
HPSS/band proxies; Signal schema is `/fosfora/v1/`, additive-only once shipped.

## R — Rename (branch `fosfora-rename`)

- [x] R1 `paths.rs`: shared config root + auto-move migration from `~/.config/phosphor/`
- [x] R2 OSC: dual-prefix RX, canonical `/fosfora` addresses, `tx_prefix` toggle,
      saved-bindings source-id rewrite
- [x] R3 Internal identifiers: thread names, wgpu labels, MIDI/Pulse client names,
      `FOSFORA_*` env vars, recording filename prefix
- [x] R4 WGSL lib: `fosfora_*` functions + deprecated `phosphor_*` aliases (31 fns);
      GPU probe tests can now pick Metal, so the shader gates run on macOS too
- [x] R5 Crate rename `crates/phosphor-app` → `crates/fosfora-app`, binary `fosfora`,
      CI workflows + scripts + doc paths; `RUST_LOG` target is now `fosfora`
- [x] R6 Docs/bridges/packaging: bundle id, bridge rename + shim, README stale-claim
      fixes (74→83, no chord, HPSS not stems), CHANGELOG entry

Deliberately kept as "phosphor" (allowlist): the **Phosphor** CRT effect (+ its
`phosphor.pfx`/`phosphor_sim.wgsl`/`phosphor_history.wgsl` assets and preset refs),
`phosphor_demo.ply` (cached release asset), **`PhosphorUniforms`** (the shader ABI v3
struct name user custom effects compile against — rename only with a future ABI bump),
WGSL `phosphor_*` alias wrappers, OSC `/phosphor/` RX compat + TX legacy option,
`osc./phosphor/` and config-dir migration literals, bridges `phosphor_bridge.py` shim +
`PHOSPHOR_*` env fallbacks, CRT-phosphor prose in GALLERY/catalog, historical CHANGELOG
entries.

## A — Signal v1 (branch `signal-v1`)

- [x] A1 `signal/schema.rs` — `/fosfora/v1/` pinned address table
- [x] A2 `signal/sink.rs` — `SignalSink` + Udp/Jsonl/Vec sinks + `OscSender::send_message`
- [x] A3 `signal/section.rs` — `SectionEstimator` trait + heuristic v1 (bars-dwell,
      hysteresis, confidence; live path never emits `outro`) + `signal/clock.rs` BarClock
- [x] A4 `signal/phrase.rs` — PhraseTracker (8/16/32 inference) + `/predict/drop`
      (slope term tried and cut — decays mid-build; harness may earn it back in)
- [x] A5 `signal/emitter.rs` — events per hop, sample-clock decimation, 1 Hz re-broadcast;
      golden-signal test reconciles emitted counts against engine pulse counters
- [x] A6 `signal/types.rs` — `SignalConfig` → `signal.json` (port 9010)
- [x] A7 `--signal` live loop (`frame_receiver`, `ctrlc`, status + pulse reconciliation)
- [x] A8 `--signal-dump` JSONL (feature `analyze`; verified byte-identical across runs)
- [x] A9 `docs/SIGNAL.md` complete + CHANGELOG + README pointers
- [x] A10 live rig smoke test (2026-08-06): `--signal` on the MacBook mic at 48 kHz
      (93.75 Hz hop path), 120 BPM track through speakers → 18,483 valid OSC messages,
      BPM locked 119.0-119.5, 189 contiguous beat counts, kick onsets detected
      acoustically, continuous groups at exactly 30 Hz, clean-shutdown
      `/status/online 0` observed. Reconciliation caught 2 engine-side frame drops
      during status ticks → live loop now drains the channel backlog per wake.
- Follow-up (minor): phrase/len announcements can flap between hypotheses while
  confidence ≈ 0 (consumers should gate on it, but consider holding announcements
  under a confidence floor); consider widening the frame channel from bounded(4).
- Phase A2 (deferred): MIDI clock out + note/CC emit (`midi/output.rs` greenfield),
  windowed-mode Signal TX sharing, multiple OSC destinations.

## Backlog (one-liners; plan before starting)

- **B Link**: evaluate `rusty_link`; input+output; feed beat-driven scene advance;
  `/fosfora/v1/link/*` (address space reserved in SIGNAL.md). No Link dep exists today.
- **C harness**: extend `src/analyze/` (it's ~90% of the runner — `drive_with` +
  `HopStream`). Score A8's JSONL vs annotations: beat F/CMLt/AMLt, downbeat F, tempo
  Acc1/2, key MIREX, structure boundary F1 + pairwise, drop hit rate ±1 bar +
  false-drop rate, stem-proxy correlation vs MUSDB18-HQ. **Addendum: predict/drop
  lead-time distribution (beats before annotated drop at confidence 0.5/0.8) and
  false-alarm rate — measured, not asserted.** Datasets: Harmonix, GiantSteps,
  Ballroom/SMC, SALAMI, MUSDB18-HQ; download scripts w/ checksums, audio out of repo;
  tiny fixture subset in CI.
- **D tiers**: criterion micro-benches first (none exist); per-feature µs/frame on
  Pi/laptop/desktop; T0-T3 tiering; `signal-lite`/`full` profiles; runtime governor
  shedding T2 with warning; tier over `/fosfora/v1/status/tier` (hook already in A);
  measured latency budget doc.
- **E UI**: collapse `draw_panels`'s 27 args into a context struct first; then
  Perform/Design/Signal workspaces behind a `layout_mode` setting (ThemeMode is the
  template). Panels are relocatable (pure `fn draw(ui,&Info)` + temp-id drain; layout
  lives solely in `ui/panels/mod.rs`). JSX mockups are NOT in the repo — get them from
  the user before starting.
- **F FFGL**: spike naga WGSL→GLSL fidelity on the simplest effect; then `ffgl-rs`
  eval; fallback = document the Syphon/Spout/NDI interop story.
- **G Art-Net/sACN**: new output following `FrameSink` (`output/sink.rs:46`) +
  `XSystem{config,pipeline}` house pattern; N×M grid sampling on the sender thread;
  fixture map CSV/JSON; evaluate `artnet_protocol`/`sacn` crates.
- **Deferred by design**: portable preset bundle format (define only); role-scoped web
  surface (seams: `server.rs` route match, `run_client` role param, clients vec,
  `parse_client_message` gating — see ARCHITECTURE-NOTES).

## H — Virtual venue / projector previz (added 2026-08-06)

Goal: a 3D "virtual venue" mode — import a mesh, place physically-modeled virtual
projectors, render Fosfora's live audio-reactive output through them onto the mesh for
practice + pre-show previz, with a path to per-projector physical output routing.

- **H1 Scene & import** — new `Scene3D` module: orbit/fly camera, grid, gizmos (egui
  panels as usual). Mesh import via `gltf` (already a dep) + `tobj` (glTF/GLB/OBJ);
  bundle 3-4 primitive venues (cube, facade, arch); center/scale-normalize on load,
  expose decimate + recenter. Lives as a new tab/mode alongside the compositor — the
  existing 8-layer pipeline is untouched.
- **H2 Virtual projector (core)** — `VirtualProjector { pose: Mat4, throw_ratio,
  lens_shift_h/v, resolution, lumens, aspect }`; off-axis perspective projection from
  throw ratio + lens shift. Pipeline: (1) depth-only pass from projector POV → depth
  texture; (2) main pass samples the **existing compositor output texture** as
  projector content, transforms fragment → projector clip space, depth-compares for
  occlusion, applies inverse-square + cosine falloff + albedo + ambient. WebGPU NDC
  bias matrix for the projection. Integration: one new bind group on the compositor's
  final RGBA texture — no changes to effects.
- **H3 Views & practice mode** — projector-POV / audience-POV / free-orbit; readouts
  (pixels-on-surface, coverage %, throw distance). Practice mode = bundled model +
  one projector + guided prompts.
- **H4 Multi-projector + output routing (V2)** — N projectors, each mapped to an
  existing NDI/Spout/Syphon/virtual-camera output (reuse the output module — each
  projector's content view becomes an output feed; disguise's
  content-independent-of-position model). Edge-blend preview: per-overlap gradient
  falloff (preview only).
- **H5 Calibration/scan (V3, deferred)** — phone-scan import (OBJ/glTF from
  Polycam/Scaniverse) + cleanup; camera calibration via gray-code/structured-light or
  PnP (≥6 non-coplanar correspondences) to solve projector pose and conform the mesh
  to reality. Needs a Rust CV path (OpenCV bindings or an nalgebra-based solver).

Reused Fosfora modules: compositor output texture (content source), output module
(NDI/Spout/Syphon routing), preset/scene-cue system (store venue + projector layouts
as scenes/cues), OSC/MIDI (drive projector params live), planned Art-Net/sACN (G) +
Signal (A) to co-visualize lighting states in the same 3D scene.

## Positioning guardrails (addendum — product constraints)

Every stateful signal carries a confidence value. Signal never triggers anything —
it informs the operator's rig. Frame it as telemetry for humans, never "automation of
the performance."

## Session log

- **2026-08-06** — Phase 0 recon (3 parallel explorations); corrections found: 83
  features not 74, no chord detection, no stem separation (HPSS only), OSC TX on render
  thread loses pulses. Decisions locked (see above). Plans written for R + A.
  **R complete** on branch `fosfora-rename` (6 commits: config auto-move ran live on
  this machine, dual-prefix OSC, identifiers, 31 WGSL fns + aliases, crate/binary/CI,
  docs/bridges/packaging + stale-claim fixes; GPU shader gates now run on macOS via
  Metal). **A complete** on branch `signal-v1` (schema/sinks/section/phrase core +
  emitter/config/CLI/dump/docs; `--signal-dump` verified deterministic). A10 live
  smoke test passed (48 kHz mic, BPM locked ~119 vs true 120, 18,483 valid messages,
  clean-shutdown goodbye on the wire) and caught a frame-drop issue → backlog drain
  fix. Both branches merged to main. Next: B (Link), C (harness scores A8's JSONL).
