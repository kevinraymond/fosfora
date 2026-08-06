#!/usr/bin/env python3
"""Author a Fosfora scene from an offline song analysis (#2027, dev-side tool).

    ./target/release/fosfora --analyze song.mp3 --out song.analysis.json
    ./target/release/fosfora --dump-schema --out capabilities.json
    uv run --with anthropic scripts/generate_scene.py \
        --analysis song.analysis.json --capabilities capabilities.json --out gen/
    ./target/release/fosfora --validate gen/

This is the one-stage generator: analysis in, scene out, no human checkpoint.
The two-stage screenplay pipeline (write_screenplay.py + realize_screenplay.py,
board #2040) supersedes it for real authoring; this stays as the quick baseline.

Nothing here ships to users: it reads two JSON files the `analyze`-gated
subcommands produce and writes preset/bindings/scene JSON for Kevin to edit.

THREE DESIGN DECISIONS, because each replaces a class of failure:

1. The model never writes a `Preset`. Strict JSON Schema requires
   `additionalProperties: false` and cannot express `params: HashMap<String,
   ParamValue>` — arbitrary keys whose values are an *externally* tagged enum
   (`{"Float": 0.5}`). So the model emits a flat intermediate (params as a list
   of {name, type, value}) and scenelib transforms it into the real shapes.

2. Every closed set is an `enum` in the schema, filled in from
   capabilities.json at runtime: effect names, binding sources, blend modes,
   uniform/postfx/particle leaf names, curve types, transitions. Structured
   outputs guarantee the response validates, so a hallucinated effect name or
   dead source key becomes *unrepresentable* rather than something the validator
   catches afterwards.

3. Binding targets are emitted structurally ({kind, layer, param} …) and the
   dotted string is assembled in scenelib. The model never writes
   `param.0.Aurora.speed` by hand, so it cannot typo the grammar — which matters
   more than usual because `parse_target` is infallible and a malformed target
   loads silently inert.

What is left for `--validate` is what a schema cannot know: whether a param name
exists on the effect that layer actually runs, whether a value sits inside that
param's declared range, and whether the cue list stalls. Those come back as
repair feedback (--attempts).
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

from scenelib import (
    cached_system,
    call_model,
    check_variants,
    compact,
    emit,
    field_rules,
    run_validator,
    schema_blocks,
    spreads,
    usage_line,
)


# ----------------------------------------------------------------- the schema
def build_schema(caps: dict[str, Any], all_sources: bool = False) -> dict[str, Any]:
    """The one-stage intermediate: presets plus one cue per section, with the
    model owning hold_secs. See scenelib.schema_blocks for the shared pieces and
    the two structured-output API limits their shape answers to."""
    blocks = schema_blocks(caps, all_sources)

    preset = {
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Preset name; cues reference it, and it becomes the filename.",
            },
            "for_section_label": {
                "type": "string",
                "description": "Which section identity (A, B, C…) this look is for.",
            },
            "rationale": {
                "type": "string",
                "description": (
                    "One or two sentences: which measured features of this section "
                    "drove this choice of effect, params and bindings."
                ),
            },
            "layers": {
                "type": "array",
                "description": f"1..{caps['limits']['max_layers']} layers, bottom-first.",
                "items": blocks["layer"],
            },
            "bindings": {"type": "array", "items": blocks["binding"]},
        },
        "required": ["name", "for_section_label", "rationale", "layers", "bindings"],
        "additionalProperties": False,
    }

    cue = {
        "type": "object",
        "properties": {
            "preset": {"type": "string", "description": "Name of one of the presets above."},
            "label": {"type": "string"},
            "hold_secs": {
                "type": "number",
                "description": (
                    "Seconds to hold. REQUIRED on every cue: under Timer advance a "
                    "cue without it never advances. Use the section's duration."
                ),
            },
            "transition": {"enum": list(caps["enums"]["transitions"])},
            "transition_secs": {"type": "number"},
            "param_overrides": {
                "type": "array",
                "description": (
                    "Per-layer param values for this cue only. This is how one "
                    "preset serves several sections of the same identity at "
                    "different intensities."
                ),
                "items": blocks["override_group"],
            },
        },
        "required": [
            "preset",
            "label",
            "hold_secs",
            "transition",
            "transition_secs",
            "param_overrides",
        ],
        "additionalProperties": False,
    }

    return {
        "type": "object",
        "properties": {
            "scene_name": {"type": "string"},
            "reading": {
                "type": "string",
                "description": (
                    "A short paragraph on how you read this song's structure and "
                    "what visual arc you chose. Written for a human reviewer."
                ),
            },
            "presets": {"type": "array", "items": preset},
            "cues": {
                "type": "array",
                "description": "One cue per section, in order, covering the whole song.",
                "items": cue,
            },
        },
        "required": ["scene_name", "reading", "presets", "cues"],
        "additionalProperties": False,
    }


# ----------------------------------------------------------------- the prompt
SYSTEM_INSTRUCTIONS = """\
You are authoring a scene for Fosfora, an audio-reactive VJ engine, from an \
offline analysis of one song. You will be given the engine's full capabilities \
and the song's measured structure, and you emit presets, audio bindings and a \
timed cue list.

The capabilities document is authoritative and complete. It was dumped by the \
build this scene will run on, so every effect, param, param range and audio \
source in it exists, and anything not in it does not.

How to do this well:

READ THE SONG FIRST. Sections carry a `label` (A, B, C…): sections sharing a \
label are the same musical identity recurring, so they should share a look. \
Author one preset per distinct label, then vary it per cue with \
`param_overrides` where the measured descriptors differ — that is what makes a \
recurring chorus feel like the same chorus, only bigger the third time.

BIND TO FEATURES THAT ACTUALLY MOVE ON THIS SONG. Each section carries \
descriptors, and the request tells you their spread across the song. A feature \
with a wide spread is a real lever; one with a narrow spread is an honest \
ordering of an inaudible difference and will look like nothing. Prefer the wide \
ones. `energy_rank` is a rank across sections, useful for choosing per-cue \
intensity, not as a binding source.

BINDINGS ARE THE POINT. A scene with no bindings is a slideshow. Every preset \
should have several, and they should be legible: the thing the ear notices and \
the thing the eye notices should be the same thing. Bus values arrive \
normalized 0..1 and are scaled into each param's declared range, so bind to \
what should move, and use transforms to shape the response rather than to \
rescale it.

PREFER FEWER, BETTER LAYERS. Two or three layers that read clearly beat eight \
that fight. Blend modes matter: `Add` and `Screen` accumulate light, `Multiply` \
darkens, and the last three modes (Displace, Refract, Lens) make a layer warp \
what is beneath it rather than draw itself.

COVER THE WHOLE SONG. One cue per section, in order, with `hold_secs` set from \
that section's duration so the cue list tracks the music. Every cue needs \
`hold_secs`.

Explain your reasoning in `reading` and in each preset's `rationale`, in terms \
of the measured numbers you were given. A reviewer will read those to decide \
whether to keep the scene.
"""


def build_system(caps: dict[str, Any]) -> list[dict[str, Any]]:
    return cached_system([
        SYSTEM_INSTRUCTIONS + "\n" + field_rules() + "\n",
        "Engine capabilities (authoritative):\n" + compact(caps),
    ])


def build_user(analysis: dict[str, Any], style: str | None) -> str:
    g, src = analysis["global"], analysis["source"]
    parts = [
        f"Song: {Path(src['path']).name}",
        f"{src['duration_secs']:.1f}s, {g['bpm']:.1f} BPM, "
        f"key class {g['key_class']} ({'minor' if g['key_is_minor'] else 'major'}, "
        f"agreement {g['key_agreement']:.2f}), "
        f"{len(analysis['sections'])} sections in {g['cluster_count']} identities.",
        "",
        "Descriptor spread across sections — wide spreads are the levers that will read:",
        spreads(analysis),
        "",
        "Full analysis:",
        compact(analysis),
    ]
    if style:
        parts += ["", f"Direction from the operator: {style}"]
    parts += [
        "",
        "Author the scene: one preset per section identity, bindings on each, and "
        "one cue per section in order covering the whole song.",
    ]
    return "\n".join(parts)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--analysis", required=True, type=Path)
    ap.add_argument("--capabilities", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--style", help="free-text direction, e.g. 'restrained, cold palette'")
    ap.add_argument(
        "--binary",
        type=Path,
        default=Path("target/release/fosfora"),
        help="fosfora built with --features analyze, for --validate",
    )
    ap.add_argument(
        "--attempts",
        type=int,
        default=3,
        help="validation repair rounds (1 = emit once, do not repair)",
    )
    ap.add_argument(
        "--all-sources",
        action="store_true",
        help=(
            "offer every source key including mel/mfcc/dmfcc coefficients "
            "(may overrun the output-grammar size limit)"
        ),
    )
    ap.add_argument(
        "--dry-run",
        action="store_true",
        help="assemble the prompt, count tokens, and stop without generating",
    )
    args = ap.parse_args()

    analysis = json.loads(args.analysis.read_text())
    caps = json.loads(args.capabilities.read_text())

    if caps.get("analysis_version") != analysis.get("version"):
        print(
            f"warning: capabilities.json expects analysis version "
            f"{caps.get('analysis_version')}, but this analysis is version "
            f"{analysis.get('version')} — regenerate both from the same build",
            file=sys.stderr,
        )

    schema = build_schema(caps, all_sources=args.all_sources)
    system = build_system(caps)
    user = build_user(analysis, args.style)

    import anthropic

    if args.dry_run:
        print(f"effects            {len(caps['effects'])}")
        print(f"sources in schema  {len(caps['sources'])}")
        print(f"sections           {len(analysis['sections'])}")
        print(f"system bytes       {sum(len(b['text']) for b in system):,}")
        print(f"user bytes         {len(user):,}")
        print(f"schema bytes       {len(compact(schema)):,}")
        # count_tokens is itself an API call, so it needs a credential. Report the
        # byte counts regardless — they are the part that tells you whether the
        # prompt is assembled sanely.
        try:
            counted = anthropic.Anthropic().messages.count_tokens(
                model="claude-opus-5",
                system=system,
                messages=[{"role": "user", "content": user}],
            )
            print(f"input tokens       {counted.input_tokens:,}")
        except Exception as e:  # noqa: BLE001 - any auth/network failure is the same story here
            print(f"input tokens       unavailable ({type(e).__name__}: {e})")
        return 0

    client = anthropic.Anthropic()

    messages: list[dict[str, Any]] = [{"role": "user", "content": user}]

    for attempt in range(1, args.attempts + 1):
        print(f"[{attempt}/{args.attempts}] generating…", file=sys.stderr)
        message = call_model(client, system, messages, schema, stream_to_stderr=True)
        print(f"    usage: {usage_line(message)}", file=sys.stderr)

        text = next(b.text for b in message.content if b.type == "text")
        result = json.loads(text)

        # Cheap, local, and precise — catches the one class the schema cannot
        # express before anything touches disk.
        incomplete = check_variants(result)
        if incomplete:
            ok, problems = False, "\n".join(incomplete)
        else:
            emit(result, args.out)
            ok, problems = run_validator(args.binary, args.out)
        if ok:
            written = sorted(args.out.glob("*.json"))
            print(f"\n{result['scene_name']}\n", file=sys.stderr)
            print(result["reading"], file=sys.stderr)
            print(file=sys.stderr)
            for p in result["presets"]:
                print(f"  {p['name']}  (identity {p['for_section_label']})", file=sys.stderr)
                print(f"    {p['rationale']}", file=sys.stderr)
            print(file=sys.stderr)
            for path in written:
                print(path)
            print(f"\nvalidated clean — {len(result['cues'])} cues", file=sys.stderr)
            return 0

        print(f"    validator found problems:\n{problems}", file=sys.stderr)
        if attempt == args.attempts:
            print(
                f"\nstopping after {args.attempts} attempt(s); files in {args.out} "
                f"are the last (invalid) draft",
                file=sys.stderr,
            )
            return 1

        # Repair: keep the cached system prefix, append the rejected draft and
        # what was wrong with it. The corpus never moves, so this reads the cache.
        messages += [
            {"role": "assistant", "content": text},
            {
                "role": "user",
                "content": (
                    "The offline validator rejected that scene. It checks against the "
                    "same types the app loads, so each line is a real fault:\n\n"
                    f"{problems}\n\n"
                    "Emit the corrected scene in full. Keep everything that was not "
                    "faulted."
                ),
            },
        ]

    return 1


if __name__ == "__main__":
    sys.exit(main())
