#!/usr/bin/env python3
"""Convert td_harness logs into Signal dumps, one directory per system.

    bench/td_compare/td_logs_to_dumps.py <log_dir> <dumps_root>

Timestamp rule, measured with the click calibration (calib__clicks): the cook
at timeline frame f delivers file samples [start - 1, end - 1], so an event
seen on that cook is stamped at end / rate, the time just after its last
sample. That is the earliest moment a consumer in TouchDesigner could act.

Systems written:
  lbs          lastBeatStanding `beat` rising edges (real and synthetic beats)
  lbs_real     the same without synthetic flywheel beats (`synth` == 1)
  ess          EssentiaTD Rhythm `beat` rising edges
  aa_kick      TouchDesigner's Palette audioAnalysis `kick` pulses, used as
               beats -- NOT a beat tracker; the built-in a TD user would reach for
Each beat-tracker dump also carries its tempo as /fosfora/v1/bpm at 30 Hz.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path


def rising(values, threshold=0.5):
    prev = 0.0
    for i, v in enumerate(values):
        if v > threshold and prev <= threshold:
            yield i
        prev = v


def write_dump(path: Path, source: str, beats, bpm_series):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w") as f:
        f.write(json.dumps({"meta": 1, "schema": "/fosfora/v1", "source": source,
                            "sample_rate": 44100, "hop_hz": 60.0, "tx_rate_hz": 30}) + "\n")
        records = [(t, "/fosfora/v1/beat", [{"i": n + 1}]) for n, t in enumerate(beats)]
        records += [(t, "/fosfora/v1/bpm", [{"f": b}]) for t, b in bpm_series]
        for t, addr, args in sorted(records, key=lambda r: r[0]):
            f.write(json.dumps({"ts": t, "addr": addr, "args": args}) + "\n")


def main() -> int:
    log_dir, dumps_root = Path(sys.argv[1]), Path(sys.argv[2])
    n = 0
    for log_path in sorted(log_dir.glob("*__*.json")):
        log = json.loads(log_path.read_text())
        track = log["track"]
        if track.get("calib"):
            continue
        rate = log["rate"]
        rows = log["rows"]
        times = [r[2] / rate for r in rows]
        col = 4
        cols = {}
        for system, names in log["channels"].items():
            cols[system] = {name: col + i for i, name in enumerate(names)}
            col += len(names)

        def series(system, name):
            c = cols[system][name]
            return [r[c] for r in rows]

        stem = f"{track['dataset']}__{track['track']}"
        src = track["wav"]
        if "lbs" in cols:
            beat, synth, bpm = series("lbs", "beat"), series("lbs", "synth"), series("lbs", "bpm")
            idx = list(rising(beat))
            bpm_s = [(times[i], bpm[i]) for i in range(0, len(rows), 2)]
            write_dump(dumps_root / "lbs" / f"{stem}.jsonl", src, [times[i] for i in idx], bpm_s)
            write_dump(dumps_root / "lbs_real" / f"{stem}.jsonl", src, [times[i] for i in idx if synth[i] < 0.5], bpm_s)
        if "ess" in cols:
            beat, bpm = series("ess", "beat"), series("ess", "bpm")
            bpm_s = [(times[i], bpm[i]) for i in range(0, len(rows), 2)]
            write_dump(dumps_root / "ess" / f"{stem}.jsonl", src, [times[i] for i in rising(beat)], bpm_s)
        if "aa" in cols:
            kick = series("aa", "kick")
            write_dump(dumps_root / "aa_kick" / f"{stem}.jsonl", src, [times[i] for i in rising(kick)], [])
        n += 1
    print(f"converted {n} track logs from {log_dir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
