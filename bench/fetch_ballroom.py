#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Ballroom (ISMIR 2004 tempo contest, 698 x 30 s) — beats, downbeats, tempo.

    bench/fetch_ballroom.py fetch    # data1.tar.gz (md5-pinned) + CPJKU annotations
    bench/fetch_ballroom.py prep     # .beats -> normalized bundles + index
    bench/fetch_ballroom.py verify

Audio is engine-readable wav — no transcode; bundles point into raw/. The
Sturm duplicate list lives in the manifest as exclusions (one member of each
pair keeps scoring). Tempo ground truth is the median inter-beat interval of
the CPJKU annotation (`tempo_source: derived_median_ibi`).
"""

from __future__ import annotations

import statistics
import sys
import wave
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import datasetlib as dl

ANNOTATIONS_DIR = "BallroomAnnotations"


def fetch(ctx) -> None:
    audio_src = dl.source(ctx.manifest, "audio")
    tarball = ctx.dirs.raw / "data1.tar.gz"
    dl.download(
        audio_src["url"],
        tarball,
        sha256=audio_src.get("sha256"),
        md5=audio_src.get("upstream_md5"),
    )
    ctx.status.record_file(ctx.dirs.base, tarball)
    if audio_src.get("sha256") is None:
        print(
            f"  PIN ME: manifests/ballroom.json sources[audio].sha256 = "
            f'"{dl.sha256_file(tarball)}"'
        )
    if not any(ctx.dirs.raw.glob("BallroomData/**/*.wav")):
        print("  unpacking ...")
        dl.unpack(tarball, ctx.dirs.raw, "tar.gz")

    ann_src = dl.source(ctx.manifest, "annotations")
    head = dl.git_clone_pinned(
        ann_src["url"], ctx.dirs.raw / ANNOTATIONS_DIR, ann_src.get("pin")
    )
    if ann_src.get("pin") is None:
        print(f'  PIN ME: manifests/ballroom.json sources[annotations].pin = "{head}"')

    wavs = list(ctx.dirs.raw.glob("BallroomData/**/*.wav"))
    beats = list((ctx.dirs.raw / ANNOTATIONS_DIR).glob("**/*.beats"))
    print(f"  {len(wavs)} wavs, {len(beats)} .beats files")
    for w in wavs:
        ctx.status.track(w.stem, fetch="ok")


def parse_beats(path: Path) -> tuple[list[float], list[float]]:
    """CPJKU .beats: 'time_sec beat_id' per line; beat_id 1 = downbeat."""
    beats, downbeats = [], []
    for line in path.read_text(encoding="utf-8").splitlines():
        parts = line.split()
        if not parts:
            continue
        t = float(parts[0])
        beats.append(t)
        if len(parts) > 1 and int(float(parts[1])) == 1:
            downbeats.append(t)
    return beats, downbeats


def prep(ctx) -> None:
    wavs = {p.stem: p for p in ctx.dirs.raw.glob("BallroomData/**/*.wav")}
    ann = {p.stem: p for p in (ctx.dirs.raw / ANNOTATIONS_DIR).glob("**/*.beats")}
    if not wavs:
        raise dl.DatasetError("no wavs under raw/BallroomData — run fetch first")
    excluded = {e["id"]: e["reason"] for e in ctx.manifest.get("exclusions") or []}

    stems = sorted(wavs)
    if ctx.args.only:
        stems = [s for s in stems if s in set(ctx.args.only)]
    if ctx.args.limit:
        stems = stems[: ctx.args.limit]

    index, ok = [], 0
    for stem in stems:
        if stem in excluded:
            ctx.status.track(stem, prep=f"excluded: {excluded[stem]}")
            continue
        if stem not in ann:
            ctx.status.track(stem, prep="no annotation")
            continue
        try:
            beats, downbeats = parse_beats(ann[stem])
            if len(beats) < 2:
                ctx.status.track(stem, prep="annotation has <2 beats")
                continue
            with wave.open(str(wavs[stem]), "rb") as w:
                sr = w.getframerate()
                duration = w.getnframes() / sr
            ibis = [b - a for a, b in zip(beats, beats[1:])]
            tempo = 60.0 / statistics.median(ibis)
            bundle = {
                "schema": "fosfora-bench-annotation/v1",
                "dataset": "ballroom",
                "track_id": stem,
                "audio": {
                    # bundles live in norm/, audio stays where the tar put it
                    "path": (Path("..") / wavs[stem].relative_to(ctx.dirs.base)).as_posix(),
                    "sr": sr,
                    "duration_s": round(duration, 4),
                    "sha256": None,
                    "offset_applied_s": 0.0,
                },
                "beats": beats,
                "downbeats": downbeats,
                "tempo_bpm": round(tempo, 4),
                "tempo_source": "derived_median_ibi",
                "key": None,
                "segments": None,
                "drops": None,
                "stems": None,
                "annotators": None,
                "provenance": {
                    "annotation_file": str(ann[stem].relative_to(ctx.dirs.raw)),
                    "converter": "fetch_ballroom.py prep",
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
            ok += 1
        except Exception as e:  # per-track: record and continue
            ctx.status.track(stem, prep=f"error: {e}")
    dl.write_index(ctx.dirs.norm, "ballroom", index)
    orphaned = sorted(set(ann) - set(wavs))
    if orphaned:
        print(f"  note: {len(orphaned)} .beats files with no wav (first: {orphaned[0]})")


if __name__ == "__main__":
    sys.exit(dl.dataset_cli("ballroom", fetch, prep))
