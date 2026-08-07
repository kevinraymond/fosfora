"""Drop events + /predict/drop lead time — the product claim, hand-rolled.

No mir_eval function exists for any of this; every rule here is unit-tested
in test_benchlib.py.

Drop events: greedy one-to-one matching by ascending |dt|, a match is valid
within ±1 bar of the annotated drop (bar length measured locally from the
annotation's own downbeats/beats). Zero-drop tracks still run — their
unmatched estimates feed the false-drop rate.

predict/drop: a *sustained* crossing of theta occurs at tc iff v(tc) >= theta,
the detector is armed, and every sample in [tc, tc + 0.25 s] is >= theta with
at least 2 samples in the window (the >=2 floor tolerates decimation stalls);
re-arm only below theta - 0.15. Lead time for an annotated drop d = beats
between the earliest qualifying crossing in [d - 8 bars, d] and d. A 0.5
crossing with no annotated drop within 8 bars forward is a false alarm.
"""

from __future__ import annotations

import numpy as np

from .. import dump as dump_mod
from . import AGGREGATORS, CONVENTIONS, register

_MATCH_BARS = CONVENTIONS["drop"]["match_window_bars"]
_LOCAL_S = CONVENTIONS["drop"]["bar_local_window_s"]
_THRESHOLDS = CONVENTIONS["predict_drop"]["thresholds"]
_SUSTAIN_S = CONVENTIONS["predict_drop"]["sustain_window_s"]
_SUSTAIN_N = CONVENTIONS["predict_drop"]["sustain_min_samples"]
_REARM = CONVENTIONS["predict_drop"]["rearm_hysteresis"]
_PRE_BARS = CONVENTIONS["predict_drop"]["pre_window_bars"]

# The engine's own off-grid fallback is 2 s/bar (signal/clock.rs) — use the
# same when a track has no usable beat annotations.
_FALLBACK_BAR_S = 2.0


def _local_median_interval(times: np.ndarray | None, t: float) -> float | None:
    if times is None or len(times) < 2:
        return None
    local = times[(times >= t - _LOCAL_S) & (times <= t + _LOCAL_S)]
    base = local if len(local) >= 2 else times
    return float(np.median(np.diff(base)))


def bar_duration_near(ann, t: float) -> float:
    d = _local_median_interval(ann.downbeats, t)
    if d is not None:
        return d
    b = _local_median_interval(ann.beats, t)
    if b is not None:
        return 4.0 * b
    return _FALLBACK_BAR_S


def beat_duration_near(ann, t: float) -> float:
    b = _local_median_interval(ann.beats, t)
    if b is not None:
        return b
    return bar_duration_near(ann, t) / 4.0


def _greedy_match(est: np.ndarray, ref: np.ndarray, windows: np.ndarray):
    """One-to-one pairs (est_idx, ref_idx), closest |dt| first, |dt| <= window."""
    cands = sorted(
        (abs(e - r), ei, ri)
        for ei, e in enumerate(est)
        for ri, r in enumerate(ref)
        if abs(e - r) <= windows[ri]
    )
    used_e: set[int] = set()
    used_r: set[int] = set()
    pairs = []
    for _, ei, ri in cands:
        if ei not in used_e and ri not in used_r:
            pairs.append((ei, ri))
            used_e.add(ei)
            used_r.add(ri)
    return pairs


def _track_duration(dump, ann) -> float:
    if ann.duration_s is not None:
        return float(ann.duration_s)
    return float(dump.records[-1][0]) if dump.records else 0.0


@register("drop", ("drops",), allow_empty=True)
def drop(dump, ann) -> dict:
    est = dump.drops()
    ref = ann.drop_times()
    windows = np.array([_MATCH_BARS * bar_duration_near(ann, r) for r in ref])
    pairs = _greedy_match(est, ref, windows)
    matched = len(pairs)
    duration_min = _track_duration(dump, ann) / 60.0
    false = len(est) - matched
    return {
        "n_ref": int(len(ref)),
        "n_est": int(len(est)),
        "n_matched": matched,
        "hit_rate": matched / len(ref) if len(ref) else None,
        "precision": matched / len(est) if len(est) else None,
        "n_false": false,
        "false_drops_per_min": false / duration_min if duration_min > 0 else None,
        "duration_min": duration_min,
    }


def sustained_crossings(
    ts: np.ndarray,
    v: np.ndarray,
    theta: float,
    sustain_s: float = _SUSTAIN_S,
    min_samples: int = _SUSTAIN_N,
    rearm: float = _REARM,
) -> np.ndarray:
    """Timestamps of sustained crossings of theta (see module docstring)."""
    out = []
    armed = True
    for i in range(len(v)):
        if armed and v[i] >= theta:
            win = (ts >= ts[i]) & (ts <= ts[i] + sustain_s)
            if win.sum() >= min_samples and np.all(v[win] >= theta):
                out.append(float(ts[i]))
                armed = False
        elif not armed and v[i] < theta - rearm:
            armed = True
    return np.array(out, dtype=np.float64)


@register("predict_drop", ("drops",), allow_empty=True)
def predict_drop(dump, ann) -> dict:
    ts, v = dump.series(dump_mod.PREDICT_DROP)
    ref = ann.drop_times()
    duration_min = _track_duration(dump, ann) / 60.0

    thresholds: dict[str, dict] = {}
    for theta in _THRESHOLDS:
        crossings = sustained_crossings(ts, v, theta)
        leads = []
        for d in ref:
            bar = bar_duration_near(ann, d)
            in_win = crossings[(crossings >= d - _PRE_BARS * bar) & (crossings <= d)]
            if len(in_win):
                leads.append((d - float(in_win[0])) / beat_duration_near(ann, d))
            else:
                leads.append(None)
        hit = [x for x in leads if x is not None]
        thresholds[str(theta)] = {
            "lead_beats": hit,
            "coverage": len(hit) / len(ref) if len(ref) else None,
            "median_lead_beats": float(np.median(hit)) if hit else None,
            "n_crossings": int(len(crossings)),
        }

    # False alarms judged at the 0.5 threshold: a crossing not followed by an
    # annotated drop within its forward window. Crossings inside a drop's
    # pre-window are attributed to that drop, fulfilled or not.
    low = sustained_crossings(ts, v, _THRESHOLDS[0])
    false = 0
    for tc in low:
        bar = bar_duration_near(ann, tc)
        if not np.any((ref > tc) & (ref <= tc + _PRE_BARS * bar)):
            false += 1
    return {
        "thresholds": thresholds,
        "n_false_alarms": false,
        "false_alarms_per_min": false / duration_min if duration_min > 0 else None,
        "duration_min": duration_min,
        "n_ref": int(len(ref)),
    }


def _percentiles(values: list[float]) -> dict:
    arr = np.array(values, dtype=np.float64)
    return {
        "p10": float(np.percentile(arr, 10)),
        "p25": float(np.percentile(arr, 25)),
        "median": float(np.median(arr)),
        "p75": float(np.percentile(arr, 75)),
        "p90": float(np.percentile(arr, 90)),
        "n": int(len(arr)),
    }


def _aggregate_drop(blocks: list[dict]) -> dict:
    n_ref = sum(b["n_ref"] for b in blocks)
    n_est = sum(b["n_est"] for b in blocks)
    matched = sum(b["n_matched"] for b in blocks)
    minutes = sum(b["duration_min"] for b in blocks)
    return {
        # Pooled over events, not averaged over tracks: a 1-drop track must
        # not weigh as much as a 6-drop track.
        "hit_rate": matched / n_ref if n_ref else None,
        "precision": matched / n_est if n_est else None,
        "false_drops_per_min": (
            sum(b["n_false"] for b in blocks) / minutes if minutes > 0 else None
        ),
        "n_ref": n_ref,
        "n_est": n_est,
    }


def _aggregate_predict(blocks: list[dict]) -> dict:
    out: dict = {}
    for theta in map(str, _THRESHOLDS):
        pooled: list[float] = []
        n_ref = 0
        for b in blocks:
            t = b["thresholds"].get(theta, {})
            pooled.extend(t.get("lead_beats") or [])
            n_ref += b["n_ref"]
        out[theta] = {
            "lead_beats": _percentiles(pooled) if pooled else None,
            "coverage": len(pooled) / n_ref if n_ref else None,
        }
    minutes = sum(b["duration_min"] for b in blocks)
    out["false_alarms_per_min"] = (
        sum(b["n_false_alarms"] for b in blocks) / minutes if minutes > 0 else None
    )
    return out


AGGREGATORS["drop"] = _aggregate_drop
AGGREGATORS["predict_drop"] = _aggregate_predict
