#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "soundfile"]
# ///
"""Derive drop reference times from raw audio, independent of any detector.

    bench/drop_reference.py ~/Music/*.wav [--json OUT]

A drop is scored where the sub-bass (20-80 Hz) returns from a sustained withdrawal to
full level and the broadband level steps up with it — the definition finding #2209 used
by hand on Thirty Two Hertz. Doing it mechanically over every specimen puts all three
tracks on one criterion, so Tropical Pulse's events are not eyeballed to fit.

Reports the withdrawal null, the return time, and the RMS step across the transition, so
each reference carries the evidence that made it one.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np
import soundfile as sf

FRAME_S = 0.05
# Sub-bass band. The specimens' roots sit at ~32 Hz (#2209), below the chroma floor but
# squarely in this band.
SUB_LO, SUB_HI = 20.0, 80.0
# A withdrawal must drop this far below the track's loud-section sub level...
WITHDRAW_DB = 12.0
# ...and hold for at least this long, so a single muted beat is not a breakdown.
WITHDRAW_MIN_S = 2.0
# The return must reach this fraction of the loud-section sub level (in dB terms, come
# back within this margin of it).
RETURN_MARGIN_DB = 6.0
# Broadband level must also step up by this much across the transition (4 s either side).
RMS_STEP_DB = 2.0


def band_db(x: np.ndarray, sr: int) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Per-frame (times, broadband dB, sub-band dB)."""
    n = int(round(FRAME_S * sr))
    frames = len(x) // n
    x = x[: frames * n].reshape(frames, n)
    win = np.hanning(n)
    spec = np.abs(np.fft.rfft(x * win, axis=1))
    freqs = np.fft.rfftfreq(n, 1.0 / sr)
    sub = spec[:, (freqs >= SUB_LO) & (freqs < SUB_HI)]
    eps = 1e-12
    rms_db = 20.0 * np.log10(np.sqrt((x**2).mean(axis=1)) + eps)
    sub_db = 20.0 * np.log10(np.sqrt((sub**2).mean(axis=1)) + eps)
    t = np.arange(frames) * FRAME_S
    return t, rms_db, sub_db


def smooth(v: np.ndarray, seconds: float) -> np.ndarray:
    k = max(1, int(round(seconds / FRAME_S)))
    return np.convolve(v, np.ones(k) / k, mode="same")


def find_drops(t, rms_db, sub_db) -> list[dict]:
    sub_s = smooth(sub_db, 0.3)
    rms_s = smooth(rms_db, 0.3)
    # "Full" sub level = the 80th percentile of the track's own sub curve: the level the
    # loud sections sit at, robust to the withdrawals themselves.
    full = float(np.percentile(sub_s, 80))
    low_mask = sub_s < (full - WITHDRAW_DB)

    events, i, n = [], 0, len(t)
    min_frames = int(round(WITHDRAW_MIN_S / FRAME_S))
    while i < n:
        if not low_mask[i]:
            i += 1
            continue
        j = i
        while j < n and low_mask[j]:
            j += 1
        if j - i >= min_frames and j < n:
            # Return = first frame at/after the null where sub comes back within margin.
            k = j
            while k < n and sub_s[k] < (full - RETURN_MARGIN_DB):
                k += 1
            if k < n:
                w = int(round(4.0 / FRAME_S))
                before = float(np.median(rms_s[max(0, i) : j])) if j > i else float("nan")
                after = float(np.median(rms_s[k : min(n, k + w)]))
                step = after - before
                if step >= RMS_STEP_DB:
                    events.append(
                        {
                            "time": round(float(t[k]), 2),
                            "null_start": round(float(t[i]), 2),
                            "null_end": round(float(t[j]), 2),
                            "null_len_s": round(float(t[j] - t[i]), 2),
                            "sub_null_db": round(float(sub_s[i:j].min()), 1),
                            "sub_full_db": round(full, 1),
                            "rms_before_db": round(before, 1),
                            "rms_after_db": round(after, 1),
                            "rms_step_db": round(step, 1),
                        }
                    )
        i = j + 1
    return events


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("tracks", nargs="+", type=Path)
    ap.add_argument("--json", type=Path)
    args = ap.parse_args()

    out = {}
    for p in args.tracks:
        x, sr = sf.read(str(p), dtype="float32", always_2d=True)
        x = x.mean(axis=1)
        t, rms_db, sub_db = band_db(x, sr)
        ev = find_drops(t, rms_db, sub_db)
        out[p.stem] = {"duration_s": round(len(x) / sr, 2), "drops": ev}
        print(f"\n=== {p.stem}  ({len(x) / sr:.1f}s, {sr} Hz)")
        for e in ev:
            print(
                f"  drop @ {e['time']:7.2f}s  | sub null {e['sub_null_db']:6.1f} dB for "
                f"{e['null_len_s']:5.2f}s (full {e['sub_full_db']:.1f} dB) | "
                f"RMS {e['rms_before_db']:6.1f} -> {e['rms_after_db']:6.1f} dB "
                f"(step {e['rms_step_db']:+.1f})"
            )
        if not ev:
            print("  (none)")

    if args.json:
        args.json.write_text(json.dumps(out, indent=2) + "\n")
        print(f"\nwrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
