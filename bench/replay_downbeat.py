#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""Board #4499: an exact replica of the downbeat tracker over FOSFORA_DOWNBEAT_SIDECAR files.

    bench/replay_downbeat.py <side_dir> [--rule current|v2.7.1] [--jobs 8]

A sidecar (`audio/downbeat_sidecar.rs`) carries, per scored beat, the three accent cues
the beat wrote into the ring as raw f32 bits, its grid index and the decision the binary
took. This module rebuilds the ring, scores it in float32 in the Rust evaluation order
(`update_meter_phase`) and replays a decision rule:

- `current`: the #4499 rule (downbeat.rs today): the challenger is the top alignment, or
  4/4 on the strongest phase when the top contrast is under CONFIDENCE_THRESHOLD; a beat
  counts toward HYSTERESIS_BEATS only when the challenger's contrast exceeds the lock's
  own by SWITCH_MARGIN.
- `v2.7.1`: the rule before it: the low-confidence fallback sets 4/4 on the strongest
  phase on the spot, and a challenger only has to top the ranking.

The CLI checks a directory of sidecars (`<side_dir>/<set>/<id>.db.jsonl`) against the
rule the binary ran: every recorded branch, lock, candidate and downbeat must match. The
tracker feeds nothing back into the beat path, so once this holds a replayed rule's
downbeats are exactly what a binary running it would emit.

`moves()` is the round's stability metric: a bar-line move is a fired downbeat whose
alignment (meter, grid index mod meter) differs from the previous fired downbeat's
('align'), or, alignment unchanged, whose grid-index distance disagrees with the elapsed
time by more than 0.4 beat ('grid': the beat grid itself moved).
"""

from __future__ import annotations

import argparse
import json
import struct
import sys
from concurrent.futures import ProcessPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

F32 = np.float32
# downbeat.rs constants
W_FLUX_LOW, W_CHROMA, W_LOUD = F32(0.60), F32(0.25), F32(0.15)
EPS = F32(1e-6)
CONFIDENCE_THRESHOLD = F32(0.08)
HYSTERESIS_BEATS = 8
SWITCH_MARGIN = F32(0.08)
RING_LEN = 16
MIN_BEATS_FOR_LOCK = 8
FMIN = F32(np.finfo(np.float32).min)
COMBOS = [(3, 0), (3, 1), (3, 2), (4, 0), (4, 1), (4, 2), (4, 3)]


def _f32(bits: int) -> np.float32:
    return np.frombuffer(struct.pack("<I", bits), dtype=np.float32)[0]


def load(path) -> list[dict]:
    with open(path) as f:
        return [r for r in (json.loads(x) for x in f if x.strip()) if r.get("k") == "b"]


@dataclass
class Beat:
    idx: int
    bt: float | None  # the settled beat time (/downbeat's stamp)
    ts: float  # the hop timestamp (--analyze's downbeat time)
    lk: int
    bpm: float
    contrast: dict = field(default_factory=dict)
    best: tuple | None = None
    strongest4: int | None = None


def _contrast(scored, m, p):
    in_sum = out_sum = F32(0.0)
    in_n = out_n = 0
    for idx, s in scored:
        if idx % m == p:
            in_sum, in_n = F32(in_sum + s), in_n + 1
        else:
            out_sum, out_n = F32(out_sum + s), out_n + 1
    if in_n == 0 or out_n == 0:
        return None
    return F32(F32(in_sum / F32(in_n)) - F32(out_sum / F32(out_n)))


def _strongest(scored, m):
    best_p, best_v = 0, FMIN
    for p in range(m):
        s, n = F32(0.0), 0
        for idx, v in scored:
            if idx % m == p:
                s, n = F32(s + v), n + 1
        if n and F32(s / F32(n)) > best_v:
            best_p, best_v = p, F32(s / F32(n))
    return best_p


def prepare(recs: list[dict]) -> list[Beat]:
    """Rebuild the ring beat by beat and score it as `update_meter_phase` does."""
    ring: list[list] = []
    beats = []
    for r in recs:
        if "fl_b" in r and ring:
            ring[-1][1:] = [_f32(r["fl_b"]), _f32(r["cc_b"]), _f32(r["lr_b"])]
        b = Beat(idx=r["gi"], bt=r["bt"], ts=r["ts"], lk=r["lk"], bpm=r["bpm"])
        if len(ring) >= MIN_BEATS_FOR_LOCK:
            mf = mc = ml = EPS
            for e in ring:
                mf = max(mf, e[1])
            for e in ring:
                mc = max(mc, e[2])
            for e in ring:
                ml = max(ml, e[3])
            scored = []
            for e in ring:
                s = F32(F32(W_FLUX_LOW * F32(e[1] / mf)) + F32(W_CHROMA * F32(e[2] / mc)))
                scored.append((e[0], F32(s + F32(W_LOUD * F32(e[3] / ml)))))
            for m, p in COMBOS:
                c = _contrast(scored, m, p)
                if c is not None:
                    b.contrast[(m, p)] = c
                    if b.best is None or c > b.best[2]:
                        b.best = (m, p, c)
            b.strongest4 = _strongest(scored, 4)
        if len(ring) == RING_LEN:
            ring.pop(0)
        ring.append([r["gi"], F32(0.0), F32(0.0), F32(0.0)])
        beats.append(b)
    return beats


def replay(beats: list[Beat], hyst=HYSTERESIS_BEATS, margin=SWITCH_MARGIN) -> list[tuple]:
    """The current rule. Per beat: (path, meter, phase, candidate, count, downbeat, fallback)."""
    meter, phase, cand, count = 4, 0, None, 0
    out = []
    for b in beats:
        path, fb = "warm", False
        if b.best is not None:
            bm, bp, bs = b.best
            fb = bool(bs < CONFIDENCE_THRESHOLD)
            ch = (4, b.strongest4) if fb else (bm, bp)
            if ch == (meter, phase):
                path, cand, count = "hold", None, 0
            else:
                theirs, own = b.contrast.get(ch), b.contrast.get((meter, phase))
                clear = theirs is None or own is None or bool(F32(theirs - own) >= margin)
                if not clear:
                    path, cand, count = "weak", None, 0
                elif cand == ch:
                    path, count = "cand", count + 1
                    if count >= hyst:
                        path, (meter, phase), cand, count = "sw", ch, None, 0
                else:
                    path, cand, count = "cand", ch, 1
        out.append((path, meter, phase, cand, count, (b.idx - phase) % meter == 0, fb))
    return out


def replay_v271(beats: list[Beat]) -> list[tuple]:
    """The rule before #4499 (v2.7.1), same tuple shape (fallback = took the fb branch)."""
    meter, phase, cand, count = 4, 0, None, 0
    out = []
    for b in beats:
        path, fb = "warm", False
        if b.best is not None:
            bm, bp, bs = b.best
            if bs < CONFIDENCE_THRESHOLD:
                path, fb, meter, phase, cand, count = "fb", True, 4, b.strongest4, None, 0
            elif (bm, bp) == (meter, phase):
                path, cand, count = "hold", None, 0
            elif cand == (bm, bp):
                path, count = "cand", count + 1
                if count >= HYSTERESIS_BEATS:
                    path, meter, phase, cand, count = "sw", bm, bp, None, 0
            else:
                path, cand, count = "cand", (bm, bp), 1
        out.append((path, meter, phase, cand, count, (b.idx - phase) % meter == 0, fb))
    return out


RULES = {"current": replay, "v2.7.1": replay_v271}


def recorded(recs: list[dict]) -> list[tuple]:
    """The binary's own decisions, in replay()'s tuple shape."""
    return [(r["path"], r["m"], r["p"], (r["cm"], r["cp"]) if r["cn"] else None, r["cn"], bool(r["db"]),
             bool(r.get("fb", r["path"] == "fb"))) for r in recs]


def mismatches(recs, beats, rule="current") -> list:
    bad = []
    for r, b, d, g in zip(recs, beats, recorded(recs), RULES[rule](beats)):
        if d[:3] + d[4:] != g[:3] + g[4:] or (d[4] and d[3] != g[3]):
            bad.append((b.ts, d, g))
        if b.best is not None and (r.get("bm"), r.get("bp"), r.get("bs_b")) != (
                b.best[0], b.best[1], int(np.float32(b.best[2]).view(np.uint32))):
            bad.append((b.ts, "best", r.get("bm"), b.best))
    return bad


def moves(beats: list[Beat], dec: list[tuple], revert_s=10.0, grid_tol=0.4) -> list[dict]:
    """Bar-line moves at fired downbeats (see the module doc); `reverted` = the pre-move
    alignment returns within `revert_s`."""
    dbs = [(i, b.bt, d[1], b.idx % d[1], b.lk, b.bpm, b.idx) for i, (b, d) in enumerate(zip(beats, dec)) if d[5]]
    out = []
    for k in range(1, len(dbs)):
        i0, t0, m0, a0, _, _, g0 = dbs[k - 1]
        i1, t1, m1, a1, lk, bpm, g1 = dbs[k]
        if (m1, a1) != (m0, a0):
            dg = g1 - g0
            out.append(dict(t=t1, kind="align", size=int(dg - m0 * round(dg / m0)), locked=lk,
                            meter_change=m1 != m0, frm=(m0, a0), to=(m1, a1), k=k,
                            # the lock changed on a beat whose challenger was the fallback
                            fallback=any(dec[j][6] and dec[j][1:3] != dec[j - 1][1:3] for j in range(i0 + 1, i1 + 1))))
        elif bpm and bpm > 1 and abs((t1 - t0) * bpm / 60.0 - (g1 - g0)) > grid_tol:
            out.append(dict(t=t1, kind="grid", size=round((t1 - t0) * bpm / 60.0 - (g1 - g0)), locked=lk,
                            meter_change=False, frm=(m0, a0), to=(m1, a1), k=k, fallback=False))
    for mv in out:
        mv["reverted"] = mv["kind"] == "align" and any(
            (dbs[k2][2], dbs[k2][3]) == mv["frm"]
            for k2 in range(mv["k"] + 1, len(dbs)) if dbs[k2][1] - mv["t"] <= revert_s)
    return out


def locked_minutes(beats: list[Beat], cap=2.0) -> float:
    """Minutes of locked beat time: inter-beat gaps after a locked beat, each capped."""
    return sum(min(b.bt - a.bt, cap) for a, b in zip(beats, beats[1:])
               if a.lk and a.bt is not None and b.bt is not None) / 60.0


def _check(args):
    path, rule = args
    recs = load(path)
    beats = prepare(recs)
    return str(path), len(beats), mismatches(recs, beats, rule)[:3]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("side_dir", type=Path)
    ap.add_argument("--rule", default="current", choices=list(RULES))
    ap.add_argument("--jobs", type=int, default=8)
    a = ap.parse_args()
    paths = sorted(a.side_dir.glob("*/*.db.jsonl"))
    with ProcessPoolExecutor(a.jobs) as pool:
        res = list(pool.map(_check, [(p, a.rule) for p in paths], chunksize=4))
    bad = [r for r in res if r[2]]
    print(f"{len(res)} tracks, {sum(r[1] for r in res)} beats, rule {a.rule}: "
          f"{len(bad)} tracks with mismatches")
    for p, _, b in bad[:10]:
        print("  ", p, b)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
