#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""The beat model's golden vector: evaluate an exported model on a hop dump in Python, the
way the Rust runtime does it, so audio/beat_model.rs can be checked hop by hop.

    make_golden.py --model crates/fosfora-app/src/audio/beat_model.json \
                   --dump scripts/beat_model/fixtures/click_200hops.jsonl \
                   --out scripts/beat_model/fixtures/click_200hops.golden.json

Mirrors the runtime exactly: inputs are the named columns at their offsets (zero before the
first hop), thresholds and leaf values are f32, and the sum starts at the baseline. If the
dump is v2 it carries the binary's own activation per hop; the maximum difference against
this evaluation is reported and must be ~1e-6, or the runtime and this script disagree.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import hopdump  # noqa: E402


def column(d: hopdump.HopDump, name: str) -> np.ndarray:
    if name.startswith("kf."):
        return d.kf[:, d.kf_names.index(name[3:])]
    if name.startswith("sf."):
        return d.sf[:, d.sf_names.index(name[3:])]
    if name.startswith("pn."):
        return d.pn[:, d.pn_names.index(name[3:])]
    if name == "kp":
        return d.kick_prob
    if name == "sil":
        return d.silent.astype(np.float32)
    raise KeyError(name)


def design(d: hopdump.HopDump, model: dict) -> np.ndarray:
    X = np.zeros((d.n_hops, len(model["inputs"])), np.float32)
    for j, inp in enumerate(model["inputs"]):
        c = column(d, inp["name"]).astype(np.float32)
        back = -int(inp["offset"])
        assert back >= 0, inp
        if back == 0:
            X[:, j] = c
        elif back < d.n_hops:
            X[back:, j] = c[:-back]
    return X


def activation(model: dict, X: np.ndarray) -> np.ndarray:
    f = np.asarray(model["feature"]); th = np.asarray(model["threshold"], np.float32)
    L = np.asarray(model["left"]); R = np.asarray(model["right"])
    v = np.asarray(model["value"], np.float32); leaf = np.asarray(model["leaf"])
    raw = np.full(X.shape[0], np.float32(model["baseline"]), np.float32)
    rows = np.arange(X.shape[0])
    for root in model["roots"]:
        i = np.full(X.shape[0], root)
        while True:
            internal = leaf[i] == 0
            if not internal.any():
                break
            go_left = X[rows, f[i]] <= th[i]
            i = np.where(internal, np.where(go_left, L[i], R[i]), i)
        raw = (raw + v[i]).astype(np.float32)
    return (1.0 / (1.0 + np.exp(-raw.astype(np.float64)))).astype(np.float32)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--model", type=Path, required=True)
    ap.add_argument("--dump", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    a = ap.parse_args()
    model = json.loads(a.model.read_text())
    assert model["format"] == 2 and model["kind"] == "beat_activation", "not a format 2 beat model"
    d = hopdump.read(a.dump)
    act = activation(model, design(d, model))
    out = dict(model=str(a.model), model_sha256=hashlib.sha256(a.model.read_bytes()).hexdigest(),
               dump=str(a.dump), n_hops=int(d.n_hops), lag_hops=model["lag_hops"], act=[float(x) for x in act])
    a.out.write_text(json.dumps(out))
    msg = f"wrote {a.out}: {d.n_hops} hops, activation mean {act.mean():.4f} max {act.max():.4f}"
    if np.isfinite(d.act).any():
        diff = float(np.abs(d.act - act).max())
        msg += f"; max |binary - python| {diff:.2e}"
        if diff > 1e-5:
            print(msg)
            sys.exit("the binary's activation does not match this evaluation")
    print(msg)
    return 0


if __name__ == "__main__":
    sys.exit(main())
