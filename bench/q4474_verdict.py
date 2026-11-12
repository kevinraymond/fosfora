#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Board #4474 round: an arm against bench/manifests/q4474_lead_frozen.json.

    bench/q4474_verdict.py <arm_root> --round A|B [--half tune|holdout] [--against <root>]

Round A (the beat lead) and round B (the activation-only phase vote) share the guards
(tempo identical, beatless identical, batida, downbeat) and differ in the P targets; the
numbers come from q4163_report.py on the arm's root and the baseline's (default: the
manifest's baseline root; --against for round B's combined arm against the lead15 arm).
The default half is TUNE (read freely); --half holdout is the single read at round end.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import q4163_report as rep  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
MANIFEST = REPO / "bench/manifests/q4474_lead_frozen.json"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("root", type=Path)
    ap.add_argument("--round", required=True, choices=["A", "B"])
    ap.add_argument("--half", default="tune", choices=["tune", "holdout"])
    ap.add_argument("--against", type=Path)
    ap.add_argument("--p1", type=float, help="round B: the P1 ballroom bar for this arm (default .005)")
    a = ap.parse_args()
    man = json.loads(MANIFEST.read_text())
    base_root = a.against or Path(man["baseline"]["_root"])
    h = a.half
    cand = rep.report(a.root, h)
    base = rep.report(base_root, h)
    d = rep.compare(cand, base)
    print(f"arm {a.root.name} vs {base_root.name}, half {h}, round {a.round}")
    ok_all = True

    def check(name, ok, detail):
        nonlocal ok_all
        ok_all &= bool(ok)
        print(f"  [{'PASS' if ok else 'MISS'}] {name}: {detail}")

    def m(ds, k):
        return cand["datasets"][ds][h][k], base["datasets"][ds][h][k]

    hb, hbb = m("harmonix", "beat_f")
    bb, bbb = m("ballroom", "beat_f")
    fl = d["flips"]
    if a.round == "A":
        check("P1 harmonix beat_f >= base + .004", hb >= hbb + 0.004 - 1e-9, f"{hbb:.4f} -> {hb:.4f} ({hb - hbb:+.4f})")
        check("P2 ballroom beat_f >= base - .002", bb >= bbb - 0.002 - 1e-9, f"{bbb:.4f} -> {bb:.4f} ({bb - bbb:+.4f})")
        check("P3 at most 1 track lost > .1 per dataset", fl["harmonix"]["n_lost"] <= 1 and fl["ballroom"]["n_lost"] <= 1,
              f"harmonix lost {fl['harmonix']['n_lost']} / gained {fl['harmonix']['n_gained']}; ballroom lost {fl['ballroom']['n_lost']} / gained {fl['ballroom']['n_gained']}")
    else:
        p1 = a.p1 if a.p1 is not None else 0.005
        check(f"P1 ballroom beat_f >= base + {p1}", bb >= bbb + p1 - 1e-9, f"{bbb:.4f} -> {bb:.4f} ({bb - bbb:+.4f})")
        check("P2 harmonix beat_f >= base - .003", hb >= hbb - 0.003 - 1e-9, f"{hbb:.4f} -> {hb:.4f} ({hb - hbb:+.4f})")
        hl, hg = fl["harmonix"]["n_lost"], fl["harmonix"]["n_gained"]
        check("P3 flips: harmonix lost <= 3 and <= gained + 1; ballroom lost <= 2", hl <= 3 and hl <= hg + 1 and fl["ballroom"]["n_lost"] <= 2,
              f"harmonix lost {hl} / gained {hg}; ballroom lost {fl['ballroom']['n_lost']} / gained {fl['ballroom']['n_gained']}")
    # guards
    same = []
    for ds in ("ballroom", "harmonix", "giantsteps_tempo"):
        for k in ("acc1", "acc2", "no_est"):
            c, b = m(ds, k)
            same.append(abs(c - b) < 1e-9)
    check("G1 tempo identical (acc1 / acc2 / no_est, 3 datasets)", all(same),
          "; ".join(f"{ds} {m(ds, 'acc1')[1]:.4f}->{m(ds, 'acc1')[0]:.4f}" for ds in ("ballroom", "harmonix", "giantsteps_tempo")))
    cn, bn = cand["negatives"], base["negatives"]
    check("G2 beatless identical (publish, locked)", cn["publish"] == bn["publish"] and abs(cn["locked_mean"] - bn["locked_mean"]) < 1e-9,
          f"publish {bn['publish']} -> {cn['publish']}, locked {bn['locked_mean']:.3f} -> {cn['locked_mean']:.3f}, beats/min {bn['beats_per_min']:.1f} -> {cn['beats_per_min']:.1f}")
    worst = []
    for t, v in d["batida"].items():
        for i, x in enumerate(v["beat_f_delta"]):
            if x is not None and x < -0.01:
                worst.append((t, i, x))
    check("G3 batida no render down > .01", not worst, f"{len(worst)} renders down: {worst[:6]}" if worst else "none; techno " + ", ".join(f"{x:+.3f}" for x in d["batida"].get("techno", {}).get("beat_f_delta", [])))
    dh = m("harmonix", "downbeat_f"); db = m("ballroom", "downbeat_f")
    check("G4 downbeat_f within .01", abs(dh[0] - dh[1]) <= 0.01 and abs(db[0] - db[1]) <= 0.01,
          f"harmonix {dh[1]:.4f} -> {dh[0]:.4f}, ballroom {db[1]:.4f} -> {db[0]:.4f}")
    print(f"  cmlt harmonix {m('harmonix', 'cmlt')[1]:.4f} -> {m('harmonix', 'cmlt')[0]:.4f}, ballroom {m('ballroom', 'cmlt')[1]:.4f} -> {m('ballroom', 'cmlt')[0]:.4f}")
    for ds in ("harmonix", "ballroom"):
        for t, b, c in fl[ds]["lost"][:8]:
            print(f"    LOST {ds} {t} {b} -> {c}")
        for t, b, c in fl[ds]["gained"][:8]:
            print(f"    GAIN {ds} {t} {b} -> {c}")
    ph = d["phase"]
    for k, v in ph.items():
        print(f"  phase {k}: " + ", ".join(f"{kk} {vv:+}" for kk, vv in v.items() if vv))
    print("VERDICT:", "ALL PASS" if ok_all else "MISS")
    return 0 if ok_all else 1


if __name__ == "__main__":
    sys.exit(main())
