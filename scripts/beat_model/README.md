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

## Tests

```sh
cargo test -p fosfora-app --lib -- audio::hop_sidecar audio::hop::tests::hop_sidecar
uv run --script scripts/beat_model/test_hopdump.py
```

The Rust test drives the analysis chain and reads every record back against the values the
detectors held on that hop. The Python test reads `fixtures/click_50hops.jsonl`, a dump the
real binary wrote (`fixtures/make_click_fixture.py` regenerates it), so the reader is
checked against the writer's actual output.
