#!/usr/bin/env python3
"""Check that two td_harness runs logged the same thing.

    bench/td_compare/diff_runs.py <log_dir_a> <log_dir_b>

Per track and per system: identical frame/sample columns, then the number of
cooks whose channel values differ, the largest difference, and whether the
beat events (rising edges of the scored channel) are identical. A system that
is deterministic under Realtime-off shows 0 differing cooks.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

EVENT = {"lbs": "beat", "ess": "beat", "aa": "kick"}


def load(path: Path):
    log = json.loads(path.read_text())
    col, cols = 4, {}
    for system, names in log["channels"].items():
        cols[system] = (col, names)
        col += len(names)
    return log, cols


def edges(values):
    prev, out = 0.0, []
    for i, v in enumerate(values):
        if v > 0.5 >= prev:
            out.append(i)
        prev = v
    return out


def main() -> int:
    a_dir, b_dir = Path(sys.argv[1]), Path(sys.argv[2])
    totals = {}
    bad_timing = 0
    for pa in sorted(a_dir.glob("*__*.json")):
        pb = b_dir / pa.name
        if not pb.exists():
            print(f"{pa.name}: missing in B")
            continue
        la, ca = load(pa)
        lb, cb = load(pb)
        ra, rb = la["rows"], lb["rows"]
        if [r[:4] for r in ra] != [r[:4] for r in rb]:
            bad_timing += 1
            print(f"{pa.name}: frame/sample columns differ ({len(ra)} vs {len(rb)} rows)")
            continue
        for system, (start, names) in ca.items():
            n = len(names)
            diff_rows = 0
            max_diff = 0.0
            for x, y in zip(ra, rb):
                d = max((abs(p - q) for p, q in zip(x[start:start + n], y[start:start + n])), default=0.0)
                if d > 0:
                    diff_rows += 1
                    max_diff = max(max_diff, d)
            ev = EVENT.get(system)
            same_events = None
            if ev in names:
                i = start + names.index(ev)
                same_events = edges([r[i] for r in ra]) == edges([r[i] for r in rb])
            t = totals.setdefault(system, dict(tracks=0, differing_cooks=0, cooks=0, max_diff=0.0, event_mismatch_tracks=0))
            t["tracks"] += 1
            t["differing_cooks"] += diff_rows
            t["cooks"] += len(ra)
            t["max_diff"] = max(t["max_diff"], max_diff)
            t["event_mismatch_tracks"] += 0 if same_events in (None, True) else 1
    print(f"timing mismatches: {bad_timing}")
    for system, t in totals.items():
        print(f"{system:4} tracks {t['tracks']:3}  differing cooks {t['differing_cooks']:7} / {t['cooks']}  "
              f"max diff {t['max_diff']:.3g}  tracks with different events {t['event_mismatch_tracks']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
