#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Beat PHASE report: where do emitted beats sit relative to the reference grid?

    bench/analyze_beat_phase.py <out_root> <dataset> [<dataset>...] [--bin HASH] [--json F]

<out_root> is a run_bench --out-root (its dumps/<dataset>/ holds the cached
Signal dumps). For every emitted /beat after the 5 s trim, its position inside
the reference beat interval it falls in, as a fraction 0..1, classifies it:
  on       within 1/8 beat of a reference beat
  anti     within 1/8 beat of the half-beat (the "and": an off-beat lock)
  quarter  everything else (quarter-off, drifting, or a different tempo)
Per track the dominant class is reported too: an "anti-locked" track has more
anti than on beats. Results per split half: sha256(track_id) parity, even ->
tune, odd -> holdout (the convention of every q-round manifest).

Beat F (benchlib convention) is printed next to it so a phase change can be
read against the headline metric.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).parent))

from benchlib.annotations import Annotations, load_index  # noqa: E402
from benchlib.dump import SignalDump  # noqa: E402
from benchlib.metrics.beats import _score  # noqa: E402

TRIM = 5.0
REPO = Path(__file__).resolve().parent.parent


def half(track_id: str) -> str:
    return "tune" if int(hashlib.sha256(track_id.encode()).hexdigest(), 16) % 2 == 0 else "holdout"


def classify(est: np.ndarray, ref: np.ndarray) -> np.ndarray:
    """Fractional position of each est beat inside its reference interval."""
    est = est[(est >= max(TRIM, ref[0])) & (est < ref[-1])]
    k = np.searchsorted(ref, est, side="right") - 1
    return (est - ref[k]) / (ref[k + 1] - ref[k])


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("out_root", type=Path)
    ap.add_argument("datasets", nargs="+")
    ap.add_argument("--bin", default="", help="binary hash in the dump file names (default: any, must be unique)")
    ap.add_argument("--json", type=Path)
    ap.add_argument("--half", default="all", choices=["tune", "holdout", "all"],
                    help="only read tracks of this split half (a variant run on the tune half only)")
    args = ap.parse_args()

    report = {}
    for ds in args.datasets:
        index = load_index(REPO / "bench" / "datasets" / ds / "norm" / "index.json")
        rows = []
        for t in index:
            if args.half != "all" and half(t["track_id"]) != args.half:
                continue
            ann = Annotations.load(t["annotations"])
            if ann.beats is None or len(ann.beats) < 4:
                continue
            hits = sorted((args.out_root / "dumps" / ds).glob(f"{t['track_id']}.*{args.bin}*.jsonl"))
            if len(hits) != 1:
                raise SystemExit(f"{ds}/{t['track_id']}: expected one dump, found {len(hits)}")
            est = np.asarray(SignalDump.load(hits[0]).beats(), dtype=float)
            ref = np.asarray(ann.beats, dtype=float)
            pos = classify(est, ref)
            on = int(np.sum((pos < 0.125) | (pos >= 0.875)))
            anti = int(np.sum((pos >= 0.375) & (pos < 0.625)))
            n = len(pos)
            rows.append(dict(track=t["track_id"], half=half(t["track_id"]), n=n, on=on, anti=anti,
                             quarter=n - on - anti, beat_f=_score(est, ref)["f_measure"]))
        report[ds] = {}
        print(f"\n{ds}")
        print(f"  {'half':8} {'tracks':>6} {'beat F':>7} {'on':>6} {'anti':>6} {'quarter':>7}  {'anti-locked':>11} {'no beats':>8}")
        for h in ("tune", "holdout", "all"):
            sel = [r for r in rows if h == "all" or r["half"] == h]
            if not sel:
                continue
            n = sum(r["n"] for r in sel) or 1
            agg = dict(
                tracks=len(sel),
                beat_f=float(np.mean([r["beat_f"] for r in sel])),
                on=sum(r["on"] for r in sel) / n,
                anti=sum(r["anti"] for r in sel) / n,
                quarter=sum(r["quarter"] for r in sel) / n,
                anti_locked=sum(1 for r in sel if r["n"] and r["anti"] > r["on"]),
                no_beats=sum(1 for r in sel if r["n"] == 0),
            )
            report[ds][h] = agg
            print(f"  {h:8} {agg['tracks']:6} {agg['beat_f']:7.4f} {agg['on']:6.3f} {agg['anti']:6.3f} {agg['quarter']:7.3f}  "
                  f"{agg['anti_locked']:11} {agg['no_beats']:8}")
        report[ds]["tracks"] = rows
    if args.json:
        args.json.write_text(json.dumps(report, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
