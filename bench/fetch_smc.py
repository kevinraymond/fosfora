#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""SMC_MIREX (Holzapfel et al. 2012, 217 x 40 s, hard beat cases) — beats only.

    bench/fetch_smc.py fetch    # canonical URL when up; else manual-drop flow
    bench/fetch_smc.py prep     # annotations -> normalized bundles + index
    bench/fetch_smc.py verify

Source reality (2026-08-06): the canonical INESC page refuses connections and
the one known author mirror (J. Zapata's SharePoint, via bit.ly/33SlutJ on
joserzapata.github.io) 403s for non-browser clients. So `fetch` tries the
canonical URL if the manifest ever carries one, and otherwise looks for a
manually dropped archive or unpacked tree under raw/ and prints instructions.
Per the fetch contract, total unavailability reports and exits 0.
"""

from __future__ import annotations

import statistics
import sys
import wave
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import datasetlib as dl

MIRROR_NOTE = """\
  SMC_MIREX is not script-fetchable right now:
    canonical:  http://smc.inescporto.pt/research/data-2/   (host down 2026-08)
    mirror:     https://bit.ly/33SlutJ  (author's SharePoint; needs a browser)
  Manual drop: download the archive in a browser and place it (or its unpacked
  tree containing SMC_*.wav + annotation txts) under:
    {raw}
  then re-run fetch."""


def _find_archives(raw: Path) -> list[Path]:
    return sorted(
        p for p in raw.iterdir()
        if p.is_file() and p.suffix.lower() in (".zip", ".gz", ".tgz", ".tar")
    )


def fetch(ctx) -> None:
    src = dl.source(ctx.manifest, "bundle")
    if src.get("url"):
        archive = ctx.dirs.raw / Path(src["url"]).name
        dl.download(
            src["url"], archive, sha256=src.get("sha256"), md5=src.get("upstream_md5")
        )
        ctx.status.record_file(ctx.dirs.base, archive)
        dl.unpack(archive, ctx.dirs.raw, src.get("unpack"))
    else:
        for archive in _find_archives(ctx.dirs.raw):
            print(f"  found manually dropped archive: {archive.name}")
            ctx.status.record_file(ctx.dirs.base, archive)
            if src.get("sha256"):
                dl.check_pins(archive, src["sha256"], None)
            else:
                print(
                    f'  PIN ME: manifests/smc.json sources[bundle].sha256 = '
                    f'"{dl.sha256_file(archive)}"'
                )
            dl.unpack(archive, ctx.dirs.raw)

    wavs = sorted(ctx.dirs.raw.glob("**/SMC_*.wav"))
    if not wavs:
        print(MIRROR_NOTE.format(raw=ctx.dirs.raw))
        return
    print(f"  {len(wavs)} SMC wavs present")
    for w in wavs:
        ctx.status.track(w.stem, fetch="ok")


def _annotation_for(stem: str, raw: Path) -> Path | None:
    """Find the beat annotation for SMC_NNN — the distribution has used both
    SMC_NNN.txt and longer suffixed names across versions; match loosely."""
    cands = [p for p in raw.glob(f"**/{stem}*.txt") if "readme" not in p.name.lower()]
    return sorted(cands, key=lambda p: len(p.name))[0] if cands else None


def prep(ctx) -> None:
    wavs = {p.stem: p for p in ctx.dirs.raw.glob("**/SMC_*.wav")}
    if not wavs:
        print(MIRROR_NOTE.format(raw=ctx.dirs.raw))
        return

    stems = sorted(wavs)
    if ctx.args.only:
        stems = [s for s in stems if s in set(ctx.args.only)]
    if ctx.args.limit:
        stems = stems[: ctx.args.limit]

    index = []
    for stem in stems:
        try:
            ann = _annotation_for(stem, ctx.dirs.raw)
            if ann is None:
                ctx.status.track(stem, prep="no annotation")
                continue
            beats = [
                float(line.split()[0])
                for line in ann.read_text(encoding="utf-8", errors="replace").splitlines()
                if line.strip()
            ]
            if len(beats) < 2:
                ctx.status.track(stem, prep="annotation has <2 beats")
                continue
            with wave.open(str(wavs[stem]), "rb") as w:
                sr = w.getframerate()
                duration = w.getnframes() / sr
            ibis = [b - a for a, b in zip(beats, beats[1:])]
            bundle = {
                "schema": "fosfora-bench-annotation/v1",
                "dataset": "smc",
                "track_id": stem,
                "audio": {
                    "path": (Path("..") / wavs[stem].relative_to(ctx.dirs.base)).as_posix(),
                    "sr": sr,
                    "duration_s": round(duration, 4),
                    "sha256": None,
                    "offset_applied_s": 0.0,
                },
                "beats": beats,
                "downbeats": None,
                # SMC is deliberately hard/rubato — a single median-IBI tempo is
                # not honest ground truth here; beats only (manifest signals).
                "tempo_bpm": None,
                "tempo_source": None,
                "key": None,
                "segments": None,
                "drops": None,
                "stems": None,
                "annotators": None,
                "provenance": {
                    "annotation_file": str(ann.relative_to(ctx.dirs.raw)),
                    "converter": "fetch_smc.py prep",
                    "median_ibi_s": round(statistics.median(ibis), 4),
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
    dl.write_index(ctx.dirs.norm, "smc", index)


if __name__ == "__main__":
    sys.exit(dl.dataset_cli("smc", fetch, prep))
