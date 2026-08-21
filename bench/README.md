# bench/ — the Fosfora benchmark harness (workstream C)

Honest accuracy numbers for the **causal streaming engine**: audio is streamed
through the exact live analysis path (`--signal-dump`, byte-deterministic, see
[docs/SIGNAL.md](../docs/SIGNAL.md)), the JSONL is scored against annotated
datasets with mir_eval-convention metrics, and published batch numbers
(madmom / BeatNet / allin1) are **cited as clearly-labeled offline comparison
targets — never re-run locally**.

Everything here is dev tooling: uv PEP-723 entry scripts + two shared modules
(`benchlib/` for scoring, `datasetlib.py` for fetching). Nothing ships to users.

## Quickstart

```sh
# CI fixture (no datasets needed) — same run CI does on every push:
cargo build --features analyze
bench/make_fixture.py -o bench/out/fixture
FOSFORA_BIN=target/debug/fosfora bench/check_fixture.py bench/out/fixture

# A real dataset:
bench/fetch_ballroom.py fetch     # download raw audio + annotations
bench/fetch_ballroom.py prep      # decode + emit normalized bundles
bench/run_bench.py bench/datasets/ballroom --jobs 4
#   -> bench/out/results/ballroom/<track>.json + summary.json

# Unit tests:
bench/test_benchlib.py
```

## Layout

```
bench/
  benchlib/              scoring library (dump parser, metrics, results)
  datasetlib.py          fetch/prep library (downloads, checksums, status)
  fetch_<dataset>.py     one per dataset: fetch | prep | verify
  manifests/<ds>.json    CHECKED IN: what is pinnable (see checksum policy)
  manifests/dev_subset.json  CHECKED IN: deterministic iteration subsets
                         (workstream Q) + their frozen baselines; ~40 tracks for
                         beat/tempo, 48 taxonomy-stratified tune-half tracks for
                         key (Q3); selection rules + expansion command in _policy
  manifests/q<N>_*_frozen.json  CHECKED IN: one per quality round — the split,
                         the targets pre-registered BEFORE any DSP change, and
                         afterwards the chosen config, what was rejected with
                         the evidence, and the verdict against each target
  dump_<x>_sidecar.py    dev-only: re-run the binary with FOSFORA_<X>_SIDECAR set
                         to dump one detector's raw per-hop inputs, TUNE HALF ONLY.
                         The structure sidecar is at schema v4; every bump is
                         ADDITIVE and readers ignore unknown fields, because the
                         374-track Harmonix corpus is v2 and is not being re-dumped.
                         Its meta line carries both the live config AND the
                         detector's module constants, so a replay reads the
                         binary's own numbers instead of a second copy in Python
  labels/                CHECKED IN: hand-labelled drop ground truth (see below)
  sweep_<x>.py           replay that detector's back-end over those sidecars in
                         Python, so constants sweep in seconds instead of hours.
                         ALWAYS run --validate first: it replays the SHIPPED
                         config and diffs against cached run_bench results, and a
                         replay that has drifted makes every sweep number a lie
  datasets/<ds>/         GITIGNORED: raw/ audio/ norm/ status.json
  out/                   GITIGNORED: dumps cache + results (regenerable)
```

## Schemas (all versioned, all produced/consumed only inside bench/)

- **`fosfora-bench-manifest/v1`** (`manifests/<ds>.json`, checked in): sources
  (kind: archive | git | file | per-track | manual) with whatever pin exists —
  sha256 for stable archives, upstream md5s where that's all upstream gives,
  commit pins for annotation repos — plus expected track counts, exclusion
  lists with reasons, license/provenance notes.
- **`fosfora-bench-status/v1`** (`datasets/<ds>/status.json`, local only):
  observations — hashes of everything fetched/derived, tool versions,
  per-track fetch/prep outcomes, alignment verdicts.
- **`fosfora-bench-annotation/v1`** (`datasets/<ds>/norm/<track>.json`): the
  normalized per-track bundle the scorer consumes. All times in seconds on the
  **local audio file's clock** (offset corrections baked in — the engine has
  no resampler). Fields null/absent when the dataset lacks that signal;
  metrics key off field presence. Segment labels stay **raw** (boundary and
  pairwise F1 are vocabulary-agnostic); only drop events carry a derivation
  `kind`: `direct` | `proxy_chorus_onset` | `local_manual` | `constructed`.
- **`fosfora-bench-index/v1`** (`norm/index.json`): the track list
  `run_bench.py` walks.
- **`fosfora-bench/v1`** (`out/results/...`): per-track scores + the echoed
  metric CONVENTIONS, sorted keys, 4-decimal floats — reruns diff cleanly.

## Fetch-script contract

Every `fetch_<dataset>.py` exposes `fetch | prep | verify` with
`--only ID --limit N --force`, is idempotent and resumable, and follows one
exit-code rule: **per-track failures are normal** (preview URLs rot, videos
vanish) — recorded in status.json, run ends 0 with an "N of M" coverage
report; **structural failures** (pinned checksum mismatch, annotation source
gone) exit nonzero, because numbers built on unverified inputs are worse than
no numbers.

## Checksum policy

Manifests pin what is pinnable. Refetched YouTube/Beatport audio is not
byte-reproducible, so for those datasets the manifest pins the *track list*,
the *annotations*, and the *acceptance gate*; the audio hashes live in
status.json as observations. `verify` re-hashes everything recorded and
reports drift.

## What a "drop" is

`bench/labels/` is hand-labelled by ear and is the sole ground truth for the drop
workstream. The target it records is one sentence:

> **Mark where the visuals should slam.**

Deliberately not musicological. Fosfora is a VJ engine, so the operational question
is whether the room should hit at this instant — and the listener cannot be wrong
about that, being the one running the visuals. These labels are not an attempt to
recover a genre convention; they are a direct recording of the product requirement.

Two consequences that look like corpus bugs and are not. **Cross-genre inconsistency
is expected** — "drop" is a well-defined moment in big-room and barely a concept
elsewhere, and a track with no drops in that sense can still carry moments worth
punching. And **a textbook drop the listener would not punch is a negative**, not a
missed positive. It also picks the corpus: a musicological target would want
genre-balanced EDM, this one wants whatever actually gets played.

Each bundle carries a free-text `note` — one sentence on what made the listener press
the button. A target defined only by a list of timestamps is one no later corpus can
be checked against. Raised as board #2371; `bench/label_drops.py` states the same
spec on screen, which is where it actually has to be read.

## Honesty rules (positioning guardrails, program addendum)

- Every reported table carries coverage: expected / fetched / gated-out /
  scored. Exclusions are listed by id with reasons, never silently dropped.
- Every number is labeled causal vs offline; cited numbers name their source.
- Metric conventions ride inside every results file (`conventions` block) —
  a number without its convention is not reproducible.
- The timestamp policy is "score what the wire says": dump `ts` is hop-end
  sample-clock time and the ≤11.6 ms grid bias stays uncorrected.
- Drop ground-truth tiers are never merged in reporting: `direct` label
  matches, `proxy_chorus_onset` (EDM-genre subset; a chorus onset is not a
  drop), and the `local_manual` hand-annotated set that carries the headline
  `/predict/drop` lead-time distribution.

## Interfaces

The scorer consumes **only** the dump JSONL + the annotation bundle — never
engine internals. The dump cache keys on sha256(audio) + sha256(binary) +
flags, so re-scoring never re-runs analysis and any rebuild invalidates
honestly. Dumps use the shipped default 30 Hz continuous rate: we score what
a rig receives.

Full phase plan and session state: `TASKS.md` (repo root, local-only) and the
plan file it references.
