# Beat model

Tooling for the beat-likelihood model: a per-hop activation from Fosfora's own analysis
chain, used as evidence by the beat grid's phase arbiter. It is built in phases; this
directory grows with them.

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
Fosfora's own confident beats, batida's exact ones, and beatless clips as negatives. The
bench TUNE halves and the 155 guard clips are evaluation only; the bench HOLDOUT halves
are never read by anything here. `build_dataset.py --lag-check` shows where each raw
column peaks relative to exact beat labels, which is how the target lag was chosen.

## Tests

```sh
cargo test -p fosfora-app --lib -- audio::hop_sidecar audio::hop::tests::hop_sidecar
uv run --script scripts/beat_model/test_hopdump.py
```

The Rust test drives the analysis chain and reads every record back against the values the
detectors held on that hop. The Python test reads `fixtures/click_50hops.jsonl`, a dump the
real binary wrote (`fixtures/make_click_fixture.py` regenerates it), so the reader is
checked against the writer's actual output.
