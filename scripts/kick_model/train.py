#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy", "scipy", "scikit-learn==1.9.*"]
# ///
"""Label, train and export the kick model.

    train.py --work W [--tracks tracks.json]

Expects W/audio/<id>.mp3 (fma.py download) and Demucs stems at
W/stems/htdemucs/<id>/{drums,bass}.wav (see README.md). Writes the ensemble to
crates/fosfora-app/src/audio/kick_model.json and the attribution list beside it.
"""

import argparse
import json
import subprocess
from pathlib import Path

import numpy as np
from scipy.io import wavfile
from sklearn.ensemble import HistGradientBoostingClassifier

import kickfeat
import kicklabel

HERE = Path(__file__).parent
CRATE_AUDIO = HERE.parent.parent / "crates" / "fosfora-app" / "src" / "audio"
KICK_THRESHOLD = 0.8  # chosen on held-out MDB Drums and batida renders, confirmed by ear


def decode(path, sr=44100):
    raw = subprocess.run(["ffmpeg", "-v", "error", "-i", str(path), "-ac", "2", "-ar", str(sr), "-f", "f32le", "-"],
                         capture_output=True, check=True).stdout
    return np.frombuffer(raw, np.float32).reshape(-1, 2).astype(np.float64).mean(1)


def load_wav(path):
    sr, x = wavfile.read(path)
    x = x.astype(np.float64) / float(np.iinfo(x.dtype).max + 1) if x.dtype.kind == "i" else x.astype(np.float64)
    return sr, (x.mean(1) if x.ndim == 2 else x)


def export(model, dst):
    """Flat node arrays, one root per tree; an internal node goes left when x[feature] <= threshold."""
    feat, thr, left, right, value, leaf, roots = [], [], [], [], [], [], []
    for (pred,) in model._predictors:
        base = len(feat)
        roots.append(base)
        for nd in pred.nodes:
            assert not nd["is_categorical"]
            is_leaf = bool(nd["is_leaf"])
            feat.append(int(nd["feature_idx"]))
            thr.append(float(np.float32(nd["num_threshold"])))
            left.append(0 if is_leaf else int(nd["left"]) + base)
            right.append(0 if is_leaf else int(nd["right"]) + base)
            value.append(float(np.float32(nd["value"])))
            leaf.append(int(is_leaf))
    out = dict(format=1, kick_threshold=KICK_THRESHOLD, baseline=float(np.ravel(model._baseline_prediction)[0]),
               n_features=int(model.n_features_in_), roots=roots, feature=feat, threshold=thr,
               left=left, right=right, value=value, leaf=leaf)
    dst.write_text(json.dumps(out, separators=(",", ":")))
    return len(roots), len(feat)


def attribution(rows, dst):
    lines = ["# Kick model training data", "",
             "`kick_model.json` was trained on these tracks from the Free Music Archive",
             "(https://freemusicarchive.org), used under the licenses listed. Kick labels were",
             "derived from Demucs drum stems; no audio is distributed with Fosfora.", "",
             "| Track | Artist | License | FMA id |", "|---|---|---|---|"]
    for r in sorted(rows, key=lambda r: (r["artist"].lower(), r["tid"])):
        title = r.get("title", "").replace("|", "/")
        lines.append(f"| {title} | {r['artist'].replace('|', '/')} | {r['license'].strip()} | {r['tid']} |")
    dst.write_text("\n".join(lines) + "\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--work", type=Path, required=True)
    ap.add_argument("--tracks", type=Path, default=HERE / "tracks.json")
    a = ap.parse_args()
    rows = json.loads(a.tracks.read_text())
    X, y, used = [], [], []
    for r in rows:
        stems = a.work / "stems" / "htdemucs" / f"{r['tid']:06d}"
        audio = a.work / "audio" / f"{r['tid']:06d}.mp3"
        if not (stems / "drums.wav").exists() or not audio.exists():
            print(f"skip {r['tid']}: missing audio or stems")
            continue
        sr_d, drums = load_wav(stems / "drums.wav")
        _, bass = load_wav(stems / "bass.wav")
        kicks = sorted(kicklabel.label(drums, bass, sr_d))
        F = kickfeat.hop_features(decode(audio), 44100)
        X.append(F)
        y.append(kickfeat.hop_labels(len(F), 44100, kicks))
        used.append(r)
    print(f"{len(used)} tracks, {sum(int(v.sum()) for v in y)} positive hops")
    model = HistGradientBoostingClassifier(max_iter=300, max_depth=6, learning_rate=0.1,
                                           class_weight="balanced", random_state=0)
    model.fit(np.vstack(X), np.concatenate(y))
    trees, nodes = export(model, CRATE_AUDIO / "kick_model.json")
    attribution(used, CRATE_AUDIO / "kick_model_tracks.md")
    print(f"exported {trees} trees, {nodes} nodes")


if __name__ == "__main__":
    main()
