#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Score beat trackers head to head on identical audio.

    bench/td_compare/compare.py <dumps_root> <index.json> [--json out.json]

<dumps_root>/<system>/<dataset>__<track>.jsonl holds each system's events in
the Signal dump format (Fosfora writes it with --signal-dump; the TouchDesigner
logs are converted to it). Every system is scored with the bench's own beat
convention (mir_eval, 70 ms window, 5 s trim). On top of that:

- latency: one constant offset per system and dataset, chosen to maximize mean
  F-measure over -200..+200 ms, reported as the system's delay. The aligned
  scores use 2-fold cross-validation across tracks so the offset is never fit
  on the track it is scored on.
- jitter: median and IQR of the signed error of matched beats after alignment.
- fosfora@cook: Fosfora's beats moved up to the next 60 fps frame boundary,
  the time an in-TouchDesigner consumer could first see them.
- Wilcoxon signed-rank p against Fosfora, paired per track.
- tempo: the bench's Acc1/Acc2 on each system's /bpm (median of the final half).
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

import mir_eval.beat
import numpy as np
from scipy.stats import wilcoxon

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from benchlib.annotations import Annotations  # noqa: E402
from benchlib.dump import SignalDump  # noqa: E402
from benchlib.metrics.beats import _score  # noqa: E402
from benchlib.metrics.tempo import tempo as tempo_metric  # noqa: E402

OFFSETS = np.arange(-0.200, 0.2005, 0.001)
WINDOW = 0.07
TRIM = 5.0


def f_only(est, ref):
    r = mir_eval.beat.trim_beats(ref, min_beat_time=TRIM)
    e = mir_eval.beat.trim_beats(est, min_beat_time=TRIM)
    return mir_eval.beat.f_measure(r, e, f_measure_threshold=WINDOW)


def best_offset(pairs):
    means = [np.mean([f_only(est - o, ref) for est, ref in pairs]) for o in OFFSETS]
    return float(OFFSETS[int(np.argmax(means))])


def signed_errors(est, ref):
    errs = []
    for r in ref[ref >= TRIM]:
        if len(est) == 0:
            break
        i = int(np.argmin(np.abs(est - r)))
        if abs(est[i] - r) <= WINDOW:
            errs.append(est[i] - r)
    return errs


def load_beats(path):
    return np.asarray(SignalDump.load(path).beats(), dtype=float)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dumps_root", type=Path)
    ap.add_argument("index", type=Path)
    ap.add_argument("--json", type=Path)
    args = ap.parse_args()

    index = json.loads(args.index.read_text())
    systems = sorted(p.name for p in args.dumps_root.iterdir() if p.is_dir())
    refs = {(t["dataset"], t["track"]): np.asarray(Annotations.load(t["annotations"]).beats, dtype=float) for t in index}

    anns = {(t["dataset"], t["track"]): Annotations.load(t["annotations"]) for t in index}
    est, tempos = {}, {}
    for s in systems:
        for t in index:
            f = args.dumps_root / s / f"{t['dataset']}__{t['track']}.jsonl"
            if f.exists():
                dump = SignalDump.load(f)
                est[(s, t["dataset"], t["track"])] = np.asarray(dump.beats(), dtype=float)
                tm = tempo_metric(dump, anns[(t["dataset"], t["track"])])
                if not tm.get("no_estimate"):
                    tempos[(s, t["dataset"], t["track"])] = tm
    if "fosfora" in systems:
        systems.append("fosfora@cook")
        for (s, ds, tr), b in list(est.items()):
            if s == "fosfora":
                est[("fosfora@cook", ds, tr)] = np.ceil(b * 60.0 - 1e-9) / 60.0

    report = {}
    for ds in sorted({t["dataset"] for t in index}):
        tracks = sorted(t["track"] for t in index if t["dataset"] == ds)
        report[ds] = {}
        for s in systems:
            have = [tr for tr in tracks if (s, ds, tr) in est]
            if not have:
                continue
            raw = {tr: _score(est[(s, ds, tr)], refs[(ds, tr)]) for tr in have}
            pairs = {tr: (est[(s, ds, tr)], refs[(ds, tr)]) for tr in have}
            folds = [have[0::2], have[1::2]]
            aligned = {}
            for k in (0, 1):
                o = best_offset([pairs[tr] for tr in folds[k]]) if folds[k] else 0.0
                for tr in folds[1 - k]:
                    e, r = pairs[tr]
                    aligned[tr] = _score(e - o, r)
            latency = best_offset(list(pairs.values()))
            errs = sum((signed_errors(e - latency, r) for e, r in pairs.values()), [])
            q = np.percentile(errs, [25, 50, 75]) if errs else [math.nan] * 3
            row = {
                "n_tracks": len(have),
                "raw": {m: float(np.mean([raw[tr][m] for tr in have])) for m in ("f_measure", "cmlt", "amlt")},
                "aligned_cv": {m: float(np.mean([aligned[tr][m] for tr in have])) for m in ("f_measure", "cmlt", "amlt")},
                "latency_ms": round(latency * 1000),
                "jitter_ms": {"median": float(q[1] * 1000), "iqr": float((q[2] - q[0]) * 1000)},
                "per_track_f": {tr: raw[tr]["f_measure"] for tr in have},
            }
            tm = [tempos[(s, ds, tr)] for tr in have if (s, ds, tr) in tempos]
            if tm:
                row["tempo"] = {"acc1": float(np.mean([x["acc1"] for x in tm])),
                                "acc2": float(np.mean([x["acc2"] for x in tm])),
                                "locked_fraction": float(np.mean([x["locked_fraction"] for x in tm])),
                                "n": len(tm)}
            report[ds][s] = row
        base = report[ds].get("fosfora")
        for s, row in report[ds].items():
            if base is None or s == "fosfora":
                continue
            common = sorted(set(row["per_track_f"]) & set(base["per_track_f"]))
            a = [row["per_track_f"][tr] for tr in common]
            b = [base["per_track_f"][tr] for tr in common]
            row["wilcoxon_p_vs_fosfora"] = float(wilcoxon(a, b).pvalue) if len(common) > 5 and a != b else None

    for ds, rows in report.items():
        print(f"\n{ds}")
        print(f"  {'system':24} {'n':>3} {'F raw':>6} {'CMLt':>6} {'AMLt':>6} | {'F alig':>6} {'CMLt':>6} | {'lat ms':>6} {'jit IQR':>7} {'p':>7} | {'Acc1':>5} {'Acc2':>5}")
        for s, r in rows.items():
            p = r.get("wilcoxon_p_vs_fosfora")
            print(f"  {s:24} {r['n_tracks']:>3} {r['raw']['f_measure']:6.3f} {r['raw']['cmlt']:6.3f} {r['raw']['amlt']:6.3f} | "
                  f"{r['aligned_cv']['f_measure']:6.3f} {r['aligned_cv']['cmlt']:6.3f} | {r['latency_ms']:>6} "
                  f"{r['jitter_ms']['iqr']:7.1f} {('-' if p is None else f'{p:.3f}'):>7} | "
                  + (f"{r['tempo']['acc1']:5.2f} {r['tempo']['acc2']:5.2f}" if 'tempo' in r else f"{'-':>5} {'-':>5}"))
    if args.json:
        args.json.write_text(json.dumps(report, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
