#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Board #4163 round: one arm's numbers from its q4163_arm.sh root, and the comparison
against another arm (the baseline).

    bench/q4163_report.py <root> [--half tune|holdout|all] [--against <base_root>] [--json F]

Per dataset and half: the split metrics (q_split_baseline.py), the beat phase report
(analyze_beat_phase.py), and for Harmonix the structure / drop metrics of the full set.
Batida: beat F, tempo ratio and lag per render, by template. Negatives: the 155 clips'
published-tempo count, locked fraction and beats per minute, overall and for the pulse-low
third (neg_real.py's definitions). With --against: every metric's delta, and the tracks
whose beat F moved by more than 0.1 either way, lost next to gained.

Only the tune half is read before the verdict; --half holdout is for the verdict run.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parent.parent
NEG = Path.home() / "ai/audio/kick-work/fma/negatives"
DATASETS = ["ballroom", "harmonix", "giantsteps_tempo"]
P = "/fosfora/v1/"
FLIP = 0.1


def half_of(track_id: str) -> str:
    return "tune" if int(hashlib.sha256(track_id.encode()).hexdigest(), 16) % 2 == 0 else "holdout"


def split_metrics(root: Path) -> dict:
    out = subprocess.run([sys.executable, str(REPO / "bench/q_split_baseline.py"), str(root), *DATASETS],
                         capture_output=True, text=True, check=True)
    return json.loads(out.stdout)


def phase(root: Path, half: str) -> dict:
    tmp = root / f"phase_{half}.json"
    subprocess.run([str(REPO / "bench/analyze_beat_phase.py"), str(root), "ballroom", "harmonix",
                    "--half", half, "--json", str(tmp)], capture_output=True, check=True)
    d = json.loads(tmp.read_text())
    return {ds: {k: v for k, v in d[ds].items() if k != "tracks"} for ds in ("ballroom", "harmonix") if ds in d}


def structure_drop(root: Path) -> dict:
    s = json.loads((root / "results/harmonix/summary.json").read_text())["metrics"]
    return dict(drop_hit_rate=s["drop"]["hit_rate"], false_drops_per_min=s["drop"]["false_drops_per_min"],
                predict_drop_false_alarms_per_min=s["predict_drop"]["false_alarms_per_min"],
                predict_drop_coverage_0_5=s["predict_drop"]["0.5"]["coverage"],
                predict_drop_coverage_0_8=s["predict_drop"]["0.8"]["coverage"],
                boundary_3_0s_f=s["structure"]["boundary_3_0s.f"]["mean"])


def per_track(root: Path, ds: str, half: str) -> dict:
    out = {}
    for f in (root / "results" / ds).glob("*.json"):
        if f.name == "summary.json":
            continue
        r = json.loads(f.read_text())
        if half == "all" or half_of(r["track_id"]) == half:
            out[r["track_id"]] = r["metrics"]["beat"]["f_measure"]
    return out


def batida(root: Path) -> dict:
    rows = [json.loads(l) for l in (root / "batida.jsonl").read_text().splitlines() if l.strip()]
    by = {}
    for r in rows:
        name = r["track"]  # "template:techno s1"
        template = name.split(":")[1].split()[0]
        seed = int(name.split("s")[-1])
        by.setdefault(template, {})[seed] = dict(beat_f=r.get("beat_f"), bpm_ratio=r.get("bpm_ratio"),
                                                 beat_lag_ms=r.get("beat_lag_ms"), down_f=r.get("down_f"),
                                                 beats_est=(r.get("beats_true_est") or [None, None])[1])
    summary = {}
    for t, seeds in by.items():
        fs = [v["beat_f"] for v in seeds.values() if v["beat_f"] is not None]
        summary[t] = dict(n=len(seeds), beat_f_mean=float(np.mean(fs)) if fs else None,
                          beat_f=[seeds[s]["beat_f"] for s in sorted(seeds)],
                          bpm_ratio=[seeds[s]["bpm_ratio"] for s in sorted(seeds)],
                          beat_lag_ms=[seeds[s]["beat_lag_ms"] for s in sorted(seeds)],
                          beats_est=[seeds[s]["beats_est"] for s in sorted(seeds)], seeds=sorted(seeds))
    return summary


def neg_stats(path: Path):
    published = False
    locked = n = beats = 0
    t_end = 0.0
    with open(path) as f:
        for line in f:
            r = json.loads(line)
            a = r.get("addr", "")
            if a == P + "bpm" and r["args"][0]["f"] > 0:
                published = True
            elif a == P + "feat/beat_locked":
                locked += r["args"][0]["f"] > 0.5
                n += 1
            elif a == P + "beat":
                beats += 1
            t_end = max(t_end, r.get("ts", 0.0))
    return published, (locked / n if n else 0.0), beats / max(t_end / 60, 1e-9)


def negatives(root: Path) -> dict:
    rows = json.loads((NEG / "negatives.json").read_text())
    seen = set()
    rows = [r for r in rows if not (r["id"] in seen or seen.add(r["id"]))]
    pulses = sorted(r["pulse"] for r in rows)
    low_cut = pulses[len(pulses) // 3]
    stats = {}
    for r in rows:
        p = root / "negatives" / f"{r['id']:06d}.jsonl"
        if p.is_file():
            stats[r["id"]] = neg_stats(p)
    xs = list(stats.values())
    low = [stats[r["id"]] for r in rows if r["pulse"] < low_cut and r["id"] in stats]
    return dict(n=len(xs), publish=int(sum(x[0] for x in xs)), locked_mean=float(np.mean([x[1] for x in xs])),
                beats_per_min=float(np.mean([x[2] for x in xs])), pulse_low_n=len(low),
                pulse_low_beats_per_min=float(np.mean([x[2] for x in low])) if low else None)


def report(root: Path, half: str) -> dict:
    halves = ["tune", "holdout", "all"] if half == "all" else [half]
    sm = split_metrics(root)
    out = dict(root=str(root), half=half, arm=(root / "arm.txt").read_text().splitlines()[0] if (root / "arm.txt").exists() else None,
               datasets={ds: {h: sm[ds][h] for h in halves if h in sm.get(ds, {})} for ds in DATASETS},
               phase={h: phase(root, h) for h in (halves if half != "all" else ["tune", "holdout"])},
               harmonix_all=structure_drop(root), batida=batida(root), negatives=negatives(root),
               per_track={ds: per_track(root, ds, half) for ds in ("harmonix", "ballroom")})
    return out


def compare(cand: dict, base: dict) -> dict:
    d = dict(datasets={}, phase={}, harmonix_all={}, batida={}, negatives={}, flips={})
    for ds, halves in cand["datasets"].items():
        for h, m in halves.items():
            b = base["datasets"].get(ds, {}).get(h, {})
            d["datasets"][f"{ds}/{h}"] = {k: (round(v - b[k], 4) if isinstance(v, (int, float)) and isinstance(b.get(k), (int, float)) else None)
                                          for k, v in m.items()}
    for h, dss in cand["phase"].items():
        for ds, m in dss.items():
            b = base["phase"].get(h, {}).get(ds, {}).get(h, {})
            mm = m.get(h, {})
            d["phase"][f"{ds}/{h}"] = {k: (round(mm[k] - b[k], 4) if isinstance(mm.get(k), (int, float)) and isinstance(b.get(k), (int, float)) else None) for k in mm}
    for k, v in cand["harmonix_all"].items():
        b = base["harmonix_all"].get(k)
        d["harmonix_all"][k] = round(v - b, 4) if isinstance(v, (int, float)) and isinstance(b, (int, float)) else None
    for t, m in cand["batida"].items():
        b = base["batida"].get(t, {})
        d["batida"][t] = dict(beat_f_delta=[round(x - y, 3) if x is not None and y is not None else None
                                            for x, y in zip(m["beat_f"], b.get("beat_f", []))])
    for k, v in cand["negatives"].items():
        b = base["negatives"].get(k)
        d["negatives"][k] = round(v - b, 3) if isinstance(v, (int, float)) and isinstance(b, (int, float)) else None
    for ds in ("harmonix", "ballroom"):
        c, b = cand["per_track"][ds], base["per_track"][ds]
        lost = sorted(((t, round(b[t], 3), round(c[t], 3)) for t in c if t in b and c[t] - b[t] < -FLIP), key=lambda r: r[2] - r[1])
        gained = sorted(((t, round(b[t], 3), round(c[t], 3)) for t in c if t in b and c[t] - b[t] > FLIP), key=lambda r: r[1] - r[2])
        d["flips"][ds] = dict(n_lost=len(lost), n_gained=len(gained), lost=lost, gained=gained)
    return d


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("root", type=Path)
    ap.add_argument("--half", default="tune", choices=["tune", "holdout", "all"])
    ap.add_argument("--against", type=Path)
    ap.add_argument("--json", type=Path)
    a = ap.parse_args()
    cand = report(a.root, a.half)
    out = dict(candidate=cand)
    if a.against:
        base = report(a.against, a.half)
        out["baseline"] = base
        out["delta"] = compare(cand, base)
    if a.json:
        a.json.write_text(json.dumps(out, indent=1))
    # terse print
    for ds, halves in cand["datasets"].items():
        for h, m in halves.items():
            line = f"{ds:16s} {h:7s} " + " ".join(f"{k}={v}" for k, v in m.items())
            if a.against:
                dd = out["delta"]["datasets"].get(f"{ds}/{h}", {})
                line += "   Δ " + " ".join(f"{k}={v:+.4f}" for k, v in dd.items() if v is not None and k != "n")
            print(line)
    for h, dss in cand["phase"].items():
        for ds, m in dss.items():
            mm = m.get(h, {})
            print(f"phase {ds:10s} {h:7s} on={mm.get('on', 0):.3f} anti={mm.get('anti', 0):.3f} anti_locked={mm.get('anti_locked')} beat_f={mm.get('beat_f', 0):.4f}")
    print("harmonix all:", " ".join(f"{k}={v}" for k, v in cand["harmonix_all"].items()))
    for t, m in sorted(cand["batida"].items()):
        print(f"batida {t:9s} beat_f {m['beat_f']}")
    print("negatives:", cand["negatives"])
    if a.against:
        for ds, fl in out["delta"]["flips"].items():
            print(f"flips {ds}: lost {fl['n_lost']} gained {fl['n_gained']}")
            for t, b, c in fl["lost"][:12]:
                print(f"   lost   {t:40s} {b:.3f} -> {c:.3f}")
            for t, b, c in fl["gained"][:12]:
                print(f"   gained {t:40s} {b:.3f} -> {c:.3f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
