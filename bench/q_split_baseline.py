#!/usr/bin/env python3
"""Per-split-half means of the bench metrics from a run_bench results dir.

    bench/q_split_baseline.py <out_root> <dataset>...

Split: sha256(track_id) parity, even -> tune, odd -> holdout. Tempo accuracy
counts a no-estimate as a miss (the q-round convention).
"""
import hashlib
import json
import sys
from pathlib import Path


def half(track_id):
    return "tune" if int(hashlib.sha256(track_id.encode()).hexdigest(), 16) % 2 == 0 else "holdout"


def get(m, *path):
    for p in path:
        if not isinstance(m, dict) or p not in m:
            return None
        m = m[p]
    return m


def main():
    root = Path(sys.argv[1])
    out = {}
    for ds in sys.argv[2:]:
        rows = []
        for f in sorted((root / "results" / ds).glob("*.json")):
            if f.name == "summary.json":
                continue
            r = json.loads(f.read_text())
            m = r["metrics"]
            tempo = m.get("tempo") or {}
            no_est = bool(tempo.get("no_estimate")) or not tempo
            rows.append(dict(
                half=half(r["track_id"]),
                acc1=0.0 if no_est else float(bool(tempo.get("acc1"))),
                acc2=0.0 if no_est else float(bool(tempo.get("acc2"))),
                no_est=int(no_est),
                beat_f=get(m, "beat", "f_measure"), cmlt=get(m, "beat", "cmlt"), amlt=get(m, "beat", "amlt"),
                downbeat_f=get(m, "downbeat", "f_measure"),
            ))
        out[ds] = {}
        for h in ("tune", "holdout", "all"):
            sel = [r for r in rows if h == "all" or r["half"] == h]
            agg = {"n": len(sel), "no_est": sum(r["no_est"] for r in sel)}
            for k in ("acc1", "acc2", "beat_f", "cmlt", "amlt", "downbeat_f"):
                vals = [r[k] for r in sel if r[k] is not None]
                if vals:
                    agg[k] = round(sum(vals) / len(vals), 4)
            out[ds][h] = agg
    print(json.dumps(out, indent=1))


if __name__ == "__main__":
    main()
