#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""GiantSteps Tempo (664 EDM Beatport previews) — crowdsourced v2 tempo.

    bench/fetch_giantsteps_tempo.py fetch   # per-track mp3s, JKU mirror -> Beatport
    bench/fetch_giantsteps_tempo.py prep    # annotations_v2 -> normalized bundles
    bench/fetch_giantsteps_tempo.py verify

Ground truth is annotations_v2 (Schreiber & Mueller 2018 crowdsourced single
BPM); tracks whose v2 value is 0.0 are unusable per upstream and are skipped
with a recorded outcome. The v1 value and the two-tempo MIREX line ride in
provenance. Same refetched_mismatch policy as giantsteps_key.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import datasetlib as dl

REPO_DIR = "giantsteps-tempo-dataset"


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


def _read(path: Path) -> str | None:
    return path.read_text(encoding="utf-8").strip() if path.is_file() else None


def prep(ctx) -> None:
    repo = _repo(ctx)
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
            v2 = _read(repo / "annotations_v2" / "tempo" / f"{stem}.bpm")
            if not mp3.is_file() or v2 is None:
                ctx.status.track(stem, prep="missing audio or annotation")
                continue
            bpm = float(v2)
            if bpm <= 0.0:
                ctx.status.track(stem, prep="v2 tempo 0.0 (unusable per upstream)")
                continue
            bundle = {
                "schema": "fosfora-bench-annotation/v1",
                "dataset": "giantsteps_tempo",
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
                "tempo_bpm": bpm,
                "tempo_source": "crowdsourced_v2",
                "key": None,
                "segments": None,
                "drops": None,
                "stems": None,
                "annotators": None,
                "provenance": {
                    "annotation_file": f"{REPO_DIR}/annotations_v2/tempo/{stem}.bpm",
                    "v1_bpm": _read(repo / "annotations" / "tempo" / f"{stem}.bpm"),
                    "v2_mirex": _read(repo / "annotations_v2" / "mirex" / f"{stem}.mirex"),
                    "audio_fetch": fetched[stem],
                    "converter": "fetch_giantsteps_tempo.py prep",
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
    dl.write_index(ctx.dirs.norm, "giantsteps_tempo", index)


if __name__ == "__main__":
    sys.exit(dl.dataset_cli("giantsteps_tempo", fetch, prep))
