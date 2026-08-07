#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""Synthesize the CI benchmark fixture: a WAV and its annotations from ONE score.

Ground truth by construction — beats, downbeats, tempo, key, sections and the
drop instant are emitted from the same arrangement that renders the audio, so
the fixture needs no human annotation and no licensing thought.

The voices and detector-driving arrangement are copied from
scripts/capture/make_loop.py (the rights-clear README capture loop), which was
measured against the real analyzer: BPM locks, buildup holds above the 0.6 arm
threshold long enough, the drop fires exactly once. Deliberately COPIED, not
imported: this fixture must stay byte-stable when capture tooling is retuned,
and its floors in fixture_expectations.json are calibrated against these exact
bytes. Differences from make_loop: 44.1 kHz (the documented hop-math rate and
the common dataset rate), and an A harmonic-minor cadence (Am-Dm-E-Am) so key
detection has a single right answer instead of a relative-major ambiguity.

Usage:  bench/make_fixture.py [-o bench/out/fixture] [--bpm 124] [--bars 20]

Writes fixture.wav, fixture.annotations.json (fosfora-bench-annotation/v1)
and index.json (fosfora-bench-index/v1) into the output directory.
"""

from __future__ import annotations

import argparse
import json
import struct
import wave
from pathlib import Path

import numpy as np

SR = 44_100
RNG = np.random.default_rng(0x50F0)  # fixed seed: the fixture must be reproducible

# --------------------------------------------------------------------------- helpers
# (make_loop.py lineage — see module docstring)


def env(n: int, attack: float, decay: float, curve: float = 2.0) -> np.ndarray:
    a = max(1, int(attack * SR))
    d = max(1, int(decay * SR))
    out = np.zeros(n, dtype=np.float32)
    head = min(a, n)
    out[:head] = np.linspace(0.0, 1.0, head, dtype=np.float32)
    tail = min(d, max(0, n - head))
    if tail:
        out[head : head + tail] = np.linspace(1.0, 0.0, tail, dtype=np.float32) ** curve
    return out


def onepole_lp(x: np.ndarray, cutoff: np.ndarray | float) -> np.ndarray:
    c = np.broadcast_to(np.asarray(cutoff, dtype=np.float32), x.shape)
    alpha = 1.0 - np.exp(-2.0 * np.pi * np.clip(c, 20.0, SR * 0.45) / SR)
    out = np.empty_like(x)
    acc = 0.0
    for i in range(x.size):
        acc += alpha[i] * (x[i] - acc)
        out[i] = acc
    return out.astype(np.float32)


def onepole_hp(x: np.ndarray, cutoff: float) -> np.ndarray:
    return (x - onepole_lp(x, cutoff)).astype(np.float32)


def add(buf: np.ndarray, sig: np.ndarray, at: int, gain: float = 1.0) -> None:
    if at >= buf.size:
        return
    n = min(sig.size, buf.size - at)
    buf[at : at + n] += sig[:n] * gain


# --------------------------------------------------------------------------- voices


def kick(dur: float = 0.42) -> np.ndarray:
    n = int(dur * SR)
    t = np.arange(n, dtype=np.float32) / SR
    f = 44.0 + 74.0 * np.exp(-t * 38.0)
    body = np.sin(2 * np.pi * np.cumsum(f) / SR) * env(n, 0.001, dur, 2.4)
    click = onepole_hp(RNG.normal(0, 1, n).astype(np.float32), 1800.0) * env(
        n, 0.0002, 0.012, 3.0
    )
    return np.tanh(body * 1.6 + click * 0.35).astype(np.float32)


def snare(dur: float = 0.20, bright: float = 1.0) -> np.ndarray:
    n = int(dur * SR)
    noise = RNG.normal(0, 1, n).astype(np.float32)
    body = onepole_hp(noise, 1200.0 * bright) * env(n, 0.001, dur, 2.0)
    tone = np.sin(2 * np.pi * 190.0 * np.arange(n, dtype=np.float32) / SR) * env(
        n, 0.001, 0.09, 2.0
    )
    return (body * 0.8 + tone * 0.4).astype(np.float32)


def hat(dur: float = 0.055, open_: bool = False) -> np.ndarray:
    d = dur * (4.5 if open_ else 1.0)
    n = int(d * SR)
    noise = RNG.normal(0, 1, n).astype(np.float32)
    return (onepole_hp(noise, 7000.0) * env(n, 0.0005, d, 2.6)).astype(np.float32)


def sub(freq: float, dur: float) -> np.ndarray:
    n = int(dur * SR)
    t = np.arange(n, dtype=np.float32) / SR
    s = np.sin(2 * np.pi * freq * t) + 0.22 * np.sin(4 * np.pi * freq * t)
    e = env(n, 0.006, dur, 1.2)
    return np.tanh(s * e * 1.3).astype(np.float32)


def pad(freqs: list[float], dur: float, cutoff: np.ndarray | float) -> np.ndarray:
    n = int(dur * SR)
    t = np.arange(n, dtype=np.float32) / SR
    out = np.zeros(n, dtype=np.float32)
    for f in freqs:
        for detune in (-0.16, 0.0, 0.19):
            ph = ((f + detune) * t) % 1.0
            out += (2.0 * ph - 1.0).astype(np.float32)
    out /= len(freqs) * 3
    e = np.minimum(env(n, 0.25, dur, 0.7) * 3.0, 1.0)
    return (onepole_lp(out, cutoff) * e).astype(np.float32)


def stab(freqs: list[float], dur: float = 0.34) -> np.ndarray:
    n = int(dur * SR)
    t = np.arange(n, dtype=np.float32) / SR
    out = np.zeros(n, dtype=np.float32)
    for f in freqs:
        out += np.sin(2 * np.pi * f * t) + 0.4 * np.sin(4 * np.pi * f * t)
    out /= len(freqs)
    return (onepole_lp(out, 3800.0) * env(n, 0.004, dur, 2.2)).astype(np.float32)


def riser(dur: float) -> np.ndarray:
    n = int(dur * SR)
    t = np.arange(n, dtype=np.float32) / SR
    ramp = (t / dur).astype(np.float32)
    noise = RNG.normal(0, 1, n).astype(np.float32)
    swept = noise - onepole_lp(noise, 300.0 + 6500.0 * ramp**2)
    tone = np.sin(2 * np.pi * np.cumsum(220.0 + 1400.0 * ramp**3) / SR).astype(
        np.float32
    )
    return ((swept * 0.7 + tone * 0.3) * (0.08 + 0.92 * ramp**2)).astype(np.float32)


# --------------------------------------------------------------------------- score

# A harmonic-minor cadence, two bars per chord: Am - Dm - E - Am. The raised
# G# in the E chord is what pins Krumhansl-Kessler to A minor rather than
# letting it drift to the relative C major.
NOTE = {
    "A": 55.00,
    "B": 61.74,
    "C": 65.41,
    "D": 73.42,
    "E": 82.41,
    "F": 87.31,
    "G#": 103.83,
}
PROGRESSION = [
    ("A", ["A", "C", "E"]),
    ("D", ["D", "F", "A"]),
    ("E", ["E", "G#", "B"]),
    ("A", ["A", "C", "E"]),
]


def section_bars(bars: int) -> tuple[int, int]:
    """(build_start, drop_start) in bars — same fractions as make_loop.py."""
    return round(bars * 0.40), round(bars * 0.75)


def build(bpm: float, bars: int) -> np.ndarray:
    beat = 60.0 / bpm
    bar = beat * 4
    total = int(bars * bar * SR)
    buf = np.zeros(total + int(bar * SR), dtype=np.float32)

    def at(bar_i: float) -> int:
        return int(bar_i * bar * SR)

    build_start, drop_start = section_bars(bars)

    for b in range(bars):
        root, chord = PROGRESSION[(b // 2) % len(PROGRESSION)]
        in_build = build_start <= b < drop_start
        in_drop = b >= drop_start
        prog = (b - build_start) / max(1, drop_start - build_start)

        ramp = 0.42 + 0.76 * np.clip(prog, 0, 1) ** 0.7 if in_build else 0.92

        if in_build:
            cut = 800.0 + 6900.0 * np.clip(prog, 0, 1) ** 0.9
        elif in_drop:
            cut = 7600.0
        else:
            cut = 1500.0
        pad_freqs = [NOTE[n] * 4 for n in chord]
        add(buf, pad(pad_freqs, bar * 1.02, cut), at(b), (0.30 if in_drop else 0.24) * ramp)

        div = 8 if in_build else 4
        for i in range(div):
            openish = (i % 4) == 2 and not in_build
            g = (0.10 if i % 2 else 0.16) * ramp
            add(buf, hat(open_=openish), at(b) + int(i * bar / div * SR), g)

        kick_hits = 4
        if in_build:
            kick_hits = 4 if prog < 0.3 else (2 if prog < 0.55 else 0)
        for i in range(kick_hits):
            stride = 4 // kick_hits
            add(
                buf,
                kick(),
                at(b) + int(i * stride * beat * SR),
                (0.95 if in_drop else 0.85) * ramp,
            )

        if not (in_build and prog >= 0.3):
            for i in (0, 2):
                add(buf, sub(NOTE[root], beat * 1.6), at(b) + int(i * beat * SR), 0.55 * ramp)

        for i in (1, 3):
            add(buf, snare(), at(b) + int(i * beat * SR), 0.34 * ramp)

        if in_build:
            rate = 2 ** int(1 + 3 * np.clip(prog, 0, 0.999))
            for i in range(rate):
                g = (0.14 + 0.24 * (i / rate)) * ramp
                add(buf, snare(0.13, 1.0 + prog), at(b) + int(i * bar / rate * SR), g)

        if in_drop:
            for i in (1, 2, 3):
                add(buf, stab(pad_freqs), at(b) + int((i + 0.5) * beat * SR), 0.20)

    add(buf, riser((drop_start - build_start) * bar), at(build_start), 0.42)

    gap_end = at(drop_start)
    gap_start = gap_end - int(0.95 * beat * SR)
    fade = np.linspace(1.0, 0.015, gap_end - gap_start, dtype=np.float32) ** 3
    buf[gap_start:gap_end] *= fade

    tail = buf[total:]
    buf[: tail.size] += tail
    return buf[:total]


def stereoize(mono: np.ndarray) -> np.ndarray:
    hi = onepole_hp(mono, 900.0)
    d = int(0.010 * SR)
    left = mono.copy()
    right = mono.copy()
    left[d:] += hi[:-d] * 0.22
    right[:-d] += hi[d:] * 0.22
    return np.stack([left, right], axis=1)


def normalize(x: np.ndarray, peak_db: float = -1.0) -> np.ndarray:
    peak = float(np.max(np.abs(x))) or 1.0
    return (x / peak * (10 ** (peak_db / 20.0))).astype(np.float32)


# --------------------------------------------------------------------------- output


def annotations(bpm: float, bars: int) -> dict:
    beat = 60.0 / bpm
    bar = beat * 4
    build_start, drop_start = section_bars(bars)
    duration = bars * bar
    drop_t = drop_start * bar
    return {
        "schema": "fosfora-bench-annotation/v1",
        "dataset": "fixture",
        "track_id": "fixture",
        "audio": {
            "path": "fixture.wav",
            "sr": SR,
            "duration_s": duration,
            "sha256": None,
            "offset_applied_s": 0.0,
        },
        "beats": [i * beat for i in range(bars * 4)],
        "downbeats": [b * bar for b in range(bars)],
        "tempo_bpm": bpm,
        "tempo_source": "constructed",
        "key": {"tonic": "A", "mode": "minor", "raw": "A harmonic minor cadence"},
        "segments": [
            [0.0, build_start * bar, "steady"],
            [build_start * bar, drop_t, "build"],
            [drop_t, duration, "drop"],
        ],
        "drops": [{"time": drop_t, "source": "constructed", "kind": "constructed"}],
        "stems": None,
        "annotators": None,
        "provenance": {"generator": "bench/make_fixture.py", "seed": "0x50F0"},
    }


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("-o", "--out", type=Path, default=Path("bench/out/fixture"))
    ap.add_argument("--bpm", type=float, default=124.0)
    ap.add_argument("--bars", type=int, default=20)
    args = ap.parse_args()

    mono = build(args.bpm, args.bars)
    stereo = normalize(stereoize(mono))
    pcm = (np.clip(stereo, -1.0, 1.0) * 32767.0).astype(np.int16)

    args.out.mkdir(parents=True, exist_ok=True)
    wav_path = args.out / "fixture.wav"
    with wave.open(str(wav_path), "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(struct.pack(f"<{pcm.size}h", *pcm.flatten().tolist()))

    ann = annotations(args.bpm, args.bars)
    (args.out / "fixture.annotations.json").write_text(
        json.dumps(ann, indent=1, sort_keys=True) + "\n", encoding="utf-8"
    )
    (args.out / "index.json").write_text(
        json.dumps(
            {
                "schema": "fosfora-bench-index/v1",
                "dataset": "fixture",
                "tracks": [
                    {
                        "track_id": "fixture",
                        "audio": "fixture.wav",
                        "annotations": "fixture.annotations.json",
                    }
                ],
            },
            indent=1,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    dur = stereo.shape[0] / SR
    print(f"wrote {wav_path}  {dur:.1f}s  {args.bpm:g} BPM  {args.bars} bars + annotations")


if __name__ == "__main__":
    main()
