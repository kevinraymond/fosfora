#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Board #4160: the round's chosen config against bench/manifests/q4160_phase_frozen.json.

    bench/q4160_verdict.py <candidate_out_root> [<baseline_out_root>]

Reads the candidate's full-dataset run_bench results (holdout included) and
checks P1-P3 and G1-G4, G7 mechanically. Batida (G6), negatives (G5), fixture
(G8), build (G9) and the owner's listen (G10) are run separately.
"""

from __future__ import annotations

import collections
import json
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
FROZEN = json.loads((REPO / "bench/manifests/q4160_phase_frozen.json").read_text())
TOL = 0.005


def split(root: Path) -> dict:
    out = subprocess.run([sys.executable, str(REPO / "bench/q_split_baseline.py"), str(root),
                          "ballroom", "harmonix", "giantsteps_tempo"], capture_output=True, text=True, check=True)
    return json.loads(out.stdout)


def phase(root: Path) -> dict:
    out = REPO / "bench/out" / "q4160_phase_tmp.json"
    subprocess.run([str(REPO / "bench/analyze_beat_phase.py"), str(root), "ballroom", "harmonix",
                    "--half", "holdout", "--json", str(out)], capture_output=True, check=True)
    return json.loads(out.read_text())


def per_track(root: Path, ds: str) -> dict:
    res = {}
    for f in (root / "results" / ds).glob("*.json"):
        if f.name != "summary.json":
            r = json.loads(f.read_text())
            res[r["track_id"]] = r["metrics"]
    return res


def main() -> int:
    cand = Path(sys.argv[1])
    base_root = Path(sys.argv[2]) if len(sys.argv) > 2 else Path.home() / "ai/audio/kick-work/q-onset/base"
    b = FROZEN["baseline"]
    s = split(cand)
    p = phase(cand)
    checks = []

    def check(name, ok, detail):
        checks.append((name, ok, detail))

    hh, bh = s["harmonix"]["holdout"], b["harmonix"]["holdout"]
    ph = p["harmonix"]["holdout"]
    check("P1 harmonix phase_on +.03", ph["on"] >= bh["phase_on"] + 0.03, f"{bh['phase_on']:.3f} -> {ph['on']:.3f}")
    check("P1 harmonix anti_locked fewer", ph["anti_locked"] < bh["anti_locked"], f"{bh['anti_locked']} -> {ph['anti_locked']}")
    check("P2 harmonix holdout beat_f +.02", hh["beat_f"] >= bh["beat_f"] + 0.02, f"{bh['beat_f']:.4f} -> {hh['beat_f']:.4f}")
    bo, bb = s["ballroom"]["holdout"], b["ballroom"]["holdout"]
    check("P3 ballroom holdout beat_f no harm", bo["beat_f"] >= bb["beat_f"] - TOL, f"{bb['beat_f']:.4f} -> {bo['beat_f']:.4f}")
    for k in ("acc1", "acc2", "cmlt", "amlt", "downbeat_f"):
        check(f"G1 ballroom holdout {k}", bo[k] >= bb[k] - TOL, f"{bb[k]:.4f} -> {bo[k]:.4f}")
    check("G1 ballroom holdout no_est", bo["no_est"] <= bb["no_est"], f"{bb['no_est']} -> {bo['no_est']}")
    gh, gb = s["giantsteps_tempo"]["holdout"], b["giantsteps_tempo"]["holdout"]
    for k in ("acc1", "acc2"):
        check(f"G2 giantsteps holdout {k}", gh[k] >= gb[k] - TOL, f"{gb[k]:.4f} -> {gh[k]:.4f}")
    check("G2 giantsteps holdout no_est", gh["no_est"] <= gb["no_est"], f"{gb['no_est']} -> {gh['no_est']}")
    for k in ("acc1", "acc2", "cmlt", "amlt", "downbeat_f"):
        check(f"G3 harmonix holdout {k}", hh[k] >= bh[k] - TOL, f"{bh[k]:.4f} -> {hh[k]:.4f}")
    check("G3 harmonix holdout no_est", hh["no_est"] <= bh["no_est"], f"{bh['no_est']} -> {hh['no_est']}")

    # G4: per-genre ballroom beat_f (holdout) and up/down track counts (holdout).
    import hashlib
    half = lambda t: "tune" if int(hashlib.sha256(t.encode()).hexdigest(), 16) % 2 == 0 else "holdout"
    genre = {}
    for t in json.loads((REPO / "bench/datasets/ballroom/norm/index.json").read_text())["tracks"]:
        bundle = json.loads((REPO / "bench/datasets/ballroom/norm" / t["annotations"]).read_text())
        genre[t["track_id"]] = bundle["audio"]["path"].split("/")[-2]
    for ds in ("ballroom", "harmonix"):
        c, o = per_track(cand, ds), per_track(base_root, ds)
        ids = [t for t in c if half(t) == "holdout" and c[t].get("beat") and o[t].get("beat")]
        up = sum(1 for t in ids if c[t]["beat"]["f_measure"] > o[t]["beat"]["f_measure"] + 0.1)
        down = sum(1 for t in ids if c[t]["beat"]["f_measure"] < o[t]["beat"]["f_measure"] - 0.1)
        check(f"G4 {ds} holdout gains > losses", up > down, f"up {up} / down {down} (by > 0.1 beat F)")
        if ds == "ballroom":
            by = collections.defaultdict(list)
            for t in ids:
                by[genre[t]].append(c[t]["beat"]["f_measure"] - o[t]["beat"]["f_measure"])
            for g, d in sorted(by.items()):
                m = sum(d) / len(d)
                check(f"G4 ballroom {g} (n={len(d)})", m >= -0.02, f"{m:+.4f}")

    # G7: structure/drop on harmonix ALL from the summary.
    sm = json.loads((cand / "results/harmonix/summary.json").read_text())["metrics"]
    ba = b["harmonix"]["all"]
    check("G7 drop hit_rate", sm["drop"]["hit_rate"] >= ba["drop_hit_rate"] - 0.01, f"{ba['drop_hit_rate']:.4f} -> {sm['drop']['hit_rate']:.4f}")
    check("G7 false drops/min", sm["drop"]["false_drops_per_min"] <= ba["false_drops_per_min"] + 0.02, f"{ba['false_drops_per_min']:.4f} -> {sm['drop']['false_drops_per_min']:.4f}")
    pd = sm["predict_drop"]
    check("G7 predict_drop false alarms/min", pd["false_alarms_per_min"] <= ba["predict_drop_false_alarms_per_min"] + 0.05, f"{ba['predict_drop_false_alarms_per_min']:.4f} -> {pd['false_alarms_per_min']:.4f}")
    check("G7 predict_drop coverage .5", pd["0.5"]["coverage"] >= ba["predict_drop_coverage_0_5"] - 0.01, f"{ba['predict_drop_coverage_0_5']:.4f} -> {pd['0.5']['coverage']:.4f}")
    check("G7 predict_drop coverage .8", pd["0.8"]["coverage"] >= ba["predict_drop_coverage_0_8"] - 0.01, f"{ba['predict_drop_coverage_0_8']:.4f} -> {pd['0.8']['coverage']:.4f}")
    bf = sm["structure"]["boundary_3_0s.f"]["mean"]
    check("G7 boundary 3 s F", bf >= ba["boundary_3_0s_f"] - TOL, f"{ba['boundary_3_0s_f']:.4f} -> {bf:.4f}")

    width = max(len(n) for n, _, _ in checks)
    for name, ok, detail in checks:
        print(f"{'PASS' if ok else 'MISS'}  {name:{width}}  {detail}")
    print(f"\n{sum(ok for _, ok, _ in checks)}/{len(checks)} pass")
    return 0


if __name__ == "__main__":
    sys.exit(main())
