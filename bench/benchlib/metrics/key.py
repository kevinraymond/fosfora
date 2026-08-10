"""Key scoring — MIREX weighted score over the deduped /key stream.

Headline: the duration-weighted majority key (what a rig consumed over the
song, robust to early flapping). Secondaries: the final key, change count and
time-to-first-emit — the emitter's 0.3 confidence gate means silence until
confident, so first_emit_ts is meaningful.

A track with no /key emission scores null here; the aggregator counts it as 0
in the dataset mean and reports a separate no_estimate_rate — silence is a
miss for the dataset, but not a fabricated 0-quality estimate for the track.
"""

from __future__ import annotations

import mir_eval.key

from ..annotations import key_to_mir_eval
from .. import dump as dump_mod
from . import AGGREGATORS, register


@register("key", ("key",))
def key(dump, ann) -> dict:
    changes = dump.changes(dump_mod.KEY)
    if not changes:
        return {"no_estimate": True, "ref_key": ann.key}

    end = dump.records[-1][0] if dump.records else changes[-1][0]
    held: dict[str, float] = {}
    for i, (ts, args) in enumerate(changes):
        until = changes[i + 1][0] if i + 1 < len(changes) else max(end, ts)
        held[args[0]] = held.get(args[0], 0.0) + (until - ts)

    majority_wire = max(held, key=held.get)
    majority = key_to_mir_eval(majority_wire)
    final = key_to_mir_eval(changes[-1][1][0])
    return {
        "score": mir_eval.key.weighted_score(ann.key, majority),
        "estimated_key": majority,
        "final_key": final,
        "final_key_score": mir_eval.key.weighted_score(ann.key, final),
        "ref_key": ann.key,
        "n_changes": len(changes),
        "first_emit_ts": changes[0][0],
    }


#: mir_eval weighted-score credits and the error-taxonomy bucket each one names.
CREDIT_BUCKETS = ((1.0, "exact"), (0.5, "fifth"), (0.3, "relative"), (0.2, "parallel"))


def credit_bucket(score: float) -> str:
    for credit, name in CREDIT_BUCKETS:
        if abs(score - credit) < 1e-6:
            return name
    return "other"


def _mode(key: str | None) -> str | None:
    return key.rsplit(" ", 1)[-1] if key else None


def _aggregate(blocks: list[dict]) -> dict:
    n = len(blocks)
    misses = sum(1 for b in blocks if b.get("no_estimate"))
    scores = [b["score"] for b in blocks if not b.get("no_estimate")]

    # Error taxonomy + mode accuracy. Silence counts as a miss everywhere
    # (bucket "none", wrong mode) — same policy as the score mean.
    taxonomy = {name: 0 for _, name in CREDIT_BUCKETS} | {"other": 0, "none": 0}
    mode_ok = 0
    ref_major = major_ok = 0
    for b in blocks:
        ref_mode = _mode(b.get("ref_key"))
        est_mode = None if b.get("no_estimate") else _mode(b.get("estimated_key"))
        if b.get("no_estimate"):
            taxonomy["none"] += 1
        else:
            taxonomy[credit_bucket(b["score"])] += 1
        if est_mode is not None and est_mode == ref_mode:
            mode_ok += 1
        if ref_mode == "major":
            ref_major += 1
            if est_mode == "major":
                major_ok += 1

    return {
        # no-estimate tracks count as 0: the dataset mean must not improve
        # because the detector stayed silent on hard tracks.
        "score": {"mean": sum(scores) / n if n else 0.0, "n": n},
        "no_estimate_rate": misses / n if n else 0.0,
        "taxonomy": taxonomy,
        "mode_accuracy": mode_ok / n if n else 0.0,
        # Recall on ref-major tracks: mode_accuracy alone is gameable on a
        # minor-heavy dataset (always-guess-minor scores 0.85 on GiantSteps).
        "major_mode_recall": major_ok / ref_major if ref_major else None,
        "first_emit_ts": {
            "mean": (
                sum(b["first_emit_ts"] for b in blocks if not b.get("no_estimate"))
                / len(scores)
                if scores
                else None
            ),
            "n": len(scores),
        },
    }


AGGREGATORS["key"] = _aggregate
