#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Lock-decision report (board #4153): which logged signal separates TRUE tempo
locks from FALSE ones, and supported acquisition fires from unsupported ones?

    bench/analyze_lock_decisions.py <sidecar_root> [--json F]

<sidecar_root>/<dataset>/<track_id>.jsonl are FOSFORA_BEAT_SIDECAR files (one
"u" line per tempo update, "e" lines for events); <dataset> is one of the bench
sets (reference beats from bench/datasets/<ds>/norm) or `negatives` /
`synthetic` (every lock is false by construction).

A lock EPISODE runs from a "lock" event to the next "unlock" (or the end). On
music it is TRUE when the published tempo at lock entry is within 4% of the
reference tempo (median inter-beat interval) at any related ratio (1, 2, 1/2,
3/2, 2/3, 4/3, 3/4), else FALSE. An acquisition fire is TRUE when it lands
within 70 ms of a reference beat.

Each logged feature is scored at lock entry (the last "u" line at or before the
event) by AUC of true-vs-false, with the SIGN reported: an AUC below 0.5 means
the feature points the wrong way, not that it is weak. Features are ALSO scored
false-locks-vs-true-locks within music alone, since separating music from noise
is not the question (the detector never sees the label).
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

REPO = Path(__file__).resolve().parent.parent
RELATED = [1.0, 2.0, 0.5, 1.5, 2 / 3, 4 / 3, 0.75]
FEATURES = ["conf", "comb", "contrast", "harm", "support", "on_sup", "kick_sup", "challenge"]
TRIM = 5.0


def half(track_id: str) -> str:
    return "tune" if int(hashlib.sha256(track_id.encode()).hexdigest(), 16) % 2 == 0 else "holdout"


def auc(pos: np.ndarray, neg: np.ndarray) -> float:
    """Mann-Whitney AUC: P(pos > neg) with ties counted half."""
    if len(pos) == 0 or len(neg) == 0:
        return float("nan")
    allv = np.concatenate([pos, neg])
    ranks = np.empty(len(allv))
    order = np.argsort(allv, kind="mergesort")
    sv = allv[order]
    i = 0
    while i < len(sv):
        j = i
        while j + 1 < len(sv) and sv[j + 1] == sv[i]:
            j += 1
        ranks[order[i : j + 1]] = (i + j) / 2 + 1
        i = j + 1
    return float((ranks[: len(pos)].sum() - len(pos) * (len(pos) + 1) / 2) / (len(pos) * len(neg)))


def features(u: dict) -> dict:
    c2 = u.get("c2")
    peak = u.get("peak") or 0.0
    return dict(
        conf=u["conf"],
        comb=u["comb"],
        contrast=peak - (u.get("floor") or 0.0),
        harm=(c2 / peak) if (c2 is not None and peak > 1e-9) else 0.0,
        support=u["support"],
        on_sup=u["on_sup"],
        kick_sup=u["kick_sup"],
        challenge=u["challenge"],
    )


def tempo_ok(bpm: float, ref_bpm: float) -> bool:
    return bpm > 0 and any(abs(bpm / (ref_bpm * r) - 1.0) <= 0.04 for r in RELATED)


def episodes(lines: list[dict], ref_bpm: float | None, ref_beats: np.ndarray | None) -> tuple[list, list]:
    """(lock episodes, acquisition fires) with labels and the features at entry."""
    locks, fires = [], []
    last_u = None
    open_lock = None
    for d in lines:
        if d["k"] == "u":
            last_u = d
            if open_lock is not None:
                open_lock["n_updates"] += 1
            continue
        ev, ts = d["ev"], d["ts"]
        if ts < TRIM:
            continue
        if ev == "lock" and last_u is not None:
            bpm = last_u["pub_bpm"] or last_u["filt_bpm"]
            true = tempo_ok(bpm, ref_bpm) if ref_bpm else False
            open_lock = dict(t0=ts, bpm=bpm, true=true, n_updates=0, **features(last_u))
            locks.append(open_lock)
        elif ev == "unlock" and open_lock is not None:
            open_lock["t1"] = ts
            open_lock = None
        elif ev in ("acq_fire", "stale_reanchor") and last_u is not None:
            true = bool(ref_beats is not None and len(ref_beats) and np.min(np.abs(ref_beats - ts)) <= 0.07)
            fires.append(dict(t=ts, ev=ev, true=true, str=d.get("str", 0.0), **features(last_u)))
    return locks, fires


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("sidecar_root", type=Path)
    ap.add_argument("--json", type=Path)
    args = ap.parse_args()

    all_locks, all_fires = [], []
    for ds_dir in sorted(p for p in args.sidecar_root.iterdir() if p.is_dir()):
        ds = ds_dir.name
        refs = {}
        if ds not in ("negatives", "synthetic"):
            for t in load_index(REPO / "bench" / "datasets" / ds / "norm" / "index.json"):
                ann = Annotations.load(t["annotations"])
                if ann.beats is not None and len(ann.beats) >= 4:
                    b = np.asarray(ann.beats, float)
                    refs[t["track_id"]] = (60.0 / float(np.median(np.diff(b))), b)
        n_tracks = 0
        for f in sorted(ds_dir.glob("*.jsonl")):
            tid = f.stem
            if ds not in ("negatives", "synthetic") and tid not in refs:
                continue
            n_tracks += 1
            ref_bpm, ref_beats = refs.get(tid, (None, None))
            lines = [json.loads(line) for line in f.open() if line.strip()]
            locks, fires = episodes(lines, ref_bpm, ref_beats)
            for l in locks:
                l.update(ds=ds, track=tid, half=half(tid))
            for x in fires:
                x.update(ds=ds, track=tid, half=half(tid))
            all_locks += locks
            all_fires += fires
        print(f"{ds}: {n_tracks} tracks, {sum(1 for l in all_locks if l['ds'] == ds)} lock episodes, "
              f"{sum(1 for x in all_fires if x['ds'] == ds)} acquisition fires")

    def table(title: str, rows: list[dict]) -> dict:
        pos = [r for r in rows if r["true"]]
        neg = [r for r in rows if not r["true"]]
        print(f"\n{title}: {len(pos)} true / {len(neg)} false")
        print(f"  {'feature':10} {'AUC':>6} {'true med':>9} {'false med':>9}")
        out = {}
        for k in FEATURES:
            p = np.asarray([r[k] for r in pos], float)
            n = np.asarray([r[k] for r in neg], float)
            a = auc(p, n)
            out[k] = dict(auc=a, true_med=float(np.median(p)) if len(p) else None,
                          false_med=float(np.median(n)) if len(n) else None)
            print(f"  {k:10} {a:6.3f} {np.median(p) if len(p) else float('nan'):9.3f} "
                  f"{np.median(n) if len(n) else float('nan'):9.3f}")
        return out

    music_locks = [l for l in all_locks if l["ds"] not in ("negatives", "synthetic")]
    report = dict(
        locks_all=table("LOCK ENTRY, all sets (true locks vs every false lock)", all_locks),
        locks_music=table("LOCK ENTRY, music only (true vs false locks within music)", music_locks),
        locks_vs_beatless=table(
            "LOCK ENTRY, true music locks vs beatless/synthetic locks",
            [l for l in all_locks if l["true"] or l["ds"] in ("negatives", "synthetic")]),
        fires_all=table("ACQUISITION FIRES, all sets", all_fires),
        fires_music=table("ACQUISITION FIRES, music only", [x for x in all_fires if x["ds"] not in ("negatives", "synthetic")]),
    )
    # Per-set lock counts and false-lock share, the plain numbers behind the AUCs.
    print("\nper set: lock episodes, false share, median episode updates")
    per_set = {}
    for ds in sorted({l["ds"] for l in all_locks}):
        sel = [l for l in all_locks if l["ds"] == ds]
        fs = sum(1 for l in sel if not l["true"]) / len(sel)
        per_set[ds] = dict(n=len(sel), false_share=fs, median_updates=float(np.median([l["n_updates"] for l in sel])))
        print(f"  {ds:18} {len(sel):5} {fs:6.3f} {per_set[ds]['median_updates']:8.0f}")
    report["per_set"] = per_set
    if args.json:
        args.json.write_text(json.dumps(dict(report=report, locks=all_locks, fires=all_fires), indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
