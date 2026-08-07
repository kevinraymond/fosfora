"""Shared machinery for the benchmark harness (workstream C).

Scores `fosfora --signal-dump` JSONL against normalized annotation bundles with
mir_eval-convention metrics. Consumed by the thin PEP-723 entry scripts in
`bench/` (`run_bench.py`, `score_dump.py`, `check_fixture.py`); unit-tested by
`bench/test_benchlib.py`. Nothing here ships to users.

The dump format is the frozen wire contract (`src/signal/sink.rs`, docs/SIGNAL.md);
the annotation bundle schema is `fosfora-bench-annotation/v1` (bench/README.md).
"""

from __future__ import annotations
