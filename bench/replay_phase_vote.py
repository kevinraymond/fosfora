#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Board #4160: can an evidence stream on the wire tell the beat from the off-beat?

    bench/replay_phase_vote.py <out_root> <dataset> [--half tune] [--evidence kick|onset]
                               [--win 8] [--kmin 4] [--ratio 2.0]

Replays a phase ARBITER's vote over cached baseline dumps, without changing the
scheduler: at each emitted /beat, the evidence events of the last `win` grid
periods are folded onto the grid (their fractional position between emitted
beats) and counted at four candidate phases (0, 1/4, 1/2, 3/4 of a beat,
+-1/8 each). The vote is the candidate with the most events when it beats the
current phase (0) by `ratio` and at least `kmin` events were seen; otherwise
"stay". Truth: where the reference beat sits relative to this grid (the shift
that would put the grid on the beat, rounded to a quarter).

Evidence: kick = /stem/drums/onset (the kick model's events), onset = /onset
(all-band onset edges, what the PLL already bids on: the control).

Prints the confusion of vote vs truth. What matters: on grids that are
already right (truth 0) the vote must almost never move; on off-beat grids
(truth 1/2) it should say 1/2.
"""

from __future__ import annotations

import argparse
import sys
from collections import Counter
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).parent))

from analyze_beat_phase import REPO, half  # noqa: E402
from benchlib.annotations import Annotations, load_index  # noqa: E402
from benchlib.dump import SignalDump  # noqa: E402

CANDS = (0.0, 0.25, 0.5, 0.75)
ADDR = {"kick": "/fosfora/v1/stem/drums/onset", "onset": "/fosfora/v1/onset"}


def frac_positions(events: np.ndarray, grid: np.ndarray) -> np.ndarray:
    """Fractional position of each event between its surrounding grid beats."""
    k = np.searchsorted(grid, events, side="right") - 1
    ok = (k >= 0) & (k < len(grid) - 1)
    k = k[ok]
    return (events[ok] - grid[k]) / (grid[k + 1] - grid[k])


def nearest_cand(frac: float) -> float:
    return min(CANDS, key=lambda c: min(abs(frac - c), 1 - abs(frac - c)))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("out_root", type=Path)
    ap.add_argument("dataset")
    ap.add_argument("--half", default="tune", choices=["tune", "holdout", "all"])
    ap.add_argument("--evidence", default="kick", choices=[*ADDR, "union"])
    ap.add_argument("--cands", type=int, default=4, choices=[2, 4], help="2: only on/half-beat may win a vote")
    ap.add_argument("--win", type=int, default=8)
    ap.add_argument("--kmin", type=int, default=4)
    ap.add_argument("--ratio", type=float, default=2.0)
    ap.add_argument("--streak", type=int, default=0,
                    help="if > 0: simulate the arbiter as a state machine that shifts a virtual grid after "
                         "this many consecutive identical votes, and report the on-beat share before/after")
    args = ap.parse_args()
    sim = Counter()

    conf = Counter()
    for t in load_index(REPO / "bench" / "datasets" / args.dataset / "norm" / "index.json"):
        if args.half != "all" and half(t["track_id"]) != args.half:
            continue
        ann = Annotations.load(t["annotations"])
        if ann.beats is None or len(ann.beats) < 4:
            continue
        ref = np.asarray(ann.beats, dtype=float)
        dump = SignalDump.load(next((args.out_root / "dumps" / args.dataset).glob(f"{t['track_id']}.*.jsonl")))
        grid = np.asarray(dump.beats(), dtype=float)
        addrs = list(ADDR.values()) if args.evidence == "union" else [ADDR[args.evidence]]
        ev = np.sort(np.asarray([ts for a in addrs for ts, _ in dump.events(a)], dtype=float))
        allowed = (0.0, 0.5) if args.cands == 2 else CANDS
        if len(grid) < args.win + 2:
            continue
        offset, streak_vote, streak_n = 0.0, None, 0
        for i in range(args.win, len(grid) - 1):
            if grid[i] < 5.0 or grid[i] > ref[-1]:
                continue
            per = np.median(np.diff(grid[i - args.win:i + 1]))
            if np.any(np.diff(grid[i - args.win:i + 1]) > 1.6 * per):
                offset, streak_vote, streak_n = 0.0, None, 0  # the scheduler re-anchors after a gap
                continue  # a gap (mute/re-anchor) inside the window: no stable grid
            lo, hi = grid[i - args.win], grid[i]
            fr = frac_positions(ev[(ev >= lo) & (ev < hi)], grid[i - args.win:i + 1])
            counts = {c: int(np.sum(np.minimum(np.abs(fr - c), 1 - np.abs(fr - c)) <= 0.125)) for c in CANDS}
            best = max(CANDS, key=lambda c: counts[c])
            total = sum(counts.values())
            vote = best if (best != 0.0 and total >= args.kmin and counts[best] >= args.ratio * max(counts[0.0], 1)) else 0.0
            # truth: shift that puts this grid beat on the nearest reference beat
            j = int(np.argmin(np.abs(ref - grid[i])))
            truth = nearest_cand(((ref[j] - grid[i]) / per) % 1.0)
            if args.streak:
                # the virtual grid sits at grid + offset: rotate the counts onto it and vote again
                rc = {c: counts[(c + offset) % 1.0] for c in CANDS}
                rbest = max(allowed, key=lambda c: rc[c])
                v = rbest if (rbest != 0.0 and total >= args.kmin and rc[rbest] >= args.ratio * max(rc[0.0], 1)) else 0.0
                if v != 0.0 and v == streak_vote:
                    streak_n += 1
                else:
                    streak_vote, streak_n = (v, 1) if v != 0.0 else (None, 0)
                if streak_n >= args.streak:
                    sim["moves_from_right" if offset == truth else ("moves_to_right" if (offset + v) % 1.0 == truth else "moves_other")] += 1
                    offset = (offset + v) % 1.0
                    streak_vote, streak_n = None, 0
                sim["beats"] += 1
                sim["on_before"] += int(truth == 0.0)
                sim["on_after"] += int(offset == truth)
                sim["anti_before"] += int(truth == 0.5)
                sim["anti_after"] += int((truth - offset) % 1.0 == 0.5)
            if total < args.kmin:
                vote = "abstain"
            conf[(truth, vote)] += 1
    print(f"{args.dataset} {args.half}  evidence={args.evidence} win={args.win} kmin={args.kmin} ratio={args.ratio}")
    print(f"  {'truth':>7} | " + " ".join(f"{str(v):>8}" for v in (*CANDS, "abstain")) + " |  total  correct")
    for tr in CANDS:
        row = [conf[(tr, v)] for v in (*CANDS, "abstain")]
        n = sum(row)
        voted = n - row[-1]
        correct = conf[(tr, tr)]
        print(f"  {tr:>7} | " + " ".join(f"{x:8}" for x in row) + f" | {n:6}  {correct / voted if voted else 0:6.3f}")
    if args.streak:
        n = sim["beats"] or 1
        print(f"  arbiter replay streak={args.streak}: on-beat {sim['on_before'] / n:.3f} -> {sim['on_after'] / n:.3f}, "
              f"off-beat {sim['anti_before'] / n:.3f} -> {sim['anti_after'] / n:.3f}; moves: to right {sim['moves_to_right']}, "
              f"away from right {sim['moves_from_right']}, other {sim['moves_other']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
