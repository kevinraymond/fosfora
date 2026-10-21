#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pandas"]
# ///
"""Write the beat model's training-data credits: every Free Music Archive track that
contributed rows, with artist, title, license and FMA id, as the CC BY licenses require.

    credits.py --work <work> --metadata <fma_metadata dir> \
               --out crates/fosfora-app/src/audio/beat_model_tracks.md

Music tracks are the fma_on_disk rows whose labels carried at least one labeled hop (the
labels manifest); beatless clips are every negatives_train clip. Titles come from the FMA
metadata (tracks.csv); the clips' json already carries them.
"""

from __future__ import annotations

import argparse
import csv
import json
import sys
from pathlib import Path

import pandas as pd


def table(rows):
    out = ["| Track | Artist | License | FMA id |", "|---|---|---|---|"]
    for r in sorted(rows, key=lambda r: (str(r["artist"]).lower(), int(r["tid"]))):
        clean = lambda s: str(s).replace("|", "/").replace("\n", " ").strip()  # noqa: E731
        out.append(f"| {clean(r['title'])} | {clean(r['artist'])} | {clean(r['license'])} | {int(r['tid'])} |")
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--work", type=Path, required=True)
    ap.add_argument("--metadata", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    a = ap.parse_args()

    manifest = json.loads((a.work / "labels" / "manifest.json").read_text())
    used = {r["id"] for r in manifest["fma"] if r["labeled_hops"] > 0}
    t = pd.read_csv(a.metadata / "tracks.csv", index_col=0, header=[0, 1])
    titles = t[("track", "title")]
    music = []
    with open(a.work / "fma_on_disk.csv", newline="") as f:
        for r in csv.DictReader(f):
            tid = f"{int(r['tid']):06d}"
            if tid in used and r["cls"] in ("BY", "CC0/PD"):
                music.append(dict(tid=int(r["tid"]), artist=r["artist"], license=r["lic"],
                                  title=titles.get(int(r["tid"]), "")))
    assert len(music) == len(used), (len(music), len(used))
    negs = {}
    for r in json.loads((a.work / "data" / "negatives_train" / "negatives_train.json").read_text()):
        negs[int(r["id"])] = dict(tid=int(r["id"]), artist=r["artist"], license=r["license"], title=r["title"])
    lines = [
        "# Beat model training data", "",
        "`beat_model.json` was trained on these tracks from the Free Music Archive",
        "(https://freemusicarchive.org), used under the licenses listed, together with Fosfora's own",
        "synthetic renders. Beat labels are Fosfora's own confident beats on the music; the beatless",
        "clips are negative examples. No audio is distributed with Fosfora.", "",
        f"## Music ({len(music)} tracks)", "", *table(music), "",
        f"## Beatless clips ({len(negs)} tracks)", "", *table(negs.values()), "",
    ]
    a.out.write_text("\n".join(lines))
    print(f"wrote {a.out}: {len(music)} music tracks, {len(negs)} beatless clips")
    return 0


if __name__ == "__main__":
    sys.exit(main())
