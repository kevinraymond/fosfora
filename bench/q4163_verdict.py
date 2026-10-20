#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Board #4163 round: an arm against bench/manifests/q4163_arbiter_frozen.json.

    bench/q4163_verdict.py <arm_root> [--half holdout|tune]

Checks P1-P3 and G1-G7 mechanically from the arm's q4163_arm.sh root and the frozen
baseline. The default half is the HOLDOUT: run it once, on the chosen arm, at round end.
`--half tune` is the selection pass (the same checks on the tune half, read freely).
G8 (fixture), G9 (build) and G10 (Kevin listens) are run separately.
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
MANIFEST = REPO / "bench/manifests/q4163_arbiter_frozen.json"
TOL = 0.005


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("root", type=Path)
    ap.add_argument("--half", default="holdout", choices=["holdout", "tune"])
    a = ap.parse_args()
    frozen = json.loads(MANIFEST.read_text())
    b = frozen["baseline"]
    if b is None:
        sys.exit("the manifest has no frozen baseline yet")
    h = a.half
    base_root = Path(b["_root"])
    cand = rep.report(a.root, h)
    checks = []

    def check(name, ok, detail):
        checks.append((name, bool(ok), detail))

    def ds(name):
        return cand["datasets"][name][h], b["datasets"][name][h]

    hh, bh = ds("harmonix")
    ph, pb = cand["phase"][h]["harmonix"][h], b["phase"][h]["harmonix"][h]
    check("P1 harmonix beat_f +.02", hh["beat_f"] >= bh["beat_f"] + 0.02, f"{bh['beat_f']:.4f} -> {hh['beat_f']:.4f}")
    check("P2 harmonix anti_locked -3", ph["anti_locked"] <= pb["anti_locked"] - 3, f"{pb['anti_locked']} -> {ph['anti_locked']}")
    flips = rep.compare(cand, rep.report(base_root, h))["flips"]
    fh = flips["harmonix"]
    check("P2 harmonix flips lost < gained", fh["n_lost"] < fh["n_gained"], f"lost {fh['n_lost']} / gained {fh['n_gained']} (by > 0.1 beat F)")
    bo, bb = ds("ballroom")
    check("P3 ballroom beat_f no harm", bo["beat_f"] >= bb["beat_f"] - TOL, f"{bb['beat_f']:.4f} -> {bo['beat_f']:.4f}")
    for k in ("acc1", "acc2", "cmlt", "amlt", "downbeat_f"):
        check(f"G1 ballroom {k}", bo[k] >= bb[k] - TOL, f"{bb[k]:.4f} -> {bo[k]:.4f}")
    check("G1 ballroom no_est", bo["no_est"] <= bb["no_est"], f"{bb['no_est']} -> {bo['no_est']}")
    gh, gb = ds("giantsteps_tempo")
    for k in ("acc1", "acc2"):
        check(f"G2 giantsteps {k}", gh[k] >= gb[k] - TOL, f"{gb[k]:.4f} -> {gh[k]:.4f}")
    check("G2 giantsteps no_est", gh["no_est"] <= gb["no_est"], f"{gb['no_est']} -> {gh['no_est']}")
    for k in ("acc1", "acc2", "cmlt", "amlt", "downbeat_f"):
        check(f"G3 harmonix {k}", hh[k] >= bh[k] - TOL, f"{bh[k]:.4f} -> {hh[k]:.4f}")
    check("G3 harmonix no_est", hh["no_est"] <= bh["no_est"], f"{bh['no_est']} -> {hh['no_est']}")

    # G4: lost vs gained on both datasets, per-genre ballroom
    for name in ("harmonix", "ballroom"):
        f = flips[name]
        check(f"G4 {name} lost <= gained", f["n_lost"] <= f["n_gained"], f"lost {f['n_lost']} / gained {f['n_gained']}")
    genre = {}
    for t in json.loads((REPO / "bench/datasets/ballroom/norm/index.json").read_text())["tracks"]:
        bundle = json.loads((REPO / "bench/datasets/ballroom/norm" / t["annotations"]).read_text())
        genre[t["track_id"]] = bundle["audio"]["path"].split("/")[-2]
    c, o = cand["per_track"]["ballroom"], rep.per_track(base_root, "ballroom", h)
    by = collections.defaultdict(list)
    for t in c:
        if t in o:
            by[genre[t]].append(c[t] - o[t])
    for g, d in sorted(by.items()):
        m = sum(d) / len(d)
        check(f"G4 ballroom {g} (n={len(d)})", m >= -0.02, f"{m:+.4f}")

    # G5: negatives
    n, nb = cand["negatives"], b["negatives"]
    check("G5 negatives beats/min", n["beats_per_min"] <= nb["beats_per_min"] + 1.0, f"{nb['beats_per_min']:.1f} -> {n['beats_per_min']:.1f}")
    check("G5 negatives pulse-low beats/min", n["pulse_low_beats_per_min"] <= nb["pulse_low_beats_per_min"] + 1.0,
          f"{nb['pulse_low_beats_per_min']:.1f} -> {n['pulse_low_beats_per_min']:.1f}")
    check("G5 negatives publish", n["publish"] <= nb["publish"], f"{nb['publish']} -> {n['publish']}")

    # G6: batida per template x seed
    for t, m in sorted(cand["batida"].items()):
        mb = b["batida"].get(t)
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

    # G7: structure / drop on harmonix ALL
    s, sb = cand["harmonix_all"], b["harmonix_all"]
    check("G7 drop hit_rate", s["drop_hit_rate"] >= sb["drop_hit_rate"] - 0.01, f"{sb['drop_hit_rate']:.4f} -> {s['drop_hit_rate']:.4f}")
    check("G7 false drops/min", s["false_drops_per_min"] <= sb["false_drops_per_min"] + 0.02, f"{sb['false_drops_per_min']:.4f} -> {s['false_drops_per_min']:.4f}")
    check("G7 predict_drop false alarms/min", s["predict_drop_false_alarms_per_min"] <= sb["predict_drop_false_alarms_per_min"] + 0.05,
          f"{sb['predict_drop_false_alarms_per_min']:.4f} -> {s['predict_drop_false_alarms_per_min']:.4f}")
    for k in ("predict_drop_coverage_0_5", "predict_drop_coverage_0_8"):
        check(f"G7 {k}", s[k] >= sb[k] - 0.01, f"{sb[k]:.4f} -> {s[k]:.4f}")
    check("G7 boundary 3 s F", s["boundary_3_0s_f"] >= sb["boundary_3_0s_f"] - TOL, f"{sb['boundary_3_0s_f']:.4f} -> {s['boundary_3_0s_f']:.4f}")

    width = max(len(n) for n, _, _ in checks)
    for name, ok, detail in checks:
        print(f"{'PASS' if ok else 'MISS'}  {name:{width}}  {detail}")
    print(f"\n[{h}] {sum(ok for _, ok, _ in checks)}/{len(checks)} pass; G8 fixture, G9 build, G10 listen are run separately")
    return 0


if __name__ == "__main__":
    sys.exit(main())
