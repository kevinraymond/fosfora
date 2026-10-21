#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "pandas", "remotezip", "librosa", "soundfile"]
# ///
"""Beatless TRAINING negatives for the beat model: CC BY / CC0 30 s clips from fma_large in
Ambient, Drone, Field Recordings and Sound Art, disjoint from the clips already used as the
#4154 evaluation guards, each with an independent pulse score (librosa onset-envelope
autocorrelation, deliberately not Fosfora's estimator), and kept only when that score says
the clip is beatless.

    fetch_negatives.py --out <work>/data/negatives_train --metadata <fma_metadata dir>
                       --exclude negatives.json [--exclude more.json] [--per-genre 75]
                       [--max-pulse 0.35] [--seed 4163]

Writes <out>/clips/<id>.mp3 and <out>/negatives_train.json (id, genre, license, artist,
title, url, pulse). Same source and selection shape as kick-work/fma/negatives.py, with a
different seed and ShareAlike excluded (a shipped model's training data is BY / CC0 only).
"""

from __future__ import annotations

import argparse
import ast
import json
import sys
from pathlib import Path

import librosa
import numpy as np
import pandas as pd
from remotezip import RemoteZip

BEATLESS = ["Ambient", "Drone", "Field Recordings", "Sound Art"]
BEAT = ["Hip-Hop", "Rock", "Pop", "Techno", "House", "Dance", "Drum & Bass", "Dubstep",
        "Breakbeat", "Jungle", "Trip-Hop", "Downtempo", "Electro", "Disco", "Funk",
        "Reggae - Dub", "Glitch", "IDM", "Chip Music", "Trap", "Footwork"]
FMA_LARGE = "https://os.unil.cloud.switch.ch/fma/fma_large.zip"


def pulse(path: Path) -> float:
    y, sr = librosa.load(str(path), sr=22050, mono=True)
    env = librosa.onset.onset_strength(y=y, sr=sr, hop_length=256)
    env = env - env.mean()
    if env.std() < 1e-9:
        return 0.0
    ac = np.correlate(env, env, mode="full")[len(env) - 1:]
    ac = ac / ac[0]
    fr = sr / 256
    lo, hi = int(60 * fr / 300), int(60 * fr / 40)
    return float(ac[lo:hi + 1].max())


def excluded_ids(paths):
    ids = set()
    for p in paths:
        d = json.loads(Path(p).read_text())
        rows = d if isinstance(d, list) else list(d.values())
        for r in rows:
            if isinstance(r, dict):
                v = r.get("id", r.get("tid"))
            else:
                v = r[0] if isinstance(r, (list, tuple)) else r
            if v is not None:
                ids.add(int(v))
    return ids


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--metadata", type=Path, required=True, help="dir with tracks.csv and genres.csv")
    ap.add_argument("--exclude", action="append", default=[], help="json with ids to leave out (repeatable)")
    ap.add_argument("--per-genre", type=int, default=75)
    ap.add_argument("--max-pulse", type=float, default=0.35)
    ap.add_argument("--seed", type=int, default=4163)
    a = ap.parse_args()

    t = pd.read_csv(a.metadata / "tracks.csv", index_col=0, header=[0, 1])
    g = pd.read_csv(a.metadata / "genres.csv", index_col=0)
    ids = {v: k for k, v in g["title"].items()}
    lic = t[("track", "license")].fillna("")
    by_or_cc0 = (
        lic.str.contains("Attribution", case=False)
        & ~lic.str.contains("NonCommercial|Noncommercial|NoDerivatives|No Derivative|ShareAlike|Share Alike",
                            case=False, regex=True)
    ) | lic.str.contains("Public Domain|CC0", case=False, regex=True)
    gall = t[("track", "genres_all")].apply(ast.literal_eval)
    beat_ids = {ids[b] for b in BEAT if b in ids}
    skip = excluded_ids(a.exclude)
    print(f"excluding {len(skip)} ids", flush=True)

    rng = np.random.default_rng(a.seed)
    chosen = []
    for gname in BEATLESS:
        gid = ids[gname]
        pool = [int(i) for i in t.index[by_or_cc0]
                if gid in gall[i] and not (set(gall[i]) & beat_ids) and int(i) not in skip]
        rng.shuffle(pool)
        print(f"{gname}: {len(pool)} candidates", flush=True)
        chosen += [(i, gname) for i in pool[: a.per_genre * 3]]  # spares: missing members, pulse rejects

    clips = a.out / "clips"
    clips.mkdir(parents=True, exist_ok=True)
    rows, per, rejected = [], {gname: 0 for gname in BEATLESS}, 0
    with RemoteZip(FMA_LARGE) as z:
        names = set(z.namelist())
        for tid, gname in chosen:
            if per[gname] >= a.per_genre:
                continue
            n = f"fma_large/{tid // 1000:03d}/{tid:06d}.mp3"
            if n not in names:
                continue
            p = clips / f"{tid:06d}.mp3"
            if not p.is_file():
                p.write_bytes(z.read(n))
            try:
                pc = pulse(p)
            except Exception as e:  # noqa: BLE001  undecodable clip: drop it
                print("skip", tid, e, flush=True)
                p.unlink(missing_ok=True)
                continue
            if pc > a.max_pulse:
                rejected += 1
                p.unlink(missing_ok=True)
                continue
            per[gname] += 1
            rows.append(dict(
                id=int(tid), genre=gname, license=str(t.loc[tid, ("track", "license")]),
                artist=str(t.loc[tid, ("artist", "name")]), title=str(t.loc[tid, ("track", "title")]),
                url=f"https://freemusicarchive.org/track/{tid}", pulse=round(pc, 4)))
            if len(rows) % 25 == 0:
                print(f"  {len(rows)} kept, {rejected} rejected by pulse", flush=True)
                (a.out / "negatives_train.json").write_text(json.dumps(rows, indent=0))
    (a.out / "negatives_train.json").write_text(json.dumps(rows, indent=0))
    print(f"kept {len(rows)} clips {per}, rejected {rejected} with pulse > {a.max_pulse}; "
          f"{sum(1 for _ in clips.glob('*.mp3'))} mp3 under {clips}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
