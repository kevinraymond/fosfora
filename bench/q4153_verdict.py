#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Board #4153: a candidate against bench/manifests/q4153_lock_frozen.json.

    bench/q4153_verdict.py <candidate_out_root> <candidate_neg_stats.json> [<baseline_out_root>]

<candidate_out_root> is a full run_bench tree (holdout included) for the
selected candidate; <candidate_neg_stats.json> is neg_stats.py's JSON for the
same binary over the 155 beatless clips. Checks P1-P4 and G3 mechanically.
Synthetic negatives (G1), batida (G2), the -30 dB run (G4), the fixture (G5),
the build (G6) and the owner's listen (G7) are run and reported separately.
"""

from __future__ import annotations

import collections
import hashlib
import json
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
FROZEN = json.loads((REPO / "bench/manifests/q4153_lock_frozen.json").read_text())
TOL = 0.005
NEG_INDEX = Path.home() / "ai/audio/kick-work/fma/negatives/negatives.json"


def split(root: Path) -> dict:
    out = subprocess.run([sys.executable, str(REPO / "bench/q_split_baseline.py"), str(root),
                          "ballroom", "harmonix", "giantsteps_tempo"], capture_output=True, text=True, check=True)
    return json.loads(out.stdout)


def phase(root: Path) -> dict:
    out = REPO / "bench/out" / "q4153_phase_tmp.json"
    subprocess.run([str(REPO / "bench/analyze_beat_phase.py"), str(root), "ballroom", "harmonix",
                    "--json", str(out)], capture_output=True, check=True)
    return json.loads(out.read_text())


def per_track(root: Path, ds: str) -> dict:
    res = {}
    for f in (root / "results" / ds).glob("*.json"):
        if f.name != "summary.json":
            r = json.loads(f.read_text())
            res[r["track_id"]] = r["metrics"]
    return res


def half(t: str) -> str:
    return "tune" if int(hashlib.sha256(t.encode()).hexdigest(), 16) % 2 == 0 else "holdout"


def main() -> int:
    cand = Path(sys.argv[1])
    neg = json.loads(Path(sys.argv[2]).read_text())
    base_root = Path(sys.argv[3]) if len(sys.argv) > 3 else REPO / "bench/out"
    b = FROZEN["baseline"]
    s = split(cand)
    p = phase(cand)
    checks = []

    def check(name, ok, detail):
        checks.append((name, bool(ok), detail))

    # P1: the beatless corpus.
    bn, agg = b["beatless_155"], neg["agg"]
    check("P1 beatless beats/min <= half", agg["beats_per_min"] <= bn["beats_per_min"] / 2, f"{bn['beats_per_min']} -> {agg['beats_per_min']}")
    check("P1 beatless published <= half", agg["published"] <= bn["published"] // 2, f"{bn['published']} -> {agg['published']}")
    check("P1 beatless locked-time <= .07", agg["locked_time"] <= 0.07, f"{bn['locked_time_share']} -> {agg['locked_time']}")
    pulse = {f"{r['id']:06d}": r["pulse"] for r in json.loads(NEG_INDEX.read_text())}
    clips = neg["clips"]
    low = sorted(clips, key=lambda k: pulse[k])[: len(clips) // 3]
    low_bpm = sum(clips[k]["beats"] for k in low) / (sum(clips[k]["secs"] for k in low) / 60)
    check("P1 lowest-pulse third beats/min <= half", low_bpm <= bn["by_pulse_third"]["low"]["beats_per_min"] / 2,
          f"{bn['by_pulse_third']['low']['beats_per_min']} -> {low_bpm:.2f}")

    # P2 / P3: holdout tempo and beat metrics.
    for ds, keys in (("ballroom", ("acc1", "acc2")), ("harmonix", ("acc1", "acc2")), ("giantsteps_tempo", ("acc1", "acc2"))):
        c, o = s[ds]["holdout"], b[ds]["holdout"]
        for k in keys:
            check(f"P2 {ds} holdout {k}", c[k] >= o[k] - TOL, f"{o[k]:.4f} -> {c[k]:.4f}")
        check(f"P2 {ds} holdout no_est <= base+2", c["no_est"] <= o["no_est"] + 2, f"{o['no_est']} -> {c['no_est']}")
    for ds in ("ballroom", "harmonix"):
        c, o = s[ds]["holdout"], b[ds]["holdout"]
        for k in ("beat_f", "cmlt", "amlt", "downbeat_f"):
            check(f"P3 {ds} holdout {k}", c[k] >= o[k] - TOL, f"{o[k]:.4f} -> {c[k]:.4f}")
        al = p[ds]["holdout"]["anti_locked"]
        check(f"P3 {ds} holdout anti_locked not up", al <= b["phase_anti_locked"][f"{ds}_holdout"], f"{b['phase_anti_locked'][f'{ds}_holdout']} -> {al}")

    # P4: structure / drop on harmonix ALL, counts for the tiers.
    sm = json.loads((cand / "results/harmonix/summary.json").read_text())["metrics"]
    ba = b["harmonix_all_structure"]
    pd = sm["predict_drop"]
    for tier, key in (("0.5", "predict_drop_count_0_5"), ("0.8", "predict_drop_count_0_8")):
        n = pd[tier]["lead_beats"]["n"]
        check(f"P4 predict_drop count at {tier} >= base-3", n >= ba[key] - 3, f"{ba[key]} -> {n} of {ba['n_ref_drops']} (list lost/gained if down)")
    check("P4 predict_drop false alarms/min", pd["false_alarms_per_min"] <= ba["predict_drop_false_alarms_per_min"] + 0.05,
          f"{ba['predict_drop_false_alarms_per_min']:.4f} -> {pd['false_alarms_per_min']:.4f}")
    check("P4 drop hit_rate", sm["drop"]["hit_rate"] >= ba["drop_hit_rate"] - 0.01, f"{ba['drop_hit_rate']:.4f} -> {sm['drop']['hit_rate']:.4f}")
    check("P4 false drops/min", sm["drop"]["false_drops_per_min"] <= ba["false_drops_per_min"] + 0.02,
          f"{ba['false_drops_per_min']:.4f} -> {sm['drop']['false_drops_per_min']:.4f}")
    bf = sm["structure"]["boundary_3_0s.f"]["mean"]
    check("P4 boundary 3 s F", bf >= ba["boundary_3_0s_f"] - TOL, f"{ba['boundary_3_0s_f']:.4f} -> {bf:.4f}")

    # G3: per-genre ballroom and up/down track counts, both halves.
    genre = {}
    for t in json.loads((REPO / "bench/datasets/ballroom/norm/index.json").read_text())["tracks"]:
        bundle = json.loads((REPO / "bench/datasets/ballroom/norm" / t["annotations"]).read_text())
        genre[t["track_id"]] = bundle["audio"]["path"].split("/")[-2]
    for ds in ("ballroom", "harmonix"):
        c, o = per_track(cand, ds), per_track(base_root, ds)
        for h in ("tune", "holdout"):
            ids = [t for t in c if half(t) == h and c[t].get("beat") and o.get(t, {}).get("beat")]
            up = sum(1 for t in ids if c[t]["beat"]["f_measure"] > o[t]["beat"]["f_measure"] + 0.1)
            down = sum(1 for t in ids if c[t]["beat"]["f_measure"] < o[t]["beat"]["f_measure"] - 0.1)
            check(f"G3 {ds} {h} gains >= losses", up >= down, f"up {up} / down {down} (by > 0.1 beat F)")
        if ds == "ballroom":
            by = collections.defaultdict(list)
            for t in c:
                if t in o and c[t].get("beat") and o[t].get("beat"):
                    by[genre[t]].append(c[t]["beat"]["f_measure"] - o[t]["beat"]["f_measure"])
            for g, d in sorted(by.items()):
                m = sum(d) / len(d)
                check(f"G3 ballroom {g} beat_f (n={len(d)})", m >= -0.02, f"{m:+.4f}")

    width = max(len(n) for n, _, _ in checks)
    for name, ok, detail in checks:
        print(f"{'PASS' if ok else 'MISS'}  {name:{width}}  {detail}")
    print(f"\n{sum(ok for _, ok, _ in checks)}/{len(checks)} pass")
    return 0


if __name__ == "__main__":
    sys.exit(main())
