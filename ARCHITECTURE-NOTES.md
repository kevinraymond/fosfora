# Fosfora — architecture notes

Source-level map of the engine. First written 2026-08-06 as the phase-0 survey for the
feature program, refreshed 2026-10-10; line numbers drift, names don't.

## Workspace and crates

- `crates/fosfora-app` — the engine as a library (`fosfora_app`, `src/lib.rs`) plus the
  `fosfora` desktop binary (`src/main.rs`), a thin shell over it.
- `xtask` — repo chores via the `cargo xtask` alias (`.cargo/config.toml`); currently only
  `new-effect`, which scaffolds a trama effect.
- `bench/` — Python (uv) accuracy harness, not a crate. See [Offline analysis and
  benchmarks](#offline-analysis-and-benchmarks).

**Lib/bin split.** The default `desktop` feature gates everything that needs a desktop OS:
winit, egui-winit and the editor widgets, file dialogs, MIDI, the web remote, file
watching, HTTP downloads. The `fosfora` binary has `required-features = ["desktop"]`, and
`lib.rs` gates `app`, `ui`, `osc`, `midi`, `web` and friends at their `mod` declarations
(without `desktop`, only `ui::theme` remains, because settings persist a `ThemeMode`).
Built with `--no-default-features` (plus `decode` for song-file decoding), the lib
compiles for `aarch64-linux-android`, which is what lets a separate XR frontend consume
the engine without winit. The `xr-lib` CI job clippy-checks exactly those two builds.

## Corrections to README/docs claims

- **85 audio features** (`audio/features.rs::NUM_FEATURES`; struct pinned at 340 bytes).
  The README is now right; `docs/TECHNICAL.md` still says 74 and an `analyze/report.rs`
  doc comment says 83.
- Feature rate is `analysis_rate / 512`: **86.13 Hz at 44.1 kHz** (93.75 Hz at 48 k).
  Input above 88.2 kHz is decimated by powers of two into 44.1–88.2 kHz first
  (`audio/decimate.rs`), so the chain only ever sees the rates it was tuned at.
- **No chord detection** (key detection only, `audio/key.rs`). Chroma change is used
  internally as a downbeat cue, nothing more.
- **No stem separation.** `audio/hpss.rs` is Fitzgerald median-filter HPSS masking on
  the 1024-pt spectrum → `percussive_energy`/`harmonic_energy`/`harmonic_ratio`. No
  drums/bass/melody split; Signal's "stem" addresses are proxies built on these.

## Audio pipeline (capture → features)

```
cpal / PulseCapture / WasapiCapture callback      (no DSP in callback)
  └─ SPSC RingBuffer (65536 slots, AtomicU32)      audio/capture.rs
"fosfora-audio" thread                             audio/mod.rs (audio_thread)
  ├─ decimate to the analysis rate                 audio/decimate.rs
  ├─ FIFO-slices exactly ANALYSIS_HOP=512 samples; sample-clock timestamps (no wall clock)
  ├─ HopAnalyzer::process_hop                      audio/hop.rs — THE canonical chain:
  │    DC strip → FFT (3 res: 4096/1024/512) → loudness → stereo → HPSS → pitch(YIN) →
  │    contrast → dMFCC → kick → key → pre-norm snapshot → normalize → beat →
  │    downbeat → structure → smooth → AudioFrame
  └─ drop-oldest bounded(4) queue of AudioFrame    inbound.rs
Render thread: AudioSystem::latest_features(dt) — drain-to-newest + FeatureInterpolator
  (audio/interp.rs: per-slot interp policy, beat/bar phase PLLs, 1.5-hop delay playhead)
```

- `AudioFrame` (`audio/mod.rs`): `features: AudioFeatures` (85 × f32, `#[repr(C)]`),
  `spectrum` (512 log bins), `mel` (64), `dmfcc` (13), `timestamp` (sample clock),
  `phase_frozen`, `bar_duration`.
- **Pulse latching**: 1-frame pulses (beat/downbeat/drop) survive queue overflow via
  `Arc<AtomicU32>` counters — `PulseCounts` / `AudioSystem::pulse_counts()`.
- Live-tunable config: `Arc<Mutex<TempoControl>>` + `Arc<Mutex<StructureConfig>>`,
  **try-locked** once per hop (`try_lock_now`): on contention the hop keeps the previous
  snapshot, so a UI panel holding the lock never stalls analysis. No other lock in the
  hot path.

**Sources of truth** (must agree; layout-guard tests exist):
- `audio/features.rs::AudioFeatures` — the ABI. **Treat as frozen**: golden-vector test
  (`audio/mod.rs` tests: `GOLDEN_HOPS`, `golden_signal()` synthetic generator) pins
  values at 1e-5; `analyze/mod.rs` asserts the offline driver matches the same vector —
  the live/offline equivalence guarantee. Appending fields = ABI + golden churn (the
  v4 `bar_index`/`beat_index` and tempo-trust appends are the precedent).
- `audio/schema.rs::FEATURES` — ordered `[FeatureDef; NUM_FEATURES]` with string names
  (`"sub_bass"` … `"beat_locked"`) + norm/smooth/decay/interp policies. **The name
  source for any external bus** (doc comment reserves it for exactly that).
- `bindings/sources.rs::collect_audio` — binding ids (`audio.band.0`, `audio.mfcc.3`,
  `audio.key_hue`…), a third naming scheme; `--dump-schema` emits the real list.

Detector notes: beat = 64-band SuperFlux → autocorrelation tempo (log-Gaussian prior,
Kalman in log2-BPM) → scheduler on the sample clock (`audio/beat.rs`). Downbeat =
accent-contrast scoring over a 16-beat ring, meter ∈ {3,4}, hysteresis, ~70-80% on 4/4
EDM (`audio/downbeat.rs`). Key = profile correlation (Faraldo "braw" profiles) over a
~20 s energy-weighted rolling mean of CQT chroma, with hysteresis (`audio/key.rs`).
Structure (`audio/structure.rs`): `section_novelty` (Foote checkerboard, ~3 s causal
lag), `buildup` (logistic over loudness/centroid/onset-density rise + sub-bass
withdrawal), `drop` (armed by sustained buildup, fired on loudness jump + sub-bass
return, 16 s refractory) — reads the **pre-norm** snapshot. Offline segmenter with
lookahead: `analyze/structure_offline.rs`.

## Threading & headless viability

Analysis has **zero** GPU/winit imports. `AudioSystem::new_with_device(...)` is fully
standalone. The coupling is `App` (`app.rs`): `App::new(window)` builds `GpuContext`
(surface field non-optional) then owns every subsystem; the pump cadence is the redraw
loop. No-window CLI paths early-exit in `main.rs` before `EventLoop::new()`:
`--audio-test`, `--signal` and `--render-loop` in every build; `--analyze`,
`--signal-dump`, `--dump-schema`, `--render-scene` and `--validate` with feature
`analyze`. `src/headless/` is offline *rendering* (windowless GPU via `headless/gpu.rs`,
scene renders and loop export), not analysis broadcast.

**OSC TX vs. one-frame pulses**: the app's OSC TX happens in `App::update()` →
`OscSystem::send_state`, rate-limited to `tx_rate_hz` (30) — one-frame pulses mostly
never reach the wire, hence the `*_count` addresses. `--signal` (`src/signal/`) avoids
this by construction: it is the *sole* consumer of `AudioSystem::frame_receiver()` and
sees every hop.

## I/O + config

- **OSC** (`src/osc/`, rosc): RX thread + fire-and-forget UDP sender. Namespace
  `/fosfora/`; the pre-rename `/phosphor/` prefix is still accepted (`receiver.rs`).
  Config `osc.json` (rx 9000, tx 9001, tx_rate 30, learn maps).
- **Signal** (`src/signal/`): headless analysis broadcast over versioned
  `/fosfora/v1/...` OSC addresses; wire contract in `docs/SIGNAL.md` and
  `signal/schema.rs`. `--signal-dump` drives the same emitter offline to JSONL.
- **MIDI** (`src/midi/`, midir): **input only** — CC/note + clock IN (24 ppqn,
  `midi/clock.rs`). No MIDI output exists.
- **Bindings** (`src/bindings/`): dotted string sources → typed `BindingTarget`;
  `BindingBus::evaluate` once per frame; persistence `global-bindings.json` +
  per-preset `<name>.bindings.json` sidecars (version field, never read;
  `migration.rs` is a one-time import of legacy MIDI/OSC mappings, not a schema
  migration).
- **Web** (`src/web/`, tungstenite, no tokio): HTTP + WS on 9002, 10 Hz audio snapshot.
  Other devices must present a shared access key (`WebConfig::access_key`); no
  per-client roles. Role-scoping seams: `server.rs` route match; `run_client`
  client_id; `WebSystem::clients` vec; `parse_client_message` gating.
- **Inbound queues** (`inbound.rs`): OSC, web, MIDI and audio feed the render thread
  through bounded queues that drop the *oldest* message when full, so the newest value
  always arrives after a render stall.
- **Config layer**: each subsystem has `src/<mod>/types.rs` with `XConfig` +
  `config_path()/load()/save()` → own JSON under `paths::config_root()`
  (`<config dir>/fosfora/`; a pre-rename `phosphor/` dir is moved over at launch).
  `version` fields exist, nothing reads them; forward-compat is `#[serde(default)]` +
  `unwrap_or_default()`.
- **Outputs** (NDI/Spout/Syphon/v4l2): one pattern — `FrameSink` trait
  (`output/sink.rs`, deliberately not Send; sink constructed inside its sender thread)
  + `OutputPipeline` (capture target, bounded channel, health) + thin
  `XSystem{config,pipeline}` + panel + cargo feature. A new sink (Art-Net, say)
  follows this.
- **Link** (`src/link/`, feature `link`, GPL — never in `release`): Ableton Link via
  `rusty_link`. Follow mode pins the beat tracker's tempo prior to the session tempo
  (beat phase still comes from the audio); Lead mode commits the detected BPM once it is
  stable. Beat-grid crossings also feed the scene timeline.

## trama (node graph)

`src/trama/` is the node-graph effect-chain system: a graph model, a manifest registry of
WGSL effect nodes (`assets/trama/effects/`, validated with naga before pipeline
creation), and an executor behind `gpu::frame_graph::execute_and_composite`. Each layer
carries its own chain (`gpu::layer::LayerChain`); `TramaSystem` (owned by `App`) holds
the shared registry, executor, modulation and the master chain that post-processes the
composite. The canvas is egui-snarl and desktop-only (`trama/ui_headless.rs` stands in
without `desktop`). Design record: `docs/trama/INTEGRATION.md`, `docs/trama/DECISIONS.md`.

## Offline analysis and benchmarks

`src/analyze/` (feature `analyze`): symphonia decode (`decode` feature, whole-file, same
decimation as live, no other resampling) → `drive_with(&DecodedAudio, FnMut(hop, ts,
&HopOutput))` — same 512-slicing, same code path as live (golden-proven). `HopStream` =
timestamped features + beat/downbeat/drop event lists. `--analyze` emits
`analysis.json` (v1).

`bench/` is the accuracy harness: uv PEP-723 scripts that stream audio through the live
path (`--signal-dump` JSONL) and score it against annotated datasets fetched out-of-repo
(`bench/README.md`, results in `docs/BENCHMARKS.md`). CI's `bench-fixture` job builds
`--features analyze`, runs `test_benchlib.py`, then `make_fixture.py` → `check_fixture.py`
against floor expectations. There is no criterion/performance bench.

CI (`.github/workflows/ci.yml`): fmt; clippy per feature set (default, each optional
feature, the output sinks together, `release`, `profiling`); `cargo test` on defaults and
on the union of optional features; the `xr-lib` Android clippy; Miri on the capture
ring; the bench fixture; a 4-target build matrix (Linux, macOS arm64/x86_64, Windows);
demo-preset checks; cargo-deny plus a check that GPL `rusty_link` is absent from
`release`. GPU renders (golden loops) stay dev-run.

## UI

Raw winit+wgpu+egui (no eframe). Since v2.0 the layout is the workspace shell,
`ui/shell.rs::draw_shell`: a top bar switches between three workspaces (Perform, Build,
Setup), the output is a preview rather than the backdrop, and everything the shell draws
arrives in one `ShellState` struct built in `main.rs`. Setup's pages live in `ui/setup/`.
Per-section draw functions stay in `ui/panels/` as pure `fn draw_x(ui, &Info)`; reads
via DTO structs built in `main.rs`, writes via `insert_temp`/`remove_temp` request drain
after `end_frame()` — **panels are relocatable**; only the shell knows geometry.
Widgets: `section`/`subsection`/`ParamRow`/`combo_row` etc. (`ui/widgets/`),
persistent-id collapsing state (survives re-parent). Themes (`ui/theme/`): four
built-in `ThemeMode`s plus custom token files, tokens in `theme/tokens.rs`. Adding a
setting = panel fn + a place for it in the shell or a Setup page + settings field + drain
block in `main.rs`.

## Workstream status

- **A Signal**: landed — `src/signal/`, `--signal` / `--signal-dump`, consuming
  `AudioSystem::frame_receiver()` directly with no interpolator. Section + phrase state
  live in the signal layer — no ABI change.
- **B Link**: landed — `src/link/`, addresses `/fosfora/v1/link/*`.
- **C harness**: landed — `bench/` scores Signal's JSONL; datasets out-of-repo.
- **D tiers**: `SectionEstimator::tier()` + `/fosfora/v1/status/tier` are the reporting
  hook; no governor or criterion benches yet.
- **E UI**: landed as the v2.0 workspace shell (`ShellState` replaced the old
  positional-argument `draw_panels`).
- **F FFGL**: not started (naga GLSL-out spike on simplest `.pfx` shader; params from
  `ParamDef`).
- **G Art-Net**: not started (`FrameSink` + grid sampling on sender thread).
