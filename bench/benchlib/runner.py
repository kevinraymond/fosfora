"""Locate/build the fosfora binary and produce cached --signal-dump JSONL.

Cache key = sha256(audio) + sha256(binary) + the dump flags, so re-scoring
never re-runs analysis, and *any* rebuild (dirty tree, toolchain bump)
invalidates honestly — the binary's bytes are the version.
"""

from __future__ import annotations

import hashlib
import os
import subprocess
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path


class RunnerError(RuntimeError):
    pass


def repo_root(start: Path | None = None) -> Path:
    p = (start or Path(__file__)).resolve()
    for parent in [p, *p.parents]:
        if (parent / "Cargo.toml").is_file():
            return parent
    raise RunnerError("no Cargo.toml above bench/ — not inside the repo?")


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def resolve_binary(root: Path | None = None) -> Path:
    """FOSFORA_BIN env var wins; else target/release/fosfora; else build it."""
    env = os.environ.get("FOSFORA_BIN")
    if env:
        p = Path(env).resolve()
        if not p.is_file():
            raise RunnerError(f"FOSFORA_BIN={env} does not exist")
        return p
    root = root or repo_root()
    release = root / "target" / "release" / "fosfora"
    if release.is_file():
        return release
    print("bench: building fosfora (release, --features analyze) ...", flush=True)
    subprocess.run(
        ["cargo", "build", "-p", "fosfora-app", "--features", "analyze", "--release"],
        cwd=root,
        check=True,
    )
    if not release.is_file():
        raise RunnerError(f"build succeeded but {release} is missing")
    return release


def flags_digest(rate: int | None, feat_bus: bool, no_stems: bool) -> str:
    """Canonical short form of the dump flags (defaults: 30 Hz, no bus, stems)."""
    return f"r{rate if rate is not None else 30}_fb{int(feat_bus)}_st{int(not no_stems)}"


def dump_args(rate: int | None, feat_bus: bool, no_stems: bool) -> list[str]:
    args = []
    if rate is not None:
        args += ["--rate", str(rate)]
    if feat_bus:
        args.append("--feat-bus")
    if no_stems:
        args.append("--no-stems")
    return args


class DumpRunner:
    """Hands out cached dump paths, running the binary only on cache miss."""

    def __init__(
        self,
        dumps_dir: Path,
        binary: Path | None = None,
        rate: int | None = None,
        feat_bus: bool = False,
        no_stems: bool = False,
    ):
        self.binary = binary or resolve_binary()
        self.binary_sha256 = sha256_file(self.binary)
        self.dumps_dir = Path(dumps_dir)
        self.rate, self.feat_bus, self.no_stems = rate, feat_bus, no_stems
        self.flags = flags_digest(rate, feat_bus, no_stems)

    def cache_path(self, audio: Path, audio_sha256: str | None = None) -> Path:
        akey = (audio_sha256 or sha256_file(audio))[:16]
        bkey = self.binary_sha256[:16]
        return self.dumps_dir / f"{audio.stem}.{akey}-{bkey}-{self.flags}.jsonl"

    def ensure_dump(self, audio: Path, force: bool = False) -> Path:
        audio = Path(audio)
        out = self.cache_path(audio)
        if out.is_file() and not force:
            return out
        self.dumps_dir.mkdir(parents=True, exist_ok=True)
        # Dump to a .part and rename, so a killed run never leaves a truncated
        # file that a later run would trust.
        part = out.with_suffix(".jsonl.part")
        cmd = [
            str(self.binary),
            "--signal-dump",
            str(audio),
            "--out",
            str(part),
            *dump_args(self.rate, self.feat_bus, self.no_stems),
        ]
        proc = subprocess.run(cmd, capture_output=True, text=True)
        if proc.returncode != 0:
            part.unlink(missing_ok=True)
            raise RunnerError(
                f"--signal-dump failed for {audio.name} "
                f"(exit {proc.returncode}): {proc.stderr.strip()}"
            )
        part.replace(out)
        return out

    def ensure_dumps(self, audios: list[Path], jobs: int = 1, force: bool = False):
        """Parallel ensure_dump over many tracks; returns {audio: path | exception}."""
        results: dict[Path, Path | Exception] = {}
        with ThreadPoolExecutor(max_workers=max(1, jobs)) as pool:
            futures = {
                pool.submit(self.ensure_dump, a, force): a for a in map(Path, audios)
            }
            for fut, audio in futures.items():
                try:
                    results[audio] = fut.result()
                except Exception as e:  # recorded, not fatal — coverage reports it
                    results[audio] = e
        return results
