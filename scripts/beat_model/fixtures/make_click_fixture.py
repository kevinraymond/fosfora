#!/usr/bin/env python3
"""Regenerate the fixtures: the header and the first N hops of a FOSFORA_HOP_SIDECAR dump
of a synthetic signal, written by the real binary so the readers' tests run against the
writer's output, not a hand-written imitation.

    scripts/beat_model/fixtures/make_click_fixture.py [--bin target/release/fosfora]

click_50hops.jsonl:  2 s at 44.1 kHz, silent for 0.5 s, then a 120 BPM kick (a decaying sine
                     burst with a pitch drop), first 50 hops: the silence gate, then kick flux.
click_200hops.jsonl: the same kick from 0.25 s on, first 200 hops: the beat model's golden
                     vector (make_golden.py), with activations the binary itself wrote. The binary needs `--features analyze` (`--signal-dump`).
Standard library only, so it runs without an environment.
"""

from __future__ import annotations

import argparse
import math
import os
import struct
import subprocess
import sys
import tempfile
import wave
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
SR = 44100
FIXTURES = [("click_50hops.jsonl", 0.5, 2.0, 50), ("click_200hops.jsonl", 0.25, 3.0, 200)]


def kick(t: float, silence: float) -> float:
    if t < silence:
        return 0.0
    tb = t % 0.5
    return math.sin(math.tau * (50.0 + 100.0 * math.exp(-tb * 40.0)) * tb) * math.exp(-tb * 10.0) * 0.7


def write_wav(path: Path, seconds: float, silence: float) -> None:
    n = int(seconds * SR)
    frames = bytearray()
    for i in range(n):
        v = max(-1.0, min(1.0, kick(i / SR, silence)))
        s = int(round(v * 32767))
        frames += struct.pack("<hh", s, s)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(bytes(frames))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--bin", default=os.environ.get("FOSFORA_BIN", str(REPO / "target" / "release" / "fosfora")))
    args = ap.parse_args()
    if not Path(args.bin).exists():
        sys.exit(f"binary not found: {args.bin} (cargo build --release --features release,analyze)")
    for name, silence, seconds, keep in FIXTURES:
        out = HERE / name
        with tempfile.TemporaryDirectory() as td:
            wav = Path(td) / "click.wav"
            dump = Path(td) / "hops.jsonl"
            write_wav(wav, seconds, silence)
            env = dict(os.environ, FOSFORA_HOP_SIDECAR=str(dump), RUST_LOG="error")
            subprocess.run(
                [args.bin, "--signal-dump", str(wav), "--out", "-"],
                check=True, env=env, stdout=subprocess.DEVNULL,
            )
            lines = dump.read_text(encoding="utf-8").splitlines(keepends=True)
        if len(lines) < 1 + keep:
            sys.exit(f"{name}: dump has {len(lines)} lines, need {1 + keep}")
        out.write_text("".join(lines[: 1 + keep]), encoding="utf-8")
        print(f"wrote {out} ({1 + keep} lines, {out.stat().st_size} bytes) from {len(lines) - 1} hops")
    return 0


if __name__ == "__main__":
    sys.exit(main())
