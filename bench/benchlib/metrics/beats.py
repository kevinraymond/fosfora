"""Beat + downbeat scoring — mir_eval.beat, the cited convention.

Headline numbers adopt mir_eval's 5 s trim (every published table we cite as a
comparison target uses it); `f_measure_untrimmed` is kept as a secondary field
because cold-start behavior is genuine causal product behavior, not noise.
"""

from __future__ import annotations

import mir_eval.beat
import numpy as np

from . import CONVENTIONS, register

_WINDOW = CONVENTIONS["beat"]["f_measure_window_s"]
_TRIM = CONVENTIONS["beat"]["trim_first_s"]


def _score(est: np.ndarray, ref: np.ndarray) -> dict:
    ref_trim = mir_eval.beat.trim_beats(ref, min_beat_time=_TRIM)
    est_trim = mir_eval.beat.trim_beats(est, min_beat_time=_TRIM)
    cmlc, cmlt, amlc, amlt = mir_eval.beat.continuity(ref_trim, est_trim)
    return {
        "f_measure": mir_eval.beat.f_measure(
            ref_trim, est_trim, f_measure_threshold=_WINDOW
        ),
        "cmlc": cmlc,
        "cmlt": cmlt,
        "amlc": amlc,
        "amlt": amlt,
        "f_measure_untrimmed": mir_eval.beat.f_measure(
            ref, est, f_measure_threshold=_WINDOW
        ),
        "n_ref": int(len(ref)),
        "n_est": int(len(est)),
    }


@register("beat", ("beats",))
def beat(dump, ann) -> dict:
    return _score(dump.beats(), ann.beats)


@register("downbeat", ("downbeats",))
def downbeat(dump, ann) -> dict:
    return _score(dump.downbeats(), ann.downbeats)
