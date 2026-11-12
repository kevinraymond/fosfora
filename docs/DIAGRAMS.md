# Diagrams

How Fosfora is put together, drawn from four perspectives, then one diagram per flow the
engine runs. Each image has a light and a dark version; GitHub picks the one that matches
your theme. The source is `scripts/diagrams.py` (`uv run scripts/diagrams.py` regenerates
every pair into `docs/images/`).

The grammar is the same in every diagram: white boxes are stages, orange pills are what goes
in and what comes out, blue diamonds are gates and blue edges their outcomes, dashed boxes
are optional or dev-only, and a ring is a loop.

## Four perspectives

### By tier: what ships, what records, what never ships

Everything in the production band runs in every download and is the same code live, in
headless `--signal` and offline. The instrumentation band is compiled into every build but
inert until an env var names a file, and it only records. The dev band exists only behind
`--features analyze`; its one product that ships is the trained beat model.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/tiers-dark.svg">
  <img src="images/tiers-light.svg" alt="Fosfora by tier: the production chain in every build, the sidecars and logs that only record, and the dev tooling behind the analyze feature whose trained model ships back into the chain." width="920">
</picture>

### At runtime: threads and channels

The windowed app. Input threads feed the main thread through channels, the main thread
renders one frame per loop, and readback feeds the sink and recording threads. There is no
mutex in the hot path.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/runtime-dark.svg">
  <img src="images/runtime-light.svg" alt="Fosfora at runtime: MIDI, OSC, web, capture, file-watcher and depth threads feed the main thread; it drains inputs, applies bindings, renders, composites, post-processes, draws egui and presents; readback feeds sink and recording threads." width="900">
</picture>

### Data flow: in and out

The analysis has two consumers: Signal, the OSC telemetry a rig listens to, and the bindings
that drive the visuals. Controls, presets and media enter on the left; the window and the
sinks are the outputs.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/dataflow-dark.svg">
  <img src="images/dataflow-light.svg" alt="Fosfora's data flow: audio, controls, presets and media in; analysis to Signal and to bindings, params, layers, compositor and the outputs; the app's own telemetry over OSC and to web clients." width="900">
</picture>

### Builds: one crate, several feature sets

The `release` feature set is what a download ships, and that one binary is the app, the
headless `--signal` mode and `--render-loop`. `link` is opt-in and makes the binary GPL.
`analyze` adds the offline CLI the bench and CI use. The lib with no default features is
the XR frontend's base.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/builds-dark.svg">
  <img src="images/builds-light.svg" alt="Fosfora's builds: the release feature set as the download, link as an opt-in GPL build, analyze as the dev build, and the lib alone under fosfora-xr." width="900">
</picture>

## One diagram per flow

### The hop: 86 times a second

The per-hop chain in the order the code runs it, from the 512-sample hop to the frame of 85
features and events. The dashed taps are the sidecars.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/hop-dark.svg">
  <img src="images/hop-light.svg" alt="The per-hop chain: DC strip, multi-resolution FFT, loudness, stereo field, HPSS, pitch, contrast, timbre, kick envelope, key, snapshot, normalize, beat detector, downbeat, structure, smooth, frame; sidecars tap key, beat, hop evidence, downbeat and structure." width="900">
</picture>

### The beat path

The onset flux earns the tempo through the seed gate, the activation chooses the metrical
level once seeded, and the grid runs as a loop per instant: emit 10 ms early, settle, correct
the phase, fold the evidence, vote. Three votes in a row move the grid later.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/beat-dark.svg">
  <img src="images/beat-light.svg" alt="The beat path: the seed gate, the tempo estimator, the activation, and the grid's per-instant loop of emit, settle, PLL, fold and vote, with the arbiter's shift and the kick and activation events in its window." width="900">
</picture>

### One frame

The main thread per frame: drain inputs, apply bindings, run every enabled layer through
compute, effect passes and particle render, composite, post-process, egui, present, read
back for the sinks.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/frame-dark.svg">
  <img src="images/frame-light.svg" alt="One frame: drain inputs, bindings to params, the per-layer loop of compute, effect passes and particle render, then compositor, post-process, egui overlay, present, and the window and sinks." width="900">
</picture>

### Signal: from a hop to an OSC packet

The four cadences of `--signal`: events the hop they are detected, continuous values at the
TX rate, on-change addresses re-broadcast at 1 Hz, and a 1 Hz status heartbeat. `--signal-dump`
is the same emitter on a file, which is what the bench scores.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/signal-dark.svg">
  <img src="images/signal-light.svg" alt="Signal: the hop chain feeds four lanes, events, continuous, on change and status, over UDP to a rig; --signal-dump is the same emitter on a file." width="900">
</picture>

### A bench round

The loop that ships a constant: a measured gap with its mechanism named, a frozen manifest,
an exact replica forecast, and only then arms; the tune halves decide, the holdout is read
once, and a miss is recorded rather than re-tuned.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/round-dark.svg">
  <img src="images/round-light.svg" alt="A bench round: freeze the manifest, forecast on a replica, a gate, then for each arm build, dump and verdict on tune; select by the frozen rule, read the holdout once, a gate, ship and commit; a miss goes to the on-miss rule." width="900">
</picture>

### Training the beat activation

The model is trained on the raw evidence the detectors read, dumped per hop through the hop
sidecar, and the export is golden-tested before the runtime evaluates it every hop.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/model-dark.svg">
  <img src="images/model-light.svg" alt="Training the beat activation: corpus, hop dumps, labels, dataset, train, export, golden test, runtime, activation; beatless clips as negatives." width="900">
</picture>

### Offline: the same chain driven by a file

A file is decoded and run through the hop chain faster than realtime. Three of the four
outputs are dev-only; `--render-loop` ships in every download.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/offline-dark.svg">
  <img src="images/offline-light.svg" alt="Offline modes: file, decode, the hop chain, then --analyze to a report, --signal-dump to the bench, --render-scene to a scene check, --render-loop to a loop video." width="900">
</picture>
