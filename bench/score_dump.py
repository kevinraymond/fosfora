#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Score one existing dump against one annotation bundle (debug tool).

    bench/score_dump.py song.signal.jsonl song.annotations.json

Prints the per-track result JSON to stdout — no dump cache, no results dir.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from benchlib import metrics, results
from benchlib.annotations import Annotations
from benchlib.dump import SignalDump


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("dump", type=Path)
    ap.add_argument("annotations", type=Path)
    args = ap.parse_args()

    dump = SignalDump.load(args.dump)
    ann = Annotations.load(args.annotations)
    result = results.make_result(
        dataset=ann.dataset,
        track_id=ann.track_id,
        dump_info={
            "source": dump.source,
            "hop_hz": dump.hop_hz,
            "tx_rate_hz": dump.tx_rate_hz,
            "n_events": {
                "beat": len(dump.beats()),
                "downbeat": len(dump.downbeats()),
                "drop": len(dump.drops()),
            },
        },
        conventions=metrics.CONVENTIONS,
        metrics=metrics.score_all(dump, ann),
    )
    json.dump(results.round_floats(result), sys.stdout, sort_keys=True, indent=1)
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
