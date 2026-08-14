#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# ///
"""Dump structure-path sidecars (27-dim fingerprint + label-machine inputs) for a
dataset's tune half (workstream Q4), or for loose audio files outside any dataset.

    bench/dump_structure_sidecar.py bench/datasets/harmonix [--jobs 4] [--all] [--force]
    bench/dump_structure_sidecar.py --files ~/Music/*.wav [--out-dir D] [--force]

Runs the fosfora binary once per track with `FOSFORA_STRUCTURE_SIDECAR` set, writing
`bench/out/structsweep/<dataset>/<track_id>.jsonl` — the production front-end's exact
per-tick structure inputs, for `bench/sweep_structure.py` to replay the boundary
back-end against. Tune-half only by default (`is_tune` sha256 parity — selection must
never read holdout); `--all` dumps every track. The `--signal-dump` output itself is
discarded: sweeps recompute boundaries from the sidecar.

`--files` names audio paths directly, for rounds whose corpus is a handful of hand-picked
tracks rather than a prepared dataset (board #2299). It keys output on the file stem and
defaults to `bench/out/dropsweep/music/`, matching the specimen sidecars already there.
It also KEEPS the `--signal-dump` output as `<stem>.signal.jsonl`, because a loose-file
round scores the wire with bench/score_dump.py and reads /section/boundary announcements
back — neither of which the sidecar carries. The three specimen sidecars were originally
made by hand-setting FOSFORA_STRUCTURE_SIDECAR, which is why the sidecar behind finding
#2259 was never on disk to re-derive.

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
    ap.add_argument("dataset", type=Path, nargs="?")
    ap.add_argument("--files", type=Path, nargs="+",
                    help="audio paths to dump directly, instead of a prepared dataset")
    ap.add_argument("--out-dir", type=Path, default=None,
                    help="override the output directory (--files defaults to dropsweep/music)")
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--all", action="store_true", help="dump holdout tracks too")
    ap.add_argument("--force", action="store_true", help="re-dump existing sidecars")
    args = ap.parse_args()

    if (args.dataset is None) == (args.files is None):
        ap.error("need either a dataset path or --files")

    binary = os.environ.get("FOSFORA_BIN", REPO / "target" / "release" / "fosfora")
    if not Path(binary).exists():
        sys.exit(f"binary not found: {binary} (set FOSFORA_BIN or build --release)")

    todo = []
    if args.files:
        out_dir = args.out_dir or REPO / "bench" / "out" / "dropsweep" / "music"
        out_dir.mkdir(parents=True, exist_ok=True)
        for audio in args.files:
            audio = audio.resolve()
            if not audio.exists():
                sys.exit(f"no such audio file: {audio}")
            # Stem-keyed, matching the specimen sidecars already in dropsweep/music.
            sidecar = out_dir / f"{audio.stem}.jsonl"
            if sidecar.exists() and not args.force:
                continue
            todo.append((audio.stem, audio, sidecar, out_dir / f"{audio.stem}.signal.jsonl"))
    else:
        index = json.loads((args.dataset / "norm" / "index.json").read_text())
        rows = index["tracks"] if isinstance(index, dict) else index
        out_dir = args.out_dir or REPO / "bench" / "out" / "structsweep" / args.dataset.name
        out_dir.mkdir(parents=True, exist_ok=True)
        for row in rows:
            tid = row["track_id"]
            if not args.all and not is_tune(tid):
                continue
            sidecar = out_dir / f"{tid}.jsonl"
            if sidecar.exists() and not args.force:
                continue
            # `audio` in the index is relative to norm/ (e.g. "../audio/<id>.mp3").
            audio = (args.dataset / "norm" / row["audio"]).resolve()
            todo.append((tid, audio, sidecar, None))

    print(f"structsweep: {len(todo)} tracks to dump -> {out_dir}", flush=True)

    def dump(job: tuple[str, Path, Path, Path | None]) -> str | None:
        tid, audio, sidecar, signal_out = job
        # PID in the temp name: two dumpers pointed at one directory would otherwise share
        # a .part path and interleave their writes into it, and the result still parses as
        # JSONL — a corrupt corpus that looks fine. (Observed; the tick-index check in
        # sweep_drop.py is the other half of the guard.)
        part = sidecar.with_suffix(f".jsonl.{os.getpid()}.part")
        # Same .part discipline for the wire dump when we keep it, for the same reason.
        sig_part = signal_out.with_suffix(f".jsonl.{os.getpid()}.part") if signal_out else None
        env = os.environ | {"FOSFORA_STRUCTURE_SIDECAR": str(part)}
        # Niced: a fan-out of these must never contend with whatever the human is doing
        # on this machine (#2206).
        r = subprocess.run(
            ["nice", "-n", "19", str(binary), "--signal-dump", str(audio),
             "--out", str(sig_part) if sig_part else os.devnull],
            env=env,
            capture_output=True,
            text=True,
        )
        if r.returncode != 0 or not part.exists() or part.stat().st_size == 0:
            part.unlink(missing_ok=True)
            if sig_part:
                sig_part.unlink(missing_ok=True)
            return f"{tid}: exit {r.returncode} {r.stderr.strip()[:200]}"
        if sig_part:
            if not sig_part.exists() or sig_part.stat().st_size == 0:
                part.unlink(missing_ok=True)
                sig_part.unlink(missing_ok=True)
                return f"{tid}: sidecar written but --signal-dump produced nothing"
            sig_part.rename(signal_out)
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
