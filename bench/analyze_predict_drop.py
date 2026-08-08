#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Offline calibration study for /predict/drop over cached bench dumps.

The C12 Harmonix run measured coverage 0.0 at both documented thresholds:
the current formula's analytic ceiling (~0.54 with real build levels and
phrase confidence ~0) sits below 0.5. Every recalibrated constant must earn
its value from data — this script is where that happens, against the 374
cached dumps, without re-running any audio.

Subcommands (each writes JSON under bench/out/analysis/predict_drop/):

    fidelity       Gate A — replay the CURRENT formula from dumped inputs and
                   compare to the dumped /predict/drop. Nothing downstream is
                   trusted until p95 of per-track max |sim - dumped| < 0.03.
    distributions  Measurement B — build_ema percentiles inside pre-drop
                   windows vs elsewhere vs the 327 negatives, per candidate τ.
    phase          Measurement C — where in the phrase do proxy drops land
                   (is the boundary term carrying signal?).
    confidence     Measurement D — /phrase/len confidence reality on all 374,
                   plus an offline replay of the evidence histogram with
                   candidate const changes and a loudness-step event source.
    sweep          Measurement E — grid over the preserved formula shape,
                   scored with the exact benchlib crossing semantics,
                   tune/holdout split by sha256(track_id) % 2.
    report         Freeze the chosen config + targets (pre-registration) and
                   emit the 47-track `run_bench.py --only` command line.
    selftest       Verify the vectorized crossing detector against
                   benchlib.metrics.drops.sustained_crossings.

Dumps are read-only; the .npz extraction cache is keyed on the dump filename,
which already encodes sha256(audio)+sha256(binary)+flags.
"""

from __future__ import annotations

import argparse
import hashlib
import itertools
import json
import sys
from pathlib import Path
from types import SimpleNamespace

import numpy as np

sys.path.insert(0, str(Path(__file__).parent))

from benchlib import dump as dump_mod
from benchlib.annotations import Annotations, load_index
from benchlib.dump import SignalDump
from benchlib.metrics import CONVENTIONS
from benchlib.metrics.drops import (
    bar_duration_near,
    beat_duration_near,
    sustained_crossings,
)

BENCH = Path(__file__).parent
OUT = BENCH / "out" / "analysis" / "predict_drop"
CACHE = OUT / "cache"

PHRASE_BAR = f"{dump_mod.PREFIX}/phrase/bar"
PHRASE_BEATS_LEFT = f"{dump_mod.PREFIX}/phrase/beats_left"

_PRE_BARS = CONVENTIONS["predict_drop"]["pre_window_bars"]
_REARM = CONVENTIONS["predict_drop"]["rearm_hysteresis"]
_SUSTAIN_S = CONVENTIONS["predict_drop"]["sustain_window_s"]
_SUSTAIN_N = CONVENTIONS["predict_drop"]["sustain_min_samples"]

# The shipped formula (phrase.rs:181-198) as a config — the fidelity baseline
# and the sweep's origin point.
CURRENT = {"lo": 0.30, "hi": 0.90, "base": 0.35, "p": 1.5, "c0": 0.5, "tau": 0.5, "slope_k": 0.0}

# Warm-up exclusion: the engine's own cold start (tempo lock, first phrase
# announce ~2 s) dominates the first seconds and is not what we calibrate.
_SKIP_S = 5.0


# ---------------------------------------------------------------------------
# Extraction (dump + annotations -> cached per-track arrays)


def _sample_hold(ev_ts: np.ndarray, ev_vals: np.ndarray, grid: np.ndarray, default: float) -> np.ndarray:
    """Value of the most recent event at or before each grid instant."""
    if len(ev_ts) == 0:
        return np.full(len(grid), default)
    idx = np.searchsorted(ev_ts, grid, side="right") - 1
    out = np.where(idx >= 0, ev_vals[np.clip(idx, 0, None)], default)
    return out.astype(np.float64)


def extract(dump_path: Path, ann_path: Path) -> dict[str, np.ndarray]:
    """Per-track arrays for the study; cached as npz keyed on the dump name."""
    cache_file = CACHE / (dump_path.stem + ".npz")
    if cache_file.is_file():
        with np.load(cache_file) as z:
            return {k: z[k] for k in z.files}

    dump = SignalDump.load(dump_path)
    ann = Annotations.load(ann_path)

    ts, predict = dump.series(dump_mod.PREDICT_DROP)
    series = {"predict": predict}
    for name, addr in [
        ("build", dump_mod.BUILD),
        ("energy", dump_mod.ENERGY),
        ("stem_bass", dump_mod.STEM_ENERGY["bass"]),
        ("phrase_bar", PHRASE_BAR),
        ("beats_left", PHRASE_BEATS_LEFT),
        ("bar_phase", dump_mod.BAR_PHASE),
    ]:
        s_ts, s_v = dump.series(addr)
        if len(s_ts) != len(ts) or not np.array_equal(s_ts, ts):
            raise dump_mod.DumpError(f"{dump_path.name}: {addr} grid differs from predict/drop")
        series[name] = s_v

    len_events = dump.events(dump_mod.PHRASE_LEN)  # keep 1 Hz re-broadcasts: conf updates
    len_ts = np.array([t for t, _ in len_events])
    len_vals = np.array([a[0] for _, a in len_events], dtype=np.float64)
    conf_vals = np.array([a[1] for _, a in len_events], dtype=np.float64)

    section_ts = np.array([t for t, _ in dump.changes(dump_mod.SECTION)])

    # Opportunistic raw-feature capture (present only in --feat-bus dumps):
    # the episode-predictor calibration inputs.
    feats: dict[str, np.ndarray] = {}
    for name in ("kick", "sub_bass", "bass", "loudness_m", "loudness_s", "loudness_trend"):
        f_ts, f_v = dump.series(f"{dump_mod.PREFIX}/feat/{name}")
        if len(f_ts):
            if len(f_ts) != len(ts) or not np.array_equal(f_ts, ts):
                raise dump_mod.DumpError(f"{dump_path.name}: feat/{name} grid differs")
            feats[f"feat_{name}"] = f_v

    kick_onset_ts = np.array([t for t, _ in dump.events(dump_mod.PREFIX + "/stem/drums/onset")])

    out: dict[str, np.ndarray] = {
        **feats,
        "kick_onset_ts": kick_onset_ts,
        "ts": ts,
        **series,
        "phrase_len": _sample_hold(len_ts, len_vals, ts, 16.0),
        "phrase_conf": _sample_hold(len_ts, conf_vals, ts, 0.0),
        "drop_ts": dump.drops(),
        "downbeat_ts": dump.downbeats(),
        "section_ts": section_ts,
        "ref_drops": ann.drop_times(),
        "ann_downbeats": ann.downbeats if ann.downbeats is not None else np.array([]),
        "ann_beats": ann.beats if ann.beats is not None else np.array([]),
        "seg_bounds": (
            np.unique([t for s, e, _ in ann.segments for t in (s, e)])
            if ann.segments
            else np.array([])
        ),
        "duration_s": np.array([ann.duration_s if ann.duration_s is not None else (ts[-1] if len(ts) else 0.0)]),
    }
    # Build-EMA replays are config-independent per τ — precompute once.
    for tau in (0.2, 0.35, 0.5):
        out[f"ema{int(tau * 100):03d}"] = replay_build_ema(ts, out["build"], out["drop_ts"], tau)

    CACHE.mkdir(parents=True, exist_ok=True)
    np.savez_compressed(cache_file, **out)
    return out


def replay_build_ema(ts: np.ndarray, build: np.ndarray, drop_ts: np.ndarray, tau: float) -> np.ndarray:
    """The tracker's build EMA (incl. the ×0.3 on-drop collapse), replayed on
    the 30 Hz grid. The Rust EMA runs at hop rate with dt-derived alpha, so a
    dt-derived replay on the decimated grid is first-order equivalent; the
    fidelity gate measures the residual."""
    ema = np.empty_like(build)
    v = 0.0
    prev_t = None
    di = 0
    for i, t in enumerate(ts):
        while di < len(drop_ts) and drop_ts[di] <= t:
            v *= 0.3
            di += 1
        dt = 0.0 if prev_t is None else min(max(t - prev_t, 0.0), 0.25)
        alpha = 1.0 - np.exp(-dt / tau)
        v += (build[i] - v) * alpha
        ema[i] = v
        prev_t = t
    return ema


def _ema_key(tau: float) -> str:
    return f"ema{int(tau * 100):03d}"


def ann_like(track: dict) -> SimpleNamespace:
    """Duck-typed stand-in accepted by benchlib's bar/beat duration helpers."""
    return SimpleNamespace(
        downbeats=track["ann_downbeats"] if len(track["ann_downbeats"]) else None,
        beats=track["ann_beats"] if len(track["ann_beats"]) else None,
    )


# ---------------------------------------------------------------------------
# Track discovery


def find_tracks(dataset: str, flags: str = "r30_fb0_st1", dumps_dir: Path | None = None) -> list[dict]:
    """[{track_id, audio, dump, annotations}] for index tracks with a cached dump."""
    index = load_index(BENCH / "datasets" / dataset / "norm" / "index.json")
    dumps_dir = dumps_dir or BENCH / "out" / "dumps" / dataset
    out = []
    missing = 0
    for t in index:
        matches = sorted(dumps_dir.glob(f"{t['track_id']}.*-{flags}.jsonl"))
        if not matches:
            missing += 1
            continue
        out.append(
            {
                "track_id": t["track_id"],
                "audio": t["audio"],
                "dump": matches[-1],
                "annotations": t["annotations"],
            }
        )
    if missing:
        print(f"[{dataset}] {missing} index tracks have no cached {flags} dump — skipped", file=sys.stderr)
    return out


def load_all(
    dataset: str = "harmonix",
    limit: int | None = None,
    flags: str = "r30_fb0_st1",
    dumps_dir: Path | None = None,
) -> list[dict]:
    tracks = []
    for t in find_tracks(dataset, flags=flags, dumps_dir=dumps_dir)[:limit]:
        data = extract(t["dump"], t["annotations"])
        data["track_id"] = t["track_id"]
        tracks.append(data)
    return tracks


def is_tune(track_id: str) -> bool:
    return int(hashlib.sha256(track_id.encode()).hexdigest(), 16) % 2 == 0


# ---------------------------------------------------------------------------
# Crossing detection (exact benchlib semantics, sparse/vectorized)


def fast_crossings(ts: np.ndarray, v: np.ndarray, theta: float) -> np.ndarray:
    """Sustained crossings, identical to benchlib sustained_crossings.

    Vectorizes the sustain test (window all >= theta, >= 2 samples), then
    walks only candidate/re-arm indices — O(crossings), not O(samples).
    """
    n = len(v)
    if n == 0:
        return np.array([])
    above = v >= theta
    j = np.searchsorted(ts, ts + _SUSTAIN_S, side="right")
    pref = np.concatenate(([0], np.cumsum(~above)))
    sustained = above & ((j - np.arange(n)) >= _SUSTAIN_N) & ((pref[j] - pref[np.arange(n)]) == 0)
    cand = np.flatnonzero(sustained)
    if len(cand) == 0:
        return np.array([])
    rearm_idx = np.flatnonzero(v < theta - _REARM)
    out = []
    pos = 0
    while True:
        ci = np.searchsorted(cand, pos)
        if ci >= len(cand):
            break
        c = cand[ci]
        out.append(ts[c])
        ri = np.searchsorted(rearm_idx, c)
        if ri >= len(rearm_idx):
            break
        pos = rearm_idx[ri] + 1
    return np.array(out)


def score_track(track: dict, ts: np.ndarray, v: np.ndarray, exact: bool = False) -> dict:
    """Per-track coverage/lead/FA under the benchlib metric contract."""
    ann = ann_like(track)
    ref = track["ref_drops"]
    detect = sustained_crossings if exact else fast_crossings
    res: dict = {"leads": {}, "n_ref": int(len(ref))}
    low = detect(ts, v, 0.5)
    for theta, crossings in (("0.5", low), ("0.8", detect(ts, v, 0.8))):
        leads = []
        for d in ref:
            bar = bar_duration_near(ann, d)
            in_win = crossings[(crossings >= d - _PRE_BARS * bar) & (crossings <= d)]
            if len(in_win):
                leads.append((d - float(in_win[0])) / beat_duration_near(ann, d))
        res["leads"][theta] = leads
    false_ts = [
        tc
        for tc in low
        if not np.any((ref > tc) & (ref <= tc + _PRE_BARS * bar_duration_near(ann, tc)))
    ]
    seg = track["seg_bounds"]
    boundary_adjacent = sum(
        1
        for tc in false_ts
        if len(seg) and np.min(np.abs(seg - tc)) <= bar_duration_near(ann, tc)
    )
    res["n_false"] = len(false_ts)
    res["n_false_midsection"] = len(false_ts) - boundary_adjacent
    res["minutes"] = float(track["duration_s"][0]) / 60.0
    return res


def formula(track: dict, cfg: dict) -> np.ndarray:
    """The candidate /predict/drop value on this track's 30 Hz grid."""
    ema = track[_ema_key(cfg["tau"])]
    bs = np.clip((ema - cfg["lo"]) / (cfg["hi"] - cfg["lo"]), 0.0, 1.0)
    if cfg.get("slope_k", 0.0) > 0.0:
        slope = np.clip(np.gradient(ema, track["ts"]), 0.0, None)
        bs = np.clip(bs + cfg["slope_k"] * slope, 0.0, 1.0)
    bars_left = np.maximum(track["beats_left"] / 4.0, 0.0)
    prox = np.clip(1.0 - bars_left / track["phrase_len"], 0.0, 1.0)
    boundary = prox ** cfg["p"] * (cfg["c0"] + (1.0 - cfg["c0"]) * track["phrase_conf"])
    return np.clip(bs * (cfg["base"] + (1.0 - cfg["base"]) * boundary), 0.0, 1.0)


# ---------------------------------------------------------------------------
# Subcommands


def _write(name: str, payload: dict) -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    path = OUT / name
    path.write_text(json.dumps(payload, indent=1, sort_keys=True) + "\n")
    print(f"wrote {path.relative_to(BENCH)}")


def cmd_selftest(args) -> int:
    tracks = load_all(limit=args.tracks)
    rng = np.random.default_rng(0)
    configs = [CURRENT] + [
        {
            "lo": rng.choice([0.15, 0.25]),
            "hi": rng.choice([0.5, 0.65]),
            "base": rng.choice([0.35, 0.55]),
            "p": rng.choice([1.0, 1.5]),
            "c0": rng.choice([0.7, 1.0]),
            "tau": rng.choice([0.2, 0.35]),
        }
        for _ in range(3)
    ]
    checked = 0
    for track in tracks:
        for cfg in configs:
            v = formula(track, cfg)
            for theta in (0.5, 0.8):
                a = fast_crossings(track["ts"], v, theta)
                b = sustained_crossings(track["ts"], v, theta)
                if not np.array_equal(a, b):
                    print(f"MISMATCH {track['track_id']} θ={theta} cfg={cfg}: {a} vs {b}")
                    return 1
                checked += 1
    print(f"selftest OK: {checked} crossing sets identical to benchlib")
    return 0


def cmd_fidelity(args) -> int:
    tracks = load_all()
    errs = []
    for track in tracks:
        keep = track["ts"] >= _SKIP_S
        sim = formula(track, CURRENT)
        err = np.abs(sim - track["predict"])[keep]
        errs.append(
            {
                "track_id": track["track_id"],
                "max_abs": float(err.max()) if len(err) else 0.0,
                "median_abs": float(np.median(err)) if len(err) else 0.0,
            }
        )
    max_abs = np.array([e["max_abs"] for e in errs])
    payload = {
        "gate": "p95(per-track max |sim - dumped|) < 0.03, first 5 s excluded",
        "p95_max_abs": float(np.percentile(max_abs, 95)),
        "median_max_abs": float(np.median(max_abs)),
        "worst": sorted(errs, key=lambda e: -e["max_abs"])[:10],
        "n_tracks": len(errs),
        "passed": bool(np.percentile(max_abs, 95) < 0.03),
    }
    _write("fidelity.json", payload)
    print(f"Gate A: p95 max err {payload['p95_max_abs']:.4f} -> {'PASS' if payload['passed'] else 'FAIL'}")
    return 0 if payload["passed"] else 1


def _percentiles(a: np.ndarray) -> dict:
    if len(a) == 0:
        return {"n": 0}
    return {
        "n": int(len(a)),
        **{f"p{p}": float(np.percentile(a, p)) for p in (10, 25, 50, 75, 90, 95)},
    }


def cmd_distributions(args) -> int:
    tracks = load_all()
    payload: dict = {}
    for tau in (0.2, 0.35, 0.5):
        key = _ema_key(tau)
        pre_max, pos_elsewhere, neg_pool, neg_track_max = [], [], [], []
        for track in tracks:
            ema = track[key]
            ts = track["ts"]
            keep = ts >= _SKIP_S
            ref = track["ref_drops"]
            if len(ref):
                ann = ann_like(track)
                in_any = np.zeros(len(ts), dtype=bool)
                for d in ref:
                    bar = bar_duration_near(ann, d)
                    win = (ts >= d - _PRE_BARS * bar) & (ts <= d)
                    in_any |= win
                    if win.any():
                        pre_max.append(float(ema[win].max()))
                pos_elsewhere.extend(ema[keep & ~in_any])
            else:
                neg_pool.extend(ema[keep])
                neg_track_max.append(float(ema[keep].max()) if keep.any() else 0.0)
        payload[f"tau_{tau}"] = {
            "pre_drop_window_event_max": _percentiles(np.array(pre_max)),
            "positives_elsewhere": _percentiles(np.array(pos_elsewhere)),
            "negatives_pooled": _percentiles(np.array(neg_pool)),
            "negatives_per_track_max": _percentiles(np.array(neg_track_max)),
        }
    raw_pre = []
    for track in tracks:
        ref = track["ref_drops"]
        if not len(ref):
            continue
        ann = ann_like(track)
        for d in ref:
            bar = bar_duration_near(ann, d)
            win = (track["ts"] >= d - _PRE_BARS * bar) & (track["ts"] <= d)
            if win.any():
                raw_pre.append(float(track["build"][win].max()))
    payload["raw_build_pre_drop_event_max"] = _percentiles(np.array(raw_pre))
    _write("distributions.json", payload)
    return 0


def cmd_phase(args) -> int:
    tracks = load_all()
    bars_left_at_drop, frac_at_drop, len_at_drop = [], [], []
    for track in tracks:
        for d in track["ref_drops"]:
            i = np.searchsorted(track["ts"], d, side="right") - 1
            if i < 0:
                continue
            bl = track["beats_left"][i] / 4.0
            ln = track["phrase_len"][i]
            bars_left_at_drop.append(float(bl))
            len_at_drop.append(float(ln))
            frac_at_drop.append(float(1.0 - bl / ln))
    bl = np.array(bars_left_at_drop)
    hist, edges = np.histogram(bl, bins=[0, 1, 2, 3, 4, 6, 8, 12, 16, 33])
    payload = {
        "n_events": int(len(bl)),
        "bars_left_histogram": {f"[{edges[i]:g},{edges[i+1]:g})": int(hist[i]) for i in range(len(hist))},
        "fraction_bars_left_le_2": float(np.mean(bl <= 2.0)) if len(bl) else None,
        "fraction_bars_left_le_4": float(np.mean(bl <= 4.0)) if len(bl) else None,
        "phase_fraction": _percentiles(np.array(frac_at_drop)),
        "len_at_drop_counts": {
            str(int(k)): int(v) for k, v in zip(*np.unique(np.array(len_at_drop), return_counts=True))
        },
        "gate_c": "boundary term carries signal iff fraction_bars_left_le_2 >= 0.40",
    }
    _write("phase.json", payload)
    print(f"Gate C: fraction with bars_left<=2 = {payload['fraction_bars_left_le_2']:.3f}")
    return 0


# --- Measurement D: evidence-histogram replay -------------------------------

LENGTHS = (8, 16, 32)
DECAY_TAU_BARS = 64.0
LENGTH_TIE_FRACTION = 0.8


def _best(scores: list[np.ndarray], floor: float, sat: float) -> float:
    per_len = []
    for lane in scores:
        part = np.partition(lane, -2) if len(lane) >= 2 else lane
        s1 = float(part[-1])
        s2 = float(part[-2]) if len(lane) >= 2 else 0.0
        per_len.append((s1, s2))
    global_best = max(s1 for s1, _ in per_len)
    if global_best < floor:
        return 0.0
    for s1, s2 in reversed(per_len):
        if s1 >= global_best * LENGTH_TIE_FRACTION:
            sharp = max(0.0, min(1.0, (s1 - s2) / (s1 + 1e-9)))
            evid = max(0.0, min(1.0, (s1 - floor) / (sat - floor)))
            return sharp * evid
    return 0.0


def _bar_of(t: float, downbeat_ts: np.ndarray) -> int:
    return int(np.searchsorted(downbeat_ts, t, side="right"))


def _loudness_step_events(track: dict, delta: float) -> list[float]:
    """Per-bar mean loudness step-up vs trailing 8-bar median — candidate
    phrase-evidence events (chorus/drop landings). Returns event bar indices."""
    db = track["downbeat_ts"]
    if len(db) < 3:
        return []
    ts, energy = track["ts"], track["energy"]
    bar_means = []
    for i in range(len(db) - 1):
        win = (ts >= db[i]) & (ts < db[i + 1])
        bar_means.append(float(energy[win].mean()) if win.any() else 0.0)
    events = []
    prev_fired = False
    for i, m in enumerate(bar_means):
        trail = bar_means[max(0, i - 8) : i]
        if len(trail) >= 4 and m >= float(np.median(trail)) + delta and not prev_fired:
            events.append(i + 1)  # the step lands at bar boundary i+1's bar count
            prev_fired = True
        elif len(trail) >= 4 and m < float(np.median(trail)) + delta / 2:
            prev_fired = False
    return events


def _replay_confidence(track: dict, floor: float, sat: float, w_section: float, loudstep_delta: float | None) -> tuple[np.ndarray, list[float]]:
    """conf per bar for one track under candidate consts; also conf at ref drops."""
    db = track["downbeat_ts"]
    n_bars = len(db) + 1
    events: list[tuple[int, float]] = []
    for t in track["drop_ts"]:
        events.append((_bar_of(t, db), 1.0))
    for t in track["section_ts"]:
        events.append((_bar_of(t, db), w_section))
    # Buildup-onset hysteresis (enter 0.6 / re-arm 0.4) on the 30 Hz stream.
    armed = True
    for i, b in enumerate(track["build"]):
        if b < 0.4:
            armed = True
        elif armed and b >= 0.6:
            armed = False
            events.append((_bar_of(track["ts"][i], db), 0.4))
    if loudstep_delta is not None:
        for bar in _loudness_step_events(track, loudstep_delta):
            events.append((bar, 0.5))
    by_bar: dict[int, list[float]] = {}
    for bar, w in events:
        by_bar.setdefault(bar, []).append(w)
    scores = [np.zeros(l) for l in LENGTHS]
    k = np.exp(-1.0 / DECAY_TAU_BARS)
    conf = np.zeros(n_bars)
    for bar in range(n_bars):
        for lane in scores:
            lane *= k
        for w in by_bar.get(bar, []):
            for i, l in enumerate(LENGTHS):
                scores[i][bar % l] += w
        conf[bar] = _best(scores, floor, sat)
    at_drops = [float(conf[min(_bar_of(d, db), n_bars - 1)]) for d in track["ref_drops"]]
    return conf, at_drops


def cmd_confidence(args) -> int:
    tracks = load_all()
    dumped_max = [float(t["phrase_conf"].max()) for t in tracks]
    dumped_mean = [float(t["phrase_conf"][t["ts"] >= _SKIP_S].mean()) for t in tracks]
    payload: dict = {
        "dumped_conf_per_track_max": _percentiles(np.array(dumped_max)),
        "dumped_conf_per_track_mean": _percentiles(np.array(dumped_mean)),
        "replay": {},
        "replay_note": "lower bound: novelty events are not in dumps; loudstep is the candidate new source",
        "gate_d": "touch the confidence path only if median over positives of second-half mean conf >= 0.2",
    }
    positives = [t for t in tracks if len(t["ref_drops"])]
    grid = list(
        itertools.product([0.5, 0.75, 1.0], [1.5, 2.0, 3.0], [0.6, 0.8], [None, 0.08, 0.12])
    )
    for floor, sat, w_sec, step in grid:
        if sat <= floor:
            continue
        means, at_drop_pool = [], []
        for track in positives:
            conf, at_drops = _replay_confidence(track, floor, sat, w_sec, step)
            half = conf[len(conf) // 2 :]
            means.append(float(half.mean()) if len(half) else 0.0)
            at_drop_pool.extend(at_drops)
        key = f"floor{floor}_sat{sat}_wsec{w_sec}_step{step}"
        payload["replay"][key] = {
            "median_secondhalf_mean_conf": float(np.median(means)),
            "conf_at_ref_drops": _percentiles(np.array(at_drop_pool)),
        }
    best = max(payload["replay"].items(), key=lambda kv: kv[1]["median_secondhalf_mean_conf"])
    payload["best_config"] = {"key": best[0], **best[1]}
    payload["current_config_key"] = "floor1.0_sat3.0_wsec0.6_stepNone"
    _write("confidence.json", payload)
    print(f"Gate D best: {best[0]} median conf {best[1]['median_secondhalf_mean_conf']:.3f}")
    return 0


# --- Measurement E: the sweep -----------------------------------------------


def _grid(with_slope: bool) -> list[dict]:
    if with_slope:
        # Earn-back study: the no-slope sweep's fa<=1.0 frontier neighborhood
        # (sweep.json), crossed with slope gains. Same config count, so the
        # escalation criterion gets a like-for-like answer.
        los = [0.15, 0.20, 0.25, 0.30]
        his = [0.45, 0.50, 0.55, 0.60, 0.65]
        bases = [0.35, 0.45]
        ps = [1.0, 1.25, 1.5]
        c0s = [0.5, 0.7, 1.0]
        taus = [0.2, 0.35, 0.5]
        slopes = [0.25, 0.5, 1.0, 2.0]
    else:
        los = [0.15, 0.20, 0.25, 0.30, 0.35]
        his = [0.45, 0.50, 0.55, 0.60, 0.65, 0.70, 0.75]
        bases = [0.35, 0.45, 0.55]
        ps = [0.75, 1.0, 1.25, 1.5]
        c0s = [0.5, 0.7, 0.85, 1.0]
        taus = [0.2, 0.35, 0.5]
        slopes = [0.0]
    return [
        {"lo": lo, "hi": hi, "base": b, "p": p, "c0": c0, "tau": tau, "slope_k": k}
        for lo, hi, b, p, c0, tau, k in itertools.product(los, his, bases, ps, c0s, taus, slopes)
        if hi - lo > 0.15
    ]


def _agg_config(per_track: list[dict], tune_only: set[str] | None) -> dict:
    rows = [r for r in per_track if tune_only is None or r["track_id"] in tune_only]
    n_ref = sum(r["n_ref"] for r in rows)
    minutes = sum(r["minutes"] for r in rows)
    out: dict = {}
    for theta in ("0.5", "0.8"):
        leads = [x for r in rows for x in r["leads"][theta]]
        out[f"cov{theta}"] = len(leads) / n_ref if n_ref else None
        out[f"lead{theta}"] = float(np.median(leads)) if leads else None
    n_false = sum(r["n_false"] for r in rows)
    out["fa_per_min"] = n_false / minutes if minutes else None
    out["fa_midsection_share"] = (
        sum(r["n_false_midsection"] for r in rows) / n_false if n_false else 0.0
    )
    neg = [r for r in rows if r["n_ref"] == 0]
    neg_minutes = sum(r["minutes"] for r in neg)
    out["fa_neg_per_min"] = (
        sum(r["n_false"] for r in neg) / neg_minutes if neg_minutes else None
    )
    out["n_ref"] = n_ref
    return out


def cmd_sweep(args) -> int:
    if cmd_selftest(SimpleNamespace(tracks=6)) != 0:
        print("selftest failed — sweep aborted", file=sys.stderr)
        return 1
    tracks = load_all()
    tune_ids = {t["track_id"] for t in tracks if is_tune(t["track_id"])}
    configs = _grid(args.slope)
    print(f"sweep: {len(configs)} configs × {len(tracks)} tracks (tune half: {len(tune_ids)} tracks)")

    # Outer loop over tracks so per-track precomputation (bs per lo/hi/τ,
    # boundary per p/c0) is shared across all configs.
    results: list[list[dict]] = [[] for _ in configs]
    for ti, track in enumerate(tracks):
        ts = track["ts"]
        bars_left = np.maximum(track["beats_left"] / 4.0, 0.0)
        prox = np.clip(1.0 - bars_left / track["phrase_len"], 0.0, 1.0)
        boundary_cache = {
            (p, c0): prox**p * (c0 + (1.0 - c0) * track["phrase_conf"])
            for p in (0.75, 1.0, 1.25, 1.5)
            for c0 in (0.5, 0.7, 0.85, 1.0)
        }
        bs_cache: dict[tuple, np.ndarray] = {}
        for cfg in {(c["lo"], c["hi"], c["tau"], c["slope_k"]) for c in configs}:
            lo, hi, tau, k = cfg
            ema = track[_ema_key(tau)]
            bs = np.clip((ema - lo) / (hi - lo), 0.0, 1.0)
            if k > 0.0:
                slope = np.clip(np.gradient(ema, ts), 0.0, None)
                bs = np.clip(bs + k * slope, 0.0, 1.0)
            bs_cache[cfg] = bs
        for ci, cfg in enumerate(configs):
            bs = bs_cache[(cfg["lo"], cfg["hi"], cfg["tau"], cfg["slope_k"])]
            boundary = boundary_cache[(cfg["p"], cfg["c0"])]
            v = np.clip(bs * (cfg["base"] + (1.0 - cfg["base"]) * boundary), 0.0, 1.0)
            if v.max() < 0.5:
                results[ci].append(
                    {
                        "track_id": track["track_id"],
                        "leads": {"0.5": [], "0.8": []},
                        "n_ref": int(len(track["ref_drops"])),
                        "n_false": 0,
                        "n_false_midsection": 0,
                        "minutes": float(track["duration_s"][0]) / 60.0,
                    }
                )
                continue
            row = score_track(track, ts, v)
            row["track_id"] = track["track_id"]
            results[ci].append(row)
        if (ti + 1) % 50 == 0:
            print(f"  {ti + 1}/{len(tracks)} tracks")

    scored = []
    for cfg, per_track in zip(configs, results):
        tune = _agg_config(per_track, tune_ids)
        scored.append({"config": cfg, "tune": tune})
    # The full coverage-vs-FA frontier: best tune coverage under each FA cap.
    # This is what Gate E reads when the budget can't be met — it distinguishes
    # "slope study" (0.4-0.6 reachable at budget) from "escalate" (frontier
    # can't reach 0.4 even at 1.0/min).
    frontier = {}
    for cap in (0.1, 0.25, 0.5, 1.0, 2.0, 5.0, float("inf")):
        under = [s for s in scored if (s["tune"]["fa_per_min"] or 0) <= cap]
        if under:
            best = max(under, key=lambda s: s["tune"]["cov0.5"] or 0)
            frontier[str(cap)] = {"config": best["config"], **best["tune"]}
    # Pareto-ish ranking on the tune half under the balanced FA budget.
    budget_met = True
    ok = [
        s
        for s in scored
        if (s["tune"]["fa_per_min"] or 0) <= args.fa_budget
        and s["tune"]["fa_midsection_share"] <= 0.5
        and s["tune"]["cov0.5"] is not None
    ]
    if not ok:
        # Nothing under the budget: re-score the escalation-criterion frontier
        # (fa <= 1.0, no midsection filter) so the decision has exact numbers.
        budget_met = False
        ok = [s for s in scored if (s["tune"]["fa_per_min"] or 0) <= 1.0]
    ok.sort(key=lambda s: (-(s["tune"]["cov0.5"] or 0), s["tune"]["fa_per_min"] or 0))
    top = ok[: args.top]

    # Verbatim re-score of the top configs (exact benchlib crossing loop),
    # now also reporting the holdout half and the mid-build dip rate.
    for s in top:
        per_track = []
        dip_windows = dips = 0
        for track in tracks:
            v = formula(track, s["config"])
            row = score_track(track, track["ts"], v, exact=True)
            row["track_id"] = track["track_id"]
            per_track.append(row)
            ann = ann_like(track)
            for d in track["ref_drops"]:
                bar = bar_duration_near(ann, d)
                win = (track["ts"] >= d - _PRE_BARS * bar) & (track["ts"] <= d)
                if win.sum() >= 2:
                    dip_windows += 1
                    w = v[win]
                    if float(np.max(np.maximum.accumulate(w) - w)) > 0.05:
                        dips += 1
        s["tune_exact"] = _agg_config(per_track, tune_ids)
        s["holdout_exact"] = _agg_config(per_track, {t["track_id"] for t in tracks} - tune_ids)
        s["all_exact"] = _agg_config(per_track, None)
        s["dip_rate"] = dips / dip_windows if dip_windows else None

    current_rows = []
    for track in tracks:
        row = score_track(track, track["ts"], formula(track, CURRENT), exact=True)
        row["track_id"] = track["track_id"]
        current_rows.append(row)
    payload = {
        "n_configs": len(configs),
        "budget_met": budget_met,
        "n_in_top_pool": len(ok),
        "fa_budget": args.fa_budget,
        "frontier": frontier,
        "all_configs": [
            {
                "config": s["config"],
                **{
                    k: (round(v, 4) if isinstance(v, float) else v)
                    for k, v in s["tune"].items()
                },
            }
            for s in scored
        ],
        "slope_axis": bool(args.slope),
        "split": "tune = sha256(track_id) % 2 == 0, over all tracks",
        "current_formula_all": _agg_config(current_rows, None),
        "top": top,
        "gate_e": (
            "recalibrate if cov0.5 >= 0.6 @ fa <= budget (tune, confirmed on holdout); "
            "slope study if 0.4-0.6; escalate below 0.4 even with slope"
        ),
    }
    _write("sweep.json" if not args.slope else "sweep_slope.json", payload)
    if not budget_met:
        print(f"NO config met the {args.fa_budget}/min budget — top pool is the fa<=1.0 escalation frontier")
    if top:
        b = top[0]
        print(
            f"best: cov@0.5 {b['tune_exact']['cov0.5']:.3f} (holdout {b['holdout_exact']['cov0.5']:.3f}) "
            f"lead {b['tune_exact']['lead0.5']} FA/min {b['tune_exact']['fa_per_min']:.3f} "
            f"cfg {b['config']}"
        )
    else:
        print("nothing under fa<=1.0 either — inspect frontier in sweep.json")
    return 0


# --- Phase 1b: episode-gated predictor calibration --------------------------

FEATBUS_DIR = BENCH / "out" / "dumps" / "harmonix_featbus"
FEATBUS_FLAGS = "r30_fb1_st1"
N_NEG_SAMPLE = 100

# Episode-machine constants not swept in stage 1 (structural, or pinned by the
# metric contract: idle must sit below the 0.35 re-arm line or a long loud
# steady section blocks the next crossing).
IDLE_CAP = 0.32
IDLE_LO, IDLE_HI = 0.30, 0.70
EXIT_LEVEL = 0.35
MAX_EPISODE_BARS = 24
KICK_GAP_BARS = 1.25
KICK_MIN_ONSETS = 4
BOUNDARY_CONF_FLOOR = 0.4
DEFAULT_WEIGHTS = {"wp": 0.35, "ww": 0.30, "wb": 0.20, "wi": 0.35}


def _featdump_selection() -> tuple[list[dict], list[str]]:
    """(index rows to dump, positive track ids). 47 positives from both halves
    + a deterministic sample of tune-half negatives."""
    rows = find_tracks("harmonix")
    positives, tune_negatives = [], []
    for t in rows:
        data = extract(t["dump"], t["annotations"])
        if len(data["ref_drops"]):
            positives.append(t)
        elif is_tune(t["track_id"]):
            tune_negatives.append(t)
    tune_negatives.sort(key=lambda t: t["track_id"])
    chosen = positives + tune_negatives[:N_NEG_SAMPLE]
    return chosen, [t["track_id"] for t in positives]


def cmd_featdump(args) -> int:
    from benchlib.runner import DumpRunner

    chosen, _ = _featdump_selection()
    runner = DumpRunner(FEATBUS_DIR, feat_bus=True)
    print(f"feat-bus dumps: {len(chosen)} tracks -> {FEATBUS_DIR.relative_to(BENCH)} (binary {runner.binary_sha256[:16]})")
    results = runner.ensure_dumps([t["audio"] for t in chosen], jobs=args.jobs)
    failed = {a.name: e for a, e in results.items() if isinstance(e, Exception)}
    for name, e in failed.items():
        print(f"  FAILED {name}: {e}", file=sys.stderr)
    print(f"done: {len(results) - len(failed)}/{len(chosen)} dumps ready, {len(failed)} failed")
    return 1 if failed else 0


def _bar_table(track: dict) -> SimpleNamespace | None:
    """Config-independent per-bar aggregates for the episode simulator."""
    db = track["downbeat_ts"]
    ts = track["ts"]
    if len(db) < 10:
        return None
    n = len(db) - 1
    idx = np.searchsorted(ts, db)  # sample index of each bar boundary

    def bar_mean(v: np.ndarray) -> np.ndarray:
        c = np.concatenate(([0.0], np.cumsum(v)))
        lo, hi = idx[:-1], idx[1:]
        width = np.maximum(hi - lo, 1)
        return (c[hi] - c[lo]) / width

    build = bar_mean(track["build"])
    build_max = np.array(
        [
            float(track["build"][idx[i] : max(idx[i + 1], idx[i] + 1)].max())
            for i in range(n)
        ]
    )
    loud = bar_mean(track["feat_loudness_s"])
    trend = bar_mean(track["feat_loudness_trend"])
    bass_src = {
        "sub_bass": bar_mean(track["feat_sub_bass"]),
        "halfband": bar_mean(0.5 * (track["feat_sub_bass"] + track["feat_bass"])),
    }

    def withdrawal(means: np.ndarray) -> np.ndarray:
        e = np.zeros(n)
        for b in range(n):
            trail = means[max(0, b - 8) : b]
            if len(trail) < 4:
                continue
            med = float(np.median(trail))
            loud_trail = loud[max(0, b - 8) : b]
            if loud[b] < 0.35 * float(np.median(loud_trail)):
                continue  # loudness collapsed too: a break, not a pre-drop cut
            e[b] = min(max((med - means[b]) / max(med, 0.05), 0.0), 1.0)
        return e

    # Boundary bonus at bar end, confidence-floored (Gate D: conf stays ~0).
    end_i = np.clip(idx[1:] - 1, 0, len(ts) - 1)
    bars_left = np.maximum(track["beats_left"][end_i] / 4.0, 0.0)
    prox = np.clip(1.0 - bars_left / track["phrase_len"][end_i], 0.0, 1.0)
    e_boundary = prox**1.5 * (
        BOUNDARY_CONF_FLOOR + (1.0 - BOUNDARY_CONF_FLOOR) * track["phrase_conf"][end_i]
    )

    kick_count = np.searchsorted(track["kick_onset_ts"], db)  # onsets before each boundary
    last_kick = np.full(n + 1, -np.inf)
    ko = track["kick_onset_ts"]
    for i in range(n + 1):
        j = np.searchsorted(ko, db[i]) - 1
        if j >= 0:
            last_kick[i] = ko[j]
    ndrops = np.diff(np.searchsorted(track["drop_ts"], db))

    return SimpleNamespace(
        n=n,
        db=db,
        bar_len=np.diff(db),
        build=build,
        build_max=build_max,
        trend=trend,
        e_wd={k: withdrawal(v) for k, v in bass_src.items()},
        e_boundary=e_boundary,
        kick_count=kick_count,
        last_kick=last_kick,
        ndrops=ndrops,
        bar_of_sample=np.searchsorted(db, ts, side="right") - 1,
    )


def sim_episode(bt: SimpleNamespace, cfg: dict) -> np.ndarray:
    """v per bar. Bar b's value becomes visible at the END of bar b (causal);
    the Rust predictor updates per hop and can only be earlier, never later."""
    w = cfg["weights"]
    v = np.zeros(bt.n)
    idle_v = IDLE_CAP * np.clip((bt.build - IDLE_LO) / (IDLE_HI - IDLE_LO), 0.0, 1.0)
    e_wd = bt.e_wd[cfg["wd_src"]]
    committed = False
    acc = 0.0
    ep_v = 0.0
    start = 0
    for b in range(bt.n):
        if bt.ndrops[b] > 0:
            committed, acc, ep_v = False, 0.0, 0.0
            v[b] = 0.15
            continue
        if not committed:
            commit_sig = bt.build_max[b] if cfg.get("commit_stat") == "max" else bt.build[b]
            acc = acc + 1.0 if commit_sig >= cfg["commit_level"] else max(0.0, acc - 2.0)
            # A 9.9 threshold disables that discriminator leg; both disabled
            # means the build-only ablation (gate commits on build alone).
            wd_on, tr_on = cfg["wd_min"] < 9, cfg["trend_min"] < 9
            disc = (
                (wd_on and e_wd[b] >= cfg["wd_min"])
                or (tr_on and bt.trend[b] >= cfg["trend_min"])
                or (not wd_on and not tr_on)
            )
            if acc >= cfg["commit_bars"] and disc:
                committed, start, ep_v = True, b, 0.0
            else:
                v[b] = idle_v[b]
                continue
        bars_in = b - start + 1
        if bt.build[b] < EXIT_LEVEL or bars_in > MAX_EPISODE_BARS:
            committed, acc = False, 0.0
            v[b] = idle_v[b]
            continue
        gap_bars = (bt.db[b + 1] - bt.last_kick[b + 1]) / max(bt.bar_len[b], 0.25)
        kick_gap = (
            bt.kick_count[b + 1] - bt.kick_count[start] >= KICK_MIN_ONSETS
            and gap_bars >= KICK_GAP_BARS
        )
        e_i = 1.0 if (kick_gap or e_wd[b] >= 0.5) else 0.0
        evidence = (
            w["wp"] * min(bars_in / 4.0, 1.0)
            + w["ww"] * e_wd[b]
            + w["wb"] * bt.e_boundary[b]
            + w["wi"] * e_i
        )
        ep_v = max(ep_v, 0.5 + 0.45 * min(1.0, evidence))
        v[b] = ep_v
    return v


def _episode_v30(track: dict, bt: SimpleNamespace, v_bar: np.ndarray) -> np.ndarray:
    """Sample-hold the causal bar values onto the 30 Hz grid: bar b's value
    applies during bar b+1."""
    src = bt.bar_of_sample - 1
    return np.where((src >= 0) & (src < bt.n), v_bar[np.clip(src, 0, bt.n - 1)], 0.0)


def _episode_grid() -> list[dict]:
    out = []
    for cl, cb, wd, tr, src, stat in itertools.product(
        [0.35, 0.40, 0.45, 0.50],
        [1, 2],
        [0.10, 0.20, 0.30, 9.9],
        [0.10, 0.20, 9.9],
        ["sub_bass", "halfband"],
        ["mean", "max"],
    ):
        if wd > 9 and tr > 9 and src != "sub_bass":
            continue  # build-only ablation once per (cl, cb, stat), not per source
        out.append(
            {
                "commit_level": cl,
                "commit_bars": cb,
                "wd_min": wd,
                "trend_min": tr,
                "wd_src": src,
                "commit_stat": stat,
                "weights": dict(DEFAULT_WEIGHTS),
            }
        )
    return out


def _weight_grid() -> list[dict]:
    return [
        {"wp": wp, "ww": ww, "wb": DEFAULT_WEIGHTS["wb"], "wi": wi}
        for wp in (0.25, 0.35, 0.5)
        for ww in (0.2, 0.3, 0.45)
        for wi in (0.25, 0.35, 0.5)
    ]


def _chorus_starts(ann_path: Path) -> np.ndarray:
    """Chorus-block onsets (label starts with 'chorus', previous doesn't) —
    the same rule fetch_harmonix.py uses to derive proxy drops on D&E tracks."""
    ann = Annotations.load(ann_path)
    if not ann.segments:
        return np.array([])
    out = []
    prev = ""
    for s, _e, label in ann.segments:
        if label.startswith("chorus") and not prev.startswith("chorus"):
            out.append(s)
        prev = label
    return np.array(out)


def cmd_episode(args) -> int:
    tracks = load_all(flags=FEATBUS_FLAGS, dumps_dir=FEATBUS_DIR)
    if not tracks:
        print("no feat-bus dumps — run `featdump` first", file=sys.stderr)
        return 1
    chorus = {
        t["track_id"]: _chorus_starts(t["annotations"])
        for t in find_tracks("harmonix", flags=FEATBUS_FLAGS, dumps_dir=FEATBUS_DIR)
    }
    tables = []
    for t in tracks:
        if "feat_sub_bass" not in t:
            print(f"  {t['track_id']}: no feat bus in dump?", file=sys.stderr)
            continue
        bt = _bar_table(t)
        if bt is not None:
            tables.append((t, bt))
    tune_ids = {t["track_id"] for t, _ in tables if is_tune(t["track_id"])}
    n_pos = sum(1 for t, _ in tables if len(t["ref_drops"]))
    print(f"episode calibration: {len(tables)} tracks ({n_pos} positives), tune {len(tune_ids)}")

    def run_configs(configs: list[dict]) -> list[dict]:
        scored = []
        for cfg in configs:
            per_track = []
            for t, bt in tables:
                v30 = _episode_v30(t, bt, sim_episode(bt, cfg))
                row = score_track(t, t["ts"], v30)
                row["track_id"] = t["track_id"]
                per_track.append(row)
            scored.append(
                {
                    "config": cfg,
                    "tune": _agg_config(per_track, tune_ids),
                    "holdout": _agg_config(
                        per_track, {t["track_id"] for t, _ in tables} - tune_ids
                    ),
                }
            )
        return scored

    stage1 = run_configs(_episode_grid())
    stage1.sort(key=lambda s: (-(s["tune"]["cov0.5"] or 0), s["tune"]["fa_per_min"] or 0))
    top_gates = [
        s
        for s in stage1
        if (s["tune"]["fa_per_min"] or 0) <= args.fa_budget
    ][: args.gates] or stage1[: args.gates]
    print(f"stage 1: {len(stage1)} gate configs; refining weights on top {len(top_gates)}")

    stage2 = run_configs(
        [
            {**g["config"], "weights": w}
            for g in top_gates
            for w in _weight_grid()
        ]
    )
    everything = stage1 + stage2
    everything.sort(key=lambda s: (-(s["tune"]["cov0.5"] or 0), s["tune"]["fa_per_min"] or 0))
    under = [
        s
        for s in everything
        if (s["tune"]["fa_per_min"] or 0) <= args.fa_budget
        and s["tune"]["fa_midsection_share"] <= 0.5
    ]
    # The honesty measurement: on NEGATIVE tracks, how many "false alarms" land
    # at that track's own chorus onsets — the exact acoustic event the proxy
    # truth calls a drop when the genre is Dance/Electronic?
    def chorus_analysis(cfg: dict) -> dict:
        out: dict = {}
        for theta in (0.5, 0.8):
            n_fa = n_fa_neg = n_near_chorus = 0
            min_pos = min_neg = 0.0
            for t, bt in tables:
                v30 = _episode_v30(t, bt, sim_episode(bt, cfg))
                ann = ann_like(t)
                ref = t["ref_drops"]
                if len(ref):
                    min_pos += float(t["duration_s"][0]) / 60.0
                else:
                    min_neg += float(t["duration_s"][0]) / 60.0
                for tc in fast_crossings(t["ts"], v30, theta):
                    bar = bar_duration_near(ann, tc)
                    if np.any((ref > tc) & (ref <= tc + _PRE_BARS * bar)):
                        continue
                    n_fa += 1
                    if len(ref) == 0:
                        n_fa_neg += 1
                        ch = chorus.get(t["track_id"], np.array([]))
                        # a crossing is a *prediction*: credit it if a chorus
                        # lands in the same 8-bar forward window (or 1 bar behind).
                        if len(ch) and np.any((ch > tc - bar) & (ch <= tc + _PRE_BARS * bar)):
                            n_near_chorus += 1
            out[str(theta)] = {
                "n_fa": n_fa,
                "n_fa_on_negatives": n_fa_neg,
                "n_fa_predicting_a_chorus": n_near_chorus,
                "chorus_share_of_negative_fas": n_near_chorus / n_fa_neg if n_fa_neg else None,
                "fa_per_min_on_positives": (n_fa - n_fa_neg) / min_pos if min_pos else None,
                "fa_per_min_on_negatives": n_fa_neg / min_neg if min_neg else None,
            }
        return out

    for s in (under[:3] or []) + everything[:3]:
        s["chorus_fa"] = chorus_analysis(s["config"])

    payload = {
        "n_tracks": len(tables),
        "n_positives": n_pos,
        "fa_basis": f"tune positives + {N_NEG_SAMPLE}-track tune-negative sample (not all 327)",
        "fa_budget": args.fa_budget,
        "n_under_budget": len(under),
        "best_under_budget": under[:20],
        "best_overall": everything[:20],
        "fixed": {
            "idle_cap": IDLE_CAP,
            "exit_level": EXIT_LEVEL,
            "max_episode_bars": MAX_EPISODE_BARS,
            "kick_gap_bars": KICK_GAP_BARS,
            "kick_min_onsets": KICK_MIN_ONSETS,
            "boundary_conf_floor": BOUNDARY_CONF_FLOOR,
        },
    }
    _write("episode.json", payload)
    show = under[0] if under else everything[0]
    t, h = show["tune"], show["holdout"]
    print(
        f"best{' (under budget)' if under else ' (NOTHING under budget)'}: "
        f"cov@0.5 {t['cov0.5']:.3f}/hold {h['cov0.5']:.3f} cov@0.8 {t['cov0.8']:.3f} "
        f"lead {t['lead0.5'] and round(t['lead0.5'], 1)} fa {t['fa_per_min']:.3f} mid {t['fa_midsection_share']:.2f}"
    )
    print(f"config: {show['config']}")
    return 0


def cmd_report(args) -> int:
    sweep_file = OUT / "sweep.json"
    if not sweep_file.is_file():
        print("run `sweep` first", file=sys.stderr)
        return 1
    sweep = json.loads(sweep_file.read_text())
    if not sweep["top"]:
        print("sweep found no config meeting the budget — nothing to freeze", file=sys.stderr)
        return 1
    chosen = sweep["top"][args.rank]
    tracks = load_all()
    positives = sorted(t["track_id"] for t in tracks if len(t["ref_drops"]))

    extra_fa = {}
    for ds in args.negatives:
        try:
            ds_tracks = load_all(ds)
        except FileNotFoundError as e:
            print(f"[{ds}] skipped: {e}", file=sys.stderr)
            continue
        crossings = 0
        minutes = 0.0
        for track in ds_tracks:
            v = formula(track, chosen["config"])
            crossings += len(fast_crossings(track["ts"], v, 0.5))
            minutes += float(track["duration_s"][0]) / 60.0
        extra_fa[ds] = {"crossings_per_min": crossings / minutes if minutes else None, "n_tracks": len(ds_tracks)}

    payload = {
        "frozen_config": chosen["config"],
        "sim_numbers": {k: chosen[k] for k in ("tune_exact", "holdout_exact", "all_exact", "dip_rate")},
        "targets": {
            "real_full_run_cov0.5_pooled": ">= 0.5",
            "cov0.8": ">= 0.2 with median lead >= 2 beats",
            "median_lead0.5_beats": "[4, 24]",
            "fa_per_min_pooled": "<= 0.5",
            "fa_midsection_share": "<= 0.5",
            "fixture": "n_false_alarms == 0",
            "real_vs_sim_gate": "47-track run cov@0.5 within 0.1 of sim all_exact",
        },
        "extra_negative_datasets_fa": extra_fa,
        "positive_track_ids": positives,
        "iteration_cmd": (
            "bench/run_bench.py bench/datasets/harmonix --jobs 4 "
            "--out-root bench/out/predrop-iter "
            + " ".join(f"--only {t}" for t in positives)
        ),
    }
    _write("frozen.json", payload)
    print(f"frozen config: {chosen['config']}")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("fidelity")
    sub.add_parser("distributions")
    sub.add_parser("phase")
    sub.add_parser("confidence")
    p = sub.add_parser("selftest")
    p.add_argument("--tracks", type=int, default=10)
    p = sub.add_parser("sweep")
    p.add_argument("--slope", action="store_true", help="add the slope_k axis (earn-back study)")
    p.add_argument("--fa-budget", type=float, default=0.5, help="tune-half FA/min ceiling (balanced budget)")
    p.add_argument("--top", type=int, default=50, help="configs to re-score with verbatim benchlib semantics")
    p = sub.add_parser("featdump")
    p.add_argument("--jobs", type=int, default=4)
    p = sub.add_parser("episode")
    p.add_argument("--fa-budget", type=float, default=0.5)
    p.add_argument("--gates", type=int, default=10, help="gate configs that get the weight sweep")
    p = sub.add_parser("report")
    p.add_argument("--rank", type=int, default=0, help="which sweep-top config to freeze")
    p.add_argument(
        "--negatives",
        nargs="*",
        default=["ballroom", "giantsteps_tempo"],
        help="extra datasets for an FA sanity rate under the frozen config",
    )
    args = ap.parse_args()
    return {
        "fidelity": cmd_fidelity,
        "distributions": cmd_distributions,
        "phase": cmd_phase,
        "confidence": cmd_confidence,
        "selftest": cmd_selftest,
        "sweep": cmd_sweep,
        "featdump": cmd_featdump,
        "episode": cmd_episode,
        "report": cmd_report,
    }[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
