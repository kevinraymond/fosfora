#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "mir_eval>=0.7"]
# ///
"""Replay the section-boundary back-end over production sidecars and sweep it (Q4).

    bench/sweep_structure.py bench/datasets/harmonix --validate bench/out/results/harmonix
    bench/sweep_structure.py bench/datasets/harmonix --stage blocks
    bench/sweep_structure.py bench/datasets/harmonix --stage novelty
    bench/sweep_structure.py bench/datasets/harmonix --stage dwell
    bench/sweep_structure.py bench/datasets/harmonix --stage combined

Input is `bench/out/structsweep/<ds>/<id>.jsonl` from bench/dump_structure_sidecar.py —
the production front-end's per-tick 27-dim fingerprint plus the fields the section label
machine reads. The replay ports audio/structure.rs (checkerboard kernel, ring, novelty)
and signal/section.rs (the bar-gated label state machine) so a config's sweep score
predicts its real `run_bench.py` score.

`--validate` proves that: it replays the SHIPPED label machine and diffs per-track
boundary sets against real cached bench results. Run it before trusting any sweep
number — a replay that has drifted from production makes every number below meaningless.

Sidecars exist for the tune half only (dump_structure_sidecar.py default), so sweeps
cannot read holdout tracks by construction.

Scoring matches benchlib CONVENTIONS["structure"]: mir_eval boundary detection at
0.5 s / 3.0 s, trim=True, against the reference segment intervals.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

import mir_eval.segment
import mir_eval.util
import numpy as np

REPO = Path(__file__).resolve().parent.parent

# --- audio/structure.rs constants (defaults = shipped values) ----------------------
TICK_HZ = 10.0
RING_SECONDS = 60.0
KERNEL_SECONDS = 3.0
NOVELTY_MAX_SECONDS = 30.0

# --- fingerprint block layout (structure_sidecar.rs) ------------------------------
BANDS = slice(0, 7)
MFCC = slice(7, 15)
CHROMA = slice(15, 27)

# --- signal/section.rs constants (defaults = shipped values) ----------------------
SHIPPED_DWELL = {
    "build_enter": 0.60,
    "build_exit": 0.40,
    "build_sustain_bars": 2.0,
    "build_min_age_bars": 2.0,
    "failed_build_min_age_bars": 4.0,
    "drop_min_bars": 4.0,
    "drop_max_bars": 8.0,
    "drop_fade_delta": 0.15,
    "drop_fade_bars": 2,
    "intro_loud": 0.35,
    "intro_sustain_bars": 2.0,
    "intro_max_bars": 8.0,
    "break_collapse": 0.20,
    "break_bass_ceiling": 0.15,
    "break_min_prev_age_bars": 4.0,
    "break_recover_within": 0.10,
    "break_timeout_bars": 16.0,
    "silence_loud": 0.03,
    "silence_bars": 4.0,
    "loud_tau_secs": 1.0,
    "bar_ring_len": 8,
}

# --- benchlib CONVENTIONS["structure"] --------------------------------------------
WINDOWS = [0.5, 3.0]


def is_tune(track_id: str) -> bool:
    return int(hashlib.sha256(track_id.encode()).hexdigest(), 16) % 2 == 0


# =================================================================================
# Sidecar loading
# =================================================================================


class Track:
    """One track's sidecar, as arrays on the 10 Hz tick grid."""

    __slots__ = ("tid", "ts", "fp", "loud_pre", "loud_s", "buildup", "drop",
                 "bar_index", "bar_phase", "downbeat", "sub_bass", "bass",
                 "ref_iv", "ref_lab", "duration")

    def __init__(self, tid: str, records: list[dict], ann: dict):
        self.tid = tid
        self.ts = np.array([r["ts"] for r in records], dtype=np.float64)
        self.fp = np.array([r["fp"] for r in records], dtype=np.float64)
        for name in ("loud_pre", "loud_s", "buildup", "drop", "bar_index",
                     "bar_phase", "downbeat", "sub_bass", "bass"):
            setattr(self, name, np.array([r[name] for r in records], dtype=np.float64))
        self.duration = float(ann["audio"]["duration_s"])
        segs = ann["segments"]
        self.ref_iv = np.array([[s, e] for s, e, _ in segs], dtype=np.float64)
        self.ref_lab = [lab for _, _, lab in segs]


def load_tracks(dataset: Path, limit: int | None = None) -> list[Track]:
    sidecar_dir = REPO / "bench" / "out" / "structsweep" / dataset.name
    if not sidecar_dir.exists():
        sys.exit(f"no sidecars at {sidecar_dir} — run bench/dump_structure_sidecar.py first")
    out = []
    for p in sorted(sidecar_dir.glob("*.jsonl")):
        tid = p.stem
        ann_path = dataset / "norm" / f"{tid}.json"
        if not ann_path.exists():
            continue
        ann = json.loads(ann_path.read_text())
        if not ann.get("segments"):
            continue
        records = [json.loads(line) for line in p.read_text().splitlines() if line.strip()]
        if len(records) < 100:
            continue
        out.append(Track(tid, records, ann))
        if limit and len(out) >= limit:
            break
    return out


# =================================================================================
# Fingerprint + Foote novelty (ports audio/structure.rs)
# =================================================================================


def make_kernel(half: int) -> np.ndarray:
    """Gaussian-tapered checkerboard K(i,j) = g(i,j)*sgn(i)*sgn(j), matching
    StructureTracker::new."""
    sigma = half / 2.0
    idx = np.arange(-half, half + 1)
    i = idx[:, None]
    j = idx[None, :]
    g = np.exp(-((i * i + j * j) / (2.0 * sigma * sigma)))
    return g * np.sign(i) * np.sign(j)


def weighted_unit(fp: np.ndarray, w_bands: float, w_mfcc: float, w_chroma: float) -> np.ndarray:
    """Per-block scaling, then unit-norm per frame.

    The shipped code concatenates raw blocks and unit-norms once, which hands MFCC
    ~97% of the energy because its per-dim RMS runs ~15x the others (finding #2204).
    Scaling each block to unit RMS *first* makes the weights mean what they say.
    """
    out = np.empty_like(fp)
    for sl, w in ((BANDS, w_bands), (MFCC, w_mfcc), (CHROMA, w_chroma)):
        block = fp[:, sl]
        rms = np.sqrt((block ** 2).mean()) or 1.0
        out[:, sl] = block * (w / rms)
    norm = np.linalg.norm(out, axis=1, keepdims=True)
    norm[norm < 1e-6] = 1.0
    return out / norm


def novelty_curve(vecs: np.ndarray, half: int) -> np.ndarray:
    """Causal Foote novelty, one value per tick, aligned to the tick it is REPORTED at.

    Matches `foote_novelty`: the kernel is centred `half` ticks behind the newest
    frame so the full symmetric kernel fits inside the ring, i.e. the value emitted
    at tick t describes the boundary at t - half. Ticks before the ring fills read 0.
    Also returns the kernel weight sum, the absolute normalization's denominator.
    """
    n = len(vecs)
    side = 2 * half + 1
    k = make_kernel(half)
    out = np.zeros(n, dtype=np.float64)
    if n < side:
        return out
    # Full similarity matrix is O(n^2) in memory for a 2000-tick track (~32 MB) —
    # fine here, and it lets every window be a plain slice.
    sim = vecs @ vecs.T
    for t in range(side - 1, n):
        c = t - half
        block = sim[c - half:c + half + 1, c - half:c + half + 1]
        out[t] = max(0.0, float((block * k).sum()))
    return out


def kernel_weight(half: int) -> float:
    return float(np.abs(make_kernel(half)).sum())


def normalize_running_max(raw: np.ndarray) -> np.ndarray:
    """The SHIPPED normalization: decaying running max. Saturates to 1.0 whenever the
    signal is stable — kept here only so --validate can reproduce production."""
    decay = np.exp(-(1.0 / TICK_HZ) / NOVELTY_MAX_SECONDS)
    out = np.zeros_like(raw)
    m = 1e-6
    for i, v in enumerate(raw):
        m = max(m * decay, v, 1e-6)
        out[i] = min(1.0, v / m)
    return out


def normalize_absolute(raw: np.ndarray, half: int, gain: float) -> np.ndarray:
    """Absolute normalization: divide by the kernel's own weight, as
    analyze/structure_offline.rs does. Vectors are unit-norm so the block sum is
    bounded; a flat song reads ~0 instead of 1.0."""
    return np.clip(raw / kernel_weight(half) * gain, 0.0, 1.0)


# =================================================================================
# Causal peak-picking -> boundary events
# =================================================================================


def peak_boundaries(nov: np.ndarray, ts: np.ndarray, half: int, sigma: float,
                    min_gap_s: float, stat_window_s: float,
                    confirm_s: float | None = None) -> list[tuple[float, float]]:
    """Causal peaks of the novelty curve -> (time, confidence).

    A tick is a boundary when it is a local max over +/-`confirm` ticks, confirmed
    `confirm` ticks later (so the decision only ever uses the past), clears
    mean + sigma*stddev of a trailing window, and sits at least `min_gap_s` after the
    previous boundary.

    `confirm` is deliberately NOT tied to the kernel half-width. The kernel's lag is
    irreducible — the novelty value at tick t describes the boundary at t - half — but
    how long you wait to be sure that value was a local max is a free parameter. Tying
    them doubles live latency for nothing: total detection lag is
    KERNEL_SECONDS + confirm_s, and a VJ feels every second of it.

    The reported time is corrected by the kernel lag, so scored boundary times are the
    musical times, not the moments of announcement.
    """
    n = len(nov)
    w = int(stat_window_s * TICK_HZ)
    conf_ticks = half if confirm_s is None else max(1, int(round(confirm_s * TICK_HZ)))
    out: list[tuple[float, float]] = []
    last_t = -1e9
    for t in range(half + conf_ticks, n):
        c = t - conf_ticks  # candidate tick, now fully surrounded by observed data
        lo = max(0, c - w)
        window = nov[lo:c + 1]
        if len(window) < 20:
            continue
        mean = window.mean()
        std = window.std()
        if std < 1e-9:
            continue
        v = nov[c]
        if v < mean + sigma * std:
            continue
        if v < nov[max(0, c - conf_ticks):c + conf_ticks + 1].max() - 1e-12:
            continue
        bt = ts[c] - half / TICK_HZ
        if bt - last_t < min_gap_s:
            continue
        last_t = bt
        conf = float(np.clip((v - mean) / (std * 4.0), 0.0, 1.0))
        out.append((float(bt), conf))
    return out


# =================================================================================
# The label state machine (ports signal/section.rs + signal/clock.rs)
# =================================================================================


def label_boundaries(tr: Track, cfg: dict) -> list[float]:
    """Replay HeuristicSectionEstimator and return the times of label CHANGES —
    which is exactly what benchlib's structure metric derives boundaries from
    (dump.changes(SECTION), first boundary clamped to 0)."""
    INTRO, BUILD, DROP, BREAK, STEADY = "intro", "build", "drop", "break", "steady"

    # BarClock
    bars = 0.0
    prev_bar_index = tr.bar_index[0]
    last_advance_ts = tr.ts[0]

    label = INTRO
    entry_bar = 0.0
    entry_loud = 0.0
    loud = 0.0
    cur_bar = 0
    bar_sum = 0.0
    bar_hops = 0
    bar_means: list[float] = []
    faded_bars = 0
    pre_break_median = 0.0
    build_cond_bars = 0.0
    build_armed = True
    build_exit_bars = 0.0
    intro_loud_bars = 0.0
    silence_bars = 0.0
    prev_bars = 0.0
    prev_ts = None

    changes: list[float] = []

    def enter(new_label, bars_now):
        nonlocal label, entry_bar, entry_loud, faded_bars
        nonlocal build_cond_bars, build_exit_bars, intro_loud_bars
        label = new_label
        entry_bar = bars_now
        entry_loud = loud
        faded_bars = 0
        build_cond_bars = 0.0
        build_exit_bars = 0.0
        intro_loud_bars = 0.0

    def trailing_median():
        if len(bar_means) < 4:
            return None
        return sorted(bar_means)[len(bar_means) // 2]

    for i in range(len(tr.ts)):
        ts = tr.ts[i]
        # --- BarClock.advance
        if prev_ts is None:
            prev_ts = ts
            prev_bar_index = tr.bar_index[i]
            last_advance_ts = ts
        else:
            dt_clock = max(0.0, ts - prev_ts)
            prev_ts = ts
            delta = tr.bar_index[i] - prev_bar_index
            if delta > 0.0:
                bars += min(delta, 4.0)
                last_advance_ts = ts
            elif ts - last_advance_ts > 4.0:
                bars += dt_clock / 2.0
            prev_bar_index = tr.bar_index[i]

        dbars = max(0.0, bars - prev_bars)
        prev_bars = bars
        dt = 0.0 if i == 0 else min(0.25, tr.ts[i] - tr.ts[i - 1])

        alpha = 1.0 - np.exp(-dt / cfg["loud_tau_secs"])
        loud += (tr.loud_s[i] - loud) * alpha

        bar_now = int(np.floor(bars))
        finalized = None
        if bar_now != cur_bar and bar_hops > 0:
            mean = bar_sum / bar_hops
            finalized = (mean, trailing_median())
            bar_means.append(mean)
            if len(bar_means) > cfg["bar_ring_len"]:
                bar_means.pop(0)
            bar_sum = 0.0
            bar_hops = 0
        if bar_now != cur_bar:
            cur_bar = bar_now
        bar_sum += tr.loud_s[i]
        bar_hops += 1

        buildup = tr.buildup[i]
        if buildup < cfg["build_exit"]:
            build_armed = True
        if buildup >= cfg["build_enter"] and build_armed:
            build_cond_bars += dbars
        elif buildup < cfg["build_enter"]:
            build_cond_bars = 0.0
        if buildup < cfg["build_exit"]:
            build_exit_bars += dbars
        else:
            build_exit_bars = 0.0
        if loud >= cfg["intro_loud"]:
            intro_loud_bars += dbars
        else:
            intro_loud_bars = 0.0
        if loud < cfg["silence_loud"]:
            silence_bars += dbars
        else:
            silence_bars = 0.0

        if finalized is not None:
            if finalized[0] < entry_loud - cfg["drop_fade_delta"]:
                faded_bars += 1
            else:
                faded_bars = 0

        age = bars - entry_bar
        bass = 0.5 * (tr.sub_bass[i] + tr.bass[i])
        before = label

        if tr.drop[i] > 0.5 and label != DROP:
            enter(DROP, bars)
        elif silence_bars >= cfg["silence_bars"] and label != INTRO:
            enter(INTRO, bars)
        else:
            break_entry = False
            if label in (STEADY, BUILD, DROP) and age >= cfg["break_min_prev_age_bars"] \
                    and finalized is not None and finalized[1] is not None:
                mean, med = finalized
                break_entry = (med - mean >= cfg["break_collapse"]
                               and bass < cfg["break_bass_ceiling"]
                               and loud > cfg["silence_loud"] + 0.02)
            if break_entry:
                pre_break_median = finalized[1]
                enter(BREAK, bars)
            elif label == INTRO:
                if build_cond_bars >= cfg["build_sustain_bars"]:
                    enter(BUILD, bars)
                    build_armed = False
                elif intro_loud_bars >= cfg["intro_sustain_bars"]:
                    enter(STEADY, bars)
                elif age >= cfg["intro_max_bars"] and tr.downbeat[i] > 0.5:
                    enter(STEADY, bars)
            elif label == STEADY:
                if age >= cfg["build_min_age_bars"] and build_cond_bars >= cfg["build_sustain_bars"]:
                    enter(BUILD, bars)
                    build_armed = False
            elif label == BUILD:
                if age >= cfg["failed_build_min_age_bars"] and build_exit_bars >= cfg["build_sustain_bars"]:
                    enter(STEADY, bars)
            elif label == DROP:
                if age >= cfg["drop_max_bars"] or (age >= cfg["drop_min_bars"] and faded_bars >= cfg["drop_fade_bars"]):
                    enter(STEADY, bars)
            elif label == BREAK:
                if build_cond_bars >= cfg["build_sustain_bars"]:
                    enter(BUILD, bars)
                    build_armed = False
                elif finalized is not None and finalized[0] >= pre_break_median - cfg["break_recover_within"]:
                    enter(STEADY, bars)
                elif age >= cfg["break_timeout_bars"]:
                    enter(STEADY, bars)

        if label != before:
            changes.append(float(ts))

    return changes


# =================================================================================
# Scoring (matches benchlib/metrics/structure.py)
# =================================================================================


def score_boundaries(bounds: list[float], tr: Track) -> dict:
    """mir_eval boundary detection at each convention window. `bounds` are interior
    boundary times; the estimate always starts at 0 like the live stream does."""
    starts = [0.0] + sorted(b for b in bounds if 0.0 < b < tr.duration)
    iv, lab = [], []
    for i, s in enumerate(starts):
        e = starts[i + 1] if i + 1 < len(starts) else tr.duration
        if e > s:
            iv.append([s, e])
            lab.append(str(i))
    if not iv:
        iv, lab = [[0.0, tr.duration]], ["0"]
    est_iv, est_lab = mir_eval.util.adjust_intervals(
        np.array(iv, dtype=np.float64), lab, t_min=0.0, t_max=tr.duration)
    ref_iv, ref_lab = mir_eval.util.adjust_intervals(
        tr.ref_iv, list(tr.ref_lab), t_min=0.0, t_max=tr.duration)
    out = {"n_est": len(est_lab), "n_ref": len(ref_lab)}
    for w in WINDOWS:
        p, r, f = mir_eval.segment.detection(ref_iv, est_iv, window=w, trim=True)
        out[f"b{w}"] = (f, p, r)
    return out


def summarize(scores: list[dict]) -> dict:
    out = {}
    for w in WINDOWS:
        k = f"b{w}"
        out[f"boundary_{w}s.f"] = round(float(np.mean([s[k][0] for s in scores])), 4)
        out[f"boundary_{w}s.p"] = round(float(np.mean([s[k][1] for s in scores])), 4)
        out[f"boundary_{w}s.r"] = round(float(np.mean([s[k][2] for s in scores])), 4)
    out["n_est_segments"] = round(float(np.mean([s["n_est"] for s in scores])), 4)
    out["n_ref_segments"] = round(float(np.mean([s["n_ref"] for s in scores])), 4)
    return out


def fmt(row: dict) -> str:
    return (f"b3.0 F {row['boundary_3.0s.f']:.4f} P {row['boundary_3.0s.p']:.4f} "
            f"R {row['boundary_3.0s.r']:.4f} | b0.5 F {row['boundary_0.5s.f']:.4f} "
            f"| n_est {row['n_est_segments']:.2f}")


# =================================================================================
# Stages
# =================================================================================


def stage_validate(tracks: list[Track], results_dir: Path) -> int:
    """Replay the SHIPPED label machine and diff against real bench results.

    The shipped pipeline produces boundaries from label changes ONLY — the novelty
    curve is never turned into events — so this compares the replayed label machine
    against what run_bench.py actually scored.
    """
    print(f"validate: replaying shipped config over {len(tracks)} tracks\n")
    agree = 0
    rows = []
    for tr in tracks:
        real_path = results_dir / f"{tr.tid}.json"
        if not real_path.exists():
            continue
        real = json.loads(real_path.read_text())["metrics"].get("structure")
        if not real or real.get("no_estimate"):
            continue
        bounds = label_boundaries(tr, SHIPPED_DWELL)
        got = score_boundaries(bounds, tr)
        want_n = real["n_est_segments"]
        want_f = real["boundary_3_0s"]["f"]
        # n_est within 1 and F within 0.05 counts as reproducing production.
        ok = abs(got["n_est"] - want_n) <= 1 and abs(got["b3.0"][0] - want_f) <= 0.05
        agree += ok
        rows.append((tr.tid, want_n, got["n_est"], want_f, got["b3.0"][0], ok))
    total = len(rows)
    print(f"{'track':<28} {'n_real':>6} {'n_replay':>8} {'F_real':>7} {'F_replay':>8}  ok")
    for tid, wn, gn, wf, gf, ok in rows[:25]:
        print(f"{tid:<28} {wn:>6} {gn:>8} {wf:>7.4f} {gf:>8.4f}  {'y' if ok else 'N'}")
    if total > 25:
        print(f"... {total - 25} more")
    print(f"\nfidelity: {agree}/{total} tracks reproduce production")
    if total:
        real_f = float(np.mean([r[3] for r in rows]))
        rep_f = float(np.mean([r[4] for r in rows]))
        real_n = float(np.mean([r[1] for r in rows]))
        rep_n = float(np.mean([r[2] for r in rows]))
        print(f"aggregate boundary@3s  real {real_f:.4f}  replay {rep_f:.4f}  "
              f"(delta {rep_f - real_f:+.4f})")
        print(f"aggregate n_est        real {real_n:.4f}  replay {rep_n:.4f}  "
              f"(delta {rep_n - real_n:+.4f})")
    return 0 if agree >= 0.8 * max(total, 1) else 1


def novelty_for(tr: Track, half: int, w_bands: float, w_mfcc: float, w_chroma: float,
                gain: float) -> np.ndarray:
    vecs = weighted_unit(tr.fp, w_bands, w_mfcc, w_chroma)
    raw = novelty_curve(vecs, half)
    return normalize_absolute(raw, half, gain)


def run_config(tracks: list[Track], *, half: int, weights: tuple[float, float, float],
               sigma: float, min_gap_s: float, stat_window_s: float, gain: float,
               with_labels: bool, dwell: dict | None = None,
               confirm_s: float | None = None) -> dict:
    scores = []
    for tr in tracks:
        nov = novelty_for(tr, half, *weights, gain)
        peaks = peak_boundaries(nov, tr.ts, half, sigma, min_gap_s, stat_window_s,
                                confirm_s)
        bounds = [t for t, _ in peaks]
        if with_labels:
            lab = label_boundaries(tr, dwell or SHIPPED_DWELL)
            # Union, deduped within the tighter of the two scoring windows.
            for t in lab:
                if all(abs(t - b) > 0.5 for b in bounds):
                    bounds.append(t)
        scores.append(score_boundaries(sorted(bounds), tr))
    return summarize(scores)


def stage_blocks(tracks: list[Track]) -> int:
    """Which fingerprint blocks carry boundary information, at honest scale."""
    print("stage blocks: per-block weights after RMS equalization (finding #2204)")
    print("(peak-pick held at sigma 1.5, min_gap 8 s, stat window 45 s, gain 1.0)\n")
    half = int(KERNEL_SECONDS * TICK_HZ)
    combos = [
        ("mfcc only (shipped in spirit)", (0.0, 1.0, 0.0)),
        ("bands only", (1.0, 0.0, 0.0)),
        ("chroma only", (0.0, 0.0, 1.0)),
        ("equal thirds", (1.0, 1.0, 1.0)),
        ("timbre-led + chroma", (0.5, 1.0, 0.5)),
        ("mfcc + chroma", (0.0, 1.0, 1.0)),
        ("bands + mfcc", (1.0, 1.0, 0.0)),
        ("chroma-led", (0.5, 0.5, 1.0)),
        ("raw concat (shipped scales)", None),
    ]
    for name, w in combos:
        if w is None:
            # Reproduce the shipped concatenation: no block equalization at all.
            scores = []
            for tr in tracks:
                v = tr.fp.copy()
                n = np.linalg.norm(v, axis=1, keepdims=True)
                n[n < 1e-6] = 1.0
                nov = normalize_absolute(novelty_curve(v / n, half), half, 1.0)
                peaks = peak_boundaries(nov, tr.ts, half, 1.5, 8.0, 45.0)
                scores.append(score_boundaries([t for t, _ in peaks], tr))
            row = summarize(scores)
        else:
            row = run_config(tracks, half=half, weights=w, sigma=1.5, min_gap_s=8.0,
                             stat_window_s=45.0, gain=1.0, with_labels=False)
        print(f"  {name:<30} {fmt(row)}")
    return 0


def stage_novelty(tracks: list[Track], weights: tuple[float, float, float]) -> int:
    """Peak-pick parameters: kernel width, threshold sigma, minimum section length."""
    print(f"stage novelty: peak-pick sweep at weights {weights}\n")
    print("  kernel_s sigma  min_gap  " + "score")
    best = None
    for kernel_s in (2.0, 3.0, 4.0, 6.0):
        half = int(kernel_s * TICK_HZ)
        for sigma in (0.5, 1.0, 1.5, 2.0):
            for min_gap in (4.0, 8.0, 12.0):
                row = run_config(tracks, half=half, weights=weights, sigma=sigma,
                                 min_gap_s=min_gap, stat_window_s=45.0, gain=1.0,
                                 with_labels=False)
                print(f"  {kernel_s:>8.1f} {sigma:>5.1f} {min_gap:>8.1f}  {fmt(row)}")
                key = row["boundary_3.0s.f"]
                if best is None or key > best[0]:
                    best = (key, kernel_s, sigma, min_gap, row)
    print(f"\nbest: kernel {best[1]} s, sigma {best[2]}, min_gap {best[3]} s -> {fmt(best[4])}")
    return 0


def stage_latency(tracks: list[Track], weights: tuple[float, float, float]) -> int:
    """What each second of live detection latency actually buys.

    Total lag = kernel_s (irreducible, the novelty's own centring) + confirm_s (how
    long we wait to call a sample a local max). The benchmark is blind to lag — the
    scored boundary time is corrected — so this table is the only place the product
    cost of a high-F config is visible. Read it before choosing.
    """
    print(f"stage latency: F against total detection lag, weights {weights}")
    print("  (sigma 0.5, min_gap 8 s — the shape that scored best in --stage novelty)\n")
    print(f"  {'kernel_s':>8} {'confirm_s':>9} {'lag_s':>6}  score")
    rows = []
    for kernel_s in (2.0, 3.0, 4.0, 6.0):
        half = int(kernel_s * TICK_HZ)
        for confirm_s in (0.5, 1.0, 1.5, 2.0, 3.0):
            row = run_config(tracks, half=half, weights=weights, sigma=0.5,
                             min_gap_s=8.0, stat_window_s=45.0, gain=1.0,
                             with_labels=False, confirm_s=confirm_s)
            lag = kernel_s + confirm_s
            print(f"  {kernel_s:>8.1f} {confirm_s:>9.1f} {lag:>6.1f}  {fmt(row)}")
            rows.append((lag, row["boundary_3.0s.f"], kernel_s, confirm_s, row))
    rows.sort()
    print("\n  best F at or under each lag budget:")
    for budget in (4.0, 5.0, 6.0, 8.0, 10.0, 12.0):
        under = [r for r in rows if r[0] <= budget]
        if not under:
            continue
        b = max(under, key=lambda r: r[1])
        print(f"    <= {budget:>4.1f} s: kernel {b[2]:.1f} confirm {b[3]:.1f} -> {fmt(b[4])}")
    return 0


def stage_dwell(tracks: list[Track]) -> int:
    """The label machine's bar gates, which were calibrated to the pre-Q1 beat rate."""
    print("stage dwell: label-machine bar gates, boundaries from label changes only\n")
    base = dict(SHIPPED_DWELL)
    row = summarize([score_boundaries(label_boundaries(t, base), t) for t in tracks])
    print(f"  {'shipped':<34} {fmt(row)}")
    sweeps = {
        "build_sustain_bars": [0.5, 1.0, 2.0],
        "build_min_age_bars": [1.0, 2.0, 4.0],
        "drop_max_bars": [4.0, 8.0, 16.0],
        "break_min_prev_age_bars": [2.0, 4.0, 8.0],
        "break_collapse": [0.10, 0.15, 0.20],
        "break_timeout_bars": [8.0, 16.0, 32.0],
        "intro_sustain_bars": [1.0, 2.0, 4.0],
    }
    for key, values in sweeps.items():
        for v in values:
            cfg = dict(base)
            cfg[key] = v
            row = summarize([score_boundaries(label_boundaries(t, cfg), t) for t in tracks])
            print(f"  {key + '=' + str(v):<34} {fmt(row)}")
    return 0


def stage_combined(tracks: list[Track], weights: tuple[float, float, float]) -> int:
    """The operating point: kernel 3 s + confirm 3 s (6 s total lag, chosen in --stage
    latency), sweeping the threshold and dwell, with and without unioning the label
    machine's own transitions."""
    print(f"stage combined: kernel 3.0 s, confirm 3.0 s, weights {weights}\n")
    half = int(3.0 * TICK_HZ)
    best = None
    for sigma in (0.25, 0.5, 1.0, 1.5):
        for min_gap in (6.0, 8.0, 12.0):
            for labels in (False, True):
                row = run_config(tracks, half=half, weights=weights, sigma=sigma,
                                 min_gap_s=min_gap, stat_window_s=45.0, gain=1.0,
                                 with_labels=labels, confirm_s=3.0)
                tag = f"sigma {sigma} gap {min_gap} {'peaks+labels' if labels else 'peaks only'}"
                print(f"  {tag:<36} {fmt(row)}")
                if best is None or row["boundary_3.0s.f"] > best[0]:
                    best = (row["boundary_3.0s.f"], tag, row)
    print(f"\nbest: {best[1]} -> {fmt(best[2])}")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("dataset", type=Path)
    ap.add_argument("--validate", type=Path, metavar="RESULTS_DIR",
                    help="replay the shipped config and diff against real bench results")
    ap.add_argument("--stage",
                    choices=["blocks", "novelty", "latency", "dwell", "combined"])
    ap.add_argument("--limit", type=int, help="use only the first N tracks (quick looks)")
    ap.add_argument("--weights", default="0.5,1.0,0.5",
                    help="bands,mfcc,chroma block weights (post RMS equalization)")
    args = ap.parse_args()

    tracks = load_tracks(args.dataset, args.limit)
    if not tracks:
        sys.exit("no usable sidecars loaded")
    print(f"loaded {len(tracks)} tracks "
          f"({sum(1 for t in tracks if is_tune(t.tid))} tune)\n", flush=True)

    weights = tuple(float(x) for x in args.weights.split(","))

    if args.validate:
        return stage_validate(tracks, args.validate)
    if args.stage == "blocks":
        return stage_blocks(tracks)
    if args.stage == "novelty":
        return stage_novelty(tracks, weights)
    if args.stage == "latency":
        return stage_latency(tracks, weights)
    if args.stage == "dwell":
        return stage_dwell(tracks)
    if args.stage == "combined":
        return stage_combined(tracks, weights)
    ap.error("pass --validate or --stage")


if __name__ == "__main__":
    sys.exit(main())
