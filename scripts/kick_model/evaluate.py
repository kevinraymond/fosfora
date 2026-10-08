#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy", "scipy"]
# ///
"""Score the shipped kick model (kick_model.json) the way the app runs it.

    evaluate.py [--mdb DIR] [--batida DIR] [--model kick_model.json]

--mdb    MDB Drums (https://github.com/CarlSouthall/MDBDrums, CC BY-NC-SA, so evaluation
         only, never training): DIR holds audio/full_mix/*_MIX.wav and annotations/class/.
--batida batida renders: DIR holds <name>.wav with <name>.json (exact kick times).

Each detection goes through kick_from_probability, the FeatureSmoother and the consumers'
0.5 / 0.3 hysteresis, and counts as right within 50 ms of a labeled kick.
"""

import argparse
import json
from pathlib import Path

import numpy as np
from scipy.io import wavfile

import kickfeat

HERE = Path(__file__).parent
DEFAULT_MODEL = HERE.parent.parent / "crates" / "fosfora-app" / "src" / "audio" / "kick_model.json"


def probabilities(m, X):
    feat, thr = np.array(m["feature"]), np.array(m["threshold"], np.float32)
    left, right, leaf = np.array(m["left"]), np.array(m["right"]), np.array(m["leaf"], bool)
    value = np.array(m["value"], np.float32)
    raw = np.full(len(X), m["baseline"], np.float32)
    rows = np.arange(len(X))
    for root in m["roots"]:
        i = np.full(len(X), root)
        while not leaf[i].all():
            go_left = X[rows, feat[i]] <= thr[i]
            i = np.where(leaf[i], i, np.where(go_left, left[i], right[i]))
        raw += value[i]
    return 1 / (1 + np.exp(-raw))


def load(path):
    sr, x = wavfile.read(path)
    x = x.astype(np.float64) / float(np.iinfo(x.dtype).max + 1) if x.dtype.kind == "i" else x.astype(np.float64)
    return sr, (x.mean(1) if x.ndim == 2 else x)


def run(name, items, m):
    P, R = [], []
    for wav, kicks in items:
        sr, mono = load(wav)
        p = probabilities(m, kickfeat.hop_features(mono, sr))
        det = kickfeat.app_events(kickfeat.kick_from_probability(p, m["kick_threshold"]), sr)
        pr, rc = kickfeat.score(det, sorted(kicks))
        P.append(pr)
        R.append(rc)
    p, r = np.mean(P), np.mean(R)
    print(f"{name:8} {len(items):3} tracks  precision {p:.3f}  recall {r:.3f}  F1 {2 * p * r / max(p + r, 1e-9):.3f}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mdb", type=Path)
    ap.add_argument("--batida", type=Path)
    ap.add_argument("--model", type=Path, default=DEFAULT_MODEL)
    a = ap.parse_args()
    m = json.loads(a.model.read_text())
    if a.mdb:
        items = []
        for ann in sorted((a.mdb / "annotations" / "class").glob("*_class.txt")):
            base = ann.name.replace("_class.txt", "")
            kicks = [float(l.split()[0]) for l in ann.read_text().splitlines() if l.split()[1:2] == ["KD"]]
            items.append((a.mdb / "audio" / "full_mix" / f"{base}_MIX.wav", kicks))
        run("MDB", items, m)
    if a.batida:
        items = []
        for spec in sorted(a.batida.glob("*.json")):
            wav = spec.with_suffix(".wav")
            if wav.exists() and "events" in (gt := json.loads(spec.read_text())):
                ev = gt["events"]["kick"]
                items.append((wav, [e if isinstance(e, (int, float)) else e.get("t", e.get("time", e.get("start"))) for e in ev]))
        run("batida", items, m)


if __name__ == "__main__":
    main()
