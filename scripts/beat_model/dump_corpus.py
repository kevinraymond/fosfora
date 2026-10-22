#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""Dump the beat model's corpus with dump_track.py, several tracks at a time.

    dump_corpus.py --work /mnt/t9/fosfora-work/q4163 --bin <fosfora> [--jobs 6]
                   [--only fma,batida] [--dry-run]

Sources, each under the work dir unless pointed elsewhere:
  batida            <work>/data/batida/renders/*.wav          exact synthetic labels
  fma               <work>/fma_on_disk.csv rows with cls BY or CC0/PD, audio under
                    --fma-audio (audio/, audio2/, audio3/, audio4/: #4288's non-electronic tracks), a <work>/dumps/transcoded/<tid>.wav
                    standing in where one exists (mp3s the decoder refused)
  negatives_train   <work>/data/negatives_train/clips/*.mp3   beatless, BY / CC0, training
  negatives_guard   --guard-clips (the 155 #4154 clips)       beatless, evaluation only
  harmonix, ballroom  --bench/<ds>/norm/index.json, TUNE halves only (sha256 parity even);
                    holdout audio is never listed

Output: <work>/raw/<source>/<id>.npz, plus raw/jobs.<source>.tsv (what was planned, one file
per source so `--only` reruns leave the others' lists alone), raw/run.log and raw/fail.log.
Existing npz are skipped, so a rerun picks up new clips and retries failures.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import dump_track  # noqa: E402

REPO = Path(__file__).resolve().parents[2]


def is_tune(track_id: str) -> bool:
    """The bench's canonical split (bench/analyze_beat_phase.py): sha256 parity, even = tune."""
    return int(hashlib.sha256(track_id.encode()).hexdigest(), 16) % 2 == 0


def plan(work: Path, fma_audio: Path, guard_clips: Path | None, bench: Path, only: set[str] | None):
    jobs: list[tuple[str, str, Path]] = []  # (source, id, audio)

    def want(src):
        return only is None or src in only

    if want("batida"):
        for wav in sorted((work / "data" / "batida" / "renders").glob("*.wav")):
            if wav.with_suffix(".json").exists():
                jobs.append(("batida", wav.stem, wav))
    if want("fma"):
        transcoded = work / "dumps" / "transcoded"
        with open(work / "fma_on_disk.csv", newline="") as f:
            for r in csv.DictReader(f):
                if r["cls"] not in ("BY", "CC0/PD"):
                    continue
                tid = f"{int(r['tid']):06d}"
                cands = [transcoded / f"{tid}.wav"] + [fma_audio / d / f"{tid}.mp3" for d in ("audio", "audio2", "audio3", "audio4")]
                src = next((p for p in cands if p.exists()), None)
                if src is not None:
                    jobs.append(("fma", tid, src))
    if want("negatives_train"):
        for mp3 in sorted((work / "data" / "negatives_train" / "clips").glob("*.mp3")):
            jobs.append(("negatives_train", mp3.stem, mp3))
    if want("negatives_guard") and guard_clips is not None:
        for mp3 in sorted(guard_clips.glob("*.mp3")):
            jobs.append(("negatives_guard", mp3.stem, mp3))
    for ds in ("harmonix", "ballroom"):
        if not want(ds):
            continue
        idx_path = bench / ds / "norm" / "index.json"
        if not idx_path.exists():
            print(f"warning: {idx_path} missing, {ds} skipped", file=sys.stderr)
            continue
        idx = json.loads(idx_path.read_text())
        rows = idx["tracks"] if isinstance(idx, dict) else idx
        for t in rows:
            if is_tune(t["track_id"]):
                jobs.append((ds, t["track_id"], (idx_path.parent / t["audio"]).resolve()))
    return jobs


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--work", type=Path, required=True)
    ap.add_argument("--bin", type=Path, required=True)
    ap.add_argument("--jobs", type=int, default=6)
    ap.add_argument("--only", help="comma-separated sources")
    ap.add_argument("--fma-audio", type=Path, default=Path.home() / "ai" / "audio" / "kick-work" / "fma")
    ap.add_argument("--guard-clips", type=Path, default=Path.home() / "ai" / "audio" / "kick-work" / "fma" / "negatives" / "clips")
    ap.add_argument("--bench", type=Path, default=REPO / "bench" / "datasets")
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()
    if not a.bin.exists():
        sys.exit(f"binary not found: {a.bin}")
    only = set(a.only.split(",")) if a.only else None
    jobs = plan(a.work, a.fma_audio, a.guard_clips if a.guard_clips.exists() else None, a.bench, only)
    raw = a.work / "raw"
    raw.mkdir(parents=True, exist_ok=True)
    for src in {s for s, _, _ in jobs}:
        (raw / f"jobs.{src}.tsv").write_text("".join(f"{i}\t{p}\n" for s, i, p in jobs if s == src))
    todo = [(s, i, p) for s, i, p in jobs if not (raw / s / f"{i}.npz").exists()]
    by_src = {}
    for s, _, _ in jobs:
        by_src[s] = by_src.get(s, 0) + 1
    print(f"{len(jobs)} tracks {by_src}; {len(todo)} to dump with {a.jobs} jobs", flush=True)
    if a.dry_run:
        for s, i, p in todo[:10]:
            print(f"  {s}\t{i}\t{p}")
        return 0
    t0 = time.time()
    done = failed = 0
    with open(raw / "run.log", "a") as log, open(raw / "fail.log", "a") as flog, \
            ThreadPoolExecutor(max_workers=a.jobs) as ex:
        futs = {ex.submit(dump_track.run, a.bin, p, raw / s / f"{i}.npz"): (s, i, p) for s, i, p in todo}
        for fut in as_completed(futs):
            s, i, p = futs[fut]
            try:
                m = fut.result()
                done += 1
                log.write(f"{s}/{i}: {m['n_hops']} hops, {m['n_beats']} beats, {m['silent_hops']} silent, "
                          f"{m['dump_seconds']} s\n")
            except Exception as e:  # noqa: BLE001
                failed += 1
                flog.write(f"{s}\t{i}\t{p}\t{type(e).__name__}: {e}\n")
                flog.flush()
            log.flush()
            n = done + failed
            if n % 25 == 0 or n == len(todo):
                el = time.time() - t0
                print(f"  {n}/{len(todo)} ({failed} failed) {el:.0f} s, ~{el / n * (len(todo) - n):.0f} s left", flush=True)
    print(f"done: {done} dumped, {failed} failed, {time.time() - t0:.0f} s; "
          f"{sum(1 for _ in raw.glob('*/*.npz'))} npz under {raw}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
