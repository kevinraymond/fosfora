# Fosfora — architecture notes

Phase-0 recon map for the feature program (see `TASKS.md`). Written 2026-08-06 from a
source-level survey; line numbers drift, names don't. Single crate:
`crates/phosphor-app` (rename to `fosfora-app` in progress, workstream R).

## Corrections to README/docs claims

- **83 audio features**, not 74 (`audio/features.rs::NUM_FEATURES = 83`; struct pinned
  at 332 bytes). `analyze/report.rs` comment says 81 — also stale.
- Feature rate is `sample_rate / 512`: **86.13 Hz at 44.1 kHz only** (93.75 Hz at 48 k).
- **No chord detection** (key detection only, `audio/key.rs`). README's "key and chord"
  is wrong.
- **No stem separation.** `audio/hpss.rs` is Fitzgerald median-filter HPSS masking on
  the 1024-pt spectrum → `percussive_energy`/`harmonic_energy`/`harmonic_ratio`. No
  drums/bass/melody split.
- The six React JSX panel mockups referenced by the session starter are **not in the
  repo**.

## Audio pipeline (capture → features)

```
cpal / PulseCapture / WasapiCapture callback      (no DSP in callback)
  └─ SPSC RingBuffer (65536 f32, atomics)          audio/capture.rs
"phosphor-audio" thread                            audio/mod.rs (spawn ~:408, loop ~:1175)
  ├─ FIFO-slices exactly ANALYSIS_HOP=512 samples; sample-clock timestamps (no wall clock)
  ├─ HopAnalyzer::process_hop                      audio/hop.rs — THE canonical chain:
  │    FFT (3 res: 4096/1024/512, Hann) → loudness → stereo → HPSS → pitch(YIN) →
  │    contrast → dMFCC → kick → key → pre-norm snapshot → normalize → beat →
  │    downbeat → structure → smooth → AudioFrame
  └─ crossbeam bounded(4) try_send (drop-on-full) of AudioFrame
Render thread: AudioSystem::latest_features(dt) — drain-to-newest + FeatureInterpolator
  (audio/interp.rs: per-slot Lerp/Hold, beat/bar phase PLLs, 1.5-hop delay playhead)
```

- `AudioFrame` (`audio/mod.rs`): `features: AudioFeatures` (83 × f32, `#[repr(C)]`),
  `spectrum` (512 log bins), `mel` (64), `dmfcc` (13), `timestamp` (sample clock),
  `bar_duration`.
- **Pulse latching**: 1-frame pulses (beat/downbeat/drop) survive channel drops via
  `Arc<AtomicU32>` counters — `PulseCounts` / `AudioSystem::pulse_counts()`.
- Live-tunable config: `Arc<Mutex<TempoControl>>` + `Arc<Mutex<StructureConfig>>`,
  locked once per hop. No mutex otherwise in the hot path.

**Sources of truth** (must agree; layout-guard tests exist):
- `audio/features.rs::AudioFeatures` — the ABI. **Treat as frozen**: golden-vector test
  (`audio/mod.rs` tests: `GOLDEN_HOPS`, `golden_signal()` synthetic generator) pins
  values at 1e-5; `analyze/mod.rs` asserts the offline driver matches the same vector —
  the live/offline equivalence guarantee. Appending fields = ABI + golden churn (the
  bar_index/beat_index v4 append is the precedent if ever needed).
- `audio/schema.rs::FEATURES` — ordered `[FeatureDef; 83]` with string names
  (`"sub_bass"` … `"beat_index"`) + norm/smooth/decay/interp policies. **The name
  source for any external bus** (doc comment reserves it for exactly that).
- `bindings/sources.rs::collect_audio` — binding ids (`audio.band.0`, `audio.mfcc.3`,
  `audio.key_hue`…), a third naming scheme; `--dump-schema` emits the real list.

Detector notes: beat = 64-band SuperFlux → autocorrelation tempo (log-Gaussian prior,
Kalman in log2-BPM) → scheduler on the sample clock (`audio/beat.rs`). Downbeat =
accent-contrast scoring over a 16-beat ring, meter ∈ {3,4}, hysteresis, ~70-80% on 4/4
EDM (`audio/downbeat.rs`). Key = Krumhansl-Kessler over 12-s EMA CQT chroma with
hysteresis (`audio/key.rs`). Structure (`audio/structure.rs`): `section_novelty`
(Foote checkerboard, ~3 s causal lag), `buildup` (logistic over loudness/centroid/
onset-density rise + sub-bass withdrawal), `drop` (armed by sustained buildup, fired on
loudness jump + sub-bass return, 16 s refractory) — reads the **pre-norm** snapshot.
Offline segmenter with lookahead: `analyze/structure_offline.rs`.

## Threading & headless viability

Analysis has **zero** GPU/winit imports. `AudioSystem::new_with_device(...)` is fully
standalone. The coupling is `App` (`app.rs`): `App::new(window)` builds `GpuContext`
(surface field non-optional) then owns every subsystem; the pump cadence is the redraw
loop. Existing no-window CLI early-exits in `main.rs` before `EventLoop::new()`:
`--audio-test`, `--render-loop` (release), `--analyze`, `--dump-schema`,
`--render-scene`, `--validate` (feature `analyze`). `src/headless/` is offline
*rendering* (windowless GPU via `headless/gpu.rs`), not analysis broadcast.

**The render-thread OSC problem**: OSC TX happens in `App::render()` → `send_state`,
rate-limited to `tx_rate_hz` (30) — one-frame pulses mostly never reach the wire, hence
the `*_count` addresses. A headless emitter that is the *sole* consumer of the frame
channel sees every hop and fixes this by construction.

## I/O + config

- **OSC** (`src/osc/`, rosc): RX thread + fire-and-forget UDP sender. Namespace
  `/phosphor/` (guard at `receiver.rs`; → dual-prefix in R2). Config `osc.json`
  (rx 9000, tx 9001, tx_rate 30, learn maps).
- **MIDI** (`src/midi/`, midir, patched for alsa 0.11): **input only** — CC/note +
  clock IN (24 ppqn, `midi/clock.rs`). No MIDI output exists; clock/note/CC out is
  greenfield (`midi/output.rs`, Signal phase A2).
- **Bindings** (`src/bindings/`): dotted string sources → typed `BindingTarget`;
  `BindingBus::evaluate` once per frame; persistence `global-bindings.json` +
  per-preset sidecars (version field, no migration code).
- **Web** (`src/web/`, tungstenite, no tokio): HTTP + WS on 9002, 10 Hz audio snapshot,
  no auth/roles. Role-scoping seams: `server.rs` route match; `run_client` client_id;
  `WebSystem::clients` vec; `parse_client_message` gating.
- **Config layer**: every subsystem = `src/<mod>/types.rs` with `XConfig` +
  `config_path()/load()/save()` → own JSON under `dirs::config_dir()/phosphor/`
  (17 sites; → single `paths::config_root()` in R1). `version` fields exist, nothing
  reads them; forward-compat is `#[serde(default)]` + `unwrap_or_default()`.
- **Outputs** (NDI/Spout/Syphon/v4l2): one pattern — `FrameSink` trait
  (`output/sink.rs`, deliberately not Send; sink constructed inside its sender thread)
  + `OutputPipeline` (capture target, bounded channel, health) + thin
  `XSystem{config,pipeline}` + panel + cargo feature. Art-Net (G) follows this.

## Offline analysis (Workstream C's foundation)

`src/analyze/` (feature `analyze`): symphonia decode (whole-file, no resample) →
`drive_with(&DecodedAudio, FnMut(hop, ts, &HopOutput))` — same 512-slicing, same code
path as live (golden-proven). `HopStream` = timestamped features + beat/downbeat/drop
event lists. `--analyze` emits `analysis.json` (v1). Missing for the harness: JSONL
event log (Signal A8 provides the contract), annotation scoring, datasets. **No
criterion/bench infra exists anywhere**; CI = fmt/clippy/test × 6 feature combos +
demos + deny; golden-loop GPU gate is dev-run only.

## UI

Raw winit+wgpu+egui (no eframe). Layout entirely in `ui/panels/mod.rs::draw_panels`
(27 positional args — collapse to a context struct before E): left/right SidePanels
`exact_width(315)`, bottom status bar, timeline strip. Panels are pure
`fn draw_x(ui, &Info)`; reads via DTO structs built in `main.rs`, writes via
`insert_temp`/`remove_temp` command drain after `end_frame()` — **panels are
relocatable**; only `panels/mod.rs` knows geometry. Widgets: `section`/`subsection`/
`SliderRow`/`combo_row` etc. (`ui/widgets/`), persistent-id collapsing state (survives
re-parent). Themes: 6 modes (`ui/theme/`), tokens in `theme/tokens.rs`. No workspace/
docking/tab system exists; `ThemeMode` is the template for a `layout_mode` setting.
Adding a settings subsection = panel fn + subsection hook in `mod.rs` + settings field
+ drain block in `main.rs`.

## Integration points per workstream

- **A Signal**: third driver for `HopAnalyzer` output — consume the frame channel
  directly (new `AudioSystem::frame_receiver()`), no interpolator; emit via a sink
  trait (UDP live / JSONL offline through `analyze::drive_with`). Names from
  `audio/schema.rs`. Section + phrase state live in the signal layer — no ABI change.
- **B Link**: greenfield dep; feeds `TempoControl` mailbox + scene advance; addresses
  reserved `/fosfora/v1/link/*`.
- **C harness**: extend `src/analyze/`; score Signal's JSONL; datasets out-of-repo.
- **D tiers**: criterion first; `SectionEstimator::tier()` + `/status/tier` already the
  reporting hook; governor sheds T2 (HPSS/key/structure) on deadline misses.
- **E UI**: context-struct refactor → workspaces behind `layout_mode`.
- **F FFGL**: naga GLSL-out spike on simplest `.pfx` shader; params from `ParamDef`.
- **G Art-Net**: `FrameSink` + grid sampling on sender thread.
