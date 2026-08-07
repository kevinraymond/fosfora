"""Structure scoring — boundary F1 (0.5 s / 3.0 s) + pairwise F1.

Both metrics are vocabulary-agnostic: pairwise compares co-segmentation of
frame pairs and boundary detection ignores labels entirely, so fosfora's
EDM-shaped intro|build|drop|break|steady needs no mapping against
verse/chorus-style annotations.

The causal novelty detector carries an irreducible ~3 s lag, so a fixed-shift
lag-compensated variant is included — quarantined under "lag_compensated" and
never the headline — to separate boundary *placement* quality from the delay.
The shift is a constant, never per-track fitted.
"""

from __future__ import annotations

import mir_eval.segment
import mir_eval.util
import numpy as np

from .. import dump as dump_mod
from . import CONVENTIONS, register

_WINDOWS = CONVENTIONS["structure"]["boundary_windows_s"]
_LAG = CONVENTIONS["structure"]["lag_compensation_s"]


def _to_intervals(starts: list[float], labels: list[str], duration: float):
    """Consecutive starts -> (intervals, labels), zero-length segments dropped."""
    intervals, out_labels = [], []
    for i, s in enumerate(starts):
        e = starts[i + 1] if i + 1 < len(starts) else duration
        if e > s:
            intervals.append([s, e])
            out_labels.append(labels[i])
    return np.array(intervals, dtype=np.float64), out_labels


def _adjusted(intervals, labels, duration: float):
    return mir_eval.util.adjust_intervals(
        np.asarray(intervals, dtype=np.float64),
        list(labels),
        t_min=0.0,
        t_max=duration,
    )


def _window_key(w: float) -> str:
    return f"boundary_{str(w).replace('.', '_')}s"


@register("structure", ("segments",))
def structure(dump, ann) -> dict:
    changes = dump.changes(dump_mod.SECTION)
    if not changes:
        return {"no_estimate": True}

    duration = float(ann.duration_s)
    # The estimator's first state has held since t=0 (it *started* as intro at
    # the first hop) — clamp the first boundary to 0 rather than ~11.6 ms.
    starts = [0.0] + [ts for ts, _ in changes[1:]]
    labels = [args[0] for _, args in changes]
    est_iv, est_lab = _to_intervals(starts, labels, duration)
    est_iv, est_lab = _adjusted(est_iv, est_lab, duration)

    ref_iv = np.array([[s, e] for s, e, _ in ann.segments], dtype=np.float64)
    ref_lab = [label for _, _, label in ann.segments]
    ref_iv, ref_lab = _adjusted(ref_iv, ref_lab, duration)

    out: dict = {"n_est_segments": len(est_lab), "n_ref_segments": len(ref_lab)}
    for w in _WINDOWS:
        p, r, f = mir_eval.segment.detection(ref_iv, est_iv, window=w, trim=True)
        out[_window_key(w)] = {"f": f, "p": p, "r": r}

    p, r, f = mir_eval.segment.pairwise(ref_iv, ref_lab, est_iv, est_lab)
    out["pairwise"] = {"f": f, "p": p, "r": r}

    # Fixed-shift variant: every estimated boundary moved LAG earlier.
    lag_starts = [0.0] + [max(0.0, s - _LAG) for s in starts[1:]]
    lag_iv, lag_lab = _to_intervals(lag_starts, labels, duration)
    lag_iv, lag_lab = _adjusted(lag_iv, lag_lab, duration)
    lag_block: dict = {"lag_secs": _LAG}
    for w in _WINDOWS:
        p, r, f = mir_eval.segment.detection(ref_iv, lag_iv, window=w, trim=True)
        lag_block[_window_key(w)] = {"f": f, "p": p, "r": r}
    out["lag_compensated"] = lag_block
    return out
