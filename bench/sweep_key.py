#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7"]
# ///
"""Replay the key back-end over production sidecars and sweep its constants (Q3).

    bench/sweep_key.py bench/datasets/giantsteps_key --validate bench/out/q-iter/results/giantsteps_key
    bench/sweep_key.py bench/datasets/giantsteps_key --stage profiles
    bench/sweep_key.py bench/datasets/giantsteps_key --stage dynamics --profile shaath --floor 54
    bench/sweep_key.py bench/datasets/giantsteps_key --stage frontend --profile shaath --floor 54 --tau 20

Input is `bench/out/keysweep/<ds>/<id>.jsonl` from bench/dump_key_sidecar.py — the
production front-end's per-hop e61/harmonic_ratio/silence. The replay ports key.rs
exactly (fold floor, EMA, energy/shape gates, 24-profile Pearson, hysteresis) plus
the emitter's 0.3 confidence gate and the bench's duration-weighted majority — so a
config's sweep score predicts its real `run_bench.py` score. `--validate` proves
that: it replays the SHIPPED config and diffs per-track majority keys against real
bench results. Run it before trusting any sweep number.

Sidecars exist for the tune half only (dump_key_sidecar.py default) — sweeps
cannot read holdout tracks by construction.

Profile sets transcribed from Essentia key.cpp (src/algorithms/tonal/key.cpp).
edma/edmm/braw/bgate are Faraldo Beatport-derived: if one wins, provenance vs the
604-track test set must be cleared before shipping (see #2079).
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import mir_eval.key
import numpy as np
from scipy.signal import lfilter

# --- key.rs constants (defaults = shipped values) ---------------------------------
ENERGY_FLOOR = 1e-9
SHAPE_MIN_VAR = 2e-4
KEY_MIN_CONFIDENCE = 0.3  # signal/emitter.rs
MIDI_LO = 36  # chroma.rs kernel range

NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
#: Tonic -> pitch class, sharps and flats (annotations use both spellings).
PC = {n: i for i, n in enumerate(NAMES)} | {
    "Db": 1, "Eb": 3, "Gb": 6, "Ab": 8, "Bb": 10,
}

PROFILES: dict[str, tuple[list[float], list[float]]] = {
    # Essentia key.cpp transcriptions (major, minor).
    "krumhansl": (
        [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88],
        [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17],
    ),
    "temperley": (
        [5.0, 2.0, 3.5, 2.0, 4.5, 4.0, 2.0, 4.5, 2.0, 3.5, 1.5, 4.0],
        [5.0, 2.0, 3.5, 4.5, 2.0, 4.0, 2.0, 4.5, 3.5, 2.0, 1.5, 4.0],
    ),
    "temperley2005": (
        [0.748, 0.060, 0.488, 0.082, 0.67, 0.46, 0.096, 0.715, 0.104, 0.366, 0.057, 0.4],
        [0.712, 0.084, 0.474, 0.618, 0.049, 0.46, 0.105, 0.747, 0.404, 0.067, 0.133, 0.33],
    ),
    "shaath": (
        [6.6, 2.0, 3.5, 2.3, 4.6, 4.0, 2.5, 5.2, 2.4, 3.7, 2.3, 3.4],
        [6.5, 2.7, 3.5, 5.4, 2.6, 3.5, 2.5, 5.2, 4.0, 2.7, 4.3, 3.2],
    ),
    # mixxxdj/libkeyfinder src/constants.cpp — the profiles KeyFinder actually ships
    # today (flatter than the thesis "shaath" set above), rounded to 4 decimals.
    "keyfinder": (
        [7.2390, 3.5035, 3.5845, 2.8451, 5.8190, 4.5587, 2.4478, 6.9947, 3.3911,
         4.5561, 4.0739, 4.4593],
        [7.0026, 3.1436, 4.3590, 5.4042, 3.6723, 4.0897, 3.9079, 6.1996, 3.6342,
         2.8724, 5.3547, 3.8324],
    ),
    "gomez": (
        [0.82, 0.00, 0.55, 0.00, 0.53, 0.30, 0.08, 1.00, 0.00, 0.38, 0.00, 0.47],
        [0.81, 0.00, 0.53, 0.54, 0.00, 0.27, 0.07, 1.00, 0.27, 0.07, 0.10, 0.36],
    ),
    "edma": (
        [1.00, 0.29, 0.50, 0.40, 0.60, 0.56, 0.32, 0.80, 0.31, 0.45, 0.42, 0.39],
        [1.00, 0.31, 0.44, 0.58, 0.33, 0.49, 0.29, 0.78, 0.43, 0.29, 0.53, 0.32],
    ),
    "edmm": (
        [0.083] * 12,
        [0.17235348, 0.04, 0.0761009, 0.12, 0.05621498, 0.08527853, 0.0497915,
         0.13451001, 0.07458916, 0.05003023, 0.09187879, 0.05545106],
    ),
    "braw": (
        [1.0000, 0.1573, 0.4200, 0.1570, 0.5296, 0.3669, 0.1632, 0.7711, 0.1676,
         0.3827, 0.2113, 0.2965],
        [1.0000, 0.2330, 0.3615, 0.3905, 0.2925, 0.3777, 0.1961, 0.7425, 0.2701,
         0.2161, 0.4228, 0.2272],
    ),
    "bgate": (
        [1.00, 0.00, 0.42, 0.00, 0.53, 0.37, 0.00, 0.77, 0.00, 0.38, 0.21, 0.30],
        [1.00, 0.00, 0.36, 0.39, 0.00, 0.38, 0.00, 0.74, 0.27, 0.00, 0.42, 0.23],
    ),
}


def rotated_profiles(name: str) -> np.ndarray:
    """24×12 like key.rs: rows 0..11 major (tonic = row), 12..23 minor."""
    major, minor = PROFILES[name]
    out = np.zeros((24, 12))
    for tonic in range(12):
        for pc in range(12):
            deg = (pc + 12 - tonic) % 12
            out[tonic, pc] = major[deg]
            out[tonic + 12, pc] = minor[deg]
    return out


def load_sidecar(path: Path) -> dict:
    ts, hr, e61, bass_pc, bass_mag = [], [], [], [], []
    with path.open() as f:
        for line in f:
            r = json.loads(line)
            ts.append(r["ts"])
            hr.append(r["hr"])
            e61.append(r["e61"])
            b = r.get("bass")
            bass_pc.append(b["pc"] if b else -1)
            bass_mag.append(b["mag"] if b else 0.0)
    return {
        "ts": np.array(ts),
        "hr": np.array(hr),
        "e61": np.array(e61, dtype=np.float64),
        "bass_pc": np.array(bass_pc, dtype=np.int64),
        "bass_mag": np.array(bass_mag),
    }


def fold_matrix(floor_midi: int) -> np.ndarray:
    m = np.zeros((61, 12))
    for s in range(61):
        midi = MIDI_LO + s
        if midi >= floor_midi:
            m[s, midi % 12] = 1.0
    return m


def replay(side: dict, cfg: dict, profs: np.ndarray) -> tuple[str | None, dict]:
    """Port of key.rs process() + emitter gate + duration-weighted majority.

    Returns (majority key string or None, extras)."""
    ts = side["ts"]
    x = side["e61"] @ fold_matrix(cfg["floor"])
    if cfg["w_bass"]:
        # Mirror of chroma.rs: the accepted bass observation deposits into the key
        # fold before the EMA (bass_pc == -1 where the tracker was silent).
        has = side["bass_pc"] >= 0
        x[np.nonzero(has)[0], side["bass_pc"][has]] += cfg["w_bass"] * side["bass_mag"][has]
    if cfg["comp"] == "sqrt":
        x = np.sqrt(x)
    if cfg["beta"]:
        x = x * side["hr"][:, None] ** cfg["beta"]

    dt = float(np.median(np.diff(ts))) if len(ts) > 1 else 0.046
    alpha = 1.0 - np.exp(-dt / cfg["tau"])
    means = lfilter([alpha], [1, -(1 - alpha)], x, axis=0)  # EMA from 0 state, like key.rs

    totals = means.sum(axis=1)
    shapes = means / np.maximum(totals[:, None], 1e-30)
    shape_var = shapes.var(axis=1)
    gated = (totals < ENERGY_FLOOR) | (shape_var < SHAPE_MIN_VAR)

    # Pearson of every mean against all 24 profiles, one matmul.
    mz = means - means.mean(axis=1, keepdims=True)
    mn = np.linalg.norm(mz, axis=1, keepdims=True)
    pz = profs - profs.mean(axis=1, keepdims=True)
    pn = np.linalg.norm(pz, axis=1)
    with np.errstate(divide="ignore", invalid="ignore"):
        corr = (mz / np.maximum(mn, 1e-12)) @ (pz / np.maximum(pn[:, None], 1e-12)).T
    corr = np.nan_to_num(corr)

    started = False
    current = challenger = 0
    challenger_time = 0.0
    confidence = 0.0
    last_emitted: int | None = None
    emits: list[tuple[float, int]] = []
    mode_mu = cfg.get("mode_mu", 0.0)
    for t in range(len(ts)):
        if gated[t]:
            confidence *= 0.99
        else:
            c = corr[t]
            best = int(c.argmax())
            if not started:
                current = best
                started = True
            elif best != current:
                if best == challenger and c[best] > c[current] + cfg["margin"]:
                    challenger_time += dt
                else:
                    challenger = best
                    challenger_time = 0.0
                if challenger_time >= cfg["switch_time"]:
                    current = best
                    challenger_time = 0.0
            else:
                challenger_time = 0.0
            confidence = float(np.clip(c[current], 0.0, 1.0))
        # Optional third-contrast mode override (output layer only): the voiced
        # third in the rolling mean outranks the profile's mode call when its
        # relative contrast exceeds mu; absence of contrast keeps the profile mode.
        out_key = current
        if mode_mu > 0.0 and started:
            tonic = current % 12
            m3, m4 = means[t, (tonic + 3) % 12], means[t, (tonic + 4) % 12]
            contrast = (m4 - m3) / (m4 + m3 + 1e-30)
            if contrast > mode_mu:
                out_key = tonic
            elif contrast < -mode_mu:
                out_key = tonic + 12
        if confidence >= KEY_MIN_CONFIDENCE and out_key != last_emitted:
            emits.append((ts[t], out_key))
            last_emitted = out_key

    if not emits:
        return None, {"n_changes": 0}
    end = ts[-1]
    held: dict[int, float] = {}
    for i, (t0, k) in enumerate(emits):
        t1 = emits[i + 1][0] if i + 1 < len(emits) else max(end, t0)
        held[k] = held.get(k, 0.0) + (t1 - t0)
    maj = max(held, key=held.get)
    key = NAMES[maj % 12] + (" minor" if maj >= 12 else " major")
    return key, {"n_changes": len(emits)}


def load_refs(dataset: Path, track_ids: list[str]) -> dict[str, str]:
    refs = {}
    for tid in track_ids:
        ann = json.loads((dataset / "norm" / f"{tid}.json").read_text())["key"]
        refs[tid] = f"{ann['tonic']} {ann['mode']}"
    return refs


def score_config(cfg: dict, sides: dict[str, dict], refs: dict[str, str]) -> dict:
    profs = rotated_profiles(cfg["profile"])
    scores, mode_ok, ref_major, major_ok, none = [], 0, 0, 0, 0
    taxonomy = {"exact": 0, "fifth": 0, "relative": 0, "parallel": 0, "other": 0}
    deltas = {d: 0 for d in range(12)}
    for tid, side in sides.items():
        est, _ = replay(side, cfg, profs)
        ref = refs[tid]
        if ref.endswith("major"):
            ref_major += 1
        if est is None:
            none += 1
            scores.append(0.0)
            continue
        s = mir_eval.key.weighted_score(ref, est)
        scores.append(s)
        for credit, name in ((1.0, "exact"), (0.5, "fifth"), (0.3, "relative"), (0.2, "parallel")):
            if abs(s - credit) < 1e-6:
                taxonomy[name] += 1
                break
        else:
            taxonomy["other"] += 1
        deltas[(PC[est.split()[0]] - PC[ref.split()[0]]) % 12] += 1
        if est.split()[1] == ref.split()[1]:
            mode_ok += 1
            if ref.endswith("major"):
                major_ok += 1
    n = len(sides)
    return {
        "mean": round(float(np.mean(scores)), 4),
        "mode_acc": round(mode_ok / n, 4),
        "major_recall": round(major_ok / ref_major, 4) if ref_major else None,
        "no_estimate_rate": round(none / n, 4),
        "taxonomy": taxonomy,
        "tonic_delta_hist": {d: c for d, c in deltas.items() if c},
    }


DEFAULT = {"profile": "krumhansl", "floor": 54, "tau": 12.0, "margin": 0.05,
           "switch_time": 3.0, "beta": 0, "comp": "linear", "w_bass": 0.0}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("dataset", type=Path)
    ap.add_argument("--validate", type=Path, help="results dir to diff the shipped config against")
    ap.add_argument("--stage", choices=["profiles", "dynamics", "frontend", "bass", "mode"])
    ap.add_argument("--profile", default=DEFAULT["profile"])
    ap.add_argument("--floor", type=int, default=DEFAULT["floor"])
    ap.add_argument("--tau", type=float, default=DEFAULT["tau"])
    ap.add_argument("--margin", type=float, default=DEFAULT["margin"])
    ap.add_argument("--switch-time", type=float, default=DEFAULT["switch_time"])
    ap.add_argument("--w-bass", type=float, default=DEFAULT["w_bass"])
    args = ap.parse_args()

    side_dir = Path(__file__).parent / "out" / "keysweep" / args.dataset.name
    paths = sorted(side_dir.glob("*.jsonl"))
    if not paths:
        sys.exit(f"no sidecars under {side_dir} — run bench/dump_key_sidecar.py first")
    print(f"loading {len(paths)} sidecars…", flush=True)
    sides = {p.stem: load_sidecar(p) for p in paths}
    refs = load_refs(args.dataset, list(sides))

    base = dict(DEFAULT, profile=args.profile, floor=args.floor, tau=args.tau,
                margin=args.margin, switch_time=args.switch_time, w_bass=args.w_bass)

    if args.validate:
        profs = rotated_profiles("krumhansl")
        agree = diff = missing = 0
        for tid, side in sides.items():
            rp = args.validate / f"{tid}.json"
            if not rp.exists():
                missing += 1
                continue
            real = json.loads(rp.read_text())["metrics"]["key"].get("estimated_key")
            est, _ = replay(side, DEFAULT, profs)
            if est == real:
                agree += 1
            else:
                diff += 1
                print(f"  {tid}: replay {est} vs real {real}")
        print(f"validate: {agree} agree, {diff} differ, {missing} not in results dir")
        return 0

    if args.stage == "profiles":
        rows = []
        for name in PROFILES:
            for floor in (48, 54, 60):
                r = score_config(dict(base, profile=name, floor=floor), sides, refs)
                rows.append((r["mean"], name, floor, r))
        rows.sort(reverse=True)
        for mean, name, floor, r in rows:
            print(f"{name:14s} floor={floor}  mean={mean:.4f}  mode={r['mode_acc']:.3f}"
                  f"  major_recall={r['major_recall']}  no_est={r['no_estimate_rate']}")
    elif args.stage == "dynamics":
        rows = []
        for tau in (12.0, 20.0, 30.0):
            for margin in (0.03, 0.05, 0.08):
                for st in (3.0, 5.0):
                    r = score_config(dict(base, tau=tau, margin=margin, switch_time=st),
                                     sides, refs)
                    rows.append((r["mean"], tau, margin, st, r))
        rows.sort(reverse=True)
        for mean, tau, margin, st, r in rows:
            print(f"tau={tau:4.0f} margin={margin:.2f} switch={st:.0f}  mean={mean:.4f}"
                  f"  mode={r['mode_acc']:.3f}  major_recall={r['major_recall']}")
    elif args.stage == "frontend":
        rows = []
        for beta in (0, 1):
            for comp in ("linear", "sqrt"):
                r = score_config(dict(base, beta=beta, comp=comp), sides, refs)
                rows.append((r["mean"], beta, comp, r))
        rows.sort(reverse=True)
        for mean, beta, comp, r in rows:
            print(f"beta={beta} comp={comp:6s}  mean={mean:.4f}  mode={r['mode_acc']:.3f}"
                  f"  major_recall={r['major_recall']}")
    elif args.stage == "bass":
        for w in (0.0, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0):
            r = score_config(dict(base, w_bass=w), sides, refs)
            print(f"w_bass={w:4.2f}  mean={r['mean']:.4f}  mode={r['mode_acc']:.3f}"
                  f"  major_recall={r['major_recall']}  taxonomy={r['taxonomy']}"
                  f"  Δ={r['tonic_delta_hist']}")
    elif args.stage == "mode":
        for mu in (0.0, 0.02, 0.05, 0.1, 0.15, 0.25):
            r = score_config(dict(base, mode_mu=mu), sides, refs)
            print(f"mode_mu={mu:4.2f}  mean={r['mean']:.4f}  mode={r['mode_acc']:.3f}"
                  f"  major_recall={r['major_recall']}  taxonomy={r['taxonomy']}")
    else:
        r = score_config(base, sides, refs)
        print(f"single config {base}: {r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
