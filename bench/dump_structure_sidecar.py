#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# ///
"""Dump structure-path sidecars (27-dim fingerprint + label-machine inputs) for a
dataset's tune half (workstream Q4).

    bench/dump_structure_sidecar.py bench/datasets/harmonix [--jobs 4] [--all] [--force]

Runs the fosfora binary once per track with `FOSFORA_STRUCTURE_SIDECAR` set, writing
`bench/out/structsweep/<dataset>/<track_id>.jsonl` — the production front-end's exact
per-tick structure inputs, for `bench/sweep_structure.py` to replay the boundary
back-end against. Tune-half only by default (`is_tune` sha256 parity — selection must
never read holdout); `--all` dumps every track. The `--signal-dump` output itself is
discarded: sweeps recompute boundaries from the sidecar.

Binary resolution matches benchlib.runner: $FOSFORA_BIN, else target/release/fosfora
(build it first: cargo build -p fosfora-app --features analyze --release).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent


def is_tune(track_id: str) -> bool:
    # Canonical split rule: analyze_predict_drop.py:244 (sha256 parity).
    return int(hashlib.sha256(track_id.encode()).hexdigest(), 16) % 2 == 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("dataset", type=Path)
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--all", action="store_true", help="dump holdout tracks too")
    ap.add_argument("--force", action="store_true", help="re-dump existing sidecars")
    args = ap.parse_args()

    binary = os.environ.get("FOSFORA_BIN", REPO / "target" / "release" / "fosfora")
    if not Path(binary).exists():
        sys.exit(f"binary not found: {binary} (set FOSFORA_BIN or build --release)")

    index = json.loads((args.dataset / "norm" / "index.json").read_text())
    rows = index["tracks"] if isinstance(index, dict) else index
    out_dir = REPO / "bench" / "out" / "structsweep" / args.dataset.name
    out_dir.mkdir(parents=True, exist_ok=True)

    todo = []
    for row in rows:
        tid = row["track_id"]
        if not args.all and not is_tune(tid):
            continue
        sidecar = out_dir / f"{tid}.jsonl"
        if sidecar.exists() and not args.force:
            continue
        # `audio` in the index is relative to norm/ (e.g. "../audio/<id>.mp3").
        audio = (args.dataset / "norm" / row["audio"]).resolve()
        todo.append((tid, audio, sidecar))

    print(f"structsweep: {len(todo)} tracks to dump -> {out_dir}", flush=True)

    def dump(job: tuple[str, Path, Path]) -> str | None:
        tid, audio, sidecar = job
        # PID in the temp name: two dumpers pointed at one directory would otherwise share
        # a .part path and interleave their writes into it, and the result still parses as
        # JSONL — a corrupt corpus that looks fine. (Observed; the tick-index check in
        # sweep_drop.py is the other half of the guard.)
        part = sidecar.with_suffix(f".jsonl.{os.getpid()}.part")
        env = os.environ | {"FOSFORA_STRUCTURE_SIDECAR": str(part)}
        # Niced: a fan-out of these must never contend with whatever the human is doing
        # on this machine (#2206).
        r = subprocess.run(
            ["nice", "-n", "19", str(binary), "--signal-dump", str(audio),
             "--out", os.devnull],
            env=env,
            capture_output=True,
            text=True,
        )
        if r.returncode != 0 or not part.exists() or part.stat().st_size == 0:
            part.unlink(missing_ok=True)
            return f"{tid}: exit {r.returncode} {r.stderr.strip()[:200]}"
        part.rename(sidecar)
        return None

    failures = []
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        for i, err in enumerate(pool.map(dump, todo), 1):
            if err:
                failures.append(err)
            if i % 25 == 0 or i == len(todo):
                print(f"  {i}/{len(todo)} ({len(failures)} failed)", flush=True)

    for f in failures:
        print(f"FAIL {f}", file=sys.stderr)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
