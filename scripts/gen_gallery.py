#!/usr/bin/env python3
"""Generate docs/GALLERY.md from the .pfx files themselves.

The old README claimed 24 effects when there were 38, because the catalogue was maintained by
hand in three different files and quietly fell behind every release. Generating it removes the
opportunity: run this after adding an effect and the gallery is correct by construction.

Effects are grouped the way a user would sort them rather than the way the loader does —
`effect_type` marks several particle simulations as "feedback" because that drives a UI badge,
which is not a useful distinction when you are just browsing for a look.

Usage:
    scripts/gen_gallery.py                    # write docs/GALLERY.md
    scripts/gen_gallery.py --check            # verify it is up to date (exit 1 if not)
    scripts/gen_gallery.py --local            # point tiles at assets/media/tiles instead
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
EFFECTS = REPO / "assets" / "effects"
OUT = REPO / "docs" / "GALLERY.md"

REMOTE = "https://github.com/kevinraymond/fosfora/releases/download/demo-assets"
LOCAL = "../assets/media/tiles"

GROUPS = [
    ("Shaders", "shader",
     "Pure fragment shaders — no particles, just math evaluated per pixel, every frame."),
    ("Particle simulations", "particle",
     "GPU compute simulations, from a few thousand particles up to two million."),
    ("Lattice", "lattice",
     "One engine, eight rules: a 3D cellular automaton evolving inside a volume and rendered "
     "by ray marching. Bass drives the evolution; the grid freezes in silence."),
]

HEADER = """# Effect Gallery

Every built-in effect, at **default settings** — no tweaking, no hand-picked parameters. Each
clip is two bars of the same synthesized loop, so they are all reacting to the identical
passage: a build, a moment of near silence, and a drop.

Effects respond to what they hear, so what you see here is a starting point, not a fixed look.
Change the music and they change with it; every parameter is also a slider, and can be driven
by MIDI, OSC, a phone or a webcam.

"""

FOOTER = """
---

Clips were captured from the running app by [`scripts/capture/`](../scripts/capture), which
drives real playback through the production render pipeline — post-processing and all — rather
than an offscreen approximation.

Two effects are not shown because they are hidden from the browser: **Phosphor**, the signature
intro visual you see at startup, and **Stress**, a ten-million-particle rasterizer benchmark.

[← Back to the README](../README.md)
"""


def classify(path: Path, data: dict) -> str:
    if path.stem.startswith("lattice_"):
        return "lattice"
    # Group by what the effect IS, not by `effect_type` — that field marks Accretion, Array,
    # Chaos, Mycelium, Polycephalum and Turing as "feedback" to pick a UI badge colour, even
    # though all six are particle simulations.
    return "particle" if data.get("particles") else "shader"


def load() -> list[tuple[Path, dict]]:
    out = []
    for p in sorted(EFFECTS.glob("*.pfx"), key=lambda p: p.name):
        data = json.loads(p.read_text())
        if not data.get("hidden"):
            out.append((p, data))
    return out


def caption(text: str) -> str:
    """A .pfx description, whitespace-normalised and trimmed to fit under a tile.

    Deliberately NOT clever about picking a "first sentence". These descriptions put the
    interesting half on either side of the em-dash depending on the effect — Aurora's payoff
    is after it ("a spectrogram disguised as northern lights"), Iris's is before it ("spinning
    dot with fading feedback trails") — so any rule that keeps one side throws away the good
    line for half the catalogue.
    """
    text = " ".join(text.split())
    # Drop the trailing how-to sentence a few effects carry; it is instructions, not a caption.
    for lead in ("Enable Obstacle", "Load a .ply", "Drag the emitter"):
        if lead in text:
            text = text.split(lead)[0].rstrip(" .—-")
            break
    if len(text) > 165:
        text = text[:162].rsplit(" ", 1)[0] + "…"
    return text


TRAMA = [
    ("trama_pip", "Picture-in-picture", "Astrolabe shrunk into a corner with a Transform node, over Murmur."),
    ("trama_pump", "Pump to the kick", "Prism's scale driven by the bass; Drift shows around it."),
    ("trama_spin", "One layer spinning", "Reticle on a beat-synced saw, over Symbiosis while the rest holds still."),
    ("trama_twin", "One effect, twice", "Two Astrolabes placed left and right by hand, over Polycephalum."),
    ("trama_echo", "Echo trails", "A Feedback loop through Mix and a Transform a whisker off identity, on Beam."),
    ("trama_master", "Master chain", "An echo loop into a slow hue drift, on the whole composited frame."),
]


def render(base: str) -> str:
    effects = load()
    parts = [HEADER]
    total = 0

    for title, key, blurb in GROUPS:
        rows = [(p, d) for p, d in effects if classify(p, d) == key]
        total += len(rows)
        parts.append(f"## {title} ({len(rows)})\n\n{blurb}\n\n<table>\n")
        for i in range(0, len(rows), 3):
            chunk = rows[i : i + 3]
            parts.append("<tr>\n")
            for p, d in chunk:
                name = d["name"]
                parts.append(
                    f'<td width="33%"><img src="{base}/{p.stem}.webp" width="100%" '
                    f'alt="{name}"><br><b>{name}</b><br>'
                    f"<sub>{caption(d.get('description', ''))}</sub></td>\n"
                )
            for _ in range(3 - len(chunk)):
                parts.append('<td width="33%"></td>\n')
            parts.append("</tr>\n")
        parts.append("</table>\n\n")

    # trama chains are not effects, so they are not discovered from .pfx files: these
    # are the capture demos in scripts/capture/demos (Trama *.json), filmed by
    # capture_advanced.sh. Kept here so the generated file and its --check agree.
    parts.append(
        "## trama: node chains (6)\n\n"
        "Not effects but what you can do *to* them: a chain of nodes post-processing one "
        "layer, or the whole frame. Recipes for each are in the "
        "[trama chapter of the tutorial](TUTORIALS.md#trama-node-chains).\n\n<table>\n"
    )
    for i in range(0, len(TRAMA), 3):
        parts.append("<tr>\n")
        for slug, name, text in TRAMA[i : i + 3]:
            parts.append(
                f'<td width="33%"><img src="{base}/{slug}.webp" width="100%" '
                f'alt="{name}"><br><b>{name}</b><br><sub>{text}</sub></td>\n'
            )
        parts.append("</tr>\n")
    parts.append("</table>\n\n")

    assert total == len(effects), f"grouping lost effects: {total} != {len(effects)}"
    parts.insert(1, f"**{len(effects)} effects.**\n\n")
    return "".join(parts) + FOOTER


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true", help="verify without writing")
    ap.add_argument("--local", action="store_true", help="reference assets/media/tiles")
    args = ap.parse_args()

    text = render(LOCAL if args.local else REMOTE)

    if args.check:
        if not OUT.exists() or OUT.read_text() != text:
            print(f"{OUT.relative_to(REPO)} is out of date — run scripts/gen_gallery.py", file=sys.stderr)
            return 1
        print(f"{OUT.relative_to(REPO)} is up to date ({len(load())} effects)")
        return 0

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text)
    print(f"wrote {OUT.relative_to(REPO)} — {len(load())} effects")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
