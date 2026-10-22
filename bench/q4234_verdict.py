#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Board #4234 round: an arm against bench/manifests/q4234_guard_frozen.json.

    bench/q4234_verdict.py <arm_root> [--half holdout|tune]

Checks P1-P3 and G1-G7 mechanically from the arm's q4163_arm.sh root and the frozen
baseline's root (re-read live, as q4250_verdict.py does). The default half is the HOLDOUT:
run it once, on the chosen arm, at round end. `--half tune` is the selection pass, read
freely; it also prints the selection rule's numbers (harmonix + ballroom tune beat F, and
the tracks lost by more than 0.1). G8 (fixture), G9 (build) and G10 (Kevin listens) are
run separately.
"""

from __future__ import annotations

import argparse
import collections
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import q4163_report as rep  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
MANIFEST = REPO / "bench/manifests/q4234_guard_frozen.json"
TOL = 0.005


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
    hh, bh = ds("harmonix")
    check("P1 ballroom beat_f +.010", bo["beat_f"] >= bb["beat_f"] + 0.010, d4(bo["beat_f"], bb["beat_f"]))
    check("P2 harmonix beat_f +.008", hh["beat_f"] >= bh["beat_f"] + 0.008, d4(hh["beat_f"], bh["beat_f"]))
    flips = rep.compare(cand, base)["flips"]
    for name in ("harmonix", "ballroom"):
        f = flips[name]
        check(f"P3 {name} lost < gained", f["n_lost"] < f["n_gained"], f"lost {f['n_lost']} / gained {f['n_gained']} (by > 0.1 beat F)")
    for name in ("ballroom", "harmonix", "giantsteps_tempo"):
        c, b = ds(name)
        for k in ("acc1", "acc2", "no_est"):
            check(f"G1 {name} {k} identical", c[k] == b[k], f"{b[k]} -> {c[k]}")
    for name, (c, b) in (("harmonix", (hh, bh)), ("ballroom", (bo, bb))):
        for k in ("cmlt", "amlt", "downbeat_f"):
            check(f"G2 {name} {k} (-.005)", c[k] >= b[k] - TOL, d4(c[k], b[k]))
    ph, pb = cand["phase"][h]["harmonix"][h], base["phase"][h]["harmonix"][h]
    check("G3 harmonix anti_locked <= base", ph["anti_locked"] <= pb["anti_locked"], f"{pb['anti_locked']} -> {ph['anti_locked']}")
    pbo, pbb = cand["phase"][h]["ballroom"][h], base["phase"][h]["ballroom"][h]
    check("G3 ballroom anti_locked -3", pbo["anti_locked"] <= pbb["anti_locked"] - 3, f"{pbb['anti_locked']} -> {pbo['anti_locked']}")

    genre = {}
    for t in json.loads((REPO / "bench/datasets/ballroom/norm/index.json").read_text())["tracks"]:
        bundle = json.loads((REPO / "bench/datasets/ballroom/norm" / t["annotations"]).read_text())
        genre[t["track_id"]] = bundle["audio"]["path"].split("/")[-2]
    c, o = cand["per_track"]["ballroom"], base["per_track"]["ballroom"]
    by = collections.defaultdict(list)
    for t in c:
        if t in o:
            by[genre[t]].append(c[t] - o[t])
    for g, d in sorted(by.items()):
        m = sum(d) / len(d)
        check(f"G4 ballroom {g} (n={len(d)})", m >= -0.02, f"{m:+.4f}")

    n, nb = cand["negatives"], base["negatives"]
    check("G5 beatless publish identical", n["publish"] == nb["publish"], f"{nb['publish']} -> {n['publish']}")
    check("G5 beatless beats/min +1", n["beats_per_min"] <= nb["beats_per_min"] + 1.0, f"{nb['beats_per_min']:.1f} -> {n['beats_per_min']:.1f}")
    check("G5 beatless pulse-low beats/min +1", n["pulse_low_beats_per_min"] <= nb["pulse_low_beats_per_min"] + 1.0,
          f"{nb['pulse_low_beats_per_min']:.1f} -> {n['pulse_low_beats_per_min']:.1f}")

    for t, m in sorted(cand["batida"].items()):
        mb = base["batida"].get(t)
        if mb is None:
            continue
        for i, seed in enumerate(m["seeds"]):
            if seed not in mb["seeds"]:
                continue
            j = mb["seeds"].index(seed)
            if t == "ambient":
                check(f"G6 ambient s{seed} beats fired", (m["beats_est"][i] or 0) <= (mb["beats_est"][j] or 0), f"{mb['beats_est'][j]} -> {m['beats_est'][i]}")
                continue
            f, fb = m["beat_f"][i], mb["beat_f"][j]
            if f is None or fb is None:
                continue
            if t in ("techno", "house"):
                check(f"G6 {t} s{seed} beat_f", f >= fb - 0.02 and not (fb >= 0.7 > f), f"{fb:.3f} -> {f:.3f}")
            else:
                r, rb = m["bpm_ratio"][i], mb["bpm_ratio"][j]
                ratio_ok = r is None or rb is None or abs(r - rb) <= 0.01
                check(f"G6 {t} s{seed} beat_f / bpm_ratio", f >= fb - 0.01 and ratio_ok, f"{fb:.3f} -> {f:.3f}, ratio {rb} -> {r}")

    s, sb = cand["harmonix_all"], base["harmonix_all"]
    check("G7 drop hit_rate (-.01)", s["drop_hit_rate"] >= sb["drop_hit_rate"] - 0.01, d4(s["drop_hit_rate"], sb["drop_hit_rate"]))
    check("G7 false drops/min (+.02)", s["false_drops_per_min"] <= sb["false_drops_per_min"] + 0.02, d4(s["false_drops_per_min"], sb["false_drops_per_min"]))
    check("G7 predict_drop false alarms/min (+.05)", s["predict_drop_false_alarms_per_min"] <= sb["predict_drop_false_alarms_per_min"] + 0.05,
          d4(s["predict_drop_false_alarms_per_min"], sb["predict_drop_false_alarms_per_min"]))
    for k in ("predict_drop_coverage_0_5", "predict_drop_coverage_0_8"):
        check(f"G7 {k} (-.01)", s[k] >= sb[k] - 0.01, d4(s[k], sb[k]))
    check("G7 boundary 3 s F (-.005)", s["boundary_3_0s_f"] >= sb["boundary_3_0s_f"] - TOL, d4(s["boundary_3_0s_f"], sb["boundary_3_0s_f"]))

    width = max(len(nm) for nm, _, _ in checks)
    for name, ok, detail in checks:
        print(f"{'PASS' if ok else 'MISS'}  {name:{width}}  {detail}")
    for name in ("harmonix", "ballroom"):
        f = flips[name]
        print(f"info  {name} lost: " + ", ".join(f"{t} {x:.2f}->{y:.2f}" for t, x, y in f["lost"]))
    lost = sum(flips[name]["n_lost"] for name in ("harmonix", "ballroom"))
    sel, sel_b = bo["beat_f"] + hh["beat_f"], bb["beat_f"] + bh["beat_f"]
    print(f"info  selection: {h} harmonix + ballroom beat_f {sel_b:.4f} -> {sel:.4f} ({sel - sel_b:+.4f}), tracks lost {lost}")
    print(f"\n[{h}] {sum(ok for _, ok, _ in checks)}/{len(checks)} pass; G8 fixture, G9 build, G10 listen are run separately")
    return 0


if __name__ == "__main__":
    sys.exit(main())
