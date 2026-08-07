#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Dump + score one dataset against its normalized annotations.

    bench/run_bench.py bench/datasets/ballroom [--jobs 4] [--only ID] [--force]

Expects `<dataset>/norm/index.json` (fosfora-bench-index/v1) as produced by the
dataset fetch scripts' `prep` step, or by `make_fixture.py`. Dumps are cached
under bench/out/dumps/<dataset>/ keyed on audio+binary+flags; per-track results
and summary.json land under bench/out/results/<dataset>/.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from benchlib import metrics, results
from benchlib.annotations import Annotations, load_index
from benchlib.dump import SignalDump
from benchlib.runner import DumpRunner, repo_root


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("dataset", type=Path, help="dataset dir containing norm/index.json")
    ap.add_argument("--jobs", type=int, default=4, help="parallel dump processes")
    ap.add_argument("--only", action="append", help="score only these track ids")
    ap.add_argument("--force", action="store_true", help="re-run dumps even if cached")
    ap.add_argument(
        "--out-root",
        type=Path,
        default=None,
        help="output root (default <repo>/bench/out)",
    )
    args = ap.parse_args()

    index_path = args.dataset / "norm" / "index.json"
    if not index_path.is_file():
        # make_fixture.py writes a flat dir: allow <dataset>/index.json too.
        index_path = args.dataset / "index.json"
    if not index_path.is_file():
        print(f"error: no index.json under {args.dataset}", file=sys.stderr)
        return 2
    tracks = load_index(index_path)
    if args.only:
        wanted = set(args.only)
        tracks = [t for t in tracks if t["track_id"] in wanted]
        missing = wanted - {t["track_id"] for t in tracks}
        if missing:
            print(f"error: unknown track ids {sorted(missing)}", file=sys.stderr)
            return 2
    if not tracks:
        print("error: no tracks to score", file=sys.stderr)
        return 2

    dataset_name = args.dataset.resolve().name
    out_root = args.out_root or (repo_root() / "bench" / "out")
    runner = DumpRunner(out_root / "dumps" / dataset_name)
    results_dir = out_root / "results" / dataset_name

    print(
        f"bench: {dataset_name} — {len(tracks)} tracks, "
        f"binary {runner.binary_sha256[:16]}, flags {runner.flags}"
    )
    dump_paths = runner.ensure_dumps(
        [t["audio"] for t in tracks], jobs=args.jobs, force=args.force
    )

    scored, failed = 0, []
    for t in tracks:
        dump_path = dump_paths[Path(t["audio"])]
        if isinstance(dump_path, Exception):
            failed.append((t["track_id"], str(dump_path)))
            continue
        dump = SignalDump.load(dump_path)
        ann = Annotations.load(t["annotations"])
        blocks = metrics.score_all(dump, ann)
        result = results.make_result(
            dataset=dataset_name,
            track_id=t["track_id"],
            dump_info={
                "cache_key": dump_path.stem,
                "binary_sha256": runner.binary_sha256,
                "hop_hz": dump.hop_hz,
                "tx_rate_hz": dump.tx_rate_hz,
                "n_events": {
                    "beat": len(dump.beats()),
                    "downbeat": len(dump.downbeats()),
                    "drop": len(dump.drops()),
                },
            },
            conventions=metrics.CONVENTIONS,
            metrics=blocks,
        )
        results.write_result(results_dir / f"{t['track_id']}.json", result)
        scored += 1

    all_results = results.load_results(results_dir)
    summary = results.aggregate(all_results, metrics.AGGREGATORS)
    summary["coverage"] = {
        "requested": len(tracks),
        "scored": scored,
        "dump_failures": failed,
    }
    results.write_result(results_dir / "summary.json", summary)

    print(f"bench: scored {scored}/{len(tracks)} -> {results_dir}")
    for track_id, err in failed:
        print(f"bench:   FAILED {track_id}: {err}", file=sys.stderr)
    return 0 if scored else 1


if __name__ == "__main__":
    sys.exit(main())
