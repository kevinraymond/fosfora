#!/usr/bin/env -S uv run --quiet --with numpy python
"""Audio-to-photon latency from a phone video of the headset (S6).

The app runs with `debug.fosfora.audio file`, `debug.fosfora.file click`
and `debug.fosfora.flash 1`: a 120 BPM click through the headset speakers
and a two-frame white flash on each detected beat. The phone films a lens
and records the click. This script finds the click times in the video's
audio and the flash times in its frames, pairs each flash with the click
before it, and prints the offsets.

    scripts/xr/latency.py video.mp4 [click_bpm, default 120]

Needs ffmpeg and ffprobe on PATH. Frame-time precision is one video frame
(16.7 ms at 60 fps, 4.2 ms at 240 fps); the click time is sample-accurate.
"""

import json
import subprocess
import sys

import numpy as np


def run(cmd):
    return subprocess.run(cmd, check=True, capture_output=True).stdout


def main(path, bpm=120.0):
    info = json.loads(
        run(["ffprobe", "-v", "error", "-print_format", "json", "-show_streams", path])
    )
    vstream = next(s for s in info["streams"] if s["codec_type"] == "video")
    num, den = (int(x) for x in vstream["r_frame_rate"].split("/"))
    fps = num / den
    w, h = int(vstream["width"]), int(vstream["height"])

    # Audio: mono 48 kHz f32, click = sharp envelope peaks.
    sr = 48000
    audio = np.frombuffer(
        run(["ffmpeg", "-v", "error", "-i", path, "-ac", "1", "-ar", str(sr), "-f", "f32le", "-"]),
        dtype=np.float32,
    )
    env = np.abs(audio)
    win = int(sr * 0.002)
    env = np.convolve(env, np.ones(win) / win, mode="same")
    thresh = 0.35 * env.max()
    above = env > thresh
    clicks = []
    i = 0
    while i < len(above):
        if above[i]:
            j = i
            while j < len(above) and above[j]:
                j += 1
            clicks.append((i + int(np.argmax(env[i:j]))) / sr)
            i = j + int(sr * 0.1)  # one click per 100 ms at most
        else:
            i += 1
    clicks = np.array(clicks)

    # Video: mean brightness per frame at 64x36 grayscale.
    frames = np.frombuffer(
        run([
            "ffmpeg", "-v", "error", "-i", path, "-vf", "scale=64:36", "-pix_fmt", "gray",
            "-f", "rawvideo", "-",
        ]),
        dtype=np.uint8,
    ).reshape(-1, 36, 64)
    bright = frames.reshape(len(frames), -1).astype(np.float32).mean(1)
    base = np.median(bright)
    peak = bright.max()
    fthresh = base + 0.5 * (peak - base)
    flashes = []
    k = 0
    while k < len(bright):
        if bright[k] > fthresh:
            flashes.append(k / fps)
            while k < len(bright) and bright[k] > fthresh:
                k += 1
        else:
            k += 1
    flashes = np.array(flashes)

    print(f"video {w}x{h} @ {fps:.2f} fps, {len(frames)} frames · audio {len(audio)/sr:.1f} s")
    print(f"clicks found {len(clicks)} (median spacing {np.median(np.diff(clicks))*1000:.1f} ms if >1)")
    print(f"flashes found {len(flashes)} (median spacing {np.median(np.diff(flashes))*1000:.1f} ms if >1)")
    if len(flashes) < 3:
        print("not enough flashes; check the framing (lens fills part of the frame)")
        return

    # Both events sit on the click track's 500 ms grid (120 BPM), so fit a
    # phase to each with a comb over the grid instead of pairing peaks: the
    # click fit uses the whole envelope (robust to spurious transients), the
    # flash fit the flash times. Latency = flash phase - click phase, modulo
    # the period; unambiguous while it is under 500 ms.
    period = 60.0 / bpm
    t_env = np.arange(len(env)) / sr
    ph = np.linspace(0, period, 2000, endpoint=False)
    # Comb score at 1 ms resolution: sum of envelope at phase + k*period.
    score = np.array([env[((t_env - p) % period) < 0.002].sum() for p in ph])
    click_phase = ph[int(np.argmax(score))]
    flash_ph = (flashes % period)
    z = np.exp(2j * np.pi * flash_ph / period).mean()
    flash_phase = (np.angle(z) / (2 * np.pi) % 1.0) * period
    flash_jitter = np.sqrt(-2 * np.log(max(abs(z), 1e-9))) / (2 * np.pi) * period
    lat = (flash_phase - click_phase) % period
    # Per-flash offsets to the click grid, for the spread.
    per = ((flashes - click_phase) % period) * 1000.0
    print(
        f"click grid phase {click_phase*1000:.1f} ms · flash grid phase {flash_phase*1000:.1f} ms "
        f"(circular jitter {flash_jitter*1000:.0f} ms, n {len(flashes)})"
    )
    print(
        f"AUDIO-TO-PHOTON: {lat*1000:.0f} ms (flash grid minus click grid) · per-flash offsets "
        f"median {np.median(per):.0f} ms, 10-90% {np.percentile(per,10):.0f}..{np.percentile(per,90):.0f} ms · "
        f"frame quantum {1000/fps:.1f} ms"
    )


if __name__ == "__main__":
    main(sys.argv[1], float(sys.argv[2]) if len(sys.argv) > 2 else 120.0)
