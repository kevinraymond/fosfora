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


def predict_drop_table(summary: dict) -> list[str]:
    """Drop prediction vs chorus-onset PROXY truth. No published baseline
    exists for causal drop prediction — the rows stand alone, caveats attached."""
    pd = summary["metrics"]["predict_drop"]
    dp = summary["metrics"].get("drop", {})
    rows = [
        "### Drop prediction (`/fosfora/v1/predict/drop`)",
        "",
        "Truth tier: **chorus-onset proxies** on the Dance/Electronic subset "
        "(Harmonix has no drop labels). Proxies undercount real drop-scale "
        "events, so the false-alarm rate is an upper bound; the hand-annotated "
        "local set (C13) carries the headline lead-time number.",
        "",
        "| Tier | Coverage | Median lead (beats) | p25–p75 lead |",
        "|---|---|---|---|",
    ]
    for theta in ("0.5", "0.8"):
        t = pd.get(theta) or {}
        lead = t.get("lead_beats") or {}
        rows.append(
            f"| ≥ {theta} | {fmt(t.get('coverage'))} | {fmt(lead.get('median'), 1)} | "
            f"{fmt(lead.get('p25'), 1)}–{fmt(lead.get('p75'), 1)} |"
        )
    rows.append("")
    rows.append(
        f"False alarms {fmt(pd.get('false_alarms_per_min'), 2)}/min pooled over all "
        f"{pd.get('n_tracks', '?')} tracks (≈⅓ of off-genre alarms are the predictor "
        f"correctly anticipating a chorus landing). `/drop` detection event: "
        f"hit rate {fmt(dp.get('hit_rate'))} vs the same proxies, "
        f"{fmt(dp.get('false_drops_per_min'), 2)} false/min. It fires on loudness+sub-bass "
        f"impact and these proxies are chorus onsets, so most of what it finds is not "
        f"annotated here and most of what is annotated is not a drop — the hit rate is a "
        f"floor and the false rate a ceiling, both against the wrong target."
    )
    return rows


def structure_table(summary: dict) -> list[str]:
    """Section boundaries. Two streams with different jobs, reported separately
    because averaging them would hide what each one is for."""
    # Summary metrics are flattened dotted keys with {"mean", "n"} leaves.
    if mean(summary, "structure", "boundary_events.boundary_3_0s.f") is None:
        return []

    def m(leaf):
        return fmt(mean(summary, "structure", leaf))

    def blk(prefix, window="boundary_3_0s"):
        return (m(f"{prefix}{window}.f"), m(f"{prefix}{window}.p"), m(f"{prefix}{window}.r"))

    ef, ep, er = blk("boundary_events.")
    ef5 = m("boundary_events.boundary_0_5s.f")
    af = m("boundary_events.announced.boundary_3_0s.f")
    lf, lp, lr = blk("")
    rows = [
        "### Section boundaries (`/fosfora/v1/section/boundary`)",
        "",
        "No published causal baseline exists for this task, and the offline "
        "structure systems in the literature segment a whole file at once — the "
        "rows stand alone. Boundary detection is vocabulary-agnostic (labels are "
        "ignored), so Fosfora's EDM-shaped states need no mapping onto "
        "verse/chorus annotations.",
        "",
        "| Stream | Window | F | P | R |",
        "|---|---|---|---|---|",
        f"| **`/section/boundary`, back-dated by the reported age** | **3.0 s** "
        f"| **{ef}** | **{ep}** | **{er}** |",
        f"| `/section/boundary`, back-dated | 0.5 s | {ef5} | — | — |",
        f"| `/section/boundary`, taken at the moment announced | 3.0 s | {af} | — | — |",
        f"| `/section` label changes (all this stream carried before) | 3.0 s "
        f"| {lf} | {lp} | {lr} |",
        "",
        f"Estimated {fmt(mean(summary, 'structure', 'boundary_events.n_est_segments'), 1)} "
        f"segments per track against "
        f"{fmt(mean(summary, 'structure', 'n_ref_segments'), 1)} annotated.",
        "",
        "The first two rows subtract each event's own reported age — a fixed "
        "property of the detector (a centred novelty kernel plus a peak "
        "confirmation delay), published on the wire precisely so a consumer can "
        "do this. The third row is what a consumer sees if it ignores that "
        "argument and treats every cue as happening now; the gap between them is "
        "the honest price of hearing the song once, in order. Neither is the "
        "lag-compensated variant the results files carry and this card omits — "
        "that one shifts by a constant the *scorer* picked, where these use a "
        "latency the detector states about itself.",
    ]
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
        if "structure" in metrics:
            L.extend(structure_table(summary))
            L.append("")
        if (metrics.get("predict_drop") or {}).get("0.5", {}).get("coverage") is not None:
            L.extend(predict_drop_table(summary))
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
