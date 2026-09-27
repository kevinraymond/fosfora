#!/usr/bin/env -S uv run --quiet --with numpy python
"""Audio-to-photon latency from a phone video of the headset (S6).

The app runs with `debug.fosfora.audio file`, `debug.fosfora.file click`
and `debug.fosfora.flash 1`: a 120 BPM click through the headset speakers
and a two-frame white flash on each detected beat. The phone films a lens
and records the click. This script finds the click times in the video's
audio and the flash times in its frames, pairs each flash with the click
before it, and prints the offsets.

    scripts/xr/latency.py video.mp4 [--fps-hint 60]

Needs ffmpeg and ffprobe on PATH. Frame-time precision is one video frame
(16.7 ms at 60 fps, 4.2 ms at 240 fps); the click time is sample-accurate.
"""

import json
import subprocess
import sys

import numpy as np


def run(cmd):
    return subprocess.run(cmd, check=True, capture_output=True).stdout


def main(path):
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
    if len(clicks) < 3 or len(flashes) < 3:
        print("not enough events; check the framing (lens fills part of the frame) and the volume")
        return
    offsets = []
    for f in flashes:
        prior = clicks[clicks <= f]
        if len(prior):
            offsets.append((f - prior[-1]) * 1000.0)
    offsets = np.array(offsets)
    period = 60.0 / 120.0 * 1000.0
    offsets = offsets[offsets < period]  # a flash more than a beat late paired with the wrong click
    print(
        f"flash minus click: median {np.median(offsets):.0f} ms · mean {offsets.mean():.0f} ms · "
        f"std {offsets.std():.0f} ms · n {len(offsets)} · frame quantum {1000/fps:.1f} ms"
    )


if __name__ == "__main__":
    main(sys.argv[1])
