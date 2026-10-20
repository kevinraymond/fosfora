"""Shared layout, constants and track lists for the beat model's data pipeline.

Imported by the uv scripts beside it (they add this directory to sys.path). Everything
hangs off one work dir (`--work`, e.g. /mnt/t9/fosfora-work/q4163):

  <work>/raw/<source>/<id>.npz        dump_track.py output (raw per-hop evidence + events)
  <work>/raw/jobs.tsv                 source, id, audio path of every planned dump
  <work>/labels/<source>/<id>.npz     build_dataset.py output (targets, weights, split)
  <work>/labels/manifest.json         provenance, rules, counts
  <work>/data/batida/renders/         batida wav + exact label json (+ .spec.json)
  <work>/data/negatives_train/        beatless BY / CC0 clips for TRAINING + their json
  <work>/fma_on_disk.csv              the CC FMA tracks with license class and genre

Sources and their roles:
  batida            train / pick by seed (exact labels; ambient is all-negative)
  fma               train / pick by track id (Fosfora's own confident beats)
  negatives_train   train / pick by track id (all-negative)
  negatives_guard   eval only (the 155 #4154 clips, all-negative)
  harmonix, ballroom  eval only, TUNE halves (bench reference beats); holdout never read
"""

from __future__ import annotations

import csv
import hashlib
import json
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
BENCH = REPO / "bench" / "datasets"

SR = 44100
HOP = 512
HOP_HZ = SR / HOP  # 86.1328125

TRAIN_SOURCES = ("batida", "fma", "negatives_train")
EVAL_SOURCES = ("negatives_guard", "harmonix", "ballroom")
# Split of the training sources: the last digit of the seed / track id in PICK -> "pick"
# (model selection), else "train". Frozen; the bench TUNE halves are the evaluation sets
# and the bench HOLDOUT halves are read once, at the round's verdict, by the bench itself.
PICK_DIGITS = frozenset({8, 9})


def sha256_file(path, chunk=1 << 20):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while True:
            b = f.read(chunk)
            if not b:
                break
            h.update(b)
    return h.hexdigest()


def is_tune(track_id: str) -> bool:
    """The bench's canonical split (bench/analyze_beat_phase.py): sha256 parity, even = tune."""
    return int(hashlib.sha256(track_id.encode()).hexdigest(), 16) % 2 == 0


def split_of(source: str, track_id: str) -> str:
    if source in EVAL_SOURCES:
        return "eval"
    if source == "batida":
        seed = int(track_id.rsplit("-", 1)[1])
        return "pick" if seed % 10 in PICK_DIGITS else "train"
    return "pick" if int(track_id) % 10 in PICK_DIGITS else "train"


def jobs(work: Path):
    """(source, id, audio path) for every planned dump, from raw/jobs.tsv."""
    out = []
    with open(work / "raw" / "jobs.tsv") as f:
        for line in f:
            s, i, p = line.rstrip("\n").split("\t")
            out.append((s, i, Path(p)))
    return out


def raw_path(work: Path, source: str, track_id: str) -> Path:
    return work / "raw" / source / f"{track_id}.npz"


def labels_path(work: Path, source: str, track_id: str) -> Path:
    return work / "labels" / source / f"{track_id}.npz"


def dumped(work: Path, sources=None):
    """(source, id) of every planned dump whose raw npz exists."""
    return [(s, i) for s, i, _ in jobs(work)
            if (sources is None or s in sources) and raw_path(work, s, i).exists()]


def fma_rows(work: Path) -> dict[str, dict]:
    """fma_on_disk.csv rows (license class BY or CC0/PD only) keyed by the 6-digit id."""
    out = {}
    with open(work / "fma_on_disk.csv", newline="") as f:
        for r in csv.DictReader(f):
            if r["cls"] in ("BY", "CC0/PD"):
                out[f"{int(r['tid']):06d}"] = r
    return out


def batida_labels(work: Path, track_id: str) -> dict:
    return json.loads((work / "data" / "batida" / "renders" / f"{track_id}.json").read_text())


def bench_annotation(ds: str, track_id: str) -> dict:
    idx_path = BENCH / ds / "norm" / "index.json"
    idx = json.loads(idx_path.read_text())
    rows = idx["tracks"] if isinstance(idx, dict) else idx
    for t in rows:
        if t["track_id"] == track_id:
            return json.loads((idx_path.parent / t["annotations"]).read_text())
    raise KeyError(f"{ds}: {track_id} not in {idx_path}")


def negatives_train_rows(work: Path) -> dict[str, dict]:
    p = work / "data" / "negatives_train" / "negatives_train.json"
    if not p.exists():
        return {}
    return {f"{int(r['id']):06d}": r for r in json.loads(p.read_text())}
