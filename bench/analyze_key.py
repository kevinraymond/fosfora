#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Key-detection iteration lens over cached per-track results (workstream Q3).

    bench/analyze_key.py bench/out/results/giantsteps_key
    bench/analyze_key.py bench/out/q-iter/results/giantsteps_key \
        --baseline bench/out/results/giantsteps_key
    bench/analyze_key.py bench/out/results/giantsteps_key --split

Reads the per-track JSONs a `run_bench.py` run leaves behind (they carry
`estimated_key` + `ref_key`, so no re-scoring is needed) and prints what the
dataset mean hides: the error taxonomy, mode accuracy vs major recall, and the
tonic-interval histogram. `--baseline` diffs against another results dir
(per-track movers) or against the frozen dev_subset manifest baselines.
`--split` reports the tune/holdout halves separately — profile and constant
selection must only ever read the tune half.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from benchlib.metrics.key import credit_bucket

#: Tonic name -> pitch class, sharps and flats (both appear in annotations).
PITCH_CLASS = {
    "C": 0, "C#": 1, "Db": 1, "D": 2, "D#": 3, "Eb": 3, "E": 4, "F": 5,
    "F#": 6, "Gb": 6, "G": 7, "G#": 8, "Ab": 8, "A": 9, "A#": 10, "Bb": 10,
    "B": 11,
}


def is_tune(track_id: str) -> bool:
    # Canonical split rule: analyze_predict_drop.py:244 (sha256 parity).
    return int(hashlib.sha256(track_id.encode()).hexdigest(), 16) % 2 == 0


def load_tracks(results_dir: Path) -> list[dict]:
    tracks = []
    for p in sorted(results_dir.glob("*.json")):
        if p.name == "summary.json":
            continue
        d = json.loads(p.read_text())
        k = d["metrics"].get("key")
        if k is None:
            continue
        k = dict(k)
        k["track_id"] = d["track_id"]
        tracks.append(k)
    if not tracks:
        sys.exit(f"no key results under {results_dir}")
    return tracks


def split_key(key: str) -> tuple[int, str]:
    tonic, mode = key.rsplit(" ", 1)
    return PITCH_CLASS[tonic], mode


def report(tracks: list[dict]) -> dict:
    n = len(tracks)
    est = [t for t in tracks if not t.get("no_estimate")]
    taxonomy = {"exact": 0, "fifth": 0, "relative": 0, "parallel": 0, "other": 0,
                "none": n - len(est)}
    deltas = {d: 0 for d in range(12)}
    mode_ok = 0
    ref_major = major_ok = 0
    per_mode: dict[str, list[float]] = {"major": [], "minor": []}
    for t in tracks:
        ref_pc, ref_mode = split_key(t["ref_key"])
        per_mode[ref_mode].append(0.0 if t.get("no_estimate") else t["score"])
        if ref_mode == "major":
            ref_major += 1
        if t.get("no_estimate"):
            continue
        taxonomy[credit_bucket(t["score"])] += 1
        est_pc, est_mode = split_key(t["estimated_key"])
        deltas[(est_pc - ref_pc) % 12] += 1
        if est_mode == ref_mode:
            mode_ok += 1
            if ref_mode == "major":
                major_ok += 1
    return {
        "n": n,
        "mean": round(sum(0.0 if t.get("no_estimate") else t["score"] for t in tracks) / n, 4),
        "no_estimate_rate": round(taxonomy["none"] / n, 4),
        "taxonomy": taxonomy,
        "mode_accuracy": round(mode_ok / n, 4),
        "major_mode_recall": round(major_ok / ref_major, 4) if ref_major else None,
        "n_ref_major": ref_major,
        "per_mode_mean": {
            m: round(sum(v) / len(v), 4) if v else None for m, v in per_mode.items()
        },
        "tonic_delta_hist": deltas,
        "first_emit_mean": (
            round(sum(t["first_emit_ts"] for t in est) / len(est), 4) if est else None
        ),
    }


def print_report(title: str, r: dict) -> None:
    tax = r["taxonomy"]
    print(f"{title}: n={r['n']} mean={r['mean']} no_est={r['no_estimate_rate']}")
    print(
        f"  taxonomy  exact {tax['exact']}  fifth {tax['fifth']}  "
        f"relative {tax['relative']}  parallel {tax['parallel']}  "
        f"other {tax['other']}  none {tax['none']}"
    )
    print(
        f"  mode      acc {r['mode_accuracy']}  major_recall {r['major_mode_recall']}"
        f" (n_ref_major {r['n_ref_major']})  per-mode mean"
        f" major {r['per_mode_mean']['major']} / minor {r['per_mode_mean']['minor']}"
    )
    hist = "  ".join(f"Δ{d}:{c}" for d, c in r["tonic_delta_hist"].items() if c)
    print(f"  tonic Δ   {hist}")
    print(f"  first_emit mean {r['first_emit_mean']} s")


def diff_results(tracks: list[dict], base_tracks: list[dict]) -> None:
    base = {t["track_id"]: t for t in base_tracks}
    movers = []
    for t in tracks:
        b = base.get(t["track_id"])
        if b is None:
            continue
        b_bucket = "none" if b.get("no_estimate") else credit_bucket(b["score"])
        t_bucket = "none" if t.get("no_estimate") else credit_bucket(t["score"])
        if b_bucket != t_bucket:
            movers.append((t["track_id"], b, t, b_bucket, t_bucket))
    if not movers:
        print("movers: none")
        return
    up = sum(1 for _, b, t, *_ in movers
             if (0.0 if t.get("no_estimate") else t["score"])
             > (0.0 if b.get("no_estimate") else b["score"]))
    print(f"movers: {len(movers)} ({up} up, {len(movers) - up} down)")
    for tid, b, t, b_bucket, t_bucket in sorted(movers, key=lambda m: m[0]):
        b_est = "—" if b.get("no_estimate") else b["estimated_key"]
        t_est = "—" if t.get("no_estimate") else t["estimated_key"]
        print(
            f"  {tid}: ref {t['ref_key']}  {b_est} ({b_bucket}) -> {t_est} ({t_bucket})"
        )


def diff_manifest(r: dict, manifest: Path) -> None:
    frozen = json.loads(manifest.read_text())["baselines"]["giantsteps_key"]["key"]
    print(
        f"vs frozen baseline: mean {frozen['score']['mean']} -> {r['mean']}"
        f" ({r['mean'] - frozen['score']['mean']:+.4f})"
        f"  mode_acc {frozen['mode_accuracy']} -> {r['mode_accuracy']}"
        f"  major_recall {frozen['major_mode_recall']} -> {r['major_mode_recall']}"
    )
    for bucket, count in frozen["taxonomy"].items():
        now = r["taxonomy"].get(bucket, 0)
        if now != count:
            print(f"  taxonomy {bucket}: {count} -> {now} ({now - count:+d})")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("results", type=Path, help="results dir with per-track JSONs")
    ap.add_argument("--split", action="store_true", help="report tune/holdout halves")
    ap.add_argument(
        "--baseline",
        type=Path,
        help="results dir (per-track movers) or dev_subset.json (frozen baselines)",
    )
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    args = ap.parse_args()

    tracks = load_tracks(args.results)
    r = report(tracks)
    if args.json:
        out = {"all": r}
        if args.split:
            out["tune"] = report([t for t in tracks if is_tune(t["track_id"])])
            out["holdout"] = report([t for t in tracks if not is_tune(t["track_id"])])
        print(json.dumps(out, indent=1))
        return 0

    print_report("all", r)
    if args.split:
        print_report("tune", report([t for t in tracks if is_tune(t["track_id"])]))
        print_report("holdout", report([t for t in tracks if not is_tune(t["track_id"])]))
    if args.baseline:
        if args.baseline.is_dir():
            diff_results(tracks, load_tracks(args.baseline))
        else:
            diff_manifest(r, args.baseline)
    return 0


if __name__ == "__main__":
    sys.exit(main())
