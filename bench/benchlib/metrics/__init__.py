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
    "drop": {"match_window_bars": 1.0, "bar_local_window_s": 16.0},
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
