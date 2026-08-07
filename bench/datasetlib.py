"""Shared machinery for the dataset fetch/prep scripts (workstream C).

Used by every `fetch_<dataset>.py`: manifest loading (`fosfora-bench-manifest/v1`),
resumable downloads with streaming checksums, archive unpacking, pinned git
clones, the local `status.json` observation log, coverage reporting, and the
common fetch|prep|verify CLI scaffold. Stdlib only, so any PEP-723 entry can
import it regardless of its own dependency block.

Exit-code policy (the contract every fetch script follows): per-track failures
are NORMAL — preview URLs rot, YouTube videos vanish — they are recorded in
status.json and the run exits 0 with an "N of M" coverage report. Structural
failures — a pinned checksum mismatch, an annotation source gone — raise
DatasetError and exit nonzero, because continuing would produce numbers built
on unverified inputs.

Checksum policy: manifests pin what is pinnable (stable archives by sha256,
annotation repos by commit, track lists, expected counts). Refetched
YouTube/Beatport audio is not byte-reproducible, so its hashes are recorded in
status.json as *observations*, never manifest *expectations*.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import tarfile
import urllib.error
import urllib.request
import zipfile
from pathlib import Path
from types import SimpleNamespace

MANIFEST_SCHEMA = "fosfora-bench-manifest/v1"
STATUS_SCHEMA = "fosfora-bench-status/v1"
SOURCE_KINDS = {"archive", "git", "file", "per-track", "manual"}

_CHUNK = 1 << 20


class DatasetError(RuntimeError):
    """Structural failure: pinned input can't be verified. Exit nonzero."""


# --------------------------------------------------------------------------- paths


def repo_root() -> Path:
    p = Path(__file__).resolve()
    for parent in [p, *p.parents]:
        if (parent / "Cargo.toml").is_file():
            return parent
    raise DatasetError("no Cargo.toml above bench/ — not inside the repo?")


def dataset_dirs(name: str) -> SimpleNamespace:
    base = repo_root() / "bench" / "datasets" / name
    dirs = SimpleNamespace(
        base=base,
        raw=base / "raw",
        audio=base / "audio",
        norm=base / "norm",
        status=base / "status.json",
    )
    for d in (dirs.raw, dirs.audio, dirs.norm):
        d.mkdir(parents=True, exist_ok=True)
    return dirs


# --------------------------------------------------------------------------- manifest


def load_manifest(name: str) -> dict:
    path = repo_root() / "bench" / "manifests" / f"{name}.json"
    with path.open(encoding="utf-8") as f:
        m = json.load(f)
    if m.get("schema") != MANIFEST_SCHEMA:
        raise DatasetError(f"{path}: schema is {m.get('schema')!r}")
    if m.get("dataset") != name:
        raise DatasetError(f"{path}: dataset is {m.get('dataset')!r}, want {name!r}")
    for s in m.get("sources", []):
        if s.get("kind") not in SOURCE_KINDS:
            raise DatasetError(f"{path}: source {s.get('name')!r} has bad kind")
        if "name" not in s:
            raise DatasetError(f"{path}: source without a name")
    return m


def source(manifest: dict, name: str) -> dict:
    for s in manifest["sources"]:
        if s["name"] == name:
            return s
    raise DatasetError(f"manifest has no source {name!r}")


# --------------------------------------------------------------------------- hashing


def _hash_file(path: Path, algo: str) -> str:
    h = hashlib.new(algo)
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(_CHUNK), b""):
            h.update(chunk)
    return h.hexdigest()


def sha256_file(path: Path) -> str:
    return _hash_file(path, "sha256")


def md5_file(path: Path) -> str:
    return _hash_file(path, "md5")


def check_pins(path: Path, sha256: str | None, md5: str | None) -> dict:
    """Verify a file against optional pins; raise on mismatch. Returns hashes."""
    got = {"sha256": sha256_file(path), "size": path.stat().st_size}
    if md5 is not None:
        got["md5"] = md5_file(path)
        if got["md5"] != md5:
            raise DatasetError(f"{path.name}: md5 {got['md5']} != pinned {md5}")
    if sha256 is not None and got["sha256"] != sha256:
        raise DatasetError(f"{path.name}: sha256 {got['sha256']} != pinned {sha256}")
    return got


# --------------------------------------------------------------------------- fetch


def _download_once(url: str, part: Path, quiet: bool, label: str) -> None:
    offset = part.stat().st_size if part.is_file() else 0
    req = urllib.request.Request(url, headers={"User-Agent": "fosfora-bench/1.0"})
    if offset:
        req.add_header("Range", f"bytes={offset}-")
    with urllib.request.urlopen(req, timeout=60) as resp:
        if offset and resp.status != 206:
            offset = 0  # server ignored Range: restart
        done = offset
        with part.open("ab" if offset else "wb") as f:
            while True:
                chunk = resp.read(_CHUNK)
                if not chunk:
                    break
                f.write(chunk)
                done += len(chunk)
                if not quiet:
                    print(f"\r  {label}: {done >> 20} MiB", end="", flush=True)
        if not quiet:
            print()


def download(
    url: str,
    dest: Path,
    sha256: str | None = None,
    md5: str | None = None,
    quiet: bool = False,
    attempts: int = 8,
) -> Path:
    """Resumable download to dest via dest.part; verifies pins when given.

    An existing dest that passes the pins is a no-op. A .part file resumes
    with an HTTP Range request; mid-stream resets (academic servers under a
    multi-hundred-MB pull) retry with resume up to `attempts` times, but only
    while each attempt makes forward progress.
    """
    if dest.is_file():
        check_pins(dest, sha256, md5)
        return dest
    dest.parent.mkdir(parents=True, exist_ok=True)
    part = dest.with_suffix(dest.suffix + ".part")
    last_err: Exception | None = None
    for attempt in range(attempts):
        before = part.stat().st_size if part.is_file() else 0
        try:
            _download_once(url, part, quiet, dest.name)
            last_err = None
            break
        except (urllib.error.URLError, ConnectionError, TimeoutError, OSError) as e:
            last_err = e
            after = part.stat().st_size if part.is_file() else 0
            if after <= before:
                break  # no forward progress: stop hammering the server
            if not quiet:
                print(f"\n  {dest.name}: interrupted ({e}), resuming "
                      f"({attempt + 1}/{attempts}) ...")
    if last_err is not None:
        raise DatasetError(f"download {url}: {last_err}") from last_err
    check_pins(part, sha256, md5)
    part.replace(dest)
    return dest


def unpack(archive: Path, dest: Path, kind: str | None = None) -> Path:
    dest.mkdir(parents=True, exist_ok=True)
    kind = kind or ("zip" if archive.suffix == ".zip" else "tar")
    if kind == "zip":
        with zipfile.ZipFile(archive) as z:
            z.extractall(dest)
    else:
        with tarfile.open(archive) as t:
            t.extractall(dest, filter="data")
    return dest


def git_clone_pinned(url: str, dest: Path, pin: str | None = None) -> str:
    """Clone (or reuse) a repo, check out `pin` when given, return HEAD sha."""
    if not (dest / ".git").is_dir():
        run_tool(["git", "clone", "--quiet", url, str(dest)])
    if pin:
        try:
            run_tool(["git", "-C", str(dest), "checkout", "--quiet", pin])
        except DatasetError:
            run_tool(["git", "-C", str(dest), "fetch", "--quiet", "origin"])
            run_tool(["git", "-C", str(dest), "checkout", "--quiet", pin])
    head = run_tool(["git", "-C", str(dest), "rev-parse", "HEAD"]).strip()
    if pin and head != pin:
        raise DatasetError(f"{dest.name}: HEAD {head[:12]} != pinned {pin[:12]}")
    return head


def fetch_per_track(
    ctx,
    src: dict,
    files: dict[str, str | None],
    dest_dir: Path,
    jobs: int = 8,
) -> None:
    """Per-track downloads with mirror fallback (`url_patterns`, `{file}` slot).

    Per the checksum policy, a per-track md5 mismatch on refetched preview
    audio is NOT structural — the file is kept and the track is recorded as
    `refetched_mismatch` (re-encodes happen); only total unavailability marks
    a track `unavailable`. Outcomes land in status under stage `fetch`.
    """
    from concurrent.futures import ThreadPoolExecutor

    patterns = src["url_patterns"]
    dest_dir.mkdir(parents=True, exist_ok=True)

    def one(item: tuple[str, str | None]) -> tuple[str, str]:
        fname, md5 = item
        track_id = Path(fname).stem
        dest = dest_dir / fname
        if not dest.is_file():
            last = None
            for pat in patterns:
                try:
                    download(pat.format(file=fname), dest, quiet=True, attempts=2)
                    break
                except DatasetError as e:
                    last = e
            if not dest.is_file():
                return track_id, f"unavailable: {last}"
        if md5 is not None and md5_file(dest) != md5:
            return track_id, "refetched_mismatch"
        return track_id, "ok"

    done = 0
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        for track_id, outcome in pool.map(one, sorted(files.items())):
            ctx.status.track(track_id, fetch=outcome)
            done += 1
            if done % 25 == 0:
                print(f"\r  {done}/{len(files)} tracks", end="", flush=True)
    print(f"\r  {done}/{len(files)} tracks")


# --------------------------------------------------------------------------- tools


def run_tool(cmd: list[str]) -> str:
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0:
        raise DatasetError(f"{cmd[0]} failed: {proc.stderr.strip() or proc.stdout.strip()}")
    return proc.stdout


def tool_version(name: str) -> str | None:
    if shutil.which(name) is None:
        return None
    try:
        out = subprocess.run(
            [name, "--version"], capture_output=True, text=True, timeout=10
        ).stdout
        return out.splitlines()[0].strip() if out else None
    except Exception:
        return None


def require_tools(status: "Status", *names: str) -> None:
    missing = []
    for n in names:
        v = tool_version(n)
        if v is None:
            missing.append(n)
        else:
            status.data["tools"][n] = v
    if missing:
        raise DatasetError(f"missing required tool(s): {', '.join(missing)}")


def ffprobe_duration(path: Path) -> float:
    out = run_tool(
        ["ffprobe", "-v", "quiet", "-show_entries", "format=duration",
         "-of", "csv=p=0", str(path)]
    )
    return float(out.strip())


def transcode_flac(src: Path, dest: Path, sr: int = 44100) -> Path:
    """Canonical local audio: FLAC 44.1 kHz (soxr), original channel count."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(dest.suffix + ".part")
    run_tool(
        ["ffmpeg", "-y", "-v", "error", "-i", str(src),
         "-af", f"aresample=resampler=soxr:osr={sr}", "-c:a", "flac", str(tmp)]
    )
    tmp.replace(dest)
    return dest


# --------------------------------------------------------------------------- status


class Status:
    """datasets/<name>/status.json — local observations, never committed:
    hashes of fetched/derived files, tool versions, per-track outcomes."""

    def __init__(self, path: Path, dataset: str):
        self.path = path
        if path.is_file():
            with path.open(encoding="utf-8") as f:
                self.data = json.load(f)
        else:
            self.data = {
                "schema": STATUS_SCHEMA,
                "dataset": dataset,
                "tools": {},
                "files": {},
                "tracks": {},
                "notes": [],
            }

    def save(self) -> None:
        with self.path.open("w", encoding="utf-8") as f:
            json.dump(self.data, f, indent=1, sort_keys=True)
            f.write("\n")

    def record_file(self, base: Path, path: Path) -> None:
        rel = str(path.relative_to(base))
        self.data["files"][rel] = {
            "sha256": sha256_file(path),
            "size": path.stat().st_size,
        }

    def track(self, track_id: str, **outcome) -> None:
        self.data["tracks"].setdefault(track_id, {}).update(outcome)

    def outcomes(self, stage: str) -> dict[str, str]:
        return {
            tid: t.get(stage, "pending") for tid, t in self.data["tracks"].items()
        }


def coverage_report(manifest: dict, status: Status, stage: str) -> str:
    expected = (manifest.get("expected") or {}).get("tracks")
    outcomes = status.outcomes(stage)
    ok = sum(1 for v in outcomes.values() if v == "ok")
    excluded_seen = sum(1 for v in outcomes.values() if v.startswith("excluded"))
    failed = {
        tid: v
        for tid, v in outcomes.items()
        if v not in ("ok", "pending") and not v.startswith("excluded")
    }
    excluded = len(manifest.get("exclusions") or [])
    lines = [
        f"{manifest['dataset']} {stage}: {ok} of "
        f"{expected if expected is not None else len(outcomes)} ok, "
        f"{len(failed)} failed, {excluded_seen or excluded} excluded"
    ]
    for tid, v in sorted(failed.items())[:20]:
        lines.append(f"  {tid}: {v}")
    if len(failed) > 20:
        lines.append(f"  ... and {len(failed) - 20} more (see status.json)")
    return "\n".join(lines)


# --------------------------------------------------------------------------- verify


def verify(manifest: dict, dirs: SimpleNamespace, status: Status, strict: bool) -> int:
    """Re-hash everything status.json recorded; report drift. --strict fails."""
    changed, missing = [], []
    for rel, rec in sorted(status.data["files"].items()):
        p = dirs.base / rel
        if not p.is_file():
            missing.append(rel)
        elif sha256_file(p) != rec["sha256"]:
            changed.append(rel)
    print(
        f"{manifest['dataset']} verify: {len(status.data['files'])} files recorded, "
        f"{len(changed)} changed, {len(missing)} missing"
    )
    for rel in changed:
        print(f"  changed: {rel}")
    for rel in missing:
        print(f"  missing: {rel}")
    for stage in ("fetch", "prep"):
        if any(stage in t for t in status.data["tracks"].values()):
            print(coverage_report(manifest, status, stage))
    return 1 if strict and (changed or missing) else 0


# --------------------------------------------------------------------------- CLI


def dataset_cli(name: str, fetch, prep) -> int:
    """The uniform entry point every fetch_<dataset>.py wraps.

    fetch/prep are callables(ctx) where ctx carries manifest, dirs, status and
    parsed args. They handle their own per-track failures (record + continue);
    anything they raise as DatasetError is structural and exits 1.
    """
    ap = argparse.ArgumentParser(prog=f"fetch_{name}.py")
    sub = ap.add_subparsers(dest="cmd", required=True)
    for cmd, help_ in (
        ("fetch", "download raw audio + annotations (idempotent, resumable)"),
        ("prep", "decode/transcode audio + emit normalized bundles"),
        ("verify", "re-hash recorded files, print coverage"),
    ):
        p = sub.add_parser(cmd, help=help_)
        p.add_argument("--only", action="append", help="limit to these track ids")
        p.add_argument("--limit", type=int, help="stop after N tracks")
        p.add_argument("--force", action="store_true", help="redo cached work")
        if cmd == "verify":
            p.add_argument("--strict", action="store_true", help="drift exits nonzero")
    args = ap.parse_args()

    manifest = load_manifest(name)
    dirs = dataset_dirs(name)
    status = Status(dirs.status, name)
    ctx = SimpleNamespace(manifest=manifest, dirs=dirs, status=status, args=args)

    try:
        if args.cmd == "verify":
            return verify(manifest, dirs, status, args.strict)
        handler = fetch if args.cmd == "fetch" else prep
        handler(ctx)
        status.save()
        print(coverage_report(manifest, status, args.cmd))
        return 0
    except DatasetError as e:
        status.save()
        print(f"error: {e}", file=sys.stderr)
        return 1


# --------------------------------------------------------------------------- bundles


def write_bundle(norm_dir: Path, bundle: dict) -> Path:
    path = norm_dir / f"{bundle['track_id']}.json"
    with path.open("w", encoding="utf-8") as f:
        json.dump(bundle, f, indent=1, sort_keys=True)
        f.write("\n")
    return path


def write_index(norm_dir: Path, dataset: str, tracks: list[dict]) -> Path:
    """tracks: [{track_id, audio, annotations}] with paths relative to norm/."""
    path = norm_dir / "index.json"
    with path.open("w", encoding="utf-8") as f:
        json.dump(
            {
                "schema": "fosfora-bench-index/v1",
                "dataset": dataset,
                "tracks": sorted(tracks, key=lambda t: t["track_id"]),
            },
            f,
            indent=1,
            sort_keys=True,
        )
        f.write("\n")
    return path
