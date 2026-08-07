#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""GiantSteps Key (604 EDM Beatport previews) — key ground truth.

    bench/fetch_giantsteps_key.py fetch    # per-track mp3s, JKU mirror -> Beatport
    bench/fetch_giantsteps_key.py prep     # .key files -> normalized bundles
    bench/fetch_giantsteps_key.py verify

The md5/ dir of the pinned annotations repo is both the track list and the
per-file audio pin; refetched previews that re-encode are kept and counted as
refetched_mismatch (key ground truth is track-level). Partial availability is
normal and reported, never fatal.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import datasetlib as dl

REPO_DIR = "giantsteps-key-dataset"


def _repo(ctx) -> Path:
    return ctx.dirs.raw / REPO_DIR


def _track_files(ctx) -> dict[str, str]:
    md5_dir = _repo(ctx) / "md5"
    files = {}
    for p in sorted(md5_dir.glob("*.md5")):
        files[f"{p.stem}.mp3"] = p.read_text(encoding="utf-8").split()[0]
    if not files:
        raise dl.DatasetError("annotations repo has no md5/ entries — clone failed?")
    return files


def fetch(ctx) -> None:
    ann = dl.source(ctx.manifest, "annotations")
    dl.git_clone_pinned(ann["url"], _repo(ctx), ann.get("pin"))
    files = _track_files(ctx)
    if ctx.args.only:
        files = {f: m for f, m in files.items() if Path(f).stem in set(ctx.args.only)}
    if ctx.args.limit:
        files = dict(sorted(files.items())[: ctx.args.limit])
    dl.fetch_per_track(ctx, dl.source(ctx.manifest, "audio"), files, ctx.dirs.audio)


def parse_key(text: str) -> dict:
    parts = text.strip().split()
    if len(parts) != 2 or parts[1].lower() not in ("major", "minor"):
        raise ValueError(f"unparseable key {text!r}")
    return {"tonic": parts[0], "mode": parts[1].lower(), "raw": text.strip()}


def prep(ctx) -> None:
    key_dir = _repo(ctx) / "annotations" / "key"
    fetched = ctx.status.outcomes("fetch")
    index = []
    stems = sorted(
        tid for tid, v in fetched.items() if v in ("ok", "refetched_mismatch")
    )
    if ctx.args.only:
        stems = [s for s in stems if s in set(ctx.args.only)]
    if ctx.args.limit:
        stems = stems[: ctx.args.limit]
    for stem in stems:
        try:
            mp3 = ctx.dirs.audio / f"{stem}.mp3"
            key_file = key_dir / f"{stem}.key"
            if not mp3.is_file() or not key_file.is_file():
                ctx.status.track(stem, prep="missing audio or annotation")
                continue
            bundle = {
                "schema": "fosfora-bench-annotation/v1",
                "dataset": "giantsteps_key",
                "track_id": stem,
                "audio": {
                    "path": f"../audio/{stem}.mp3",
                    "sr": None,
                    "duration_s": None,
                    "sha256": None,
                    "offset_applied_s": 0.0,
                },
                "beats": None,
                "downbeats": None,
                "tempo_bpm": None,
                "tempo_source": None,
                "key": parse_key(key_file.read_text(encoding="utf-8")),
                "segments": None,
                "drops": None,
                "stems": None,
                "annotators": None,
                "provenance": {
                    "annotation_file": f"{REPO_DIR}/annotations/key/{stem}.key",
                    "audio_fetch": fetched[stem],
                    "converter": "fetch_giantsteps_key.py prep",
                },
            }
            dl.write_bundle(ctx.dirs.norm, bundle)
            index.append(
                {
                    "track_id": stem,
                    "audio": bundle["audio"]["path"],
                    "annotations": f"{stem}.json",
                }
            )
            ctx.status.track(stem, prep="ok")
        except Exception as e:
            ctx.status.track(stem, prep=f"error: {e}")
    dl.write_index(ctx.dirs.norm, "giantsteps_key", index)


if __name__ == "__main__":
    sys.exit(dl.dataset_cli("giantsteps_key", fetch, prep))
