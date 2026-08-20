"""Metric registry + the CONVENTIONS every number is computed under.

Each metric module registers (name, required annotation fields, scorer).
`score_all` runs every metric whose required fields are present in the bundle —
absence of a field skips the metric, it never zeroes it. CONVENTIONS is echoed
verbatim into every results JSON so the numbers are self-describing.

Global timestamp policy: score what the wire says. Dump `ts` is hop-END
sample-clock time; the <=11.6 ms grid bias stays uncorrected everywhere —
every tolerance here dwarfs one hop, and "correcting" would flatter the
causal system relative to what a rig actually receives.
"""

from __future__ import annotations

import os

from ..annotations import Annotations
from ..dump import SignalDump

CONVENTIONS = {
    "timestamp_policy": "wire (hop-end sample clock, no grid-bias correction)",
    "beat": {"f_measure_window_s": 0.07, "trim_first_s": 5.0},
    "tempo": {
        "estimate": "median of /bpm over final 50% of samples (values <= 0 excluded)",
        "acc1_tolerance": 0.04,
        "acc2_factors": [1 / 3, 1 / 2, 1.0, 2.0, 3.0],
    },
    "key": {"estimate": "duration-weighted majority over deduped /key"},
    "structure": {
        "boundary_windows_s": [0.5, 3.0],
        "trim": True,
        "lag_compensation_s": 3.0,
    },
    "drop": {
        "match_window_bars": 1.0,
        "bar_local_window_s": 16.0,
        # How a fire the listener explicitly rejected is scored when it still
        # lands inside the match window. See NEGATIVE_POLICIES below; only
        # bundles carrying `not_drops` are affected at all.
        "negative_policy": "strict",
        "negative_override_beats": 1.0,
        "negative_coincide_s": 0.25,
    },
    "predict_drop": {
        "thresholds": [0.5, 0.8],
        "sustain_window_s": 0.25,
        "sustain_min_samples": 2,
        "rearm_hysteresis": 0.15,
        "pre_window_bars": 8.0,
    },
    "stems": {
        "envelope": "512-sample RMS dB, sample-held onto the dump grid",
        "joint_silence_floor_dbfs": -60.0,
    },
}

# How a recorded negative interacts with the +-1 bar match window (#2299).
#
#   bar_window  the window alone decides; a rejected fire inside it is a hit.
#               What every number published before 2026-08-20 was computed under.
#   beat_grace  a rejected fire is false UNLESS it is within
#               `negative_override_beats` of the drop it matched. Chosen first
#               on the argument that a strobe under a beat early still reads as
#               on the drop — then Kevin watched Thirty Two Hertz play and
#               called the 74.68 s fire (0.370 s / 0.79 beat early) visibly
#               early. The argument was wrong; the mode is kept for
#               reproducing the numbers it produced.
#   strict      the listener's verdict always wins; a rejected fire is false
#               however close the drop is. The default, settled by eye.
#
# Any of them for one run:
#
#     FOSFORA_DROP_NEGATIVE_POLICY=beat_grace bench/run_bench.py ...
#
# The override rewrites CONVENTIONS in place so the *active* policy, not the
# default, is what every results JSON echoes. An unknown value is fatal: a
# silent fallback would make a published number a lie about its own rule.
NEGATIVE_POLICIES = ("bar_window", "beat_grace", "strict")

_policy_override = os.environ.get("FOSFORA_DROP_NEGATIVE_POLICY")
if _policy_override is not None:
    if _policy_override not in NEGATIVE_POLICIES:
        raise ValueError(
            f"FOSFORA_DROP_NEGATIVE_POLICY={_policy_override!r} is not one of "
            f"{NEGATIVE_POLICIES}"
        )
    CONVENTIONS["drop"]["negative_policy"] = _policy_override


# (name, tuple of Annotations attributes that must be non-None, allow_empty, scorer)
REGISTRY: list[tuple[str, tuple[str, ...], bool, object]] = []

# {metric name: aggregator(list of per-track blocks) -> dict} — metrics whose
# summary is not a plain mean (distributions, rates over pooled events).
AGGREGATORS: dict[str, object] = {}


def register(name: str, requires: tuple[str, ...], allow_empty: bool = False):
    """allow_empty: run even when a required field is present but empty — the
    drop metrics need zero-drop tracks as the negative class for false-alarm
    rates; absence (None) always skips."""

    def deco(fn):
        REGISTRY.append((name, requires, allow_empty, fn))
        return fn

    return deco


def score_all(dump: SignalDump, ann: Annotations) -> dict:
    """Run every metric whose annotation fields are present."""
    out = {}
    for name, requires, allow_empty, fn in REGISTRY:
        values = [getattr(ann, field, None) for field in requires]
        if any(v is None for v in values):
            continue
        if not allow_empty and any(
            hasattr(v, "__len__") and len(v) == 0 for v in values
        ):
            continue
        out[name] = fn(dump, ann)
    return out


# Imported for their @register side effects — order fixes REGISTRY order.
from . import beats, tempo, key, structure, drops, stems  # noqa: E402, F401
