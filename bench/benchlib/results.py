"""Results plumbing — per-track JSON (`fosfora-bench/v1`) and aggregation.

Per-track files are written with sorted keys and floats rounded to 4 decimals,
so re-runs diff cleanly. Aggregation is generic (mean over numeric leaves) with
per-metric override hooks for distribution-shaped blocks (lead time etc.).
"""

from __future__ import annotations

import json
import math
from pathlib import Path

SCHEMA = "fosfora-bench/v1"


def round_floats(obj, ndigits: int = 4):
    if isinstance(obj, float):
        if math.isnan(obj):
            return None  # JSON has no NaN; absence of a value is explicit
        return round(obj, ndigits)
    if isinstance(obj, dict):
        return {k: round_floats(v, ndigits) for k, v in obj.items()}
    if isinstance(obj, (list, tuple)):
        return [round_floats(v, ndigits) for v in obj]
    return obj


def make_result(
    dataset: str,
    track_id: str,
    dump_info: dict,
    conventions: dict,
    metrics: dict,
) -> dict:
    return {
        "schema": SCHEMA,
        "dataset": dataset,
        "track_id": track_id,
        "dump": dump_info,
        "conventions": conventions,
        "metrics": metrics,
    }


def write_result(path: Path, result: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as f:
        json.dump(round_floats(result), f, sort_keys=True, indent=1)
        f.write("\n")


def load_results(results_dir: Path) -> list[dict]:
    out = []
    for p in sorted(Path(results_dir).glob("*.json")):
        if p.name == "summary.json":
            continue
        with p.open(encoding="utf-8") as f:
            out.append(json.load(f))
    return out


def _numeric_leaves(block: dict, prefix: str = "") -> dict[str, float]:
    """Flatten a metric block to {dotted.path: number}; bools count as 0/1."""
    out: dict[str, float] = {}
    for k, v in block.items():
        path = f"{prefix}{k}"
        if isinstance(v, bool):
            out[path] = float(v)
        elif isinstance(v, (int, float)) and v is not None:
            out[path] = float(v)
        elif isinstance(v, dict):
            out.update(_numeric_leaves(v, f"{path}."))
        # lists and strings are not aggregatable generically
    return out


def generic_aggregate(blocks: list[dict]) -> dict:
    """Mean of every numeric leaf present in >=1 track, with per-leaf n."""
    sums: dict[str, float] = {}
    counts: dict[str, int] = {}
    for block in blocks:
        for path, v in _numeric_leaves(block).items():
            sums[path] = sums.get(path, 0.0) + v
            counts[path] = counts.get(path, 0) + 1
    return {
        path: {"mean": sums[path] / counts[path], "n": counts[path]}
        for path in sorted(sums)
    }


def aggregate(results: list[dict], aggregators: dict | None = None) -> dict:
    """Reduce per-track results to one summary dict per dataset."""
    aggregators = aggregators or {}
    by_metric: dict[str, list[dict]] = {}
    for r in results:
        for name, block in r.get("metrics", {}).items():
            if block is not None:
                by_metric.setdefault(name, []).append(block)
    summary_metrics = {}
    for name, blocks in sorted(by_metric.items()):
        fn = aggregators.get(name, generic_aggregate)
        summary_metrics[name] = {"n_tracks": len(blocks), **fn(blocks)}
    datasets = sorted({r.get("dataset", "?") for r in results})
    return {
        "schema": f"{SCHEMA}-summary",
        "dataset": datasets[0] if len(datasets) == 1 else datasets,
        "n_tracks": len(results),
        "metrics": summary_metrics,
    }
