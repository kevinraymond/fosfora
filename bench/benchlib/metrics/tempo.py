"""Tempo Acc1/Acc2 + lock time from the causal /bpm trajectory.

Track-level estimate: median of /bpm over the final 50% of the track (values
<= 0 excluded) — the median kills octave blips, the final-half skips warm-up
on 30 s previews and 6-minute tracks alike without a genre-tuned constant.

Acc1/Acc2 are hand-rolled (~10 lines): mir_eval.tempo scores the MIREX
two-tempo + salience task, which is not this task. Acc1 = within 4% of the
reference; Acc2 = within 4% of reference x {1/3, 1/2, 1, 2, 3}.

Lock time: the earliest sample after which *every* later sample stays within
tolerance — "stays locked forever after" can't be gamed by a momentary touch.
"""

from __future__ import annotations

import numpy as np

from .. import dump as dump_mod
from . import CONVENTIONS, register

_TOL = CONVENTIONS["tempo"]["acc1_tolerance"]
_FACTORS = CONVENTIONS["tempo"]["acc2_factors"]


def acc1(est: float, ref: float, tol: float = _TOL) -> bool:
    return ref > 0 and abs(est - ref) / ref <= tol


def acc2(est: float, ref: float, tol: float = _TOL) -> bool:
    return any(acc1(est, ref * k, tol) for k in _FACTORS)


def _lock_time(ts: np.ndarray, ok: np.ndarray) -> float | None:
    """ts of the first sample of the trailing all-ok run; None if the last
    sample is not ok (never locked)."""
    if len(ok) == 0 or not ok[-1]:
        return None
    bad = np.flatnonzero(~ok)
    start = 0 if len(bad) == 0 else bad[-1] + 1
    return float(ts[start])


@register("tempo", ("tempo_bpm",))
def tempo(dump, ann) -> dict:
    ts, bpm = dump.series(dump_mod.BPM)
    valid = bpm > 0
    ts, bpm = ts[valid], bpm[valid]
    if len(bpm) == 0:
        return {"no_estimate": True, "ref_bpm": ann.tempo_bpm, "n_samples": 0}

    half = ts >= ts[-1] / 2.0
    est = float(np.median(bpm[half] if half.any() else bpm))
    ref = float(ann.tempo_bpm)

    ok1 = np.array([acc1(b, ref) for b in bpm])
    ok2 = np.array([acc2(b, ref) for b in bpm])
    return {
        "bpm_estimate": est,
        "ref_bpm": ref,
        "acc1": acc1(est, ref),
        "acc2": acc2(est, ref),
        "lock_time_secs": _lock_time(ts, ok1),
        "lock_time_acc2_secs": _lock_time(ts, ok2),
        "locked_fraction": float(ok1.mean()),
        "n_samples": int(len(bpm)),
    }
