#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["anthropic"]
# ///
"""Stage B of the screenplay pipeline (#2040): screenplay -> realized scene dir.

    uv run scripts/realize_screenplay.py --screenplay song.screenplay.md \
        --analysis song.analysis.json --out gen/
    ./target/release/fosfora --render-scene gen/ --song song.mp3

Casts the screenplay's imagery from the effect catalog (catalog/effects/, the
measured casting sheets — board #2041) into presets, bindings and a cue list,
through the same emit/--validate/repair substrate as generate_scene.py. The
screenplay text — including Kevin's edits — is the authoritative brief.

What the model does NOT do here, because a deterministic pass does it better:

- TIMING. Cues map 1:1 onto the screenplay's beats. hold_secs counts after the
  incoming transition completes, so Python plans transition lengths and holds
  such that every transition lands exactly on its beat boundary
  (scenelib.plan_cue_timing). The model only proposes transition style/length.

- REMAP INPUT RANGES (#2037). An authored range pinned Tunnel.speed at max for a
  whole song on a compressed master. The model emits remaps with placeholder
  inputs [0, 1]; scenelib.calibrate_remaps sets them to the pooled p10/p90 of
  the sections each preset actually plays over, from the analysis percentiles.

Imagery the catalog cannot reach degrades to the nearest achievable cast and is
REPORTED in <out>/gaps.md — the demand-driven backlog for new effects.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

from scenelib import (
    Screenplay,
    analysis_digest,
    cached_system,
    calibrate_remaps,
    call_model,
    check_screenplay,
    check_variants,
    compact,
    emit,
    field_rules,
    parse_screenplay,
    plan_cue_timing,
    run_validator,
    schema_blocks,
    spreads,
    usage_line,
)

MAX_TOKENS = 32000


# ----------------------------------------------------------------- the schema
def build_schema(
    caps: dict[str, Any], beat_ids: list[str], all_sources: bool = False
) -> dict[str, Any]:
    """One cue per screenplay beat. `beat` is a runtime enum of the parsed beat
    ids — a hallucinated or missing beat is unrepresentable, the same trick the
    effect-name enum plays. Timing fields are absent on purpose: Python owns
    them (see module docstring)."""
    blocks = schema_blocks(caps, all_sources)

    preset = {
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Preset name; cues reference it, and it becomes the filename.",
            },
            "for_scenes": {
                "type": "string",
                "description": "Which scenes/beats of the screenplay this look serves, in brief.",
            },
            "rationale": {
                "type": "string",
                "description": (
                    "One or two sentences: which screenplay imagery this casts, and "
                    "which catalog evidence (measured motion, luma, palette) says "
                    "these effects can play it."
                ),
            },
            "layers": {
                "type": "array",
                "description": f"1..{caps['limits']['max_layers']} layers, bottom-first.",
                "items": blocks["layer"],
            },
            "bindings": {"type": "array", "items": blocks["binding"]},
        },
        "required": ["name", "for_scenes", "rationale", "layers", "bindings"],
        "additionalProperties": False,
    }

    cue = {
        "type": "object",
        "properties": {
            "beat": {
                "enum": beat_ids,
                "description": "The screenplay beat this cue realizes. Every beat, exactly once, in order.",
            },
            "preset": {"type": "string", "description": "Name of one of the presets above."},
            "label": {"type": "string"},
            "transition": {"enum": list(caps["enums"]["transitions"])},
            "transition_secs": {
                "type": "number",
                "description": (
                    "Requested transition length; the pipeline clamps it so the "
                    "transition completes exactly on the beat boundary."
                ),
            },
            "param_overrides": {
                "type": "array",
                "description": (
                    "Per-layer param values for this cue only — how one preset "
                    "plays several beats at different intensities. Consecutive "
                    "cues on the same preset MUST differ in overrides."
                ),
                "items": blocks["override_group"],
            },
        },
        "required": [
            "beat",
            "preset",
            "label",
            "transition",
            "transition_secs",
            "param_overrides",
        ],
        "additionalProperties": False,
    }

    gap = {
        "type": "object",
        "properties": {
            "beat": {"enum": beat_ids},
            "asked_for": {"type": "string", "description": "The screenplay imagery, quoted or tightly paraphrased."},
            "closest_cast": {"type": "string", "description": "What was cast instead."},
            "why_it_falls_short": {"type": "string"},
            "what_would_close_it": {
                "type": "string",
                "description": "The capability (effect, param, behavior) that would realize the ask.",
            },
        },
        "required": ["beat", "asked_for", "closest_cast", "why_it_falls_short", "what_would_close_it"],
        "additionalProperties": False,
    }

    return {
        "type": "object",
        "properties": {
            "scene_name": {"type": "string"},
            "reading": {
                "type": "string",
                "description": (
                    "A short paragraph: how you read the screenplay's arc and how "
                    "the casting realizes it. Written for a human reviewer."
                ),
            },
            "presets": {"type": "array", "items": preset},
            "cues": {
                "type": "array",
                "description": "One cue per screenplay beat, in screenplay order.",
                "items": cue,
            },
            "gaps": {
                "type": "array",
                "description": "Imagery the catalog could not reach. Empty if none.",
                "items": gap,
            },
        },
        "required": ["scene_name", "reading", "presets", "cues", "gaps"],
        "additionalProperties": False,
    }


# ----------------------------------------------------------------- the prompt
SYSTEM_INSTRUCTIONS = """\
You are the realizer for Fosfora, an audio-reactive VJ engine. A screenwriter \
who knows nothing about the engine has written the story of one song as pure \
imagery; a human editor may have revised it. Your job is CASTING: realize each \
scene and beat of that screenplay with the engine's actual effects, params and \
audio bindings, as evidenced by the casting catalog.

THE CATALOG OUTRANKS EVERYTHING ELSE. Each entry was written against real \
rendered frames with measured motion (mean inter-frame delta: 0.001 static, \
0.03 moderate, 0.10 frantic) and luma (quiet/loud). Effect descriptions and \
param names promise; the catalog reports. When they disagree, believe the \
catalog.

PACE IS CAST, NOT DIALED. Params named "speed" or "rate" mostly do not govern \
perceived speed (measured: only Sumi and Tide sweep cleanly; Tunnel's speed=hi \
is brightness washout, not velocity). Realize each scene's `pace:` word by \
casting effects whose MEASURED motion matches — still <0.005, drifting ~0.01, \
pulsing ~0.03, driving 0.05–0.08, frantic ~0.10 — and by binding motion-adjacent \
params to audio. Match each scene's `light:` against the entries' measured luma.

RESPECT THE FLAGS. Entries flagged near-black or degenerate at defaults \
(Array, Lattice 445, Lattice Pulse, Accretion, Vessel, Polycephalum) may only \
be cast with the fixes their entries describe, or as underlays beneath a layer \
that carries the frame. Several effects invert under loud music (Chaos \
collapses, Strata darkens, Protea floods, Morph gets busier when quiet, Drift \
flips palette) — cast them where the screenplay wants that behavior, not \
against it.

THE SCREENPLAY IS THE BRIEF. Its edited text is authoritative over anything \
you might infer from the analysis. Distinct identities should be distinct \
worlds; beats within a scene are usually the same preset varied with \
`param_overrides` (that is what they exist for), and a new preset mid-scene \
only when the screenplay demands a hard turn. Consecutive cues on the same \
preset MUST vary their overrides — an identical repeat is a validation fault.

BINDINGS ARE THE POINT. A scene with no bindings is a slideshow. Bind to \
features the request's spread table says actually move on this song, and make \
them legible: the thing the ear notices and the thing the eye notices should \
be the same thing. Each entry's Energy response section says how the effect \
already behaves under loud music — bind WITH that grain.

REMAP RULE. If you shape a binding with `remap`, it must be the FIRST \
transform, with `in_lo: 0, in_hi: 1` exactly as placeholders — the pipeline \
calibrates input ranges to this song's measured per-section distributions. You \
own `out_lo`/`out_hi` and any transforms after it. Use `deadzone`, `gate`, \
`curve` for deliberate thresholds and shaping.

REPORT THE GAPS. Where the screenplay asks for imagery no catalog entry can \
play, cast the nearest achievable anyway and file a gap: what was asked, what \
you cast, why it falls short, what capability would close it. Honest gaps \
become new effects; silent substitutions become mush. An empty gap list on an \
ambitious screenplay is suspicious.

The capabilities document is authoritative on what exists: every effect, \
param, range and audio source in it is real, and anything not in it is not.
"""


def catalog_corpus(catalog_dir: Path) -> str:
    """catalog/README.md + every entry, sorted, separated. Byte-stable across
    songs, so it sits in the cached system prefix."""
    readme = catalog_dir.parent / "README.md"
    parts = []
    if readme.exists():
        parts.append(readme.read_text())
    entries = sorted(catalog_dir.glob("*.md"))
    if not entries:
        raise SystemExit(f"no catalog entries in {catalog_dir}")
    for e in entries:
        parts.append(f"--- {e.stem} ---\n{e.read_text()}")
    return "\n\n".join(parts)


def build_system(caps: dict[str, Any], corpus: str) -> list[dict[str, Any]]:
    return cached_system([
        SYSTEM_INSTRUCTIONS + "\n" + field_rules() + "\n",
        "The casting catalog (measured, authoritative on look and motion):\n\n" + corpus,
        "Engine capabilities (authoritative on what exists):\n" + compact(caps),
    ])


def build_user(screenplay_text: str, analysis: dict[str, Any], sp: Screenplay) -> str:
    beat_lines = []
    for sc, b in sp.all_beats():
        pace = (sc.direction or {}).get("pace", "?")
        beat_lines.append(
            f"  {b.id:>4}  {b.start:7.1f} – {b.end:7.1f}s  section {sc.index} "
            f"(identity {sc.label}, pace {pace})"
        )
    return "\n".join([
        "THE SCREENPLAY (authoritative, as edited):",
        "",
        screenplay_text,
        "",
        f"Cue plan: one cue per beat, {len(beat_lines)} beats in order:",
        *beat_lines,
        "",
        "Descriptor spread across sections — bind to features that actually move:",
        spreads(analysis),
        "",
        "Analysis digest (the numbers behind the screenplay's signal lines):",
        analysis_digest(analysis),
        "",
        "Cast the screenplay: presets for its worlds, one cue per beat in order, "
        "bindings that make the music visible, and an honest gap list.",
    ])


# --------------------------------------------------------- deterministic pass
def local_problems(result: dict[str, Any], sp: Screenplay) -> list[str]:
    """What the schema cannot promise: beat bijection, cue/preset references."""
    problems = check_variants(result)
    beat_ids = [b.id for _, b in sp.all_beats()]
    got = [c["beat"] for c in result["cues"]]
    if got != beat_ids:
        problems.append(
            f"cues cover beats {got}; the screenplay's beats in order are {beat_ids} — "
            "every beat exactly once, in screenplay order"
        )
    names = {p["name"] for p in result["presets"]}
    for c in result["cues"]:
        if c["preset"] not in names:
            problems.append(f"cue {c['beat']}: preset {c['preset']!r} is not defined")
    return problems


def realize_cues(result: dict[str, Any], sp: Screenplay) -> None:
    """Model cues (per-beat, no timing) -> emit-shaped cues (hold_secs planned so
    every transition completes on its beat boundary). Mutates result."""
    pairs = sp.all_beats()
    spans = [(b.start, b.end) for _, b in pairs]
    timing = plan_cue_timing(spans, result["cues"])
    cues = []
    for (sc, b), model_cue, t in zip(pairs, result["cues"], timing):
        cues.append({
            "preset": model_cue["preset"],
            "label": model_cue.get("label") or (b.title if b.title else f"beat {b.id}"),
            "hold_secs": t["hold_secs"],
            "transition": t["transition"],
            "transition_secs": t["transition_secs"],
            "param_overrides": model_cue.get("param_overrides", []),
        })
    result["cues"] = cues


def preset_sections(result: dict[str, Any], sp: Screenplay) -> dict[str, list[int]]:
    """Which analyzer sections each preset plays over — the calibration pool."""
    out: dict[str, set[int]] = {}
    for (sc, _), cue in zip(sp.all_beats(), result["cues"]):
        out.setdefault(cue["preset"], set()).add(sc.index)
    return {k: sorted(v) for k, v in out.items()}


def write_gaps(result: dict[str, Any], sp: Screenplay, screenplay_path: Path, outdir: Path) -> Path | None:
    gaps = result.get("gaps", [])
    if not gaps:
        return None
    by_id = {b.id: (sc, b) for sc, b in sp.all_beats()}
    lines = [
        "# Casting gaps",
        "",
        f"Screenplay: {screenplay_path}",
        f"Scene: {result['scene_name']}",
        "",
        "Imagery the current effect library could not reach, and what would close "
        "each gap. This file is the demand-driven backlog for new effects.",
        "",
    ]
    for g in gaps:
        sc, b = by_id.get(g["beat"], (None, None))
        where = f"beat {g['beat']}"
        if sc is not None:
            where += f" (section {sc.index}, identity {sc.label})"
            asked_line = b.prose if not b.implicit else " ".join(sc.prose)[:200]
        else:
            asked_line = ""
        lines += [
            f"## {where}",
            "",
            *([f"> {asked_line}", ""] if asked_line else []),
            f"- **Asked for:** {g['asked_for']}",
            f"- **Cast instead:** {g['closest_cast']}",
            f"- **Falls short because:** {g['why_it_falls_short']}",
            f"- **Would close it:** {g['what_would_close_it']}",
            "",
        ]
    path = outdir / "gaps.md"
    path.write_text("\n".join(lines))
    return path


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--screenplay", required=True, type=Path)
    ap.add_argument("--analysis", required=True, type=Path)
    ap.add_argument("--out", type=Path, help="scene dir to write (required unless --check-only)")
    ap.add_argument("--capabilities", type=Path, default=Path("catalog/capabilities.json"))
    ap.add_argument("--catalog", type=Path, default=Path("catalog/effects"))
    ap.add_argument(
        "--binary",
        type=Path,
        default=Path("target/release/fosfora"),
        help="fosfora built with --features analyze, for --validate",
    )
    ap.add_argument("--attempts", type=int, default=3)
    ap.add_argument("--all-sources", action="store_true")
    ap.add_argument(
        "--check-only",
        action="store_true",
        help="parse + structural-check the screenplay against the analysis and stop "
        "(run this after editing)",
    )
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    analysis = json.loads(args.analysis.read_text())
    screenplay_text = args.screenplay.read_text()
    sp = parse_screenplay(screenplay_text)
    problems, warnings = check_screenplay(sp, analysis)
    for w in warnings:
        print(f"warning: {w}", file=sys.stderr)
    if problems:
        print(f"{args.screenplay} fails its structural check:", file=sys.stderr)
        for p in problems:
            print(f"  {p}", file=sys.stderr)
        return 1
    if args.check_only:
        beats = sum(len(sc.effective_beats()) for sc in sp.scenes)
        print(
            f"{args.screenplay}: OK — {len(sp.scenes)} scenes, {beats} beats",
            file=sys.stderr,
        )
        return 0
    if args.out is None:
        print("--out is required unless --check-only", file=sys.stderr)
        return 2

    caps = json.loads(args.capabilities.read_text())
    if caps.get("analysis_version") != analysis.get("version"):
        print(
            f"warning: capabilities.json expects analysis version "
            f"{caps.get('analysis_version')}, got {analysis.get('version')} — "
            "regenerate both from the same build",
            file=sys.stderr,
        )

    beat_ids = [b.id for _, b in sp.all_beats()]
    schema = build_schema(caps, beat_ids, all_sources=args.all_sources)
    corpus = catalog_corpus(args.catalog)
    system = build_system(caps, corpus)
    user = build_user(screenplay_text, analysis, sp)

    if args.dry_run:
        print(f"beats        {len(beat_ids)}")
        print(f"catalog      {len(list(args.catalog.glob('*.md')))} entries, {len(corpus):,} bytes")
        print(f"system bytes {sum(len(b['text']) for b in system):,}")
        print(f"user bytes   {len(user):,}")
        print(f"schema bytes {len(compact(schema)):,}")
        return 0

    import anthropic

    client = anthropic.Anthropic()
    messages: list[dict[str, Any]] = [{"role": "user", "content": user}]

    for attempt in range(1, args.attempts + 1):
        print(f"[{attempt}/{args.attempts}] casting…", file=sys.stderr)
        message = call_model(
            client, system, messages, schema, stream_to_stderr=True,
            max_tokens=MAX_TOKENS,
        )
        print(f"    usage: {usage_line(message)}", file=sys.stderr)

        text = next(b.text for b in message.content if b.type == "text")
        result = json.loads(text)

        found = local_problems(result, sp)
        if found:
            ok, problems_text = False, "\n".join(found)
        else:
            realize_cues(result, sp)
            log, cal_warnings = calibrate_remaps(
                result, analysis, preset_sections(result, sp)
            )
            for w in cal_warnings:
                print(f"    warning: {w}", file=sys.stderr)
            for line in log:
                print(f"    calibrated: {line}", file=sys.stderr)
            emit(result, args.out)
            ok, problems_text = run_validator(args.binary, args.out)

        if ok:
            gaps_path = write_gaps(result, sp, args.screenplay, args.out)
            print(f"\n{result['scene_name']}\n", file=sys.stderr)
            print(result["reading"], file=sys.stderr)
            print(file=sys.stderr)
            for p in result["presets"]:
                print(f"  {p['name']}  ({p['for_scenes']})", file=sys.stderr)
                print(f"    {p['rationale']}", file=sys.stderr)
            print(file=sys.stderr)
            for path in sorted(args.out.glob("*.json")):
                print(path)
            n_gaps = len(result.get("gaps", []))
            print(
                f"\nvalidated clean — {len(result['cues'])} cues, "
                f"{len(log)} remaps calibrated, {n_gaps} gaps"
                + (f" (see {gaps_path})" if gaps_path else ""),
                file=sys.stderr,
            )
            print(
                "\nRender it:\n"
                f"  ./target/release/fosfora --render-scene {args.out} "
                f"--song <the song file>",
                file=sys.stderr,
            )
            return 0

        print(f"    rejected:\n{problems_text}", file=sys.stderr)
        if attempt == args.attempts:
            print(
                f"\nstopping after {args.attempts} attempt(s); files in {args.out} "
                "are the last (invalid) draft",
                file=sys.stderr,
            )
            return 1

        messages += [
            {"role": "assistant", "content": text},
            {
                "role": "user",
                "content": (
                    "The pipeline rejected that cast. Each line is a real fault, "
                    "checked against the same types the app loads:\n\n"
                    f"{problems_text}\n\n"
                    "Emit the corrected cast in full. Keep everything that was not "
                    "faulted."
                ),
            },
        ]

    return 1


if __name__ == "__main__":
    sys.exit(main())
