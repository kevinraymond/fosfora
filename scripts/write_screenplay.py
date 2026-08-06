#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["anthropic"]
# ///
"""Stage A of the screenplay pipeline (#2040): song analysis -> markdown screenplay.

    ./target/release/fosfora --analyze song.mp3 --out song.analysis.json
    uv run scripts/write_screenplay.py --analysis song.analysis.json \
        [--style "cold, restrained, brutalist"]

The output is a screenplay Kevin reads and edits BEFORE anything is realized —
the edit checkpoint is the point of the two-stage design, so this script ends at
the markdown file and prints the next command instead of chaining into it.

The screenwriter knows nothing about the renderer: no effects, no params, no
catalog. It writes pure imagery against the song's measured structure, and the
realizer (realize_screenplay.py) casts that imagery from what the engine can
actually do, reporting what it couldn't reach. Asking for the impossible here is
therefore fine — gaps become the demand-driven effect backlog.

The prose is free; the machine contract is a handful of backticked bracket-lines
(timecodes, per-section signals) whose numbers are normalized from analysis.json
after generation, so the realizer and the conformance checks have hard targets
that survive hand-editing.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

from scenelib import (
    PACE_WORDS,
    analysis_digest,
    cached_system,
    call_model,
    canonical_section_line,
    canonical_signals_line,
    canonical_song_line,
    check_screenplay,
    normalize_screenplay,
    parse_screenplay,
    spreads,
    usage_line,
)

# The screenplay is prose plus a small bracket-line grammar; 16K is several
# times a long screenplay, and it is an output cap, not a target.
MAX_TOKENS = 16000

FORMAT_SPEC = """\
THE FORMAT. Markdown. Title the screenplay, then one song line:

`[song | 260.5s | 128.6 BPM | key A minor | 9 sections / 3 identities | drops: 96.0s]`

Group scenes into acts with `##` headings named for what the act *is* — act
structure is yours. Every analyzed section becomes exactly one scene, in order,
under a `###` heading you title. A scene opens with its two data lines, copied
faithfully from the analysis (they are re-checked against it):

`[section 5 | 118.6s – 152.3s | 33.7s | identity B | energy 0.82, rank 8/9 | drops: 119.0s]`
`[signals: rms 0.71 · centroid 0.44 · percussive 0.63 · onsets 3.4/s · harmonic 0.32 · width 0.25 · buildup 0.12]`

Then the scene's prose: 2–6 sentences of pure imagery.

A scene may be subdivided into **beats** — distinct visual movements within the
section. Beats must tile the section exactly (first starts at section start,
last ends at section end, contiguous), each at least 2 seconds, ids numbered
`<section><letter>`:

**Beats**
- **5a (118.6 – 135.0) — Ignition.** One hard cut; molten light pouring upward, flashing white on every kick.
- **5b (135.0 – 152.3) — Full burn.** The flashing recedes; the texture of the heat becomes the subject.

Every scene closes with one Direction line. `pace` MUST be one of:
still (near-frozen) · drifting (slow continuous motion) · pulsing (motion
arrives in waves with the music) · driving (constant insistent motion) ·
frantic (strobing chaos). The other fields are free text:

**Direction** — pace: driving · light: bright, flashing on the kick · palette: molten orange over carbon black · stillness: none

A worked example scene:

## Act II — The Furnace

### Scene 5 — The Second Ascent

`[section 5 | 118.6s – 152.3s | 33.7s | identity B | energy 0.82, rank 8/9 | drops: 119.0s]`
`[signals: rms 0.71 · centroid 0.44 · percussive 0.63 · onsets 3.4/s · harmonic 0.32 · width 0.25 · buildup 0.12]`

The chorus returns, but it arrives already burning — no wind-up this time, the
drop lands half a second in and the kick never lets go. Where the first chorus
was a room filling with light, this one is the light source itself: a furnace
door swinging open, sparks climbing, everything orange-on-black. The hi-hats
are a shower of filings; the bass is the slow breathing of something enormous
underneath.

**Beats**
- **5a (118.6 – 135.0) — Ignition.** The drop at 119.0 blows the doors. One hard visual cut, then molten light pouring upward, flashing white on every kick.
- **5b (135.0 – 152.3) — Full burn.** Same furnace, but the eye adapts: the flashing recedes, slow convection and spiraling embers become the subject, the frame almost too bright to hold.

**Direction** — pace: driving · light: bright, flashing on the kick · palette: molten orange over carbon black · stillness: none
"""

SYSTEM_INSTRUCTIONS = """\
You are the screenwriter for an abstract-visuals film cut to one song. You will \
be given the song's measured structure — sections, timecodes, energy, audio \
descriptors, drops — and you write the story of the song as pure imagery: \
light, motion, material, palette, space.

You know nothing about the renderer. Do not name visual effects, parameters, \
or software; a separate craft department decides what is achievable. Anything \
imaginable is writable.

THE ARC IS THE JOB. Sections sharing an identity letter are the same musical \
material returning: they must rhyme, not repeat — same world, different \
weather, with a trajectory across the song. When one identity dominates the \
song, its scenes are where the story must move most: escalate, strip back, \
invert, pay off. A section longer than ~25 seconds should almost always be \
subdivided into beats with distinct visual ideas.

WRITE TO THE NUMBERS. Every scene's imagery must be traceable to that \
section's data: its energy rank, its drops, which descriptors run hot or cold \
against the rest of the song. The signals line under each heading is the \
evidence the prose answers to. No stock atmosphere; concrete nouns, physical \
verbs. If a drop lands mid-section, the imagery turns on it, and a beat \
boundary there is usually right.

LENGTH DISCIPLINE. 2–6 sentences of prose per scene, 1–2 per beat. The \
screenplay is read by a human editor first; make every sentence earn the read.

""" + FORMAT_SPEC


def build_user(analysis: dict[str, Any], style: str | None) -> str:
    g, src = analysis["global"], analysis["source"]
    drops = analysis["events"]["drops_secs"]
    parts = [
        f"Song: {Path(src['path']).name}",
        canonical_song_line(analysis),
        "",
        "Descriptor spread across sections — wide spreads are where the song "
        "actually moves; hang the story on those:",
        spreads(analysis),
        "",
        f"Detected drops: {', '.join(f'{d:.1f}s' for d in drops) if drops else 'none'}",
        "",
        "Analysis (sections in order; copy each scene's data lines from this):",
    ]
    for s in analysis["sections"]:
        parts.append(canonical_section_line(s, analysis))
        parts.append(canonical_signals_line(s))
    parts += ["", "Full digest:", analysis_digest(analysis)]
    if style:
        parts += ["", f"Direction from the operator, binding on tone and imagery: {style}"]
    parts += [
        "",
        f"Write the screenplay: every one of the {len(analysis['sections'])} "
        "sections as a scene, in order, acts of your choosing, beats where the "
        "music asks for them, one Direction line per scene.",
    ]
    return "\n".join(parts)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--analysis", required=True, type=Path)
    ap.add_argument(
        "--out",
        type=Path,
        help="output path (default: <analysis stem>.screenplay.md beside the analysis)",
    )
    ap.add_argument("--style", help="vibe steering, e.g. 'dark tones' or 'flower field'")
    ap.add_argument(
        "--attempts",
        type=int,
        default=3,
        help="structural-check repair rounds (1 = generate once, do not repair)",
    )
    ap.add_argument(
        "--dry-run",
        action="store_true",
        help="assemble the prompt, report sizes, and stop without generating",
    )
    args = ap.parse_args()

    analysis = json.loads(args.analysis.read_text())
    out = args.out
    if out is None:
        stem = args.analysis.name
        for suffix in (".analysis.json", ".json"):
            if stem.endswith(suffix):
                stem = stem[: -len(suffix)]
                break
        out = args.analysis.parent / f"{stem}.screenplay.md"

    system = cached_system([SYSTEM_INSTRUCTIONS])
    user = build_user(analysis, args.style)

    if args.dry_run:
        print(f"sections     {len(analysis['sections'])}")
        print(f"system bytes {sum(len(b['text']) for b in system):,}")
        print(f"user bytes   {len(user):,}")
        print(f"out          {out}")
        return 0

    import anthropic

    client = anthropic.Anthropic()
    messages: list[dict[str, Any]] = [{"role": "user", "content": user}]

    for attempt in range(1, args.attempts + 1):
        print(f"[{attempt}/{args.attempts}] writing…", file=sys.stderr)
        message = call_model(
            client, system, messages, schema=None, stream_to_stderr=True,
            max_tokens=MAX_TOKENS,
        )
        print(f"    usage: {usage_line(message)}", file=sys.stderr)

        text = next(b.text for b in message.content if b.type == "text")
        # Models like to fence the whole document; the file should be bare markdown.
        text = text.strip()
        if text.startswith("```"):
            text = text.split("\n", 1)[1].rsplit("```", 1)[0]

        sp = parse_screenplay(text)
        problems, warnings = check_screenplay(sp, analysis)
        for w in warnings:
            print(f"    warning: {w}", file=sys.stderr)
        if not problems:
            final = normalize_screenplay(sp, analysis)
            out.write_text(final)
            beats = sum(len(sc.effective_beats()) for sc in sp.scenes)
            paces = " ".join(
                (sc.direction or {}).get("pace", "?").split()[0] for sc in sp.scenes
            )
            print(f"\n{out}", file=sys.stderr)
            print(
                f"{len(sp.scenes)} scenes, {beats} beats | pace: {paces}",
                file=sys.stderr,
            )
            print(
                "\nRead and edit the screenplay, then realize it:\n"
                f"  uv run scripts/realize_screenplay.py --screenplay {out} "
                f"--analysis {args.analysis} --out gen/",
                file=sys.stderr,
            )
            print(out)
            return 0

        print("    structural check failed:", file=sys.stderr)
        for p in problems:
            print(f"      {p}", file=sys.stderr)
        if attempt == args.attempts:
            draft = out.with_suffix(".draft.md")
            draft.write_text(text if text.endswith("\n") else text + "\n")
            print(
                f"\nstopping after {args.attempts} attempt(s); last draft kept at "
                f"{draft}",
                file=sys.stderr,
            )
            return 1

        messages += [
            {"role": "assistant", "content": text},
            {
                "role": "user",
                "content": (
                    "The structural check rejected that screenplay. Every line "
                    "below is a hard fault against the analysis or the format:\n\n"
                    + "\n".join(problems)
                    + "\n\nRewrite the screenplay in full, keeping every scene, "
                    "beat and sentence that was not faulted. The pace vocabulary "
                    f"is: {', '.join(PACE_WORDS)}."
                ),
            },
        ]

    return 1


if __name__ == "__main__":
    sys.exit(main())
