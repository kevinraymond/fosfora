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


def _aggregate(blocks: list[dict]) -> dict:
    n = len(blocks)
    misses = sum(1 for b in blocks if b.get("no_estimate"))
    scores = [b["score"] for b in blocks if not b.get("no_estimate")]
    return {
        # no-estimate tracks count as 0: the dataset mean must not improve
        # because the detector stayed silent on hard tracks.
        "score": {"mean": sum(scores) / n if n else 0.0, "n": n},
        "no_estimate_rate": misses / n if n else 0.0,
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
