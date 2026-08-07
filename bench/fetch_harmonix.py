#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["yt-dlp"]
# ///
"""Harmonix Set (912 pop/EDM tracks) — beats, downbeats, tempo, segments, drops.

    bench/fetch_harmonix.py fetch [--limit N]   # annotations + melspecs + yt-dlp audio
    bench/align_harmonix.py run [--limit N]     # the alignment gate (separate entry)
    bench/fetch_harmonix.py prep                # gated tracks -> normalized bundles
    bench/fetch_harmonix.py verify

Audio is YouTube-sourced per the upstream CSV and may be a different master or
edit than what was annotated — prep therefore refuses any track without a
pass* verdict from the alignment gate, and applies the gate's measured offset
to every annotation time (bundle times live on the fetched file's clock).

Unavailable/removed/region-locked videos are normal per-track failures.
Requires ffmpeg (FLAC 44.1 kHz transcode; raw yt audio is kept — it is not
refetchable byte-identically).
"""

from __future__ import annotations

import csv
import re
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import datasetlib as dl

REPO_DIR = "harmonixset"
DROP_LABEL = re.compile(r"(^|_)drop")
PROXY_GENRES = {"Dance/Electronic", "Dubstep"}


def _repo(ctx) -> Path:
    return ctx.dirs.raw / REPO_DIR


def _csv_rows(path: Path) -> list[dict]:
    with path.open(encoding="utf-8") as f:
        return list(csv.DictReader(f))


def load_tables(ctx) -> dict[str, dict]:
    """File id -> {url, duration, bpm, genre, upstream_score} from the pinned CSVs."""
    ds = _repo(ctx) / "dataset"
    meta = {r["File"]: r for r in _csv_rows(ds / "metadata.csv")}
    urls = {r["File"]: r["URL"] for r in _csv_rows(ds / "youtube_urls.csv")}
    scores = {
        r["File"]: float(r["score"])
        for r in _csv_rows(ds / "youtube_alignment_scores.csv")
    }
    out = {}
    for file_id, m in meta.items():
        out[file_id] = {
            "url": urls.get(file_id),
            "duration": float(m["Duration"]) if m.get("Duration") else None,
            "bpm": float(m["BPM"]) if m.get("BPM") else None,
            "genre": m.get("Genre", ""),
            "upstream_score": scores.get(file_id),
        }
    return out


def fetch(ctx) -> None:
    ann = dl.source(ctx.manifest, "annotations")
    dl.git_clone_pinned(ann["url"], _repo(ctx), ann.get("pin"))

    mel = dl.source(ctx.manifest, "melspecs")
    mel_dir = ctx.dirs.raw / "melspecs"
    if not mel_dir.is_dir() or not any(mel_dir.iterdir()):
        tgz = ctx.dirs.raw / "Harmonix_melspecs.tgz"
        dl.download(mel["url"], tgz, sha256=mel.get("sha256"))
        ctx.status.record_file(ctx.dirs.base, tgz)
        if mel.get("sha256") is None:
            print(
                f'  PIN ME: manifests/harmonix.json sources[melspecs].sha256 = '
                f'"{dl.sha256_file(tgz)}"'
            )
        print("  unpacking melspecs ...")
        dl.unpack(tgz, mel_dir, "tar.gz")

    tracks = load_tables(ctx)
    ids = sorted(tracks)
    if ctx.args.only:
        ids = [i for i in ids if i in set(ctx.args.only)]
    if ctx.args.limit:
        ids = ids[: ctx.args.limit]

    import yt_dlp

    yt_dir = ctx.dirs.raw / "yt"
    yt_dir.mkdir(exist_ok=True)
    full_dir = ctx.dirs.raw / "full"  # whole-video FLACs; prep cuts the song span
    full_dir.mkdir(exist_ok=True)
    done = 0
    for file_id in ids:
        flac = full_dir / f"{file_id}.flac"
        if flac.is_file() and not ctx.args.force:
            ctx.status.track(file_id, fetch="ok")
            done += 1
            continue
        url = tracks[file_id]["url"]
        if not url:
            ctx.status.track(file_id, fetch="no youtube url in upstream csv")
            continue
        try:
            raw = next(iter(yt_dir.glob(f"{file_id}.*")), None)
            if raw is None or ctx.args.force:
                opts = {
                    "format": "bestaudio[ext=m4a]/bestaudio",
                    "outtmpl": str(yt_dir / f"{file_id}.%(ext)s"),
                    "quiet": True,
                    "no_warnings": True,
                    "noplaylist": True,
                    "retries": 2,
                }
                with yt_dlp.YoutubeDL(opts) as ydl:
                    ydl.download([url])
                raw = next(iter(yt_dir.glob(f"{file_id}.*")), None)
            if raw is None:
                ctx.status.track(file_id, fetch="yt-dlp produced no file")
                continue
            dl.transcode_flac(raw, flac, sr=44100)
            ctx.status.track(file_id, fetch="ok")
            done += 1
        except Exception as e:
            msg = str(e).splitlines()[0][:160]
            ctx.status.track(file_id, fetch=f"unavailable: {msg}")
        if done and done % 10 == 0:
            print(f"\r  {done}/{len(ids)} fetched", end="", flush=True)
            ctx.status.save()  # long runs: checkpoint
    print(f"\r  {done}/{len(ids)} fetched")


def parse_beats(path: Path) -> tuple[list[float], list[float]]:
    """'time<TAB>beat_in_bar<TAB>bar' rows; beat_in_bar 1 = downbeat."""
    beats, downbeats = [], []
    for line in path.read_text(encoding="utf-8").splitlines():
        parts = line.split()
        if len(parts) < 2:
            continue
        t = float(parts[0])
        beats.append(t)
        if int(float(parts[1])) == 1:
            downbeats.append(t)
    return beats, downbeats


def parse_segments(path: Path) -> list[tuple[float, str]]:
    rows = []
    for line in path.read_text(encoding="utf-8").splitlines():
        parts = line.split(None, 1)
        if parts:
            rows.append((float(parts[0]), parts[1].strip() if len(parts) > 1 else ""))
    return rows


def derive_drops(intervals: list[tuple[float, float, str]], genre: str) -> list[dict]:
    drops = []
    for i, (start, _end, label) in enumerate(intervals):
        if DROP_LABEL.search(label):
            drops.append({"time": start, "source": f"label:{label}", "kind": "direct"})
        elif (
            i > 0
            and genre in PROXY_GENRES
            and label.startswith("chorus")
            and not intervals[i - 1][2].startswith("chorus")
        ):
            drops.append(
                {"time": start, "source": f"transition:{intervals[i-1][2]}->{label}",
                 "kind": "proxy_chorus_onset"}
            )
    return drops


def prep(ctx) -> None:
    tracks = load_tables(ctx)
    ds = _repo(ctx) / "dataset"
    alignment = ctx.status.data.get("alignment", {})
    fetched = ctx.status.outcomes("fetch")
    label_histogram: Counter[str] = Counter()

    ids = sorted(tid for tid, v in fetched.items() if v == "ok")
    if ctx.args.only:
        ids = [i for i in ids if i in set(ctx.args.only)]
    if ctx.args.limit:
        ids = ids[: ctx.args.limit]

    index = []
    for file_id in ids:
        try:
            verdict = alignment.get(file_id, {})
            v = verdict.get("verdict", "missing")
            if not v.startswith("pass"):
                ctx.status.track(
                    file_id,
                    prep="awaiting alignment" if v == "missing" else f"gate: {v}",
                )
                continue
            offset = float(verdict.get("offset_s", 0.0))

            beats_f = ds / "beats_and_downbeats" / f"{file_id}.txt"
            segs_f = ds / "segments" / f"{file_id}.txt"
            if not beats_f.is_file() or not segs_f.is_file():
                ctx.status.track(file_id, prep="missing annotation file")
                continue
            beats, downbeats = parse_beats(beats_f)
            rows = parse_segments(segs_f)
            for _, label in rows:
                label_histogram[label] += 1

            # Cut the aligned song span out of the whole-video FLAC so no
            # unannotated video intro/outro contaminates precision. Positive
            # offset: the cut moves the clock, times stay untouched. Negative
            # offset (YouTube trimmed the song's head): nothing to cut at the
            # front — annotation times shift by the offset instead and events
            # in the missing head are dropped (they sit inside the 5 s trim).
            flac = ctx.dirs.audio / f"{file_id}.flac"
            expected = tracks[file_id]["duration"]
            cut_start = max(0.0, offset)
            shift = offset - cut_start  # 0 for positive offsets, else negative
            if not flac.is_file() or ctx.args.force:
                dl.cut_flac(
                    ctx.dirs.raw / "full" / f"{file_id}.flac", flac,
                    cut_start, expected,
                )
            duration = dl.ffprobe_duration(flac)

            def clock(t: float) -> float:
                return t + shift

            beats = [clock(t) for t in beats if 0.0 <= clock(t) <= duration]
            downbeats = [clock(t) for t in downbeats if 0.0 <= clock(t) <= duration]

            # Segment rows are starts; the final row ('end'/'silence') marks
            # the close of the last real segment. Clip to the cut clock.
            intervals = []
            for i, (start, label) in enumerate(rows[:-1]):
                end = rows[i + 1][0]
                s = max(0.0, clock(start))
                e = min(duration, clock(end))
                if e > s:
                    intervals.append((s, e, label))
            drops = derive_drops(intervals, tracks[file_id]["genre"])

            bundle = {
                "schema": "fosfora-bench-annotation/v1",
                "dataset": "harmonix",
                "track_id": file_id,
                "audio": {
                    "path": f"../audio/{file_id}.flac",
                    "sr": 44100,
                    "duration_s": round(duration, 4),
                    "sha256": None,
                    # 0 when the cut moved the clock; negative when a trimmed
                    # song head forced an annotation shift instead
                    "offset_applied_s": round(shift, 4),
                },
                "beats": beats,
                "downbeats": downbeats,
                "tempo_bpm": tracks[file_id]["bpm"],
                "tempo_source": "annotated",
                "key": None,
                "segments": [[s, e, label] for s, e, label in intervals],
                "drops": drops,
                "stems": None,
                "annotators": None,
                "provenance": {
                    "annotation_files": [
                        f"dataset/beats_and_downbeats/{file_id}.txt",
                        f"dataset/segments/{file_id}.txt",
                    ],
                    "genre": tracks[file_id]["genre"],
                    "alignment": verdict,
                    "audio_cut_start_s": cut_start,
                    "head_missing_s": round(-shift, 4),
                    "converter": "fetch_harmonix.py prep",
                },
            }
            dl.write_bundle(ctx.dirs.norm, bundle)
            index.append(
                {
                    "track_id": file_id,
                    "audio": bundle["audio"]["path"],
                    "annotations": f"{file_id}.json",
                }
            )
            ctx.status.track(file_id, prep="ok")
        except Exception as e:
            ctx.status.track(file_id, prep=f"error: {e}")
    dl.write_index(ctx.dirs.norm, "harmonix", index)

    # The audit the drop-derivation honesty rests on: what labels actually exist.
    ctx.status.data["segment_label_histogram"] = dict(label_histogram.most_common())
    direct = sum(1 for label in label_histogram if DROP_LABEL.search(label))
    print(f"  segment labels seen: {len(label_histogram)} distinct; "
          f"{direct} matching the direct drop pattern")


if __name__ == "__main__":
    sys.exit(dl.dataset_cli("harmonix", fetch, prep))
