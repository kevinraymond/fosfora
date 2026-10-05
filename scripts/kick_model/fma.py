#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy", "pandas", "remotezip"]
# ///
"""Training audio for the kick model: Creative Commons electronic music from the Free Music
Archive (FMA, https://github.com/mdeff/fma), CC BY and CC0 only.

    fma.py screen   --work W   # score candidate tracks' 30 s clips for a steady pulse
    fma.py select   --work W   # pick tracks from the screen -> W/tracks.json
    fma.py download --work W [--tracks tracks.json]   # full tracks -> W/audio/<id>.mp3

`tracks.json` beside this script is the list the shipped model was trained on; download
it to reproduce the model without screening again. Needs FMA's metadata unpacked at
W/fma_metadata (fma_metadata.zip, 342 MB). Files come out of the FMA archives by HTTP range
requests, one track at a time, so nothing near the archives' size (the full one is 944 GB)
is downloaded.

Why the screen: FMA's genre tags are self-reported, and some "dubstep" and "drum & bass"
tracks are noise or sound collages with no beat at all. Listening confirmed it; play counts
did not separate them. Pulse clarity (the onset envelope's autocorrelation peak at 60-200
BPM) ranked those tracks under the real ones, so the screen keeps tracks scoring >= 0.6.

Licenses: ShareAlike and NonCommercial tracks are excluded. Whether a model trained on
ShareAlike music must carry that license is unsettled, and Fosfora is Apache-2.0 / MIT.
"""

import argparse
import ast
import collections
import json
import subprocess
from pathlib import Path

import numpy as np
import pandas as pd
from remotezip import RemoteZip

HERE = Path(__file__).parent
LARGE = "https://os.unil.cloud.switch.ch/fma/fma_large.zip"  # 30 s clips
FULL = "https://os.unil.cloud.switch.ch/fma/fma_full.zip"  # untrimmed tracks
GENRES = ["Drum & Bass", "Techno", "House", "Dubstep", "Dance", "Minimal Electronic", "Breakcore - Hard", "Jungle"]
PULSE_MIN = 0.6
PER_ARTIST = 3


def metadata(work):
    t = pd.read_csv(work / "fma_metadata" / "tracks.csv", index_col=0, header=[0, 1])
    g = pd.read_csv(work / "fma_metadata" / "genres.csv", index_col=0)
    return t, {v: k for k, v in g["title"].items()}


def permissive(lic):
    """CC BY (any version) or CC0 / public domain; nothing NonCommercial, NoDerivatives or ShareAlike."""
    by = lic.str.contains("Attribution", case=False) & ~lic.str.contains(
        "NonCommercial|Noncommercial|NoDerivatives|No Derivative|ShareAlike|Share Alike", case=False, regex=True)
    return by | lic.str.contains("Public Domain|CC0", case=False, regex=True)


def pulse_clarity(path, sr=22050, hop=512, n=2048):
    raw = subprocess.run(["ffmpeg", "-v", "error", "-i", str(path), "-ac", "1", "-ar", str(sr), "-f", "f32le", "-"],
                         capture_output=True, check=True).stdout
    x = np.frombuffer(raw, np.float32).astype(np.float64)
    hops = len(x) // hop
    idx = np.arange(n)[None, :] + hop * np.arange(max(1, hops - n // hop))[:, None]
    lm = np.log1p(100 * np.abs(np.fft.rfft(x[idx] * np.hanning(n), axis=1)))
    o = np.maximum(np.diff(lm, axis=0), 0).sum(1)
    o = o - o.mean()
    best = []
    for s in range(0, max(1, len(o) - 1300), 650):  # ~15 s windows
        w = o[s:s + 1300]
        if len(w) < 600:
            continue
        ac = np.correlate(w, w, "full")[len(w) - 1:]
        ac /= max(ac[0], 1e-9)
        best.append(ac[int(60 / 200 * sr / hop): int(60 / 60 * sr / hop)].max())
    return float(np.median(best)) if best else 0.0


def screen(work):
    t, ids = metadata(work)
    lic = t[("track", "license")].fillna("")
    gall = t[("track", "genres_all")].apply(ast.literal_eval)
    dur = t[("track", "duration")]
    pool = t.index[permissive(lic) & dur.between(120, 480) & gall.apply(lambda L: ids["Electronic"] in L)]
    (work / "clips").mkdir(exist_ok=True)
    out = []
    with RemoteZip(LARGE) as z:
        names = set(z.namelist())
        for tid in map(int, pool):
            name = f"fma_large/{tid // 1000:03d}/{tid:06d}.mp3"
            if name not in names:
                continue
            clip = work / "clips" / f"{tid:06d}.mp3"
            clip.write_bytes(z.read(name))
            tagged = [g for g in GENRES if ids[g] in gall[tid]]
            out.append(dict(tid=tid, pulse=pulse_clarity(clip), genre=tagged[0] if tagged else "Electronic",
                            artist=str(t[("artist", "name")][tid]), title=str(t[("track", "title")][tid]),
                            license=lic[tid], dur=int(dur[tid])))
    (work / "screen.json").write_text(json.dumps(out, indent=0))
    print(f"screened {len(out)}; pulse >= {PULSE_MIN}: {sum(r['pulse'] >= PULSE_MIN for r in out)}")


def select(work):
    picks, per_artist = [], collections.Counter()
    for r in sorted(json.loads((work / "screen.json").read_text()), key=lambda r: -r["pulse"]):
        if r["pulse"] < PULSE_MIN or per_artist[r["artist"]] >= PER_ARTIST:
            continue
        picks.append(r)
        per_artist[r["artist"]] += 1
    (work / "tracks.json").write_text(json.dumps(picks, indent=0))
    print(f"{len(picks)} tracks, {sum(r['dur'] for r in picks) / 3600:.1f} h, {len(per_artist)} artists")


def download(work, tracks):
    (work / "audio").mkdir(exist_ok=True)
    rows = json.loads(Path(tracks).read_text())
    with RemoteZip(FULL) as z:
        names = set(z.namelist())
        for r in rows:
            name = f"fma_full/{r['tid'] // 1000:03d}/{r['tid']:06d}.mp3"
            dst = work / "audio" / f"{r['tid']:06d}.mp3"
            if name in names and not dst.exists():
                dst.write_bytes(z.read(name))
    print(f"{len(list((work / 'audio').glob('*.mp3')))} tracks in {work / 'audio'}")


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["screen", "select", "download"])
    ap.add_argument("--work", type=Path, required=True)
    ap.add_argument("--tracks", default=str(HERE / "tracks.json"))
    a = ap.parse_args()
    a.work.mkdir(parents=True, exist_ok=True)
    {"screen": lambda: screen(a.work), "select": lambda: select(a.work),
     "download": lambda: download(a.work, a.tracks)}[a.cmd]()
