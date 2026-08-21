#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""Attribute each specimen drop hit/miss to the conjunct that decided it (#2211).

    bench/diagnose_drop.py --sidecars bench/out/dropsweep/music \
                           --refs bench/out/dropsweep/music_refs.json

Reads v2 structure sidecars, which carry the drop machine's own per-tick account of
itself — the pre-normalization `loudness_m`/`sub_bass` it actually tested, the baseline it
computed, and every conjunct's verdict. Prior work (#2210) had to infer these from wire
proxies (`/energy` is short-term loudness, `feat/sub_bass` is adaptively re-ranged); this
reads the detector's own numbers.

Candidate drops come from `drop_reference.py` (raw-audio withdrawal->return + RMS step).
A candidate is accepted as a reference drop when the Q4 boundary detector independently
announced a boundary within --boundary-window of it: two unrelated signals agreeing,
rather than a hand-picked list. Intros out of silence are excluded by --min-time.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np

CONJUNCTS = ("armed", "not-refractory", "jump", "sub-return")


def load_sidecar(path: Path) -> tuple[dict, dict[str, np.ndarray]]:
    meta, rows = {}, []
    for line in path.read_text().splitlines():
        if not line.strip():
            continue
        r = json.loads(line)
        if "meta" in r:
            meta = r
        else:
            rows.append(r)
    if not rows:
        sys.exit(f"{path}: no tick records")
    cols = {}
    for k in ("ts", "d_tick", "d_t", "d_loud_m", "d_sub", "d_sub_ref", "d_build",
              "d_high", "d_base", "d_jump", "d_ring", "d_armed", "d_subret",
              "d_refrac", "d_fired"):
        if k not in rows[0]:
            sys.exit(f"{path}: schema v1 sidecar (no {k}) — re-dump with the v2 binary")
        cols[k] = np.array([r[k] for r in rows], dtype=np.float64)
    # v3 (#2299) candidate conjuncts and v4 (#2370) build-up logistic inputs, both optional
    # — the Harmonix corpus is still v2, and a per-event attribution is worth having there
    # too. `arm_mechanics` explains why the arm timer stalled; these say what the build-up
    # it stalled on was made of.
    for k in ("d_kick", "d_perc", "d_hratio",
              "d_f_loud", "d_f_cent", "d_f_onset", "d_f_subgone",
              "d_loud_trend", "d_loud_s", "d_cent", "d_cent_slow",
              "d_onset_fast", "d_onset_slow", "d_sub_slow"):
        if k in rows[0]:
            cols[k] = np.array([r[k] for r in rows], dtype=np.float64)
    return meta, cols


def load_boundaries(path: Path) -> list[tuple[float, float]]:
    """(musical time, confidence) for each /section/boundary announcement."""
    out = []
    if not path.exists():
        return out
    for line in path.read_text().splitlines():
        if not line.strip():
            continue
        r = json.loads(line)
        if r.get("addr", "").endswith("/section/boundary"):
            conf = float(r["args"][0]["f"])
            age = float(r["args"][1]["f"]) if len(r["args"]) > 1 else 0.0
            out.append((float(r["ts"]) - age, conf))
    return out


def load_labels(labels_dir: Path) -> dict[str, dict]:
    """bench/labels/*.json -> the same {track: {"drops": [...]}} shape as --refs.

    Drops and recorded negatives go into one list tagged by `kind`, because the question
    the round is asking spans both: at a drop, which conjunct blocked the fire; at a
    rejection, which conjunct let it through.
    """
    out: dict[str, dict] = {}
    for p in sorted(labels_dir.glob("*.json")):
        b = json.loads(p.read_text())
        marks = [{"time": float(d["time"]), "kind": "drop"} for d in b.get("drops") or []]
        marks += [{"time": float(d["time"]), "kind": "not_drop", "label": d.get("label")}
                  for d in b.get("not_drops") or []]
        if marks:
            out[b["track_id"]] = {"drops": sorted(marks, key=lambda m: m["time"])}
    if not out:
        sys.exit(f"no label bundles with marks in {labels_dir}")
    return out


def summarize_trio(hits: list[dict], miss: list[dict], fired_on_negatives: list[dict]) -> None:
    """Can the v3 HPSS trio tell a fire that was right from a fire that was wrong?

    The comparison that matters is fires-vs-fires — hits against fires on a recorded
    negative — not drops against non-drops. A feature that separates drops from arbitrary
    moments says nothing about a gate the machine only consults when it is already
    about to fire.
    """
    if not hits or not fired_on_negatives or not hits[0].get("trio"):
        return
    print("\nHPSS trio at the machine's own fires (v3 sidecar):")
    print(f"  {'':22}{'right fires':>13}{'wrong fires':>13}{'missed drops':>14}")
    for k in hits[0]["trio"]:
        def m(rows):
            vals = [r["trio"][k] for r in rows if k in (r.get("trio") or {})]
            return sum(vals) / len(vals) if vals else float("nan")
        print(f"  {k:22}{m(hits):13.4f}{m(fired_on_negatives):13.4f}{m(miss):14.4f}")
    print(f"  {'n':22}{len(hits):13}{len(fired_on_negatives):13}{len(miss):14}")


def check_alignment(cols: dict[str, np.ndarray]) -> str:
    """The sidecar self-decimates; the tracker ticks on its own clock. Prove they agree."""
    d = np.diff(cols["d_tick"])
    dt_err = np.abs(cols["d_t"] - cols["ts"]).max()
    gaps = int((d != 1).sum())
    return (f"ticks {len(cols['d_tick'])}, index gaps {gaps}, "
            f"max |d_t - ts| {dt_err:.2e}s")


def diagnose(name: str, cols: dict[str, np.ndarray], meta: dict,
             refs: list[dict], bounds: list[tuple[float, float]],
             args) -> list[dict]:
    cfg = meta.get("cfg", {})
    jump_gate = float(cfg.get("drop_loud_jump", 0.08))
    arm_sustain = float(cfg.get("drop_arm_sustain", 4.0))
    ts = cols["ts"]
    fired_t = ts[cols["d_fired"] > 0.5]

    print(f"\n=== {name}")
    print(f"    alignment: {check_alignment(cols)}")
    print(f"    ring occupancy: max {int(cols['d_ring'].max())} ticks "
          f"= {cols['d_ring'].max() / 10.0:.1f}s baseline window")
    print(f"    machine fired at: {[round(float(t), 2) for t in fired_t] or 'nothing'}")

    rows = []
    for ref in refs:
        t0 = ref["time"]
        if t0 < args.min_time:
            continue
        near = [(abs(bt - t0), bt, c) for bt, c in bounds if abs(bt - t0) <= args.boundary_window]
        if near:
            _, btime, bconf = min(near)
        elif args.boundary_window == float("inf"):
            # Labelled by ear: no Q4 boundary needed, and its absence is not a reason to
            # drop the reference.
            btime, bconf = None, None
        else:
            continue

        # Window: the drop moment plus the bar or so after it, where a fire would count.
        w = (ts >= t0 - args.pre) & (ts <= t0 + args.post)
        if not w.any():
            continue
        hit = bool(((fired_t >= t0 - args.pre) & (fired_t <= t0 + args.post)).any())

        jump_max = float(cols["d_jump"][w].max())
        armed_frac = float(cols["d_armed"][w].mean())
        subret_frac = float(cols["d_subret"][w].mean())
        refrac_frac = float(cols["d_refrac"][w].mean())
        high_max = float(cols["d_high"][w].max())
        build_max = float(cols["d_build"][w].max())

        # Which conjuncts were ever simultaneously true in the window, and which single
        # one blocked it: a conjunct is "blocking" when it is false on every tick where
        # all the others hold.
        others_ok = {
            "armed": (cols["d_refrac"] < 0.5) & (cols["d_jump"] >= jump_gate) & (cols["d_subret"] > 0.5),
            "not-refractory": (cols["d_armed"] > 0.5) & (cols["d_jump"] >= jump_gate) & (cols["d_subret"] > 0.5),
            "jump": (cols["d_armed"] > 0.5) & (cols["d_refrac"] < 0.5) & (cols["d_subret"] > 0.5),
            "sub-return": (cols["d_armed"] > 0.5) & (cols["d_refrac"] < 0.5) & (cols["d_jump"] >= jump_gate),
        }
        self_ok = {
            "armed": cols["d_armed"] > 0.5,
            "not-refractory": cols["d_refrac"] < 0.5,
            "jump": cols["d_jump"] >= jump_gate,
            "sub-return": cols["d_subret"] > 0.5,
        }
        blockers = [c for c in CONJUNCTS
                    if (others_ok[c] & w).any() and not (others_ok[c] & self_ok[c] & w).any()]
        failed = [c for c in CONJUNCTS if not (self_ok[c] & w).any()]

        rows.append({
            "track": name, "time": t0, "hit": hit,
            "kind": ref.get("kind", "drop"), "label": ref.get("label"),
            "boundary_time": round(btime, 2) if btime is not None else None,
            "boundary_conf": round(bconf, 3) if bconf is not None else None,
            "rms_step_db": ref.get("rms_step_db"), "sub_null_db": ref.get("sub_null_db"),
            "trio": {k: round(float(cols[k][w].mean()), 4)
                     for k in ("d_kick", "d_perc", "d_hratio") if k in cols},
            "jump_max": round(jump_max, 4), "jump_gate": jump_gate,
            "armed_frac": round(armed_frac, 2), "subret_frac": round(subret_frac, 2),
            "refrac_frac": round(refrac_frac, 2),
            "high_max": round(high_max, 2), "arm_sustain": arm_sustain,
            "build_max": round(build_max, 3),
            "never_true": failed, "sole_blockers": blockers,
        })

        kind = ref.get("kind", "drop")
        if kind == "not_drop":
            # For a rejected fire the roles invert: firing is the error, so "HIT" would
            # read backwards.
            mark = "FIRED" if hit else "clean"
        else:
            mark = "HIT  " if hit else "MISS "
        if btime is not None and ref.get("rms_step_db") is not None:
            prov = (f"boundary {bconf:.2f} @ {btime:.1f}s | "
                    f"RMS step {ref['rms_step_db']:+5.1f} dB")
        else:
            prov = "by ear" + (f" — {ref['label']}" if ref.get("label") else "")
        print(f"  {mark} @ {t0:7.2f}s  {prov}")
        print(f"        jump max {jump_max:.4f} / gate {jump_gate:.3f}"
              f"{'  <-- BLOCKS' if 'jump' in failed else ''}   "
              f"armed {armed_frac * 100:3.0f}% of window (high {high_max:.1f}s / "
              f"{arm_sustain:.1f}s, build max {build_max:.2f})"
              f"{'  <-- BLOCKS' if 'armed' in failed else ''}")
        print(f"        sub-return true {subret_frac * 100:3.0f}%"
              f"{'  <-- BLOCKS' if 'sub-return' in failed else ''}   "
              f"refractory {refrac_frac * 100:3.0f}%"
              f"{'  <-- BLOCKS' if 'not-refractory' in failed else ''}")
        if not hit and failed:
            print(f"        never true in window: {', '.join(failed)}")
    return rows


def counterfactual(cols: dict[str, np.ndarray], t0: float, pre: float, post: float,
                   windows_s: list[float]) -> dict[str, float]:
    """Max jump achievable at this event for other baseline-window lengths.

    The shipped ring is sized at the analysis frame rate but pushed at tick rate, so its
    real span is ~12.9 s rather than the 1.5 s the constant names. Recomputing the
    running-min over candidate spans says whether window length is the lever at all.
    """
    ts, loud = cols["ts"], cols["d_loud_m"]
    out = {}
    for w_s in windows_s:
        k = max(1, int(round(w_s * 10.0)))
        # Running min over the trailing k ticks, inclusive of the current one.
        mins = np.array([loud[max(0, i - k + 1): i + 1].min() for i in range(len(loud))])
        j = loud - mins
        w = (ts >= t0 - pre) & (ts <= t0 + post)
        out[f"{w_s:g}s"] = round(float(j[w].max()), 4) if w.any() else float("nan")
    return out


def arm_mechanics(cols: dict[str, np.ndarray], t0: float, pre_roll: float,
                  arm_level: float, arm_sustain: float) -> dict:
    """Why the arm timer stalled: is it the build-up level, or the dip penalty?

    `high_duration` integrates ticks above `arm_level` and *decays at 2x* below it, so a
    build-up that oscillates around the level loses ground faster than it gains. Separating
    "never high enough" from "punished for flickering" decides which lever can move.
    """
    ts, build = cols["ts"], cols["d_build"]
    w = (ts >= t0 - pre_roll) & (ts <= t0)
    b = build[w]
    if len(b) == 0:
        return {}
    dt = 0.1

    def sim(level: float, decay: float) -> float:
        h = peak = 0.0
        for v in b:
            h = h + dt if v > level else max(0.0, h - decay * dt)
            peak = max(peak, h)
        return peak

    above = b > arm_level
    runs, cur = [], 0
    for a in above:
        cur = cur + 1 if a else 0
        runs.append(cur)

    return {
        "build_max": round(float(b.max()), 3),
        "build_p90": round(float(np.percentile(b, 90)), 3),
        "frac_above_level": round(float(above.mean()), 3),
        "longest_run_s": round(max(runs) * dt, 2),
        "peak_high_2x": round(sim(arm_level, 2.0), 2),
        "peak_high_1x": round(sim(arm_level, 1.0), 2),
        "peak_high_nodecay": round(sim(arm_level, 0.0), 2),
        "peak_high_level_0.5": round(sim(0.5, 2.0), 2),
        "peak_high_level_0.4": round(sim(0.4, 2.0), 2),
        "arm_sustain": arm_sustain,
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--sidecars", type=Path, required=True)
    ap.add_argument("--refs", type=Path,
                    help="drop_reference.py candidates (mechanical; needs Q4 corroboration)")
    ap.add_argument("--labels", type=Path,
                    help="bench/labels/ — listener-labelled drops (#2299). A human verdict "
                         "needs neither boundary corroboration nor the intro filter, so "
                         "both are bypassed; recorded negatives are diagnosed too.")
    ap.add_argument("--boundary-window", type=float, default=4.0,
                    help="s; a candidate needs a Q4 boundary this close to be a reference")
    ap.add_argument("--min-time", type=float, default=10.0,
                    help="s; ignore candidates before this (track intros out of silence)")
    ap.add_argument("--pre", type=float, default=2.0, help="s before the reference to score")
    ap.add_argument("--post", type=float, default=4.0, help="s after the reference to score")
    ap.add_argument("--pre-roll", type=float, default=20.0,
                    help="s of run-up analysed for arm mechanics")
    ap.add_argument("--json", type=Path)
    args = ap.parse_args()

    if bool(args.refs) == bool(args.labels):
        ap.error("give exactly one of --refs (mechanical candidates) or --labels (by ear)")

    if args.labels:
        refs = load_labels(args.labels)
        # A listener's verdict is the corroboration, and "the track starts out of silence"
        # is something they already ruled on — Kevin logged 4.9 s on Prison of Pixels as a
        # rejected buildup rather than leaving it to a time filter.
        args.boundary_window = float("inf")
        args.min_time = -1.0
    else:
        refs = json.loads(args.refs.read_text())

    all_rows = []
    for name, ref in sorted(refs.items()):
        side = args.sidecars / f"{name}.jsonl"
        if not side.exists():
            print(f"(skip {name}: no sidecar at {side})")
            continue
        meta, cols = load_sidecar(side)
        bounds = load_boundaries(args.sidecars / f"{name}.signal.jsonl")
        rows = diagnose(name, cols, meta, ref["drops"], bounds, args)
        cfg = meta.get("cfg", {})
        for r in rows:
            r["jump_by_window"] = counterfactual(cols, r["time"], args.pre, args.post,
                                                 [1.5, 3.0, 6.0, 12.9, 20.0])
            r["arm"] = arm_mechanics(cols, r["time"], args.pre_roll,
                                     float(cfg.get("drop_arm_buildup", 0.6)),
                                     float(cfg.get("drop_arm_sustain", 4.0)))
        all_rows.extend(rows)

    print("\n" + "=" * 78)
    negatives = [r for r in all_rows if r["kind"] == "not_drop"]
    # Report on drops only, but keep the negatives in all_rows — they are the precision
    # evidence and the --json consumer needs them.
    drops = [r for r in all_rows if r["kind"] != "not_drop"]
    hits = [r for r in drops if r["hit"]]
    miss = [r for r in drops if not r["hit"]]
    print(f"references {len(drops)}: {len(hits)} hit, {len(miss)} missed")
    if negatives:
        fired = [r for r in negatives if r["hit"]]
        print(f"recorded negatives {len(negatives)}: {len(fired)} fired on "
              f"(the precision question), {len(negatives) - len(fired)} clean")
        summarize_trio(hits, miss, fired)
    if miss:
        blocked = {}
        for r in miss:
            for c in (r["never_true"] or ["(none — conjuncts never coincided)"]):
                blocked.setdefault(c, 0)
                blocked[c] += 1
        print("conjuncts never true in a missed window: " +
              ", ".join(f"{c} x{n}" for c, n in sorted(blocked.items(), key=lambda kv: -kv[1])))
        print("\njump ceiling by baseline-window length (max over each event window):")
        keys = list(miss[0]["jump_by_window"])
        print("  " + " " * 34 + "".join(f"{k:>9}" for k in keys))
        for r in miss:
            vals = "".join(f"{r['jump_by_window'][k]:9.4f}" for k in keys)
            print(f"  {r['track'][:22]:22} @{r['time']:7.2f}s{vals}   gate {r['jump_gate']:.3f}")
        for r in hits:
            vals = "".join(f"{r['jump_by_window'][k]:9.4f}" for k in keys)
            print(f"  {r['track'][:22]:22} @{r['time']:7.2f}s{vals}   gate {r['jump_gate']:.3f}  (HIT)")

    print("\narm mechanics over the " + f"{args.pre_roll:g}s run-up "
          f"(peak high_duration vs the {all_rows[0]['arm']['arm_sustain']:.1f}s requirement):")
    print(f"  {'event':32}{'bmax':>6}{'>lvl':>6}{'run':>6}"
          f"{'2x':>6}{'1x':>6}{'none':>6}{'l=.5':>6}{'l=.4':>6}")
    for r in sorted(all_rows, key=lambda r: (not r["hit"], r["track"], r["time"])):
        a = r["arm"]
        tag = f"{r['track'][:20]} @{r['time']:.1f}" + (" HIT" if r["hit"] else "")
        print(f"  {tag:32}{a['build_max']:6.2f}{a['frac_above_level']:6.2f}"
              f"{a['longest_run_s']:6.1f}{a['peak_high_2x']:6.1f}{a['peak_high_1x']:6.1f}"
              f"{a['peak_high_nodecay']:6.1f}{a['peak_high_level_0.5']:6.1f}"
              f"{a['peak_high_level_0.4']:6.1f}")

    if args.json:
        args.json.write_text(json.dumps(all_rows, indent=2) + "\n")
        print(f"\nwrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
