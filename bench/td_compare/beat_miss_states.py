#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Where does Fosfora lose reference beats? Classify every annotated beat.

    bench/td_compare/beat_miss_states.py <fosfora_dumps_dir> <index.json>

For each reference beat after the 5 s trim, the wire state at that instant
(from the dump's 30 Hz /bpm and /bpm/locked): no tempo (bpm <= 0), unlocked,
or locked; and whether an emitted /beat lies within 70 ms. A flywheel can only
recover beats missed while some tempo exists; "locked + missed" splits further
into silent stretches (no emitted beat for > 1.5 periods around it: the mute)
and wrong-phase/tempo misses (beats are being emitted, just not on this one).
"""

from __future__ import annotations

import json
import sys
from collections import Counter
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from benchlib.annotations import Annotations  # noqa: E402
from benchlib.dump import SignalDump  # noqa: E402

WINDOW, TRIM = 0.07, 5.0


def at(ts, vals, t):
    i = np.searchsorted(ts, t, side="right") - 1
    return vals[i] if i >= 0 else 0.0


def main() -> int:
    dumps, index = Path(sys.argv[1]), json.loads(Path(sys.argv[2]).read_text())
    for ds in sorted({t["dataset"] for t in index}):
        total = Counter()
        per_track = []
        for t in (x for x in index if x["dataset"] == ds):
            dump = SignalDump.load(dumps / f"{t['dataset']}__{t['track']}.jsonl")
            ref = np.asarray(Annotations.load(t["annotations"]).beats, dtype=float)
            ref = ref[ref >= TRIM]
            est = np.asarray(dump.beats(), dtype=float)
            bts, bpm = dump.series("/fosfora/v1/bpm")
            lts, lk = dump.series("/fosfora/v1/bpm/locked")
            c = Counter()
            for r in ref:
                hit = len(est) and np.min(np.abs(est - r)) <= WINDOW
                b = at(bts, bpm, r)
                state = "no_tempo" if b <= 0 else ("locked" if at(lts, lk, r) > 0.5 else "unlocked")
                if hit:
                    c[state + ":hit"] += 1
                elif state == "locked":
                    period = 60.0 / b
                    near = len(est) and np.min(np.abs(est - r)) <= 1.5 * period
                    c["locked:miss_" + ("offgrid" if near else "silent")] += 1
                else:
                    c[state + ":miss"] += 1
            total.update(c)
            per_track.append((t["track"], c, len(ref)))
        n = sum(total.values())
        print(f"\n{ds}: {n} reference beats after {TRIM:.0f} s")
        for k in sorted(total):
            print(f"  {k:24} {total[k]:6}  {100 * total[k] / n:5.1f}%")
        print("  worst tracks (share of beats missed, by cause):")
        for name, c, nref in sorted(per_track, key=lambda x: -sum(v for k, v in x[1].items() if "miss" in k) / max(1, x[2]))[:6]:
            misses = {k.split(':')[1] if k.startswith('locked') else k: v for k, v in c.items() if "miss" in k}
            print(f"    {name[:32]:32} missed {sum(misses.values()):4}/{nref:4}  {dict(misses)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
