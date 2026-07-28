#!/usr/bin/env python3
"""Verify that every relative link and image in the Markdown docs actually resolves.

CI skips all `**.md` changes (`paths-ignore` in ci.yml), so nothing catches a broken
cross-reference — which is how the docs reached the state this script was written to clean up.
Run it after moving or renaming anything under docs/.

Checks:
  * relative links point at a file that exists
  * `#anchors` match a real heading in the target file
  * relative <img src=...> and ![](...) targets exist
  * the effect catalogue in docs/GALLERY.md still matches assets/effects/*.pfx
  * (--check-assets) every clip GALLERY.md references exists in the demo-assets release

Remote URLs are not fetched — this is a structural check, not a link-rot check.

Usage:  scripts/check_docs.py [--check-assets]

`--check-assets` is opt-in because it needs `gh` and the network. The release gate
in .github/workflows/tag.yml passes it; the pre-commit hook does not, so committing
still works offline.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SKIP_DIRS = {".git", "target", "node_modules", ".venv", "scratch_lattice", "__pycache__"}

MD_LINK = re.compile(r"(?<!!)\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
MD_IMAGE = re.compile(r"!\[[^\]]*\]\(([^)\s]+)")
HTML_SRC = re.compile(r"<img[^>]*\ssrc=\"([^\"]+)\"", re.I)
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*$", re.M)
# GitHub honours both `id=` and the older `name=` form on explicit anchors.
HTML_ANCHOR = re.compile(r"<a\s+(?:id|name)=\"([^\"]+)\"", re.I)

# Spelled-out counts, so prose like "Six of the thirty-eight" is checked as strictly as
# "**38 built-in**". Only the tens range the catalogue can plausibly sit in — anything
# outside it is prose, not a count, and must not be matched.
_TENS = {"twenty": 20, "thirty": 30, "forty": 40, "fifty": 50, "sixty": 60}
_ONES = {"one": 1, "two": 2, "three": 3, "four": 4, "five": 5, "six": 6, "seven": 7,
         "eight": 8, "nine": 9}  # fmt: skip
NUMBER_WORDS = dict(_TENS)
NUMBER_WORDS.update(
    {f"{t}-{o}": tv + ov for t, tv in _TENS.items() for o, ov in _ONES.items()}
)


def slug(text: str) -> str:
    """GitHub's heading-anchor rule: lowercase, strip punctuation, spaces to hyphens."""
    text = re.sub(r"<[^>]+>", "", text)
    text = re.sub(r"[`*_~]", "", text).strip().lower()
    text = re.sub(r"[^\w\s-]", "", text)
    return re.sub(r"\s+", "-", text)


def anchors(path: Path) -> set[str]:
    text = path.read_text(encoding="utf-8", errors="replace")
    out: set[str] = set()
    seen: dict[str, int] = {}
    for _, title in HEADING.findall(text):
        s = slug(title)
        # GitHub disambiguates repeats as `-1`, `-2`, …
        n = seen.get(s, 0)
        out.add(s if n == 0 else f"{s}-{n}")
        seen[s] = n + 1
    out.update(HTML_ANCHOR.findall(text))
    return out


def md_files() -> list[Path]:
    return sorted(
        p for p in REPO.rglob("*.md")
        if not any(part in SKIP_DIRS for part in p.relative_to(REPO).parts)
    )


def check_links() -> list[str]:
    errors: list[str] = []
    anchor_cache: dict[Path, set[str]] = {}

    for md in md_files():
        text = md.read_text(encoding="utf-8", errors="replace")
        targets = [(m, "link") for m in MD_LINK.findall(text)]
        targets += [(m, "image") for m in MD_IMAGE.findall(text)]
        targets += [(m, "image") for m in HTML_SRC.findall(text)]

        for target, kind in targets:
            if target.startswith(("http://", "https://", "mailto:", "data:")):
                continue
            rel = md.relative_to(REPO)

            if target.startswith("#"):
                path, frag = md, target[1:]
            else:
                head, _, frag = target.partition("#")
                path = (md.parent / head).resolve()
                if not path.exists():
                    errors.append(f"{rel}: {kind} target does not exist -> {target}")
                    continue

            if frag and path.suffix == ".md":
                if path not in anchor_cache:
                    anchor_cache[path] = anchors(path)
                if frag.lower() not in {a.lower() for a in anchor_cache[path]}:
                    errors.append(f"{rel}: anchor not found -> {target}")
    return errors


def check_gallery() -> list[str]:
    gallery = REPO / "docs" / "GALLERY.md"
    if not gallery.exists():
        return ["docs/GALLERY.md is missing"]

    text = gallery.read_text()
    shipped = {}
    for p in sorted((REPO / "assets" / "effects").glob("*.pfx")):
        data = json.loads(p.read_text())
        if not data.get("hidden"):
            shipped[p.stem] = data["name"]

    errors = []
    for stem, name in shipped.items():
        if f">{name}</b>" not in text:
            errors.append(f"docs/GALLERY.md: shipped effect missing from the gallery -> {name}")
        if f"/{stem}.webp" not in text:
            errors.append(f"docs/GALLERY.md: no clip referenced for -> {stem}")

    claimed = re.search(r"\*\*(\d+) effects\.\*\*", text)
    if claimed and int(claimed.group(1)) != len(shipped):
        errors.append(
            f"docs/GALLERY.md claims {claimed.group(1)} effects, {len(shipped)} are shipped"
        )

    # The count that kept going stale, wherever it is asserted in prose.
    for doc in ("README.md", "docs/TUTORIALS.md"):
        p = REPO / doc
        if p.exists():
            body = p.read_text()
            for m in re.finditer(r"\*\*(\d+) built-in", body):
                if int(m.group(1)) != len(shipped):
                    errors.append(f"{doc} claims {m.group(1)} built-in effects, not {len(shipped)}")
            # Bare "40 effects" — no bold, no "built-in" — is still the catalogue count and
            # still goes stale (README lead + gallery link both sat at 40 into v1.18.0). The
            # patterns above sailed past it because it is neither bold nor "built-in".
            for m in re.finditer(r"(\d+) effects\b", body):
                if int(m.group(1)) != len(shipped):
                    errors.append(f"{doc} says '{m.group(1)} effects', {len(shipped)} are shipped")
            # README spells the count out ("Six of the thirty-eight"), which the digit
            # patterns above sail straight past — it sat two releases stale that way.
            for m in re.finditer(r"\bof the ([a-z]+(?:-[a-z]+)?)\b", body):
                claimed = NUMBER_WORDS.get(m.group(1))
                if claimed is not None and claimed != len(shipped):
                    errors.append(
                        f"{doc} says 'of the {m.group(1)}' ({claimed}), {len(shipped)} are shipped"
                    )
    return errors


def check_gallery_assets() -> list[str]:
    """Verify every clip GALLERY.md points at actually exists in the demo-assets release.

    The gallery's tile URLs are generated from the .pfx files, so adding an effect adds
    a reference automatically — but the asset behind it only exists once someone captures
    a clip, builds a tile and uploads it. check_gallery() confirms a clip is REFERENCED;
    nothing confirmed it RESOLVES, so v1.27.0 shipped Pegboard and Etch as broken images
    with every gate green (board #2004).

    One `gh` call for the whole release, not one request per tile.

    Note for whoever fixes a failure here: build_media.sh writes tiles to <out>/tiles/,
    not <out>/, and `gh release upload` reports a wrong path as a bare "no such file or
    directory" that reads like the build broke rather than like a typo.
    """
    gallery = REPO / "docs" / "GALLERY.md"
    if not gallery.exists():
        return ["docs/GALLERY.md is missing"]

    referenced = set(re.findall(r"/download/demo-assets/([^\"')\s]+)", gallery.read_text()))
    if not referenced:
        return ["docs/GALLERY.md references no demo-assets clips at all"]

    try:
        out = subprocess.run(
            ["gh", "release", "view", "demo-assets", "--json", "assets"],
            capture_output=True,
            text=True,
            timeout=60,
            check=True,
        ).stdout
    except FileNotFoundError:
        # Never downgrade to a skip. A silent pass here is the exact failure mode
        # this check exists to remove.
        return ["--check-assets needs the `gh` CLI, which is not on PATH"]
    except subprocess.CalledProcessError as e:
        return [f"gh release view demo-assets failed: {e.stderr.strip() or e}"]
    except subprocess.TimeoutExpired:
        return ["gh release view demo-assets timed out"]

    published = {a["name"] for a in json.loads(out).get("assets", [])}
    missing = sorted(referenced - published)
    return [
        f"docs/GALLERY.md references {m}, which is not in the demo-assets release "
        f"(the tile will render broken)"
        for m in missing
    ]


def main() -> int:
    check_assets = "--check-assets" in sys.argv[1:]
    errors = check_links() + check_gallery()
    if check_assets:
        errors += check_gallery_assets()
    if errors:
        print(f"{len(errors)} problem(s):\n", file=sys.stderr)
        for e in errors:
            print(f"  {e}", file=sys.stderr)
        return 1
    tail = ", every gallery clip is published" if check_assets else ""
    print(
        f"docs OK — {len(md_files())} markdown files, all relative links and anchors "
        f"resolve{tail}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
