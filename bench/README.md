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
