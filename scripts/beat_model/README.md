# Beat model

Tooling for the beat-likelihood model: a per-hop activation from Fosfora's own analysis
chain, used as evidence by the beat grid's phase arbiter (its peaks, at the threshold
`ARB_ACTIVATION_THRESHOLD` in `beat.rs`, join the kick events in the arbiter's window; the
round that chose this is `bench/manifests/q4163_arbiter_frozen.json`, with
`bench/q4163_arm.sh`, `q4163_report.py`, `q4163_verdict.py` and `q4163_listen.py` as its
tools). The runtime is
`crates/fosfora-app/src/audio/beat_model.rs`, which evaluates the exported
`beat_model.json` (flat trees plus the input map, the target lag and the picker) inside the
beat detector every hop; `beat_model_tracks.md` beside it credits the CC BY training audio.

## The per-hop dump

The model is trained on the RAW evidence the detectors read each hop, not on the
smoothed, normalized feature bus the app publishes. `FOSFORA_HOP_SIDECAR=<file>` makes the
hop chain write one JSONL record per analysis hop (writer:
`crates/fosfora-app/src/audio/hop_sidecar.rs`); `hopdump.py` reads it back into arrays:

```sh
cargo build --release --features release,analyze
FOSFORA_HOP_SIDECAR=track.jsonl target/release/fosfora --signal-dump track.flac --out -
uv run --script scripts/beat_model/hopdump.py track.jsonl     # a summary
```

Never set the variable on a live run: the writer does file I/O on the analysis thread.

## The data pipeline

All of it hangs off one work dir (`corpus.py` documents the layout; the bulk data lives
on the T9, not in the repo):

```sh
W=/mnt/t9/fosfora-work/q4163
uv run --script scripts/beat_model/fetch_negatives.py --out $W/data/negatives_train \
    --metadata ~/ai/audio/kick-work/fma/fma_metadata --exclude <guard negatives.json>
uv run --script scripts/beat_model/dump_corpus.py --work $W --bin <fosfora with analyze> --jobs 6
uv run --script scripts/beat_model/build_dataset.py --work $W --lag 3     # targets, weights, splits
uv run --script scripts/beat_model/train.py --work $W --name wide         # every column x offsets -6..0
uv run --script scripts/beat_model/train.py --work $W --name narrow \
    --select-from $W/models/wide.json --top-cols 16 --leaves 15 --max-depth 5 --trees 200 --export
```

Training audio is Creative Commons BY / CC0 (FMA) plus batida renders; labels are
Fosfora's own confident beats, batida's exact ones, and beatless clips as negatives.
`build_dataset.py --fma-oracle <dir>` labels the FMA tracks from an offline tracker's beats
instead (`<dir>/<id>.json` with `beats` / `downbeats`; board #4288 used Beat This, decision
#4414). That round did not ship: the relabeled activation scored better per hop but the
pipeline did not carry it through (bench/manifests/q4288_labels_frozen.json). The
bench TUNE halves and the 155 guard clips are evaluation only; the bench HOLDOUT halves
are never read by anything here. `build_dataset.py --lag-check` shows where each raw
column peaks relative to exact beat labels, which is how the target lag was chosen.

After an export, embed and credit it:

```sh
cp $W/models/<name>.beat_model.json crates/fosfora-app/src/audio/beat_model.json
uv run --script scripts/beat_model/credits.py --work $W --metadata <fma_metadata dir> \
    --out crates/fosfora-app/src/audio/beat_model_tracks.md
cargo build --release --features release,analyze
python3 scripts/beat_model/fixtures/make_click_fixture.py              # dumps with the new activation
uv run --script scripts/beat_model/make_golden.py --model crates/fosfora-app/src/audio/beat_model.json \
    --dump scripts/beat_model/fixtures/click_200hops.jsonl --out scripts/beat_model/fixtures/click_200hops.golden.json
```

## Tests

```sh
cargo test -p fosfora-app --lib -- audio::hop_sidecar audio::hop::tests::hop_sidecar
uv run --script scripts/beat_model/test_hopdump.py
```

The Rust hop test drives the analysis chain and reads every record back against the values
the detectors held on that hop. The Python test reads `fixtures/click_50hops.jsonl`, a dump
the real binary wrote (`fixtures/make_click_fixture.py` regenerates both fixtures), so the
reader is checked against the writer's actual output. The runtime's golden test replays
`fixtures/click_200hops.jsonl` through `beat_model.rs` and compares every hop's activation
with `click_200hops.golden.json`, the Python evaluation of the same model by
`make_golden.py`, and with the activation the binary itself wrote into the dump.
`cargo test --release -- cost_per_hop_vs_kick_model --ignored --nocapture` prints the per-hop
cost of both models.
