#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Board #4288 round: an arm against bench/manifests/q4288_labels_frozen.json.

    bench/q4288_verdict.py <arm_root> [--half holdout|tune]

Checks P1-P3 and G1-G6 mechanically from the arm's arm-script root and the frozen
baseline's root (re-read live, as q4250_verdict.py does). The default half is the HOLDOUT:
run it once, on the chosen arm, at round end. `--half tune` is the selection pass, read
freely; it also prints the selection rule's number (the three tune acc1 summed) and the
eight named tune doublings. G7 (golden, fixture, tests, clippy) and G8 (Kevin listens) are
run separately.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import q4163_report as rep  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
MANIFEST = REPO / "bench/manifests/q4288_labels_frozen.json"
DOUBLINGS = ("0069", "0376", "0415", "0454", "0734", "0756", "0763", "0908")


def half_of(track_id: str) -> str:
    return "tune" if int(hashlib.sha256(track_id.encode()).hexdigest(), 16) % 2 == 0 else "holdout"


def tempo_rows(root: Path, ds: str, half: str) -> dict:
    """track -> (ref bpm, est bpm, acc1) from the per-track bench results."""
    out = {}
    for f in (root / "results" / ds).glob("*.json"):
        if f.name == "summary.json":
            continue
        r = json.loads(f.read_text())
        if half_of(r["track_id"]) == half:
            t = r["metrics"]["tempo"]
            out[r["track_id"]] = (t.get("ref_bpm"), t.get("bpm_estimate"), bool(t.get("acc1")))
    return out


def at_2t(ref, est) -> bool:
    return bool(ref and est and abs(est / (2 * ref) - 1) <= 0.04)


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

    hh, bh = ds("harmonix")
    bo, bb = ds("ballroom")
    gs, gb = ds("giantsteps_tempo")
    tc, tb = tempo_rows(a.root, "harmonix", h), tempo_rows(base_root, "harmonix", h)
    slow_c = sum(v[2] for v in tc.values() if v[0] and v[0] < 100)
    slow_b = sum(v[2] for v in tb.values() if v[0] and v[0] < 100)
    n_slow = sum(1 for v in tb.values() if v[0] and v[0] < 100)
    dbl_c = sum(at_2t(v[0], v[1]) for v in tc.values())
    dbl_b = sum(at_2t(v[0], v[1]) for v in tb.values())
    check("P1 harmonix acc1 +.015", hh["acc1"] >= bh["acc1"] + 0.015, d4(hh["acc1"], bh["acc1"]))
    check("P1 harmonix acc1 below 100 BPM +3 tracks", slow_c >= slow_b + 3, f"{slow_b} -> {slow_c} of {n_slow}")
    check("P2 harmonix tracks at 2T -5", dbl_c <= dbl_b - 5, f"{dbl_b} -> {dbl_c}")
    check("P3 harmonix beat_f no loss (-.005)", hh["beat_f"] >= bh["beat_f"] - 0.005, d4(hh["beat_f"], bh["beat_f"]))
    check("G1 ballroom acc1 (-.02)", bo["acc1"] >= bb["acc1"] - 0.02, d4(bo["acc1"], bb["acc1"]))
    check("G1 ballroom beat_f (-.01)", bo["beat_f"] >= bb["beat_f"] - 0.01, d4(bo["beat_f"], bb["beat_f"]))
    check("G1 giantsteps acc1 (-.02)", gs["acc1"] >= gb["acc1"] - 0.02, d4(gs["acc1"], gb["acc1"]))
    for name, (c, b) in (("harmonix", (hh, bh)), ("ballroom", (bo, bb))):
        for k in ("cmlt", "downbeat_f"):
            check(f"G2 {name} {k} (-.02)", c[k] >= b[k] - 0.02, d4(c[k], b[k]))
    ph, pb = cand["phase"][h]["harmonix"][h], base["phase"][h]["harmonix"][h]
    check("G3 harmonix anti_locked +3", ph["anti_locked"] <= pb["anti_locked"] + 3, f"{pb['anti_locked']} -> {ph['anti_locked']}")
    pbo, pbb = cand["phase"][h]["ballroom"][h], base["phase"][h]["ballroom"][h]
    check("G3 ballroom anti_locked +5", pbo["anti_locked"] <= pbb["anti_locked"] + 5, f"{pbb['anti_locked']} -> {pbo['anti_locked']}")
    for t, m in sorted(cand["batida"].items()):
        mb = base["batida"].get(t)
        if mb is None:
            continue
        if t == "ambient":
            fired = sum(x or 0 for x in m["beats_est"])
            fired_b = sum(x or 0 for x in mb["beats_est"])
            check("G4 ambient beats fired (20 renders)", fired <= fired_b, f"{fired_b} -> {fired}")
            continue
        f = [x for x in m["beat_f"] if x is not None]
        fb = [x for x in mb["beat_f"] if x is not None]
        mf, mfb = sum(f) / len(f), sum(fb) / len(fb)
        check(f"G4 batida {t} mean beat_f (-.01)", mf >= mfb - 0.01, d4(mf, mfb))
    n, nb = cand["negatives"], base["negatives"]
    check("G5 beatless publish +3", n["publish"] <= nb["publish"] + 3, f"{nb['publish']} -> {n['publish']}")
    check("G5 beatless beats/min +2", n["beats_per_min"] <= nb["beats_per_min"] + 2, f"{nb['beats_per_min']:.1f} -> {n['beats_per_min']:.1f}")
    check("G5 beatless locked_mean +.02", n["locked_mean"] <= nb["locked_mean"] + 0.02, f"{nb['locked_mean']:.3f} -> {n['locked_mean']:.3f}")
    check("G5 beatless pulse-low beats/min +2", n["pulse_low_beats_per_min"] <= nb["pulse_low_beats_per_min"] + 2,
          f"{nb['pulse_low_beats_per_min']:.1f} -> {n['pulse_low_beats_per_min']:.1f}")
    s, sb = cand["harmonix_all"], base["harmonix_all"]
    check("G6 drop hit_rate (-.01)", s["drop_hit_rate"] >= sb["drop_hit_rate"] - 0.01, d4(s["drop_hit_rate"], sb["drop_hit_rate"]))
    check("G6 false drops/min (+.02)", s["false_drops_per_min"] <= sb["false_drops_per_min"] + 0.02, d4(s["false_drops_per_min"], sb["false_drops_per_min"]))
    check("G6 boundary 3 s F (-.01)", s["boundary_3_0s_f"] >= sb["boundary_3_0s_f"] - 0.01, d4(s["boundary_3_0s_f"], sb["boundary_3_0s_f"]))

    width = max(len(nm) for nm, _, _ in checks)
    for name, ok, detail in checks:
        print(f"{'PASS' if ok else 'MISS'}  {name:{width}}  {detail}")
    flips = rep.compare(cand, base)["flips"]
    for name in ("harmonix", "ballroom"):
        f = flips[name]
        print(f"info  {name} tracks moved by > 0.1 beat F: lost {f['n_lost']} / gained {f['n_gained']}")
    if h == "tune":
        named = {t: ("2T" if at_2t(r, e) else f"{e / r:.2f}x" if r and e else "none") for t, (r, e, _) in tc.items() if t[:4] in DOUBLINGS}
        print(f"info  the eight named tune doublings: {sum(v == '2T' for v in named.values())} still at 2T: {named}")
    acc_sum = bo["acc1"] + hh["acc1"] + gs["acc1"]
    acc_sum_b = bb["acc1"] + bh["acc1"] + gb["acc1"]
    print(f"info  selection number: {h} acc1 sum {acc_sum_b:.4f} -> {acc_sum:.4f} ({acc_sum - acc_sum_b:+.4f}); harmonix beat_f {hh['beat_f']:.4f}")
    print(f"\n[{h}] {sum(ok for _, ok, _ in checks)}/{len(checks)} pass; G7 golden/fixture/tests/clippy and G8 listen are run separately")
    return 0


if __name__ == "__main__":
    sys.exit(main())
