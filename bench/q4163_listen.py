#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "soundfile"]
# ///
"""Click-track A/B renders for the owner's listen (G10): the same 24 s of a track twice,
first with arm A's beats as clicks, then arm B's, so a change in where the beats land can
be judged by ear rather than read off a display.

    bench/q4163_listen.py --a <root A> --b <root B> --dataset harmonix --track 0982_whoknewdos \
                          [--start auto|SECONDS] [--seconds 24] --out <dir> [--index N]

Each root is a q4163_arm.sh root; the beats are its cached signal dump's /beat events.
`--start auto` picks the window where the two arms disagree most (beats of one with no
beat of the other within 70 ms), after the bench's 5 s trim. Writes
<out>/<index>_<track>.flac (music at -6 dB, clicks on top, 1 s of silence between the two
passes) and appends one line to <out>/README.txt saying which arm clicks first.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np
import soundfile as sf

REPO = Path(__file__).resolve().parent.parent
TRIM = 5.0
TOL = 0.07


def beats_of(root: Path, dataset: str, track: str) -> np.ndarray:
    files = sorted((root / "dumps" / dataset).glob(f"{track}.*.jsonl"))
    if len(files) != 1:
        sys.exit(f"{root}: {len(files)} dumps for {track} under {dataset} (need exactly one)")
    out = []
    with open(files[0]) as f:
        for line in f:
            if '"/fosfora/v1/beat"' in line:
                out.append(json.loads(line)["ts"])
    return np.asarray(out, float)


def audio_of(dataset: str, track: str):
    idx = json.loads((REPO / "bench/datasets" / dataset / "norm/index.json").read_text())
    rows = idx["tracks"] if isinstance(idx, dict) else idx
    t = next(r for r in rows if r["track_id"] == track)
    path = (REPO / "bench/datasets" / dataset / "norm" / t["audio"]).resolve()
    y, sr = sf.read(str(path), dtype="float32", always_2d=True)
    return y, sr


def disagreement(a: np.ndarray, b: np.ndarray, t0: float, t1: float) -> int:
    wa = a[(a >= t0) & (a < t1)]
    wb = b[(b >= t0) & (b < t1)]
    if len(wa) == 0 and len(wb) == 0:
        return 0
    only_a = sum(1 for t in wa if len(wb) == 0 or np.abs(wb - t).min() > TOL)
    only_b = sum(1 for t in wb if len(wa) == 0 or np.abs(wa - t).min() > TOL)
    return only_a + only_b


def pick_window(a, b, duration, seconds):
    best, best_start = -1, TRIM
    for start in np.arange(TRIM, max(TRIM, duration - seconds) + 1e-9, 1.0):
        d = disagreement(a, b, start, start + seconds)
        if d > best:
            best, best_start = d, float(start)
    return best_start, best


def click_track(beats: np.ndarray, t0: float, seconds: float, sr: int) -> np.ndarray:
    n = int(seconds * sr)
    out = np.zeros(n, np.float32)
    dur = int(0.012 * sr)
    t = np.arange(dur) / sr
    burst = (np.sin(2 * np.pi * 1000.0 * t) * np.exp(-t * 400.0)).astype(np.float32)
    for bt in beats:
        i = int(round((bt - t0) * sr))
        if 0 <= i < n:
            j = min(n, i + dur)
            out[i:j] += burst[: j - i]
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--a", type=Path, required=True)
    ap.add_argument("--b", type=Path, required=True)
    ap.add_argument("--dataset", required=True)
    ap.add_argument("--track", required=True)
    ap.add_argument("--start", default="auto")
    ap.add_argument("--seconds", type=float, default=24.0)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--index", type=int, default=1)
    args = ap.parse_args()
    y, sr = audio_of(args.dataset, args.track)
    duration = len(y) / sr
    a, b = beats_of(args.a, args.dataset, args.track), beats_of(args.b, args.dataset, args.track)
    if args.start == "auto":
        start, dis = pick_window(a, b, duration, args.seconds)
    else:
        start, dis = float(args.start), disagreement(a, b, float(args.start), float(args.start) + args.seconds)
    seconds = min(args.seconds, duration - start)
    i0, i1 = int(start * sr), int((start + seconds) * sr)
    music = y[i0:i1] * 0.5
    gap = np.zeros((sr, y.shape[1]), np.float32)
    passes = []
    for beats in (a, b):
        clicks = click_track(beats, start, seconds, sr)[: len(music)]
        passes.append(music + clicks[:, None] * 0.6)
    out = np.vstack([passes[0], gap, passes[1]])
    peak = float(np.abs(out).max())
    if peak > 0.99:
        out = out / peak * 0.99
    args.out.mkdir(parents=True, exist_ok=True)
    path = args.out / f"{args.index}_{args.track}.flac"
    sf.write(str(path), out, sr, format="FLAC")
    na = int(((a >= start) & (a < start + seconds)).sum())
    nb = int(((b >= start) & (b < start + seconds)).sum())
    line = (f"{path.name}: {args.dataset} {args.track} {start:.1f}-{start + seconds:.1f} s, first pass {args.a.name} ({na} beats), "
            f"second pass {args.b.name} ({nb} beats), {dis} beats differ")
    with open(args.out / "README.txt", "a") as f:
        f.write(line + "\n")
    print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
