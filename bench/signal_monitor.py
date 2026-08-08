#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Live diagnostic monitor for the /fosfora/v1 OSC broadcast.

    target/release/fosfora --signal &          # the rig side
    bench/signal_monitor.py -o capture.jsonl   # this side; ctrl-C to stop

Prints a human timeline of the discrete events (beat, downbeat, drop, bpm,
key, section, phrase/len, status) plus a 1 Hz feature line, and captures
EVERY message to JSONL (wall-clock t, addr, args) for offline analysis.
The live view is deliberately sparse — continuous 30 Hz features would
drown the events that matter during a rig check.

Zero dependencies: the OSC subset Signal emits (single messages, i/f/s
args, big-endian) is parsed by hand. Unknown addresses are captured to
the JSONL but never crash the timeline.
"""

from __future__ import annotations

import argparse
import json
import signal as _signal
import socket
import struct
import sys
import time

PREFIX = "/fosfora/v1/"

# Discrete events worth a timeline line, with a shape glyph each so the
# stream scans by silhouette (never by color: house rule).
EVENT_GLYPHS = {
    "beat": "|",
    "downbeat": "#",
    "drop": "*",
    "bpm": "~",
    "key": "K",
    "section": "S",
    "phrase/len": "P",
    "status/online": "@",
}

FEATURE_KEYS = ("energy", "build", "stem/drums/energy", "predict/drop")


def parse_osc(data: bytes) -> tuple[str, list] | None:
    """Parse one OSC message; None on anything malformed."""

    def read_padded(buf: bytes, off: int) -> tuple[bytes, int]:
        end = buf.index(b"\x00", off)
        nxt = (end + 4) & ~3
        return buf[off:end], nxt

    try:
        addr_b, off = read_padded(data, 0)
        tags_b, off = read_padded(data, off)
        addr, tags = addr_b.decode(), tags_b.decode()
        if not tags.startswith(","):
            return None
        args: list = []
        for t in tags[1:]:
            if t == "i":
                args.append(struct.unpack_from(">i", data, off)[0])
                off += 4
            elif t == "f":
                args.append(round(struct.unpack_from(">f", data, off)[0], 6))
                off += 4
            elif t == "s":
                s, off = read_padded(data, off)
                args.append(s.decode())
            else:
                return None  # type we never emit; treat whole msg as opaque
        return addr, args
    except (ValueError, IndexError, struct.error, UnicodeDecodeError):
        return None


def fmt_args(args: list) -> str:
    return " ".join(f"{a:.3f}" if isinstance(a, float) else str(a) for a in args)


def bar(v: float, width: int = 10) -> str:
    filled = max(0, min(width, round(v * width)))
    return "▇" * filled + "▁" * (width - filled)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=9010)
    ap.add_argument("-o", "--out", default="signal_capture.jsonl")
    ap.add_argument("--quiet", action="store_true", help="capture only, no timeline")
    args = ap.parse_args()

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.bind((args.host, args.port))
    sock.settimeout(0.25)

    running = True

    def stop(_sig, _frm):
        nonlocal running
        running = False

    _signal.signal(_signal.SIGINT, stop)
    _signal.signal(_signal.SIGTERM, stop)

    # Line-buffered on both channels: the whole point is watching live
    # (tail -f the log, or the terminal), so nothing may sit in a buffer.
    sys.stdout.reconfigure(line_buffering=True)
    out = open(args.out, "w", encoding="utf-8", buffering=1)
    print(f"listening on {args.host}:{args.port} -> {args.out}  (ctrl-C to stop)")

    t0 = None  # first-message wall clock; timeline is relative to it
    counts: dict[str, int] = {}
    latest: dict[str, list] = {}
    last_feature_line = 0.0
    first_beat_t = None
    prev_bpm: list | None = None

    while running:
        try:
            data, _ = sock.recvfrom(4096)
        except socket.timeout:
            continue
        except OSError:
            break
        now = time.time()
        parsed = parse_osc(data)
        if parsed is None:
            out.write(json.dumps({"t": now, "raw": data.hex()}) + "\n")
            continue
        addr, vals = parsed
        out.write(json.dumps({"t": now, "addr": addr, "args": vals}) + "\n")

        if t0 is None:
            t0 = now
        rel = now - t0
        short = addr.removeprefix(PREFIX)
        counts[short] = counts.get(short, 0) + 1
        latest[short] = vals

        if args.quiet:
            continue
        if short in EVENT_GLYPHS:
            # BPM re-broadcasts at 1 Hz; only voice actual changes.
            if short == "bpm":
                if vals == prev_bpm:
                    continue
                prev_bpm = vals
            g = EVENT_GLYPHS[short]
            print(f"[{rel:8.2f}] {g} {short:<13} {fmt_args(vals)}")
            if short == "beat" and first_beat_t is None:
                first_beat_t = rel
        elif rel - last_feature_line >= 1.0:
            last_feature_line = rel
            e = latest.get("energy", [0.0])[0]
            b = latest.get("build", [0.0])[0]
            d = latest.get("stem/drums/energy", [0.0])[0]
            p = latest.get("predict/drop", [0.0])[0]
            print(
                f"[{rel:8.2f}]   energy {bar(e)} {e:.2f}  drums {d:.2f}"
                f"  build {b:.2f}  predict {p:.2f}"
            )

    out.close()
    total = sum(counts.values())
    print(f"\ncaptured {total} messages -> {args.out}")
    if first_beat_t is not None:
        print(f"first beat at {first_beat_t:.2f}s after first message")
    for k in sorted(counts):
        print(f"  {counts[k]:>7}  {k}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
