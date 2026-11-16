#!/usr/bin/env python3
"""Build the XR app's NOTICE (board #4198): what the APK ships, the license
each part is under, and the full MIT and Apache-2.0 texts once each.

    scripts/xr/notice.py           write assets/xr/NOTICE
    scripts/xr/notice.py --check   exit 1 when assets/xr/NOTICE is stale

The NOTICE is committed, and the Android build copies it into the APK
beside the other staged assets, as assets/assets/NOTICE
(android/app/build.gradle.kts). `run.sh build`, `run.sh release` and
`run.sh all` run the check first, so an APK never ships a stale one.

What goes in, in order:

  * assets/xr/licenses/components.txt, by hand: Fosfora, whisper.cpp, the
    Whisper weights, the voice model, ONNX Runtime, the OpenXR loader,
    libc++, the bundled tracks and the test clip, with their copyright
    lines.
  * The Rust crates linked into libfosfora_xr.so for the headset:
    `cargo tree -p fosfora-xr --target aarch64-linux-android
    -e normal,no-proc-macro`, which resolves features for that crate alone
    as `cargo ndk build -p fosfora-xr` does, and leaves out build scripts,
    proc macros and dev dependencies, none of which is linked. Each
    crate's license is its manifest's SPDX expression (`cargo metadata`).
    The workspace's own crates are Fosfora and are not listed again.
  * The copyright lines in the license files of every crate that may be
    used under MIT, which that license asks to keep with the software.
  * The license files, verbatim, of every crate whose expression MIT or
    Apache-2.0 alone does not satisfy (MPL-2.0, ISC, BSD-3-Clause, the
    fonts' OFL and UFL, ...), each distinct text once with the crates
    that carry it, but for their LICENSE-MIT and LICENSE-APACHE, and
    every crate's NOTICE file.
  * The MIT text (LICENSE-MIT without its copyright line) and the Apache
    License 2.0 (LICENSE-APACHE).

Both cargo commands run `--locked --offline`: they read Cargo.lock and the
crate sources cargo already has, so nothing is fetched. After a dependency
changes, build once (`run.sh build` or `run.sh lint` downloads the
sources) and run this script.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "assets" / "xr" / "NOTICE"
COMPONENTS = ROOT / "assets" / "xr" / "licenses" / "components.txt"
TARGET = "aarch64-linux-android"
PACKAGE = "fosfora-xr"
# Licenses whose text this NOTICE carries in full.
CARRIED = {"MIT", "Apache-2.0"}
# A crate's license files: in its root, these names.
LICENSE_NAME = re.compile(r"^(LICEN[CS]E|COPYING|UNLICENSE|NOTICE)", re.IGNORECASE)
# License files outside a crate's root, by crate name: egui's default fonts
# keep each font's license beside it.
EXTRA_FILES = {
    "epaint_default_fonts": [
        "fonts/OFL.txt",
        "fonts/UFL.txt",
        "fonts/Hack-Regular.txt",
        "fonts/emoji-icon-font-mit-license.txt",
    ],
}
# A line that names a copyright holder; a template's placeholder is not one.
COPYRIGHT = re.compile(r"^\s*(Copyright\b|©)")
PLACEHOLDER = re.compile(r"\[yyyy\]|\{yyyy\}|<year>|\[name of|\{name of|<name of|<owner>")
RULE = "=" * 72


def cargo(*args: str) -> str:
    """Run cargo offline and locked in the repo's root; its stdout."""
    try:
        return subprocess.run(
            ["cargo", *args, "--locked", "--offline"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    except subprocess.CalledProcessError as e:
        sys.exit(
            f"notice: cargo {' '.join(args[:2])} failed (build once so cargo has the "
            f"Android crates' sources):\n{e.stderr}"
        )


def linked() -> list[tuple[str, str]]:
    """The crates linked into the XR library: (name, version), sorted."""
    out = cargo(
        "tree",
        "-p",
        PACKAGE,
        "--target",
        TARGET,
        "-e",
        "normal,no-proc-macro",
        "--prefix",
        "none",
        "-f",
        "{p}",
    )
    crates = set()
    for line in out.splitlines():
        line = line.removesuffix(" (*)").strip()
        if not line:
            continue
        name, version, *source = line.split(" ", 2)
        if source:
            # A path dependency: one of the workspace's own crates.
            if not source[0].startswith(f"({ROOT}"):
                sys.exit(f"notice: {line}: not a registry crate nor the workspace's")
            continue
        crates.add((name, version.removeprefix("v")))
    return sorted(crates)


def packages() -> dict[tuple[str, str], dict]:
    """Every package `cargo metadata` knows for the target, by (name, version)."""
    meta = json.loads(
        cargo("metadata", "--format-version", "1", "--filter-platform", TARGET)
    )
    return {(p["name"], p["version"]): p for p in meta["packages"]}


def spdx_tokens(expr: str) -> list[str]:
    return re.findall(r"\(|\)|[A-Za-z0-9.+\-]+", expr.replace("/", " OR "))


def satisfied(expr: str, allowed: set[str]) -> bool:
    """Whether the SPDX `expr` can be met with only the `allowed` licenses
    (an exception after WITH only adds permissions, so it is ignored)."""
    toks = spdx_tokens(expr)
    pos = 0

    def peek() -> str | None:
        return toks[pos] if pos < len(toks) else None

    def take() -> str:
        nonlocal pos
        pos += 1
        return toks[pos - 1]

    def leaf() -> bool:
        t = take()
        if t == "(":
            v = any_of()
            take()
            return v
        if peek() == "WITH":
            take()
            take()
        return t in allowed

    def all_of() -> bool:
        v = leaf()
        while peek() == "AND":
            take()
            v = leaf() and v
        return v

    def any_of() -> bool:
        v = all_of()
        while peek() == "OR":
            take()
            v = all_of() or v
        return v

    return any_of()


def licenses_in(expr: str) -> set[str]:
    return {t for t in spdx_tokens(expr) if t not in {"(", ")", "AND", "OR", "WITH"}}


def text_of(path: Path) -> str:
    """A text file as the NOTICE carries it: LF line ends, no trailing blanks."""
    raw = path.read_bytes().decode("utf-8", errors="replace")
    lines = [l.rstrip() for l in raw.replace("\r\n", "\n").replace("\r", "\n").split("\n")]
    return "\n".join(lines).strip("\n")


def license_files(pkg: dict) -> list[Path]:
    root = Path(pkg["manifest_path"]).parent
    files = sorted(
        (f for f in root.iterdir() if f.is_file() and LICENSE_NAME.match(f.name)),
        key=lambda f: f.name,
    )
    files += [root / f for f in EXTRA_FILES.get(pkg["name"], []) if (root / f).is_file()]
    if pkg.get("license_file"):
        named = root / pkg["license_file"]
        if named.is_file() and named not in files:
            files.append(named)
    return files


def copyright_lines(files: list[Path]) -> list[str]:
    seen: list[str] = []
    for f in files:
        for line in text_of(f).split("\n"):
            line = line.strip()
            if COPYRIGHT.match(line) and not PLACEHOLDER.search(line) and line not in seen:
                seen.append(line)
    return seen


def section(title: str) -> str:
    return f"{title}\n{'-' * len(title)}"


def build() -> str:
    crates = linked()
    pkgs = packages()
    missing = [c for c in crates if c not in pkgs]
    if missing:
        sys.exit(f"notice: no metadata for {missing}")
    out: list[str] = []
    title = "Fosfora XR: notices and licenses"
    out.append(f"{title}\n{'=' * len(title)}")
    out.append(
        "Fosfora is under the MIT license or the Apache License 2.0, at your\n"
        "option. This app ships with the components below, each under its own\n"
        "license. The full texts of the MIT license and the Apache License 2.0\n"
        "are at the end; each MIT component's copyright line is with it.\n"
        "This file is made by scripts/xr/notice.py from the files in the\n"
        "repository; do not edit it by hand."
    )
    out.append(text_of(COMPONENTS))

    # The crates.
    width = max(len(f"{n} {v}") for n, v in crates)
    rows = [f"{f'{n} {v}':<{width}}  {pkgs[(n, v)]['license']}" for n, v in crates]
    out.append(
        section(f"Rust crates in the app's library ({len(crates)})")
        + "\n\n"
        + "The crates linked into libfosfora_xr.so, each with the license its\n"
        + "manifest declares (SPDX). The source of each is at\n"
        + "https://crates.io/crates/<name>/<version>.\n\n"
        + "\n".join(rows)
    )

    # Their copyright lines, for every crate MIT may cover.
    blocks = []
    for n, v in crates:
        pkg = pkgs[(n, v)]
        if "MIT" not in licenses_in(pkg["license"]):
            continue
        lines = copyright_lines(license_files(pkg))
        if not lines:
            authors = ", ".join(a.split(" <")[0] for a in pkg.get("authors") or [])
            lines = [
                "No copyright line in its license files"
                + (f"; authors: {authors}" if authors else "")
            ]
        blocks.append(f"{n} {v}\n" + "\n".join(f"    {l}" for l in lines))
    out.append(
        section("Copyright lines of the crates under MIT")
        + "\n\n"
        + "For every crate above that may be used under the MIT license, the\n"
        + "copyright lines in its license files.\n\n"
        + "\n".join(blocks)
    )

    # License files MIT or Apache-2.0 alone do not cover, and NOTICE files.
    texts: dict[str, list[str]] = {}
    for n, v in crates:
        pkg = pkgs[(n, v)]
        other = not satisfied(pkg["license"], CARRIED)
        for f in license_files(pkg):
            # The MIT and Apache-2.0 texts are carried once, below, and the
            # MIT copyright lines are above.
            carried = re.match(r"LICEN[CS]E[-_.](MIT|APACHE)", f.name, re.IGNORECASE)
            if (other and not carried) or f.name.upper().startswith("NOTICE"):
                rel = f.relative_to(Path(pkg["manifest_path"]).parent).as_posix()
                texts.setdefault(text_of(f), []).append(f"{n} {v} ({rel})")
    parts = []
    for text, owners in texts.items():
        parts.append(f"{RULE}\n" + "\n".join(owners) + f"\n{RULE}\n\n{text}")
    out.append(
        section("License and notice files of the crates")
        + "\n\n"
        + "The license files, verbatim, of the crates whose terms the MIT and\n"
        + "Apache-2.0 texts alone do not cover, and every crate's NOTICE file.\n"
        + "A text several crates carry is here once, under their names.\n\n"
        + "\n\n".join(parts)
    )

    mit = [l for l in text_of(ROOT / "LICENSE-MIT").split("\n") if not COPYRIGHT.match(l)]
    mit_text = re.sub(r"\n{3,}", "\n\n", "\n".join(mit))
    out.append(section("The MIT license") + "\n\n" + mit_text)
    out.append(section("The Apache License 2.0") + "\n\n" + text_of(ROOT / "LICENSE-APACHE"))
    return "\n\n\n".join(out) + "\n"


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument(
        "--check",
        action="store_true",
        help="exit 1 when assets/xr/NOTICE is not what this script would write",
    )
    args = ap.parse_args()
    text = build()
    rel = OUT.relative_to(ROOT)
    if args.check:
        if not OUT.is_file() or OUT.read_text(encoding="utf-8") != text:
            sys.exit(
                f"notice: {rel} is stale against Cargo.lock or its sources; "
                "run scripts/xr/notice.py and commit it"
            )
        print(f"notice: {rel} is current")
        return
    OUT.write_text(text, encoding="utf-8")
    print(f"notice: wrote {rel} ({len(text.encode())} bytes)")


if __name__ == "__main__":
    main()
