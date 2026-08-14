#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# ///
"""Tap drops (and NOT-drops) by ear into `fosfora-bench-annotation/v1` bundles.

    bench/annotate_drops.py ~/Music/*.wav [--out-dir bench/labels]

Plays each track through mpv and records what you tap:

    [d] a drop            [b] a break / near-miss that is NOT a drop
    [space] pause         [<-/->] seek 5 s      [j/k] seek 15 s
    [n/p] next/prev candidate    [u] undo last mark    [q] save + next track
    [x] discard this track and move on

Why the negatives matter (board #2259, #2299): Kevin's ears overturned the drop round
by naming three moments the detector fired on that are NOT drops — one of them the
LOUDEST moment in the track. An unmatched fire scored against positives alone is merely
"unannotated"; scored against a labelled not-a-drop it is a known-bad fire, which is the
only thing that can drive a precision round. Harmonix cannot express this at all: its
drop truth is chorus-onset proxies, and a chorus onset is not a drop.

Candidate seeding: if a v2 sidecar and/or `.signal.jsonl` for the track is in
--sidecar-dir, the detector's own fires and the Q4 /section/boundary announcements
(back-dated by the age each event reports) are loaded as seek targets for [n]/[p], so
you jump between the moments in question instead of scrubbing blind. Seeding only moves
the playhead — you can mark anywhere, and a track with no sidecar annotates fine.

Output is one bundle per track under --out-dir, with drops carrying
`"kind": "local_manual"` — the derivation kind benchlib/annotations.py has always
declared and nothing has ever produced. That kind is what makes bench/score_dump.py,
benchlib/metrics/drops.py::drop() and predict_drop() work on these labels with no
scorer changes. Not-a-drop marks go in an additive `not_drops` array that the v1 schema
ignores, so an old reader still loads the bundle.

Tap latency is NOT guessed: marks are stored raw and the configured --tap-latency
(default 0.0) is recorded in provenance, so a systematic reaction-time offset can be
measured once against a known track and applied afterwards rather than baked in blind.
"""

from __future__ import annotations

import argparse
import json
import os
import select
import shutil
import socket
import subprocess
import sys
import termios
import time
import tty
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SCHEMA = "fosfora-bench-annotation/v1"


# ---------------------------------------------------------------- mpv control


class Mpv:
    """mpv running headless, driven over its JSON IPC socket."""

    def __init__(self, audio: Path, sock: Path, start: float = 0.0):
        self.sock_path = sock
        sock.unlink(missing_ok=True)
        self.proc = subprocess.Popen(
            ["mpv", "--no-video", "--really-quiet", "--no-terminal",
             f"--input-ipc-server={sock}", f"--start={start}", str(audio)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        deadline = time.time() + 10.0
        while time.time() < deadline:
            if self.proc.poll() is not None:
                raise RuntimeError(f"mpv exited immediately ({self.proc.returncode})")
            try:
                self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                self.sock.connect(str(sock))
                self.sock.setblocking(False)
                self._buf = b""
                self._rid = 0
                return
            except (FileNotFoundError, ConnectionRefusedError, OSError):
                time.sleep(0.05)
        raise RuntimeError(f"mpv IPC socket never appeared at {sock}")

    def _command(self, *args, timeout: float = 2.0):
        self._rid += 1
        rid = self._rid
        payload = json.dumps({"command": list(args), "request_id": rid}) + "\n"
        try:
            self.sock.sendall(payload.encode())
        except OSError:
            return None
        deadline = time.time() + timeout
        while time.time() < deadline:
            r, _, _ = select.select([self.sock], [], [], 0.05)
            if r:
                try:
                    chunk = self.sock.recv(65536)
                except (BlockingIOError, OSError):
                    continue
                if not chunk:
                    return None
                self._buf += chunk
            while b"\n" in self._buf:
                line, self._buf = self._buf.split(b"\n", 1)
                if not line.strip():
                    continue
                try:
                    msg = json.loads(line)
                except json.JSONDecodeError:
                    continue
                # Async property-change events share the stream; match on our id.
                if msg.get("request_id") == rid:
                    return msg.get("data") if msg.get("error") == "success" else None
        return None

    def time_pos(self) -> float | None:
        v = self._command("get_property", "time-pos")
        return float(v) if isinstance(v, (int, float)) else None

    def duration(self) -> float | None:
        v = self._command("get_property", "duration")
        return float(v) if isinstance(v, (int, float)) else None

    def eof(self) -> bool:
        return self.proc.poll() is not None

    def seek(self, delta: float):
        self._command("seek", delta, "relative")

    def seek_abs(self, t: float):
        self._command("seek", max(0.0, t), "absolute")

    def toggle_pause(self) -> bool:
        paused = bool(self._command("get_property", "pause"))
        self._command("set_property", "pause", not paused)
        return not paused

    def close(self):
        try:
            self._command("quit", timeout=0.3)
        except Exception:
            pass
        try:
            self.proc.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        try:
            self.sock.close()
        except Exception:
            pass
        self.sock_path.unlink(missing_ok=True)


# ------------------------------------------------------------------- seeding


def candidates(stem: str, sidecar_dir: Path) -> list[tuple[float, str]]:
    """Detector fires + Q4 boundary announcements, as (time, why), time-ordered."""
    out: list[tuple[float, str]] = []

    sidecar = sidecar_dir / f"{stem}.jsonl"
    if sidecar.exists():
        with sidecar.open(encoding="utf-8") as f:
            for line in f:
                if '"d_fired":1' not in line:
                    continue
                try:
                    out.append((float(json.loads(line)["d_t"]), "detector fire"))
                except (json.JSONDecodeError, KeyError, ValueError):
                    pass

    signal = sidecar_dir / f"{stem}.signal.jsonl"
    if signal.exists():
        with signal.open(encoding="utf-8") as f:
            for line in f:
                if "section/boundary" not in line:
                    continue
                try:
                    r = json.loads(line)
                    args = r.get("args") or []
                    conf = float(args[0]["f"])
                    age = float(args[1]["f"]) if len(args) > 1 else 0.0
                    # Events are announced late by a latency they state themselves.
                    if conf >= 0.5:
                        out.append((max(0.0, float(r["ts"]) - age), f"boundary {conf:.2f}"))
                except (json.JSONDecodeError, KeyError, ValueError, IndexError):
                    pass

    out.sort(key=lambda p: p[0])
    merged: list[tuple[float, str]] = []
    for t, why in out:
        if merged and t - merged[-1][0] < 2.0:
            continue
        merged.append((t, why))
    return merged


# ------------------------------------------------------------------ keyboard


def read_key(timeout: float) -> str | None:
    """One keypress from raw stdin, arrow escape sequences decoded."""
    r, _, _ = select.select([sys.stdin], [], [], timeout)
    if not r:
        return None
    ch = sys.stdin.read(1)
    if ch != "\x1b":
        return ch
    r, _, _ = select.select([sys.stdin], [], [], 0.05)
    if not r:
        return "\x1b"
    rest = sys.stdin.read(2)
    return {"[C": "RIGHT", "[D": "LEFT", "[A": "UP", "[B": "DOWN"}.get(rest, "\x1b")


def mmss(t: float) -> str:
    return f"{int(t // 60):02d}:{t % 60:04.1f}"


def short(p: Path) -> str:
    """Repo-relative when it can be, absolute otherwise (--out-dir may be anywhere)."""
    try:
        return str(p.relative_to(REPO))
    except ValueError:
        return str(p)


# --------------------------------------------------------------------- track


def annotate(audio: Path, args) -> dict | None:
    """Returns the bundle, or None if the track was discarded."""
    cands = candidates(audio.stem, args.sidecar_dir)
    sock = Path(f"/tmp/fosfora-annotate-{os.getpid()}.sock")
    mpv = Mpv(audio, sock, start=args.start)
    duration = mpv.duration()

    marks: list[tuple[float, str]] = []
    ci = -1
    status = ""

    print(f"\n\033[1m{audio.name}\033[0m"
          f"{f'   {mmss(duration)}' if duration else ''}"
          f"   {len(cands)} candidate(s) seeded")
    print("  [d] drop   [b] not-a-drop   [space] pause   [</>] 5s   [j/k] 15s")
    print("  [n/p] candidate   [u] undo   [q] save+next   [x] discard\n")

    try:
        while True:
            if mpv.eof():
                status = "end of track"
                break
            key = read_key(0.10)
            now = mpv.time_pos()
            if key is None:
                if now is not None:
                    line = f"\r  {mmss(now)}   {len(marks)} mark(s)   {status}"
                    sys.stdout.write(line.ljust(shutil.get_terminal_size().columns - 1))
                    sys.stdout.flush()
                continue

            if key in ("d", "b") and now is not None:
                kind = "drop" if key == "d" else "not_drop"
                marks.append((now, kind))
                glyph = "● drop    " if key == "d" else "○ not-a-drop"
                sys.stdout.write(f"\r  {mmss(now)}  {glyph}".ljust(60) + "\n")
                status = ""
            elif key == "u":
                status = f"undid {mmss(marks.pop()[0])}" if marks else "nothing to undo"
            elif key == " ":
                status = "paused" if mpv.toggle_pause() else ""
            elif key in ("RIGHT",):
                mpv.seek(5)
            elif key in ("LEFT",):
                mpv.seek(-5)
            elif key == "k":
                mpv.seek(15)
            elif key == "j":
                mpv.seek(-15)
            elif key in ("n", "p") and cands:
                if now is None:
                    continue
                if key == "n":
                    nxt = [i for i, (t, _) in enumerate(cands) if t > now + 1.0]
                    ci = nxt[0] if nxt else len(cands) - 1
                else:
                    prv = [i for i, (t, _) in enumerate(cands) if t < now - 1.0]
                    ci = prv[-1] if prv else 0
                t, why = cands[ci]
                # Land a few seconds early so the run-up is audible, not just the hit.
                mpv.seek_abs(t - 6.0)
                status = f"-> candidate {ci + 1}/{len(cands)} at {mmss(t)} ({why})"
            elif key == "q":
                status = "saved"
                break
            elif key == "x":
                mpv.close()
                print(f"\r  discarded {audio.name}".ljust(60))
                return None
    finally:
        if not mpv.eof():
            mpv.close()

    print(f"\r  {status}".ljust(60))

    if duration is None:
        # duration_s is what false_drops_per_min divides by; never write a bundle without it.
        probe = subprocess.run(
            ["ffprobe", "-v", "error", "-show_entries", "format=duration",
             "-of", "csv=p=0", str(audio)],
            capture_output=True, text=True,
        )
        try:
            duration = float(probe.stdout.strip())
        except ValueError:
            sys.exit(f"could not determine duration for {audio}")

    lat = args.tap_latency
    drops = [{"time": max(0.0, t - lat), "kind": "local_manual", "source": "kevin:tap"}
             for t, k in sorted(marks) if k == "drop"]
    not_drops = [{"time": max(0.0, t - lat), "kind": "local_manual", "source": "kevin:tap"}
                 for t, k in sorted(marks) if k == "not_drop"]

    return {
        "schema": SCHEMA,
        "track_id": audio.stem,
        "dataset": "local_drops",
        "audio": {"path": os.path.relpath(audio, args.out_dir), "duration_s": duration},
        "drops": drops,
        # Additive: v1 readers ignore it, the precision scorer reads it.
        "not_drops": not_drops,
        "provenance": {
            "tool": "bench/annotate_drops.py",
            "method": "listen + tap (mpv)",
            "tap_latency_s": lat,
            "_latency_note": "marks are raw tap times minus tap_latency_s; measure the "
                             "systematic offset once against a known track before trusting "
                             "sub-second placement",
            "candidates_seeded": len(cands),
        },
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("files", nargs="+", type=Path)
    ap.add_argument("--out-dir", type=Path, default=REPO / "bench" / "labels")
    ap.add_argument("--sidecar-dir", type=Path,
                    default=REPO / "bench" / "out" / "dropsweep" / "music")
    ap.add_argument("--tap-latency", type=float, default=0.0,
                    help="seconds subtracted from each tap; recorded in provenance")
    ap.add_argument("--start", type=float, default=0.0, help="start position, seconds")
    ap.add_argument("--force", action="store_true", help="re-annotate tracks already done")
    args = ap.parse_args()

    if not shutil.which("mpv"):
        sys.exit("mpv not found — this tool drives mpv over its JSON IPC socket")
    if not sys.stdin.isatty():
        sys.exit("needs a terminal: it reads single keypresses in raw mode")

    args.out_dir.mkdir(parents=True, exist_ok=True)
    todo = []
    for f in args.files:
        if not f.exists():
            sys.exit(f"no such file: {f}")
        out = args.out_dir / f"{f.stem}.json"
        if out.exists() and not args.force:
            print(f"skip (already annotated): {f.name}")
            continue
        todo.append((f, out))

    if not todo:
        print("nothing to do")
        return 0

    fd = sys.stdin.fileno()
    saved = termios.tcgetattr(fd)
    done = 0
    try:
        tty.setcbreak(fd)
        for f, out in todo:
            bundle = annotate(f, args)
            if bundle is None:
                continue
            out.write_text(json.dumps(bundle, indent=2) + "\n", encoding="utf-8")
            n_d, n_n = len(bundle["drops"]), len(bundle["not_drops"])
            print(f"  wrote {short(out)}  ({n_d} drop, {n_n} not-a-drop)\n")
            done += 1
    except KeyboardInterrupt:
        print("\ninterrupted")
    finally:
        termios.tcsetattr(fd, termios.TCSADRAIN, saved)

    print(f"annotated {done}/{len(todo)} track(s) -> {short(args.out_dir)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
