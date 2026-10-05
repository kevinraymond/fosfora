# Kick model

The `kick` audio feature is scored by a small gradient-boosted tree ensemble,
`crates/fosfora-app/src/audio/kick_model.json`, evaluated each analysis hop by
`audio/kick_model.rs`. This directory rebuilds it.

## Why a model

The old kick was the 30-120 Hz energy rise divided by its own 10 s 95th percentile. A bass
note is a rise in that band too, and the self-normalization rescaled whatever was left once
the kick stopped, so basslines read as kicks: about 6 detections in 10 were not kicks, on
synthetic renders with exact labels and on real recordings with human labels alike. Hand
tuned rules did not fix it; features that separated kicks from bass notes on one dataset
pointed the other way on another.

## Data

- **Training:** Creative Commons electronic music from the Free Music Archive, CC BY and
  CC0 only, listed in `tracks.json` and credited in
  `crates/fosfora-app/src/audio/kick_model_tracks.md`. ShareAlike and NonCommercial tracks
  are left out. `tracks.json` came from two passes of `fma.py screen` / `select`; download
  it as is to rebuild the shipped model.
- **Labels:** each track is separated with Demucs (`htdemucs`, four stems) and kicks are
  found on the drums stem, minus hits whose low end is mostly bass bleed (`kicklabel.py`).
  On MDB Drums this scores P .90 R .90 against human labels.
- **Evaluation only:** MDB Drums (real recordings, human labels, CC BY-NC-SA) and batida
  renders (synthetic, exact labels). Neither is trained on.

## Rebuild

```sh
W=/some/work/dir
uv run fma.py download --work $W               # ~2 GB of mp3 from the FMA archives
uvx --from demucs demucs -n htdemucs -o $W/stems $W/audio/*.mp3   # a GPU makes this minutes
uv run train.py --work $W                      # writes kick_model.json and the credits
uv run evaluate.py --mdb <MDB Drums checkout> --batida <dir of batida renders>
```

`train.py` fixes the random seed, so the same tracks give the same model. After a
retrain, run the Rust tests: `audio_thread_golden_vector` pins `kick` at three hops and
needs those values re-recorded, and `kick_fires_on_kick_drums_not_on_bass_notes` checks
the new model still tells a kick drum from a bass note.

The features in `kickfeat.py` must stay identical to `kick_model.rs`. A change to either
side means retraining.
