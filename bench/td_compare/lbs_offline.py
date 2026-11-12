#!/usr/bin/env python3
"""Run lastBeatStanding's beat library offline, fed exactly as its TouchDesigner
component is fed at 60 fps, and write Signal dumps for compare.py.

    python3 -I lbs_offline.py <td_lastBeatStanding clone> <out_dir> <wav>...

The component's per-cook code (src/script_madmom_dnn.py + script_madmom_post.py
in the repo) takes the last round(stepSeconds * rate) = 735 new samples, runs
BeatDetector.process on them, then PostProcessor.process(dt = 1/60). Settings
are the released .tox's: the 8-model ensemble (Singlemodel off), 60-190 BPM,
lambdas 100/16, gates 0.15/0.005, time multiplier 1, 16 synthetic beats max.
The post-processor's audio input only feeds its breakdown flag, not beats.

Like the TD harness, each track starts with 32 silent cooks (the harness's 30
settle frames plus the repeated frame 1) and stamps an event at the END of the
chunk it appears in: chunk k of the file -> (k + 1) * 735 / 44100 s.
Writes <out_dir>/lbs/<stem>.jsonl and <out_dir>/lbs_real/<stem>.jsonl.
"""

import json
import sys
import wave
from pathlib import Path

import numpy as np

STEP = 735
RATE = 44100
SILENT_COOKS = int(__import__("os").environ.get("LBS_SILENT", "32"))


def load_wav(path):
    with wave.open(str(path), "rb") as w:
        assert w.getframerate() == RATE and w.getnchannels() == 1 and w.getsampwidth() == 2, path
        return np.frombuffer(w.readframes(w.getnframes()), dtype="<i2").astype(np.float32) / 32768.0


def write_dump(path, source, beats, bpm_series):
    path.parent.mkdir(parents=True, exist_ok=True)
    rec = [(t, "/fosfora/v1/beat", [{"i": n + 1}]) for n, t in enumerate(beats)]
    rec += [(t, "/fosfora/v1/bpm", [{"f": b}]) for t, b in bpm_series]
    with open(path, "w") as f:
        f.write(json.dumps({"meta": 1, "schema": "/fosfora/v1", "source": source, "sample_rate": RATE,
                            "hop_hz": 60.0, "tx_rate_hz": 30}) + "\n")
        for t, addr, args in sorted(rec, key=lambda r: r[0]):
            f.write(json.dumps({"ts": t, "addr": addr, "args": args}) + "\n")


def main():
    clone, out_dir, wavs = Path(sys.argv[1]), Path(sys.argv[2]), [Path(p) for p in sys.argv[3:]]
    sys.path.insert(0, str(clone))
    from beat_detection import BeatDetector, PostProcessor

    for wav in wavs:
        det = BeatDetector(model_dir=str(clone / "beat_detection" / "models"), min_bpm=60, max_bpm=190,
                           transition_lambda=100, observation_lambda=16, act_gate=0.15, rms_gate=0.005,
                           single_model=False)
        post = PostProcessor()
        audio = load_wav(wav)
        silence = np.zeros(STEP, dtype=np.float32)
        for _ in range(SILENT_COOKS):
            post.process(beat_result=det.process(silence, sample_rate=RATE), dt=1.0 / 60.0, audio=silence)
        beats, real, bpm = [], [], []
        prev = False
        n_chunks = len(audio) // STEP
        for k in range(n_chunks):
            chunk = audio[k * STEP:(k + 1) * STEP]
            r = post.process(beat_result=det.process(chunk, sample_rate=RATE), dt=1.0 / 60.0, audio=chunk)
            t = (k + 1) * STEP / RATE
            if r["beat"] and not prev:
                beats.append(t)
                if not r["synth"]:
                    real.append(t)
            prev = r["beat"]
            if k % 2 == 1:
                bpm.append((t, float(r["bpm"])))
        write_dump(out_dir / "lbs" / f"{wav.stem}.jsonl", wav.name, beats, bpm)
        write_dump(out_dir / "lbs_real" / f"{wav.stem}.jsonl", wav.name, real, bpm)
        print(f"{wav.stem}: {len(beats)} beats ({len(real)} real)", flush=True)


if __name__ == "__main__":
    main()
