#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Dump + score the synthetic fixture and assert the checked-in floors.

    bench/make_fixture.py -o bench/out/fixture
    FOSFORA_BIN=target/debug/fosfora bench/check_fixture.py bench/out/fixture

Scores through the exact benchlib code paths real datasets use — that is the
point of the CI leg: it exercises binary -> dump -> parse -> metrics end to
end. Assertions live in bench/fixture_expectations.json as floors/bands, not
exact pins: exact scores are per-binary (toolchain bumps and platforms shift
ULPs), while byte-determinism itself is enforced where it is a real guarantee
— the in-process Rust test in src/signal/mod.rs.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from benchlib import metrics, results
from benchlib.annotations import Annotations, load_index
from benchlib.dump import SignalDump
from benchlib.runner import DumpRunner


def lookup(result: dict, path):
    """path: dotted string, or a list of keys when a key itself contains a dot
    (e.g. the "0.5" threshold)."""
    parts = path if isinstance(path, list) else path.split(".")
    node = result
    for part in parts:
        if not isinstance(node, dict) or part not in node:
            return None
        node = node[part]
    return node


def check(result: dict, expect: dict) -> list[str]:
    failures = []
    for a in expect["asserts"]:
        path, op, want = a["path"], a["op"], a.get("value")
        got = lookup(result, path)
        if isinstance(path, list):
            path = "/".join(path)
        ok = False
        if op == "not_null":
            ok = got is not None
        elif got is None:
            ok = False
        elif op == ">=":
            ok = got >= want
        elif op == "<=":
            ok = got <= want
        elif op == "==":
            ok = got == want
        elif op == "in_range":
            ok = want[0] <= got <= want[1]
        else:
            failures.append(f"{path}: unknown op {op!r}")
            continue
        if not ok:
            failures.append(f"{path}: got {got!r}, want {op} {want!r}")
    return failures


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("fixture_dir", type=Path, nargs="?", default=Path("bench/out/fixture"))
    ap.add_argument(
        "--expectations",
        type=Path,
        default=Path(__file__).parent / "fixture_expectations.json",
    )
    ap.add_argument(
        "--print-result", action="store_true", help="dump the full result JSON"
    )
    args = ap.parse_args()

    track = load_index(args.fixture_dir / "index.json")[0]
    runner = DumpRunner(args.fixture_dir / "dumps")
    dump_path = runner.ensure_dump(track["audio"], force=True)
    dump = SignalDump.load(dump_path)
    ann = Annotations.load(track["annotations"])

    result = results.make_result(
        dataset="fixture",
        track_id=track["track_id"],
        dump_info={
            "binary_sha256": runner.binary_sha256,
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
    result = results.round_floats(result)
    results.write_result(args.fixture_dir / "result.json", result)
    if args.print_result:
        json.dump(result, sys.stdout, sort_keys=True, indent=1)
        print()

    with args.expectations.open(encoding="utf-8") as f:
        expect = json.load(f)
    failures = check(result, expect)
    if failures:
        print(f"check_fixture: {len(failures)} assertion(s) FAILED:", file=sys.stderr)
        for f_ in failures:
            print(f"  {f_}", file=sys.stderr)
        return 1
    print(f"check_fixture: all {len(expect['asserts'])} assertions passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
