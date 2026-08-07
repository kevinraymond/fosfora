# trama — Phase 0 integration survey

Phase-0 recon for the trama node-graph effect-chain system (`fosfora-trama-handoff.md`
§4). Written 2026-08-07 from a source-level survey of `crates/fosfora-app` v1.32.0;
line numbers drift, names don't. Style and depth follow `ARCHITECTURE-NOTES.md`.

**Gate: PASSED 2026-08-07** — owner approved all seven sign-off items (§9);
rulings and follow-on decisions are recorded in `DECISIONS.md`.

## 0. The thesis

The handoff assumed a codebase with no UI stack, thin audio analysis, and no effect
infrastructure. The survey found the opposite: Fosfora already ships effects-with-
params (`.pfx` + `ParamDef`/`ParamStore`), a modulation bus (`bindings/`), hot reload
with off-thread compilation, an in-app WGSL editor, feedback semantics
(`PassDef.prev_inputs`), and render-to-texture everywhere. What trama actually adds:

1. An **arbitrary user-authored DAG across whole-effect instances** — today the only
   composition is the fixed 8-layer stack + linear compositor.
2. A **graph canvas UI with per-node live previews**.
3. The **Parameter/Modulation/Uniform triple** as a unified UX — today parameter
   sliders and the bindings panel are separate surfaces, and no oscillator/LFO
   modulation source exists at all.
4. **Whole-graph `.fio.json` serialization**.
5. A **transient texture pool** (none exists anywhere in the repo).

The M0 go/no-go bet is therefore the executor + the triple, not the plumbing the
handoff budgeted for. Milestones are redrawn in §8.

## 1. UI stack (handoff §4.1)

**egui is fully integrated**: egui / egui-wgpu / egui-winit all 0.33.3, raw
winit 0.30.13 + wgpu (no eframe), plus `egui_code_editor` 0.2.21. The overlay
(`ui/overlay.rs`) opens its own `"egui-pass"` against the surface view with
`LoadOp::Load` after the visual chain, constructed with the surface format so it is
sRGB-aware. 31+ panels exist under `ui/panels/`.

**Implication:** the handoff's "egui integration PR as M0 prerequisite" is moot.
The trama canvas should live in `src/trama/ui/`, *not* `ui/panels/` —
`panels/mod.rs::draw_panels` owns global geometry through 27 positional args and is
scheduled for its own refactor (workstream E); M0 must not touch it.

## 2. Render loop and the seam (handoff §4.2)

Nothing renders to the surface directly. Every layer/effect renders into
`Rgba16Float` offscreen targets (`GpuContext::hdr_format`,
`gpu/render_target.rs::RenderTarget`); the surface is written exactly twice per
frame — once by `PostProcessChain::render` (tonemap/alpha, `gpu/postprocess.rs`),
once by the egui overlay.

The already-extracted chokepoint is **`frame_graph::execute_and_composite`**
(`gpu/frame_graph.rs`): executes the enabled layers, composites, returns
`(&RenderTarget, PostProcessDef)`. It has exactly three callers — the live path and
the dissolve re-render in `App::render`, and the headless scene renderer — and its
module doc records that it exists to stop those copies drifting.

**Chosen seam:** a `RenderMode { Layers, Trama }` switch inside
`execute_and_composite`. In Trama mode the graph executes and the Output node's
target is returned as `source`. One edit covers live, dissolve, and headless;
dissolve crossfades between Layers-scenes and Trama-scenes work for free because
`TransitionRenderer::crossfade` consumes `source` textures. Trama outputs
`Rgba16Float` linear like every layer; **postprocess keeps ownership of
tonemapping** — the handoff §9.7 "final blit with sRGB handling" already exists.

Rejected: the `app.rs` source-swap between crossfade and postprocess (re-opens the
dissolve-tail duplication as two edit sites); replacing the postprocess blit (trama
should not own tonemapping). Escape hatch recorded for later: a
`LayerContent::Graph` variant is mechanical if graph-in-a-layer is ever wanted,
because the executor's contract is the same shape as `Layer::execute`.

## 3. Audio pipeline (handoff §4.3)

The handoff's `AudioFeatures` contract (§8 Globals) is almost entirely shipped:

| Handoff field | Status | Source |
|---|---|---|
| `rms` | ships | `audio/analyzer.rs`, adaptive-normalized |
| `onset` (impulse + exp decay) | ships, spec-exact | SuperFlux (`audio/beat.rs`), instant-attack/exp-release hold, τ = 0.20 s |
| `beat_phase` | ships (handoff said M2) | `BeatResult.beat_phase` + render-side PLL (`audio/interp.rs`) |
| `bpm` | ships (handoff said M2) | Kalman tempo. Field is normalized bpm/300 — **use `raw_bpm()`** (bug #2054) |
| `bass`/`mid`/`high` | trivial derivation | aggregate the 7 named bands (`analyzer.rs::bands`) |
| 32 log-spaced smoothed bands | absent | nearest: 64-mel side array on `AudioFrame` |

**Delta to build (all CPU-side, zero ABI churn):** `trama/audio.rs` assembles a
per-frame view from `AudioSystem::latest_features(dt)` + `latest_mel()` on the
render thread. bass/mid/high = means over (sub_bass,bass) / (low_mid,mid,upper_mid)
/ (presence,brilliance). 32 bands = adjacent-pair mean of the 64 mel bands, then a
per-band dt-correct asymmetric one-pole (fast attack, slow release) into a fixed
`[f32; 32]`. The smoothing template is `audio/smoother.rs::FeatureSmoother`
(`1 − exp(−dt/τ)`); the bindings `Smooth { factor }` transform is frame-rate
dependent and must **not** be copied. Mel spacing is mel-scale rather than strictly
log — VJ-grade fine, flagged in §9.4.

Hard constraints honored: `AudioFeatures` is a frozen ABI (332 bytes, layout guard +
golden vectors; growth = batched append-only bump). The mel/dmfcc side-array
precedent is the pattern trama copies. Trama must never touch
`AudioSystem::frame_receiver()` — it is a single-consumer channel owned by the
signal subsystem.

**GPU-side:** no new bindings needed. Every trama node's uniforms already carry all
83 features by name (see §5), and the preamble's `audio_spectrum` /
`audio_spectrogram` textures cover raw-spectrum access. Trama shaders end up *more*
audio-capable than the handoff's Globals specified.

## 4. Existing visuals as sources (handoff §4.4)

Every visual already renders to a `RenderTarget`, so all 58 `.pfx` effects are
wrappable in principle via an embedded `PassExecutor` (a later node kind, not M0).
Cheapest first wraps: **`aurora`** and **`tunnel`** — single-fragment, no particles,
no feedback. `MediaLayer` is a ready-made image/GIF/video source node. The
`Compositor` (13 blend modes incl. displacement) is a ready-made Mix. The 34
particle-backed effects each own large GPU buffers — defer past M4 except one
showcase. The 7 overlay-family effects (`alpha: true`) are natural effect-side
candidates since they already emit meaningful alpha.

## 5. Shader loading today (handoff §4.5)

Production path is runtime disk load: `EffectLoader` resolves under `assets_dir()`,
reads WGSL per pass, and **prepends** the contract preamble via `prepend_library` —
the 448-byte `PhosphorUniforms` ABI v3 struct (400 at v3's freeze; batched appends
since — band-pan, overlay clock — grew it per the documented policy), the shared
`lib/` sources, and
generated `input{i}()` wrappers — unless the file declares its own struct
(comment-aware check, #1855). The ABI is **one bind group**: binding 0 uniforms
(incl. `params: array<vec4f,4>` + `param(i)`), 1/2 previous-frame texture+sampler
(feedback), 3–6 audio textures + sampler, inputs at 7+2i/8+2i. 139 shaders, the
in-app editor (`ui/panels/shader_editor.rs`), and the off-thread `ShaderCompiler`
(50–500 ms compiles kept off the render thread) all assume it.

Validation today is implicit: `ShaderPipeline::new` wraps module + pipeline creation
in wgpu error scopes and returns `Err` on validation failure — the handoff's I4
last-good mechanism already ships. naga 27.0.3 is in-tree transitively; I5's
*pre-pipeline* front-end validation needs a direct `naga` dep (M0+).

Hot reload ships: `ShaderWatcher` (`shader/hot_reload.rs`, notify-debouncer-mini,
100 ms, separate `.wgsl`/`.pfx` channels) + `.pfx` structural diff → targeted
rebuild. And `ParamStore::merge_from_defs` (`params/store.rs`) already implements
the handoff §12 manifest-merge rule exactly (name-keyed, type-checked, new→defaults,
removed→dropped).

**Implication — the decided ABI ruling (§9.0):** trama adopts ABI v3 and the
existing loader/preamble/editor/compiler stack. The handoff's §7 three-group
contract and §8 index-based manifest are retired; trama manifests use the typed
`ParamDef` format (declaration order = packing order). Trama effect files live in
`assets/trama/effects/` — a separate root, because `assets/effects/` holds `.pfx`
JSON under a different schema and is already watched, and `assets/shaders/` would
make trama files indistinguishable from the 139 pass shaders.

**What trama's executor builds new** (mining, not modifying, `PassExecutor`'s
idioms — prev-frame parity flip, `@`-prefixed special inputs, hard error on unknown
names): dynamic graph topology + validation + topo sort; per-node params as one
arena uniform buffer (512-byte stride, static-offset per-node bind groups — bind
groups are per-node anyway because inputs differ); the texture pool; per-node
previews; bypass aliasing. `PassExecutor` itself stays untouched: it is per-layer,
static-topology, per-layer-params — retrofitting it is the drive-by refactor the
handoff §15 forbids.

## 6. Dependencies (handoff §4.6)

From `Cargo.toml`/`Cargo.lock` (read directly): wgpu 27.0.1 (wgpu-core/naga
27.0.3), winit 0.30.13, egui/egui-wgpu/egui-winit 0.33.3, notify 8.2.0 +
notify-debouncer-mini 0.6.0, serde 1.0.228 / serde_json 1.0.149, egui_code_editor
0.2.21, wgpu-profiler 0.25 (optional). Edition 2024, rust 1.90, clippy pedantic
`-D warnings`, cargo-deny license gate.

**New deps:** `egui-snarl` (absent today — see risk in §8/M0) and a direct `naga`
for I5. Both additions wait until no benchmark session holds the cargo/target lock.

## 7. Handoff §14 defaults scorecard

| # | Default | Verdict |
|---|---|---|
| D1 | 32 params/node | **flips → 16** (ABI v3 `params` is 4×vec4; escape = batched v4 append, precedent #1629) |
| D2 | 32 log bands | **half-flips** — 32 bands ship, derived from 64 mel (mel-spaced, not strictly log) |
| D3 | rgba16float linear | survives — already the house format |
| D4 | 192×108 previews, round-robin /3 | survives |
| D5 | max 2 texture inputs | survives (ABI supports N; trama caps at 2 per spec) |
| D6 | preamble prepended by loader | survives — **already implemented** (`prepend_library`) |
| D7 | `trama` module, `.fio.json` | survives (names still provisional per handoff header) |
| D8 | one modulation slot per param | survives |
| §7 | 3-bind-group contract | **flips → ABI v3 single group** (decided, §9.0) |
| §8 | index-based JSON manifest | **flips → typed `ParamDef` manifest** |
| §13 | beat sync in M2 | **moves to M1** — beat_phase/bpm already ship |

## 8. Milestones, redrawn (gate structure preserved)

Removed everywhere (already shipped): egui integration; beat tracking (phase, bpm,
onset + decay); feedback primitive (`PingPongTarget` + parity); hot-reload watcher +
off-thread compile; Appendix-A.1 fallback editor; §12 manifest-merge.

- **M0 — Spike (go/no-go, timebox unchanged):** `src/trama/` graph model +
  validation + topo sort; scene-level executor (uniform arena, per-node bind
  groups, texture pool); registry loading trama-manifest `.wgsl` from
  `assets/trama/effects/` (3 built-ins: `noise_field`, `hue_drift`, Output);
  `execute_and_composite` seam + `RenderMode`; egui-snarl canvas. Params at
  manifest defaults. Accept criteria unchanged (60 fps @1080p, live rewire, unit
  tests for topo/cycle/manifest). **Risk (retired 2026-08-07):** egui-snarl 0.9.0
  declares `egui = "^0.33"` (verified on crates.io) — pin `0.9`; 0.10/0.11 target
  egui 0.34/0.35 (+ Rust 1.92). Residual check stays: a single `egui` entry in
  `Cargo.lock` after adding. **Fallback:** canvas-less
  M0 (minimal list panel or shader_editor host); the go/no-go then judges the
  executor + triple, which is the actual bet.
- **M1 — Modulation Triple:** resolution rule + dt-correct smoothing; oscillators
  **including `BeatSync` rates** (pulled from M2); `AudioFeature` sources incl.
  `BeatPhase`/`Bpm`; the 32-band derivation (§3); inspector panel + ghost-value
  indicator. The dynamic-offset-UBO line item is deleted — the M0 arena is the
  params buffer.
- **M2 — Feedback, Previews, Blend:** Feedback node, `mix`, `transform`; throttled
  round-robin previews (`egui-wgpu` `register_native_texture`); I8 zero-steady-state
  -alloc verification; **first real `wgpu-profiler` scopes** (plumbed today, zero
  scopes — trama per-node scopes are I7's measurement story; no criterion infra
  exists); `.pfx`-wrapping node spike (pulled from M4).
- **M3 — Persistence + Hot Reload:** `.fio.json` per house policy
  (`#[serde(default)]`, preserve-unknown à la `BindingTarget::Unknown`,
  missing-effect placeholder); extend `ShaderWatcher` roots to trama dirs; last-good
  via the shipped error-scope pattern.
- **M4 — Library + Authoring:** ~15 effects; scaffold via a `--new-effect` CLI flag
  (no xtask crate exists; the `main.rs` flag precedent is smaller); wrapped sources
  (`aurora`, `tunnel`; `MediaLayer` as video source).

## 9. Sign-off sheet

**9.0 — DECIDED (owner, 2026-08-07):** trama adopts shader ABI v3 (single bind
group, `PhosphorUniforms`, prepended preamble); §7 contract retired.

**All seven ruled YES (owner, 2026-08-07).** Kept for the record:

1. Seam = `execute_and_composite` + per-scene `RenderMode { Layers, Trama }`
   (escape hatch: later `LayerContent::Graph`). — §2
2. 16 param slots per node in v1; batched ABI-v4 append if it pinches. — §7 D1
3. 32 bands derived from 64-mel adjacent pairs (mel-spaced, not strictly log),
   modulation-only, zero ABI churn. — §3
4. Trama manifests use the typed `ParamDef` format, not §8's index-based JSON. — §5
5. egui-snarl: authorize the canvas-less M0 fallback now, so a missing
   egui-0.33-compatible release does not stall the spike. — §8
6. Milestone re-plan as §8 (beat sync → M1; profiler scopes + `.pfx`-wrap spike →
   M2; scaffold as CLI flag).
7. Trama `Modulation` stays separate from `BindingBus` in v1 (the bus keeps routing
   `audio.*`/MIDI/OSC → layer params; unification is a real future question,
   deliberately deferred — one line in DECISIONS.md).

## 10. Housekeeping status

- `docs/README.md` index rows for this doc + DECISIONS.md — done, committed
  together with this doc (first subdirectory under `docs/`; `scripts/check_docs.py`
  gates links in CI/pre-commit).
- `TASKS.md`: workstream **T** status-board row + session-log entry (local tracker,
  per session).
- Naming: "trama" and `.fio.json` confirmed by owner (2026-08-07) — no longer
  provisional.
- M0 start: add `egui-snarl` (pinned `0.9`, see §8 risk note) + direct `naga`;
  `src/trama/` skeleton.
