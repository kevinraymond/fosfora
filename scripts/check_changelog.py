#!/usr/bin/env python3
"""Guard the CHANGELOG headers the release workflow depends on.

`release.yml` builds the GitHub release body by extracting the section between
`## vX.Y.Z` headers with awk. If the header for the version being tagged is not there —
because it still reads `## Unreleased` — awk matches nothing and the release publishes
with only the static footer. That has now happened twice, for v1.11.0 and again for
v1.13.0, where the already-published header had been reverted to `## Unreleased` with new
entries stacked on top of it.

Checks:
  * the version in crates/fosfora-app/Cargo.toml has a `## vX.Y.Z — date` header with a
    non-empty body, unless that version is already tagged (i.e. already released)
  * every released version still has its header, and still has a body
  * `## Unreleased` appears at most once, and above every released section
  * entries not yet shipped (`## Unreleased`, and the section for the Cargo.toml version
    if it is not tagged yet) keep to the CLAUDE.md rules a machine can check: at most
    ~600 characters each, no `#123` issue references, no source file paths

Released sections' PROSE is deliberately not checked. The v1.9.0–v1.12.0 entries were
rewritten in place during the docs wave to match the house style, long after they
shipped; that is an edit we want to keep making, and old entries that predate the rules
should not block a release. Only the header going missing is always a bug there.

Usage:  scripts/check_changelog.py
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CHANGELOG = REPO / "CHANGELOG.md"
CARGO = REPO / "crates" / "fosfora-app" / "Cargo.toml"

# Versions below this are not checked: the 0.3.x sections were pruned from the changelog
# on purpose and are not coming back, so their tags would report a missing header forever.
FLOOR = (1, 0, 0)

# `## v1.15.0 — 2026-07-23`. The separator is an em dash in every shipped header, but the
# awk in release.yml only requires whitespace after the version, so accept any tail and let
# the date check below be the strict one.
HEADER = re.compile(r"^## v(\d+\.\d+\.\d+)\s*(.*)$", re.M)
DATED = re.compile(r"^[—-]\s*\d{4}-\d{2}-\d{2}\s*$")

# Entry rules for sections not yet shipped (see CLAUDE.md, "Writing CHANGELOG entries").
MAX_ENTRY_CHARS = 600
# `#1796` — an internal issue number a release-notes reader cannot open.
ISSUE_REF = re.compile(r"(?<![\w&/])#\d+\b")
# `scripts/audit_pfx_rates.py`, `app.rs` — source files are implementation detail. User-facing
# names (`u.time`, `.pfx`, `/fosfora/...` OSC addresses) do not match: they carry no source
# extension, or no name in front of it.
SOURCE_PATH = re.compile(r"\b[\w./-]*\w\.(?:rs|py|sh|wgsl|toml|ya?ml|js|ts)\b")


def parse_version(text: str) -> tuple[int, ...]:
    return tuple(int(p) for p in text.split("."))


def cargo_version() -> str:
    m = re.search(r'^version\s*=\s*"([^"]+)"', CARGO.read_text(), re.M)
    if not m:
        raise SystemExit(f"no version field in {CARGO}")
    return m.group(1)


def git(*args: str) -> str | None:
    """Run a git command, returning None if it fails (missing tag, shallow clone, no git)."""
    try:
        out = subprocess.run(
            ["git", "-C", str(REPO), *args], capture_output=True, text=True, check=True
        )
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None
    return out.stdout


def sections(text: str) -> dict[str, str]:
    """Map version -> section body, exactly as release.yml's awk would slice it."""
    out: dict[str, str] = {}
    matches = list(HEADER.finditer(text))
    for i, m in enumerate(matches):
        end = matches[i + 1].start() if i + 1 < len(matches) else len(text)
        out[m.group(1)] = text[m.end() : end]
    return out


def entries(body: str) -> list[str]:
    """A section's entries: each `- ` bullet with its indented continuation lines, and each
    free paragraph. `###` headings and HTML (comments, image tables) are not entries."""
    out: list[str] = []
    current: list[str] = []

    def flush() -> None:
        if current:
            out.append(" ".join(current))
            current.clear()

    for line in body.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith(("#", "<")):
            flush()
        elif stripped.startswith("- ") and not line.startswith(" "):
            flush()
            current.append(stripped[2:])
        else:
            current.append(stripped)
    flush()
    return out


def entry_problems(label: str, body: str) -> list[str]:
    """CLAUDE.md changelog rules a script can check, for one unreleased section."""
    problems: list[str] = []
    for entry in entries(body):
        head = entry[:60] + ("…" if len(entry) > 60 else "")
        if len(entry) > MAX_ENTRY_CHARS:
            problems.append(
                f"{label}: entry is {len(entry)} chars (ceiling ~{MAX_ENTRY_CHARS}) — {head!r}"
            )
        if refs := ISSUE_REF.findall(entry):
            problems.append(
                f"{label}: issue reference {', '.join(refs)} — readers cannot open it; "
                f"put it in the commit message — {head!r}"
            )
        if paths := SOURCE_PATH.findall(entry):
            problems.append(
                f"{label}: source path {', '.join(paths)} — implementation detail, "
                f"not release notes — {head!r}"
            )
    return problems


def unreleased_body(text: str) -> str | None:
    """The `## Unreleased` section body, up to the next `## ` header."""
    m = re.search(r"^## Unreleased\s*$", text, re.M)
    if not m:
        return None
    nxt = re.search(r"^## ", text[m.end() :], re.M)
    return text[m.end() : m.end() + nxt.start()] if nxt else text[m.end() :]


def released_versions() -> tuple[list[str], list[str]]:
    """Tagged versions at or above FLOOR whose own tagged changelog carried their header.

    Returns (checkable, skipped) — skipped are tags git cannot read here, which happens in
    a shallow clone or when a local clone has not fetched the tag the CI runner created.
    """
    tags = git("tag", "--list", "v*")
    if tags is None:
        return [], []

    checkable, skipped = [], []
    for tag in tags.split():
        version = tag[1:]
        try:
            if parse_version(version) < FLOOR:
                continue
        except ValueError:
            continue  # not a plain vX.Y.Z tag
        tagged = git("show", f"{tag}:CHANGELOG.md")
        if tagged is None:
            skipped.append(tag)
        elif version in sections(tagged):
            checkable.append(version)
    return checkable, skipped


def main() -> int:
    text = CHANGELOG.read_text()
    found = sections(text)
    errors: list[str] = []

    # --- the version about to be released -------------------------------------------
    version = cargo_version()
    already_tagged = git("rev-parse", "--verify", f"refs/tags/v{version}") is not None
    if not already_tagged:
        if version not in found:
            errors.append(
                f"Cargo.toml is at {version} but CHANGELOG.md has no '## v{version}' header — "
                f"release.yml would publish an empty body. Re-header '## Unreleased' as "
                f"'## v{version} — <date>'."
            )
        elif not found[version].strip():
            errors.append(f"'## v{version}' has an empty body — the release notes would be blank")
        else:
            errors.extend(entry_problems(f"v{version}", found[version]))

    # --- headers that shipped must stay ----------------------------------------------
    released, skipped = released_versions()
    for version in released:
        if version not in found:
            errors.append(
                f"v{version} was released with a '## v{version}' section, which is now gone — "
                f"if it was re-headered as '## Unreleased', restore it and move new entries above"
            )
        elif not found[version].strip():
            errors.append(f"'## v{version}' shipped with entries and is now empty")

    # --- entries about to ship --------------------------------------------------------
    if (body := unreleased_body(text)) is not None:
        errors.extend(entry_problems("Unreleased", body))

    # --- structure --------------------------------------------------------------------
    unreleased = [m.start() for m in re.finditer(r"^## Unreleased\s*$", text, re.M)]
    if len(unreleased) > 1:
        errors.append(f"{len(unreleased)} '## Unreleased' headers — there must be at most one")
    if unreleased and (first := HEADER.search(text)) and first.start() < unreleased[0]:
        errors.append("'## Unreleased' must sit above every released section, not below")

    for m in HEADER.finditer(text):
        version, tail = m.group(1), m.group(2)
        if parse_version(version) >= FLOOR and not DATED.match(tail.strip()):
            errors.append(f"'## v{version}' is missing its '— YYYY-MM-DD' date")

    if errors:
        print(f"{len(errors)} problem(s):\n", file=sys.stderr)
        for e in errors:
            print(f"  {e}", file=sys.stderr)
        return 1

    note = f", {len(skipped)} tag(s) unreadable here and skipped" if skipped else ""
    print(f"CHANGELOG OK — {len(found)} sections, {len(released)} released headers intact{note}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
