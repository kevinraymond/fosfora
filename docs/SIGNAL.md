# Signal — the analysis engine as a broadcast

Fosfora's audio analysis, running headless — no window, no GPU — broadcasting what it
hears over versioned OSC for whatever runs your rig: TouchDesigner, Resolume, VDMX,
QLC+, grandMA, Chataigne, or anything else that speaks OSC.

```
fosfora --signal                          # broadcast to 127.0.0.1:9010
fosfora --signal --host 10.0.0.20 --port 9010 --rate 30
fosfora --signal --feat-bus               # add the raw 83-feature bus
fosfora --signal --device "BlackHole 2ch" # pick an input by name
```

Defaults persist in `~/.config/fosfora/signal.json`; CLI flags override for the run
without saving. The input device, band scale and detector tuning come from your saved
app settings, so the headless broadcast hears exactly what the windowed app heard.

Signal **informs, it never triggers**: it is telemetry for the operator's rig. Every
stateful address carries a confidence value — your rig decides what to do with it, and
is free to ignore it.

## Emission semantics

Three cadences, one clock (the audio sample clock — no wall time in the pipeline):

- **Events** fire the hop they are detected — within ~11.6 ms of the audio at 44.1 kHz
  (one 512-sample hop; 86 Hz analysis rate, `sample_rate / 512`). Event messages carry
  a **running count** as their argument, so a receiver can detect its own datagram loss
  from a gap in the count.
- **Continuous** values are decimated to the configured TX rate (default 30 Hz).
- **On-change** addresses (section, key, phrase length) are sent the moment they
  change, and re-broadcast at 1 Hz so late-joining receivers converge.

Status is a 1 Hz heartbeat. A clean shutdown sends `/status/online 0`; a killed
process cannot, so treat more than ~3 s of status silence as offline.

## Address reference — `/fosfora/v1/`

### Events (immediate; int args are running counts)

| Address | Args | Meaning |
|---|---|---|
| `/fosfora/v1/beat` | int count | A beat fired |
| `/fosfora/v1/downbeat` | int count | A bar started (the "one") |
| `/fosfora/v1/drop` | int count | Drop detected (armed by a sustained build, fired on the loudness jump + sub-bass return) |
| `/fosfora/v1/onset` | float strength | Note/hit onset (derived rising edge of the onset envelope) |
| `/fosfora/v1/stem/drums/onset` | float strength | Kick-band onset (30–120 Hz flux crossing, hysteresis) |
| `/fosfora/v1/section/boundary` | float conf, float age_s | A section boundary was confirmed `age_s` **ago** — see below |

### Continuous (at TX rate)

| Address | Args | Meaning |
|---|---|---|
| `/fosfora/v1/bpm` | float | Real BPM (not normalized) |
| `/fosfora/v1/bar_phase` | float 0..1 | Sawtooth over the current bar, hop-rate sampled |
| `/fosfora/v1/build` | float 0..1 | Build/riser tension estimate |
| `/fosfora/v1/energy` | float 0..1 | Short-term perceptual loudness (device-independent) |
| `/fosfora/v1/stem/drums/energy` | float 0..1 | Percussive energy — **proxy**, see below |
| `/fosfora/v1/stem/bass/energy` | float 0..1 | Mean of the sub-bass + bass bands — **proxy** |
| `/fosfora/v1/stem/melody/energy` | float 0..1 | Harmonic (sustained/pitched) energy — **proxy** |
| `/fosfora/v1/phrase/bar` | int | Bar within the phrase, 1-based |
| `/fosfora/v1/phrase/beats_left` | int | Whole beats until the next phrase boundary (4/4 assumption) |
| `/fosfora/v1/predict/drop` | float 0..1 | Drop likelihood, designed to rise **before** the drop lands — lead time, not detection |

`predict/drop` has three calibrated regimes: **below 0.5** is tension telemetry
(no committed build — never treat it as a warning); **0.5 and above** means a
build has sustained long enough to commit, with a drop-scale landing expected
within roughly 8 bars; **0.8 and above** adds imminence evidence (sub-bass
withdrawal, kick gap, phrase boundary) and is the tier to act on. The value is
monotone within a build and collapses when the drop fires or the build fails.
It is likelihood-*ordered*, not a calibrated probability — measured per-tier
precision lives in `docs/BENCHMARKS.md`, and it is genre-calibrated against
EDM-style arrangements: on other material the 0.5 tier fires on chorus-scale
dynamics too.

### On change (+ 1 Hz re-broadcast)

| Address | Args | Meaning |
|---|---|---|
| `/fosfora/v1/section` | string, float conf | `intro` \| `build` \| `drop` \| `break` \| `outro` \| `steady` |
| `/fosfora/v1/key` | string, float conf | `"Am"`, `"F#"`, … (only announced above 0.3 confidence) |
| `/fosfora/v1/phrase/len` | int, float conf | Inferred phrase length: 8, 16 or 32 bars |

### Status (1 Hz)

| Address | Args |
|---|---|
| `/fosfora/v1/status/online` | int — 1 while running, one final 0 on clean shutdown |
| `/fosfora/v1/status/uptime` | float seconds |
| `/fosfora/v1/status/device` | string input device name |
| `/fosfora/v1/status/hop_hz` | float analysis rate |
| `/fosfora/v1/status/tier` | string estimator tier, `heuristic-v1` today |

### Ableton Link (opt-in build: `--features link`)

Only in builds with the `link` cargo feature (Ableton Link is GPL-licensed, so the
prebuilt release binaries ship without it — build from source to get it). Emitted on
change plus a 1 Hz re-broadcast, from the live loop only: Link is wall-clock network
state, so `--signal-dump` output never contains these addresses and stays
deterministic.

| Address | Args |
|---|---|
| `/fosfora/v1/link/enabled` | int — 1 while Link is enabled in `link.json` |
| `/fosfora/v1/link/peers` | int — connected Link peers |
| `/fosfora/v1/link/tempo` | float — session tempo in BPM |
| `/fosfora/v1/link/playing` | int 0\|1 — Link transport (meaningful with start/stop sync) |

### The raw feature bus (opt-in: `--feat-bus`)

`/fosfora/v1/feat/<name>` — every one of the 83 analysis features, normalized 0..1
values verbatim, named exactly as in [AUDIO-FEATURES.md](AUDIO-FEATURES.md)
(`sub_bass` … `mfcc.0` … `chroma.11` … `beat_index`). ~2.5k datagrams/s at 30 Hz —
fine on loopback and wired LAN; turn it on only if you consume it.

## Honesty notes

**Stems are proxies.** Fosfora does not separate sources; the `stem/*` values come
from harmonic/percussive spectral masking (HPSS) and band energies. They behave like
"how much drums / bass / melody" for mixing-desk purposes, but bleed exists. The
addresses are stable: if real separation lands later (ONNX), values improve in place
with no schema change. `stem/bass/onset` and `stem/melody/onset` are **reserved but
not emitted** — no honest causal proxy exists today.

**The live path never emits `outro`.** Causally, an outro is indistinguishable from a
break or a quiet steady until the song has already ended. The label stays in the
schema (offline labeling may use it); a live guess would only teach you to distrust
the address.

**`section` is a heuristic (`heuristic-v1`)**, tuned on 4/4 electronic music, with
explicit hysteresis and per-state minimum dwell measured in bars. On material with no
beat lock (ambient), musical time falls back to a 2 s/bar equivalent. The confidence
argument is not decoration — gate on it.

**`section/boundary` is a separate stream from `section`, and the two disagree on
purpose.** `section` announces what the music *is* (intro, build, drop, break, steady) and
only speaks when that label changes; a chorus following a verse is not a label change, so
it is silent there. `section/boundary` announces *that the material changed*, from a peak
in the self-similarity novelty, whether or not the label moved. If you want cues, patch
this one; if you want to know the current state, patch `section`.

Its second argument is the boundary's **age in seconds**, and it is a fixed property of
the detector, not an estimate: the novelty kernel is centred, so it can only describe the
music some seconds behind the playhead, and a peak has to be confirmed before it is a peak.
Both delays are constants, so the event says exactly how old it is. Subtract `age_s` to
place the boundary on a timeline; ignore it if you just want a trigger and do not mind
firing late. Measured accuracy for both readings is in `docs/BENCHMARKS.md`.

Its confidence argument is worth gating on — it predicts correctness rather than decorating
the message. Measured against reference annotations, boundaries in the top third by
confidence are right about 66% of the time against 38% for the bottom third; gating at 0.5
keeps under a third of the events and takes precision from 0.51 to 0.66. Take everything for
a busy, responsive patch; gate high when a false cue is expensive.

**`phrase/*` assumes 4/4** and infers 8/16/32-bar grids from how drops, section
boundaries and build onsets align. Off-grid material keeps a position
under the best hypothesis with low confidence.

## Version policy

`/v1/` is frozen: existing addresses never change type or semantics. New addresses
may be added (additive is allowed — that is how `link/*` arrived, and why `chord` is
reserved here rather than invented later elsewhere). Anything breaking becomes
`/fosfora/v2/` alongside, not instead. Reserved: `/fosfora/v1/chord`,
`/fosfora/v1/stem/{bass,melody}/onset`; further `/fosfora/v1/link/*` additions stay
additive-only.

## Offline dumps — `--signal-dump` (needs a build with `--features analyze`)

```
fosfora --signal-dump song.flac                 # -> song.signal.jsonl
fosfora --signal-dump song.flac --out - --rate 10
```

Streams the file through the **same** emitter (same code path as live, proven
bit-identical by the golden-vector test) and writes one JSON object per line instead
of UDP. Deterministic: identical input produces byte-identical output. Dumps use
built-in defaults plus the CLI flags and never read `signal.json` — a measurement
must not depend on the operator's saved rig config (live `--signal` still honors
it). The record shapes are frozen — this is also the input format for the benchmark
harness:

```json
{"meta":1,"schema":"/fosfora/v1","source":"song.flac","sample_rate":44100,"hop_hz":86.13,"tx_rate_hz":30}
{"ts":12.345,"addr":"/fosfora/v1/beat","args":[{"i":23}]}
{"ts":12.351,"addr":"/fosfora/v1/section","args":[{"s":"build"},{"f":0.82}]}
```

First line is the meta record (`meta` key present); every other line is
`ts` (sample-clock seconds) + `addr` + `args`, where each arg is a single-key object
using the OSC type tag: `i` (int32), `f` (float32), `s` (string).

## Receiver quick-notes

- **TouchDesigner**: OSC In CHOP (port 9010) turns the continuous group into
  channels; use an OSC In DAT for the string-typed `/section` and `/key` (multi-arg
  messages arrive as one row).
- **QLC+**: add an OSC input plugin on port 9010 and map addresses in an input
  profile; the event counts work well as triggers because every change is a new value.
- **Anything**: `oscdump 9010` (liblo) prints the full stream for a first look.

## Tuning

RX/TX for *controlling* Fosfora is separate (see
[QUICK-REFERENCE.md](QUICK-REFERENCE.md) — OSC panel, ports 9000/9001). Signal is its
own socket and config so an analysis consumer and a control console never fight over
one endpoint.
