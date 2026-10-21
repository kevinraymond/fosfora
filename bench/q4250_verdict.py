#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Board #4250 round: an arm against bench/manifests/q4250_tempo_input_frozen.json.

    bench/q4250_verdict.py <arm_root> [--half holdout|tune]

Checks P1-P3 and G1-G5 mechanically from the arm's q4163_arm.sh root and the frozen
baseline's root (re-read live, as q4163_verdict.py does). The default half is the HOLDOUT:
run it once, on the chosen arm, at round end. `--half tune` is the selection pass, read
freely; it also prints the selection rule's number (the three tune acc1 summed). G6 (cost,
fixture) and G7 (tests, clippy) are run separately.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import q4163_report as rep  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
MANIFEST = REPO / "bench/manifests/q4250_tempo_input_frozen.json"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("root", type=Path)
    ap.add_argument("--half", default="holdout", choices=["holdout", "tune"])
    a = ap.parse_args()
    frozen = json.loads(MANIFEST.read_text())
    h = a.half
    base_root = Path(frozen["baseline"]["_root"])
    cand, base = rep.report(a.root, h), rep.report(base_root, h)
    checks = []

    def check(name, ok, detail):
        checks.append((name, bool(ok), detail))

    def ds(name):
        return cand["datasets"][name][h], base["datasets"][name][h]

    def d4(x, y):
        return f"{y:.4f} -> {x:.4f}"

    bo, bb = ds("ballroom")
    gs, gb = ds("giantsteps_tempo")
    hh, bh = ds("harmonix")
    check("P1 ballroom acc1 +.05", bo["acc1"] >= bb["acc1"] + 0.05, d4(bo["acc1"], bb["acc1"]))
    check("P1 giantsteps acc1 +.03", gs["acc1"] >= gb["acc1"] + 0.03, d4(gs["acc1"], gb["acc1"]))
    check("P2 harmonix acc1 no loss (-.02)", hh["acc1"] >= bh["acc1"] - 0.02, d4(hh["acc1"], bh["acc1"]))
    check("P2 harmonix beat_f no loss (-.01)", hh["beat_f"] >= bh["beat_f"] - 0.01, d4(hh["beat_f"], bh["beat_f"]))
    check("P3 ballroom beat_f +.02", bo["beat_f"] >= bb["beat_f"] + 0.02, d4(bo["beat_f"], bb["beat_f"]))
    for name, (c, b) in (("harmonix", (hh, bh)), ("ballroom", (bo, bb))):
        for k in ("cmlt", "downbeat_f"):
            check(f"G1 {name} {k} (-.02)", c[k] >= b[k] - 0.02, d4(c[k], b[k]))
    ph, pb = cand["phase"][h]["harmonix"][h], base["phase"][h]["harmonix"][h]
    check("G2 harmonix anti_locked +3", ph["anti_locked"] <= pb["anti_locked"] + 3, f"{pb['anti_locked']} -> {ph['anti_locked']}")
    pbo, pbb = cand["phase"][h]["ballroom"][h], base["phase"][h]["ballroom"][h]
    check("G2 ballroom anti_locked +5", pbo["anti_locked"] <= pbb["anti_locked"] + 5, f"{pbb['anti_locked']} -> {pbo['anti_locked']}")
    for t, m in sorted(cand["batida"].items()):
        mb = base["batida"].get(t)
        if mb is None:
            continue
        if t == "ambient":
            fired = sum(x or 0 for x in m["beats_est"])
            fired_b = sum(x or 0 for x in mb["beats_est"])
            check("G3 ambient beats fired (20 renders)", fired <= fired_b, f"{fired_b} -> {fired}")
            continue
        f = [x for x in m["beat_f"] if x is not None]
        fb = [x for x in mb["beat_f"] if x is not None]
        mf, mfb = sum(f) / len(f), sum(fb) / len(fb)
        check(f"G3 batida {t} mean beat_f (-.01)", mf >= mfb - 0.01, d4(mf, mfb))
    n, nb = cand["negatives"], base["negatives"]
    check("G4 beatless publish +5", n["publish"] <= nb["publish"] + 5, f"{nb['publish']} -> {n['publish']}")
    check("G4 beatless beats/min +3", n["beats_per_min"] <= nb["beats_per_min"] + 3, f"{nb['beats_per_min']:.1f} -> {n['beats_per_min']:.1f}")
    check("G4 beatless locked_mean +.02", n["locked_mean"] <= nb["locked_mean"] + 0.02, f"{nb['locked_mean']:.3f} -> {n['locked_mean']:.3f}")
    check("G4 beatless pulse-low beats/min +2", n["pulse_low_beats_per_min"] <= nb["pulse_low_beats_per_min"] + 2,
          f"{nb['pulse_low_beats_per_min']:.1f} -> {n['pulse_low_beats_per_min']:.1f}")
    s, sb = cand["harmonix_all"], base["harmonix_all"]
    check("G5 drop hit_rate (-.01)", s["drop_hit_rate"] >= sb["drop_hit_rate"] - 0.01, d4(s["drop_hit_rate"], sb["drop_hit_rate"]))
    check("G5 false drops/min (+.02)", s["false_drops_per_min"] <= sb["false_drops_per_min"] + 0.02, d4(s["false_drops_per_min"], sb["false_drops_per_min"]))
    check("G5 boundary 3 s F (-.01)", s["boundary_3_0s_f"] >= sb["boundary_3_0s_f"] - 0.01, d4(s["boundary_3_0s_f"], sb["boundary_3_0s_f"]))

    width = max(len(nm) for nm, _, _ in checks)
    for name, ok, detail in checks:
        print(f"{'PASS' if ok else 'MISS'}  {name:{width}}  {detail}")
    flips = rep.compare(cand, base)["flips"]
    for name in ("harmonix", "ballroom"):
        f = flips[name]
        print(f"info  {name} tracks moved by > 0.1 beat F: lost {f['n_lost']} / gained {f['n_gained']}")
    acc_sum = bo["acc1"] + hh["acc1"] + gs["acc1"]
    acc_sum_b = bb["acc1"] + bh["acc1"] + gb["acc1"]
    print(f"info  selection number: {h} acc1 sum {acc_sum_b:.4f} -> {acc_sum:.4f} ({acc_sum - acc_sum_b:+.4f})")
    print(f"\n[{h}] {sum(ok for _, ok, _ in checks)}/{len(checks)} pass; G6 cost/fixture and G7 tests/clippy are run separately")
    return 0


if __name__ == "__main__":
    sys.exit(main())
