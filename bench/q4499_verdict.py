#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Board #4499 round: an arm against bench/manifests/q4499_bar_line_frozen.json.

    bench/q4499_verdict.py <arm_root> <arm_sidecars> [--half tune|holdout]
                           [--base-root R] [--base-sidecars S] [--fixture DIR]

The bench numbers (downbeat F, beat F, tempo, batida, negatives, structure) come from the
two bench roots via q4163_report.py; the bar-line moves come from the two sidecar
directories (FOSFORA_DOWNBEAT_SIDECAR, `<dir>/<set>/<id>.db.jsonl`), read as the binaries
recorded them, after replay_downbeat.py has reproduced both binaries' decisions exactly
and the arm's tracker inputs have been checked identical to the baseline's. The default
half is TUNE (read freely); --half holdout is the single read at round end.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import subprocess
import sys
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import q4163_report as rep  # noqa: E402
import replay_downbeat as rd  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
MANIFEST = REPO / "bench/manifests/q4499_bar_line_frozen.json"
SPLIT = ("ballroom", "harmonix", "giantsteps_tempo")
EDM = "edm_20261023_111937_td"


def half_of(tid: str) -> str:
    return "tune" if int(hashlib.sha256(tid.encode()).hexdigest(), 16) % 2 == 0 else "holdout"


def _inputs(recs):
    return [(r["gi"], r["bt"], r.get("fl_b"), r.get("cc_b"), r.get("lr_b"), r["lk"]) for r in recs]


def track(args):
    base_p, arm_p = args
    base, arm = rd.load(base_p), rd.load(arm_p)
    bb, ab = rd.prepare(base), rd.prepare(arm)
    out = dict(base_bad=len(rd.mismatches(base, bb, "v2.7.1")), arm_bad=len(rd.mismatches(arm, ab, "current")),
               same_inputs=_inputs(base) == _inputs(arm), locked_min=rd.locked_minutes(ab))
    for tag, beats, recs in (("base", bb, base), ("arm", ab, arm)):
        mv = [m for m in rd.moves(beats, rd.recorded(recs)) if m["locked"]]
        out[tag] = dict(moves=len(mv), rev=sum(m["reverted"] for m in mv),
                        fallback=sum(m["fallback"] for m in mv), grid=sum(m["kind"] == "grid" for m in mv))
    return out


def sidecar_stats(base_dir: Path, arm_dir: Path, half: str, jobs: int) -> dict:
    jobs_l, keys = [], []
    for s in sorted(p.name for p in arm_dir.iterdir() if p.is_dir()):
        for p in sorted((arm_dir / s).glob("*.db.jsonl")):
            tid = p.name[: -len(".db.jsonl")]
            if s in SPLIT and half_of(tid) != half:
                continue
            jobs_l.append((base_dir / s / p.name, p))
            keys.append((s, tid))
    with ProcessPoolExecutor(jobs) as pool:
        rows = list(pool.map(track, jobs_l, chunksize=2))
    agg = {}
    for (s, tid), r in zip(keys, rows):
        a = agg.setdefault(s, dict(n=0, base_bad=0, arm_bad=0, diff_inputs=0, locked_min=0.0,
                                   base=dict(moves=0, rev=0, fallback=0, grid=0), arm=dict(moves=0, rev=0, fallback=0, grid=0)))
        a["n"] += 1
        a["base_bad"] += r["base_bad"]
        a["arm_bad"] += r["arm_bad"]
        a["diff_inputs"] += not r["same_inputs"]
        a["locked_min"] += r["locked_min"]
        for t in ("base", "arm"):
            for k in a[t]:
                a[t][k] += r[t][k]
        if tid == EDM:
            agg["edm_recording"] = dict(base=r["base"], arm=r["arm"], locked_min=r["locked_min"])
    return agg


def beats_identical(arm_root: Path, base_root: Path, half: str) -> tuple[int, int]:
    """(tracks compared, tracks whose /beat event lines differ) over the bench dumps."""
    n = bad = 0
    for ds in SPLIT:
        for res in (arm_root / "results" / ds).glob("*.json"):
            if res.name == "summary.json":
                continue
            r = json.loads(res.read_text())
            if half_of(r["track_id"]) != half:
                continue
            b = json.loads((base_root / "results" / ds / res.name).read_text())
            arm_d = arm_root / "dumps" / ds / f"{r['dump']['cache_key']}.jsonl"
            base_d = base_root / "dumps" / ds / f"{b['dump']['cache_key']}.jsonl"

            def lines(p):
                with open(p) as f:
                    return [x for x in f if '"/fosfora/v1/beat"' in x]
            n += 1
            bad += lines(arm_d) != lines(base_d)
    return n, bad


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("root", type=Path)
    ap.add_argument("sidecars", type=Path)
    ap.add_argument("--half", default="tune", choices=["tune", "holdout"])
    ap.add_argument("--base-root", type=Path)
    ap.add_argument("--base-sidecars", type=Path)
    ap.add_argument("--jobs", type=int, default=8)
    ap.add_argument("--fixture", type=Path, help="a bench/make_fixture.py dir: G9 runs check_fixture.py on it")
    a = ap.parse_args()
    man = json.loads(MANIFEST.read_text())
    base_root = a.base_root or Path(man["baseline"]["bench_root"])
    base_side = a.base_sidecars or Path(man["baseline"]["sidecars"])
    h = a.half
    cand, base = rep.report(a.root, h), rep.report(base_root, h)
    d = rep.compare(cand, base)
    ss = sidecar_stats(base_side, a.sidecars, h, a.jobs)
    print(f"arm {a.root.name} vs {base_root.name}, half {h}")
    ok_all = True

    def check(name, ok, detail):
        nonlocal ok_all
        ok_all &= bool(ok)
        print(f"  [{'PASS' if ok else 'MISS'}] {name}: {detail}")

    def m(ds, k):
        return cand["datasets"][ds][h][k], base["datasets"][ds][h][k]

    sets = [s for s in ss if s != "edm_recording"]
    check("G0 replica exact (baseline rule v2.7.1, arm rule current) and tracker inputs identical",
          all(ss[s]["base_bad"] == 0 and ss[s]["arm_bad"] == 0 and ss[s]["diff_inputs"] == 0 for s in sets),
          "; ".join(f"{s} n={ss[s]['n']} mism {ss[s]['base_bad']}/{ss[s]['arm_bad']} diff-in {ss[s]['diff_inputs']}" for s in sets))

    def rate(s, t):
        return ss[s][t]["moves"] / max(ss[s]["locked_min"], 1e-9)

    for s in sets:
        print(f"    moves {s:17s} {ss[s]['base']['moves']:5d} -> {ss[s]['arm']['moves']:5d} locked "
              f"({rate(s, 'base'):5.2f} -> {rate(s, 'arm'):5.2f}/min over {ss[s]['locked_min']:.1f} min), "
              f"reverted<10s {ss[s]['base']['rev']} -> {ss[s]['arm']['rev']}, fallback {ss[s]['base']['fallback']} -> "
              f"{ss[s]['arm']['fallback']}, grid {ss[s]['base']['grid']} -> {ss[s]['arm']['grid']}")
    lim = dict(harmonix=0.6, giantsteps_tempo=0.6, ballroom=0.85)
    check("P1 locked moves/min (harmonix, giantsteps <= .6x; ballroom <= .85x)",
          all(rate(s, "arm") <= f * rate(s, "base") + 1e-9 for s, f in lim.items() if s in ss),
          ", ".join(f"{s} {rate(s, 'arm') / max(rate(s, 'base'), 1e-9):.2f}x" for s in lim if s in ss))
    if "edm_recording" in ss:
        e = ss["edm_recording"]
        check("P1 EDM recording locked moves <= 21", e["arm"]["moves"] <= 21,
              f"{e['base']['moves']} -> {e['arm']['moves']} (reverted {e['base']['rev']} -> {e['arm']['rev']})")
    hb, ha = ss["harmonix"]["base"]["rev"], ss["harmonix"]["arm"]["rev"]
    check("P2 harmonix reverted within 10 s <= .4x", ha <= 0.4 * hb, f"{hb} -> {ha}")
    dh, db = m("harmonix", "downbeat_f"), m("ballroom", "downbeat_f")
    check("G1 harmonix downbeat_f >= base", dh[0] >= dh[1] - 1e-9, f"{dh[1]:.4f} -> {dh[0]:.4f} ({dh[0] - dh[1]:+.4f})")
    check("G2 ballroom downbeat_f >= base - .005", db[0] >= db[1] - 0.005 - 1e-9, f"{db[1]:.4f} -> {db[0]:.4f} ({db[0] - db[1]:+.4f})")
    flips = {}
    for ds in ("harmonix", "ballroom"):
        lost = gained = 0
        ex_l, ex_g = [], []
        for f in (a.root / "results" / ds).glob("*.json"):
            if f.name == "summary.json":
                continue
            r = json.loads(f.read_text())
            if half_of(r["track_id"]) != h:
                continue
            b = json.loads((base_root / "results" / ds / f.name).read_text())
            x = r["metrics"].get("downbeat", {}).get("f_measure")
            y = b["metrics"].get("downbeat", {}).get("f_measure")
            if x is None or y is None:
                continue
            if x - y < -0.1:
                lost += 1
                ex_l.append((r["track_id"], y, x))
            elif x - y > 0.1:
                gained += 1
                ex_g.append((r["track_id"], y, x))
        flips[ds] = (lost, gained, ex_l, ex_g)
    check("G3 downbeat flips: lost <= gained + 2 sqrt(lost + gained)",
          all(lo <= g + 2 * math.sqrt(lo + g) for lo, g, _, _ in flips.values()),
          "; ".join(f"{ds} lost {v[0]} / gained {v[1]}" for ds, v in flips.items()))
    rows_c = [json.loads(x) for x in (a.root / "batida.jsonl").read_text().splitlines() if x.strip()]
    rows_b = [json.loads(x) for x in (base_root / "batida.jsonl").read_text().splitlines() if x.strip()]
    mc = sum(r["down_f"] for r in rows_c) / len(rows_c)
    mb = sum(r["down_f"] for r in rows_b) / len(rows_b)
    wc = [r["down_f"] for r in rows_c if "waltz" in r["track"]]
    wb = [r["down_f"] for r in rows_b if "waltz" in r["track"]]
    wcm, wbm = sum(wc) / len(wc), sum(wb) / len(wb)
    check("G4 batida down_f mean >= base - .01, waltz >= base - .02", mc >= mb - 0.01 and wcm >= wbm - 0.02,
          f"all {mb:.4f} -> {mc:.4f}, waltz {wbm:.4f} -> {wcm:.4f} ({len(rows_c)} renders)")
    n_cmp, n_diff = beats_identical(a.root, base_root, h)
    same = all(cand["datasets"][ds][h].get(k) == base["datasets"][ds][h].get(k)
               for ds in SPLIT for k in ("acc1", "acc2", "no_est", "beat_f", "cmlt"))
    check("G5 /beat identical on every bench track; beat F, CMLt, acc1/acc2 identical", n_diff == 0 and same,
          f"{n_cmp} dumps compared, {n_diff} differ; beat_f harmonix {m('harmonix', 'beat_f')[1]} -> "
          f"{m('harmonix', 'beat_f')[0]}, ballroom {m('ballroom', 'beat_f')[1]} -> {m('ballroom', 'beat_f')[0]}")
    if "negatives" in ss:
        check("G6 negatives locked moves <= base", ss["negatives"]["arm"]["moves"] <= ss["negatives"]["base"]["moves"],
              f"{ss['negatives']['base']['moves']} -> {ss['negatives']['arm']['moves']}")
    if "marks" in ss:
        check("G7 marks locked moves <= .6x", ss["marks"]["arm"]["moves"] <= 0.6 * ss["marks"]["base"]["moves"],
              f"{ss['marks']['base']['moves']} -> {ss['marks']['arm']['moves']}")
    hc, hb2 = cand["harmonix_all"], base["harmonix_all"]
    ident = all(abs(hc[k] - hb2[k]) < 1e-9 for k in ("drop_hit_rate", "false_drops_per_min"))
    check("G8 harmonix drop metrics identical, label boundary F within .01",
          ident and abs(hc["boundary_3_0s_f"] - hb2["boundary_3_0s_f"]) <= 0.01,
          ", ".join(f"{k} {hb2[k]:.4f} -> {hc[k]:.4f}" for k in hc))
    if a.fixture:
        binary = (a.root / "arm.txt").read_text().split()[3]
        out = subprocess.run([str(REPO / "bench/check_fixture.py"), str(a.fixture)], capture_output=True, text=True,
                             env=dict(os.environ, FOSFORA_BIN=binary))
        res = json.loads((a.fixture / "result.json").read_text())
        check("G9 CI fixture (bench/check_fixture.py on the arm binary)", out.returncode == 0,
              f"{out.stdout.strip().splitlines()[-1] if out.stdout.strip() else out.stderr.strip()[-200:]}; "
              f"downbeat F {res['metrics']['downbeat']['f_measure']:.4f}")
    for ds, (lo, g, el, eg) in flips.items():
        for t, b, c in sorted(el, key=lambda x: x[2] - x[1])[:6]:
            print(f"    LOST {ds} {t} {b:.3f} -> {c:.3f}")
        for t, b, c in sorted(eg, key=lambda x: x[1] - x[2])[:6]:
            print(f"    GAIN {ds} {t} {b:.3f} -> {c:.3f}")
    print("VERDICT:", "ALL PASS" if ok_all else "MISS")
    return 0 if ok_all else 1


if __name__ == "__main__":
    sys.exit(main())
