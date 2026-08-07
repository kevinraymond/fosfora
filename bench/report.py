#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyyaml"]
# ///
"""Generate docs/BENCHMARKS.md from local bench results + cited baselines.

    bench/report.py            # regenerate docs/BENCHMARKS.md
    bench/report.py --check    # exit 1 if the tracked file is stale

Renders one section per dataset that has bench/out/results/<ds>/summary.json,
with Fosfora's causal numbers beside the cited published baselines from
bench/baselines.yaml (offline systems clearly labeled — they see the whole
file; Fosfora streams). Coverage and exclusions are printed, never hidden.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import yaml

sys.path.insert(0, str(Path(__file__).parent))

import datasetlib as dl

DOC = "docs/BENCHMARKS.md"
DATASET_TITLES = {
    "ballroom": "Ballroom (698 x 30 s, dance genres)",
    "smc": "SMC_MIREX (217 x 40 s, hard beat cases)",
    "giantsteps_tempo": "GiantSteps Tempo (664 EDM previews)",
    "giantsteps_key": "GiantSteps Key (604 EDM previews)",
    "harmonix": "Harmonix Set (pop/EDM, YouTube-sourced audio)",
    "fixture": None,  # CI fixture: never in the public report
}

DATASET_NOTES = {
    "ballroom": (
        "Context: an out-of-genre stress test for an EDM-tuned causal engine — "
        "30 s clips (so a ~15 s mean tempo lock leaves half of each excerpt "
        "unlocked, and the 5 s trim only removes part of that), 3/4 meters "
        "included, and tempi from ~60 to ~224 BPM against an EDM-centered tempo "
        "prior; the Acc1 vs Acc2 gap is octave choice. Tempo ground truth is the "
        "median inter-beat interval of the beat annotations."
    ),
    "giantsteps_tempo": (
        "Context: the in-genre test — EDM, 2-minute previews. Ground truth is "
        "the crowdsourced annotations_v2 single tempo (Schreiber & Müller 2018)."
    ),
    "giantsteps_key": (
        "Context: the in-genre test — EDM, 2-minute previews. Mixed In Key and "
        "RekordBox rows are commercial DJ tools benchmarked offline."
    ),
    "harmonix": (
        "Context: audio is re-fetched from YouTube and admitted only by the "
        "alignment gate (subsequence mel-DTW vs the authors' distributed "
        "original-audio spectrograms + onset refinement) — the coverage table "
        "counts every exclusion. Published rows are 8-fold cross-validation on "
        "the full 912; ours is zero-shot on the gated subset. Not the same "
        "test bed — both facts stated."
    ),
}


def fmt(x, digits=3):
    return "—" if x is None else f"{x:.{digits}f}"


def load_results(root: Path) -> dict[str, dict]:
    out = {}
    for summary in sorted(root.glob("bench/out/results/*/summary.json")):
        ds = summary.parent.name
        if DATASET_TITLES.get(ds, "skip") is None:
            continue
        with summary.open(encoding="utf-8") as f:
            data = json.load(f)
        # binary provenance from any per-track result
        for track in summary.parent.glob("*.json"):
            if track.name == "summary.json":
                continue
            with track.open(encoding="utf-8") as f:
                data["_binary"] = json.load(f)["dump"].get("binary_sha256", "")[:16]
            break
        out[ds] = data
    return out


def mean(summary: dict, metric: str, leaf: str):
    node = summary.get("metrics", {}).get(metric, {}).get(leaf)
    return node.get("mean") if isinstance(node, dict) else node


def baseline_rows(baselines: dict, dataset: str) -> list[dict]:
    return [e for e in baselines["entries"] if e["dataset"] == dataset]


def used_refs(baselines: dict, datasets: list[str]) -> list[str]:
    keys = []
    for e in baselines["entries"]:
        if e["dataset"] in datasets and e.get("ref") and e["ref"] not in keys:
            keys.append(e["ref"])
    for extra in ("sturm2014ballroom", "krebs2013ballroom", "gouyon2006tempo",
                  "holzapfel2012smc", "knees2015giantsteps", "schreiber2018giantsteps"):
        if extra in baselines["refs"] and extra not in keys:
            keys.append(extra)
    return keys


def beat_table(ds: str, summary: dict, baselines: dict) -> list[str]:
    rows = [
        "| System | Mode | Beat F | CMLt | AMLt | Downbeat F |",
        "|---|---|---|---|---|---|",
        "| **Fosfora** | **causal (streaming)** | "
        f"**{fmt(mean(summary, 'beat', 'f_measure'))}** | "
        f"**{fmt(mean(summary, 'beat', 'cmlt'))}** | "
        f"**{fmt(mean(summary, 'beat', 'amlt'))}** | "
        f"**{fmt(mean(summary, 'downbeat', 'f_measure'))}** |",
    ]
    for e in baseline_rows(baselines, ds):
        m = e["metrics"]
        if "beat_f" not in m:
            continue
        note = f" — {e['note']}" if e.get("note") else ""
        rows.append(
            f"| {e['system']} ([{e['ref']}](#references){note}) | {e['mode']} | "
            f"{fmt(m.get('beat_f'))} | {fmt(m.get('beat_cmlt'))} | "
            f"{fmt(m.get('beat_amlt'))} | {fmt(m.get('downbeat_f'))} |"
        )
    return rows


def tempo_table(ds: str, summary: dict, baselines: dict) -> list[str]:
    rows = [
        "| System | Mode | Acc1 | Acc2 |",
        "|---|---|---|---|",
        "| **Fosfora** | **causal (streaming)** | "
        f"**{fmt(mean(summary, 'tempo', 'acc1'))}** | "
        f"**{fmt(mean(summary, 'tempo', 'acc2'))}** |",
    ]
    for e in baseline_rows(baselines, ds):
        m = e["metrics"]
        if "acc1" not in m:
            continue
        note = f" — {e['note']}" if e.get("note") else ""
        rows.append(
            f"| {e['system']} ([{e['ref']}](#references){note}) | {e['mode']} | "
            f"{fmt(m.get('acc1'))} | {fmt(m.get('acc2'))} |"
        )
    lock = mean(summary, "tempo", "lock_time_secs")
    frac = mean(summary, "tempo", "locked_fraction")
    rows.append("")
    rows.append(
        f"Causal extras: mean lock time {fmt(lock, 1)} s (earliest instant after "
        f"which every later estimate stays within 4%), mean locked fraction "
        f"{fmt(frac)}. Offline systems have no equivalent — they see the whole file."
    )
    return rows


def key_table(ds: str, summary: dict, baselines: dict) -> list[str]:
    rows = [
        "| System | Mode | MIREX weighted |",
        "|---|---|---|",
        "| **Fosfora** | **causal (streaming)** | "
        f"**{fmt(mean(summary, 'key', 'score'))}** |",
    ]
    for e in baseline_rows(baselines, ds):
        m = e["metrics"]
        if "mirex_weighted" not in m:
            continue
        note = f" — {e['note']}" if e.get("note") else ""
        rows.append(
            f"| {e['system']} ([{e['ref']}](#references){note}) | {e['mode']} | "
            f"{fmt(m.get('mirex_weighted'), 4)} |"
        )
    nr = summary.get("metrics", {}).get("key", {}).get("no_estimate_rate")
    fe = mean(summary, "key", "first_emit_ts")
    rows.append("")
    rows.append(
        f"Causal extras: no-estimate rate {fmt(nr)} (silent detectors score 0 in "
        f"the mean above, never dropped), mean time-to-first-estimate {fmt(fe, 1)} s."
    )
    return rows


def coverage_table(results: dict[str, dict]) -> list[str]:
    rows = [
        "| Dataset | Expected | Scored | Excluded (manifest) | Dump failures | Binary |",
        "|---|---|---|---|---|---|",
    ]
    for ds, summary in results.items():
        try:
            manifest = dl.load_manifest(ds)
            expected = (manifest.get("expected") or {}).get("tracks", "—")
            excluded = len(manifest.get("exclusions") or [])
        except Exception:
            expected, excluded = "—", 0
        cov = summary.get("coverage", {})
        rows.append(
            f"| {ds} | {expected} | {summary.get('n_tracks', '—')} | {excluded} | "
            f"{len(cov.get('dump_failures', []))} | `{summary.get('_binary', '?')}` |"
        )
    return rows


def render(root: Path) -> str:
    results = load_results(root)
    with (root / "bench" / "baselines.yaml").open(encoding="utf-8") as f:
        baselines = yaml.safe_load(f)

    L: list[str] = []
    L.append("# Benchmarks")
    L.append("")
    L.append("<!-- Generated by bench/report.py — do not edit by hand. -->")
    L.append("")
    L.append(
        "Accuracy of the **causal streaming engine**: audio is streamed through the"
    )
    L.append(
        "exact live analysis path (512-sample hops, no lookahead) via"
    )
    L.append(
        "`--signal-dump`, and the deterministic JSONL is scored against annotated"
    )
    L.append(
        "datasets with [mir_eval](https://github.com/mir-evaluation/mir_eval)-backed"
    )
    L.append(
        "metrics (see [SIGNAL.md](SIGNAL.md) for the wire contract). Published"
    )
    L.append(
        "numbers are **cited, never re-run locally**, and labeled by mode: an"
    )
    L.append(
        "*offline* system sees the whole file; Fosfora and *online* systems hear it"
    )
    L.append(
        "once, in order, like a rig does. Metric conventions (trim policy, tempo"
    )
    L.append(
        "estimate derivation, dedupe rules) ride inside every results file; the"
    )
    L.append(
        "headline beat numbers adopt the literature's 5 s trim."
    )
    L.append("")
    L.append("## Coverage")
    L.append("")
    L.extend(coverage_table(results))
    L.append("")

    for ds, summary in results.items():
        metrics = summary.get("metrics", {})
        L.append(f"## {DATASET_TITLES.get(ds, ds)}")
        L.append("")
        if ds in DATASET_NOTES:
            L.append(DATASET_NOTES[ds])
            L.append("")
        if "beat" in metrics:
            L.extend(beat_table(ds, summary, baselines))
            L.append("")
        if "tempo" in metrics:
            L.extend(tempo_table(ds, summary, baselines))
            L.append("")
        if "key" in metrics:
            L.extend(key_table(ds, summary, baselines))
            L.append("")

    L.append("## References")
    L.append("")
    for key in used_refs(baselines, list(results)):
        ref = baselines["refs"][key]
        L.append(f"- **{key}** — {ref['cite']}. <{ref['url']}>")
    L.append("")
    return "\n".join(L)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true", help="fail if the doc is stale")
    args = ap.parse_args()

    root = dl.repo_root()
    content = render(root)
    doc = root / DOC
    if args.check:
        if not doc.is_file() or doc.read_text(encoding="utf-8") != content:
            print(f"{DOC} is stale — regenerate with bench/report.py", file=sys.stderr)
            return 1
        print(f"{DOC} is up to date")
        return 0
    doc.write_text(content, encoding="utf-8")
    print(f"wrote {DOC}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
