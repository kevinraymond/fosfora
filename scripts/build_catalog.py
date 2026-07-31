#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy"]
# ///
"""Casting catalog harness (board #2041): render every shipped effect solo,
headless, against a standard test track, so the screenplay realizer can cast
effects by what they actually look like instead of by name.

Per effect: a `default` variant (all Float params at shipped defaults) plus
`<param>_lo` / `<param>_hi` sweeps of its pace-relevant param, if it has one —
pace calibration is the top catalog priority after the "way too fast" verdict
on the first generated scene.

The test track is synthesized (deterministic): a quiet pad-only half then a
loud four-on-floor half, so every variant shows its low-energy and high-energy
face across the section boundary. Renders go through the app's own
`--render-scene` (real analysis, real bindings bus, real shaders); this script
never re-implements any of the app's grammar.

Outputs under catalog/renders/<Effect>/<variant>/: the renderer's clips/,
frames/, run.json, plus motion.json (mean inter-frame difference per clip —
the objective pace number). catalog/renders/summary.json aggregates everything
and is flushed after every effect, so a killed run keeps its partials and
--skip-existing resumes it.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import shutil
import subprocess
import sys
import time
import wave
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parent.parent
BIN = REPO / "target" / "release" / "phosphor-app"
CATALOG = REPO / "catalog"
RENDERS = CATALOG / "renders"
TRACK = CATALOG / "test_track.wav"
CAPS = CATALOG / "capabilities.json"

SR = 44100
BPM = 128.0
QUIET_SECS = 14.0
TOTAL_SECS = 32.0

PACE_TOKENS = {
    "speed", "rate", "velocity", "flow", "drift", "scroll",
    "spin", "rotation", "rot", "pace",
}


# ---------------------------------------------------------------- test track

def synth_test_track(path: Path) -> None:
    """32 s, 44.1 kHz stereo: 0-14 s soft detuned pad + sub pulse (quiet),
    14-32 s kick/bass/hats/bright pad at 128 BPM (loud). The hard switch at
    14 s is deliberate — it is the section boundary the analysis should find."""
    rng = np.random.default_rng(2027)
    n = int(TOTAL_SECS * SR)
    t = np.arange(n) / SR
    beat = 60.0 / BPM

    def saw(freq: float, detune: float = 0.0) -> np.ndarray:
        ph = (t * freq * (1.0 + detune)) % 1.0
        return 2.0 * ph - 1.0

    def dull(x: np.ndarray, taps: int) -> np.ndarray:
        k = np.hanning(taps)
        return np.convolve(x, k / k.sum(), mode="same")

    # Pad: A minor triad, L/R detuned for stereo width.
    chord = [220.0, 261.63, 329.63]
    pad_l = sum(saw(f, -0.003) for f in chord) / len(chord)
    pad_r = sum(saw(f, +0.003) for f in chord) / len(chord)
    pad_quiet = (dull(pad_l, 256), dull(pad_r, 256))
    pad_bright = (dull(pad_l, 32), dull(pad_r, 32))

    kick = np.zeros(n)
    hats = np.zeros(n)
    first_beat = QUIET_SECS
    b = first_beat
    while b < TOTAL_SECS - 0.1:
        i = int(b * SR)
        tau = np.arange(int(0.25 * SR)) / SR
        body = np.sin(2 * math.pi * (120 * tau - 90 * tau * tau)) * np.exp(-tau * 22)
        kick[i:i + len(body)] += body[: n - i]
        hi = int((b + beat / 2) * SR)
        burst = rng.standard_normal(int(0.03 * SR)) * np.exp(
            -np.arange(int(0.03 * SR)) / SR * 120
        )
        burst = np.diff(burst, prepend=0.0)  # crude highpass
        hats[hi:hi + len(burst)] += burst[: n - hi]
        b += beat

    # Bass: eighth-note gated saw at A1, only in the loud half.
    bass = saw(55.0)
    gate = ((t - QUIET_SECS) % (beat / 2) < beat * 0.35) & (t >= QUIET_SECS)
    bass = dull(bass * gate, 24)

    # Sub pulse for the quiet half: one soft swell every two beats.
    sub = np.sin(2 * math.pi * 41.2 * t) * (
        0.5 - 0.5 * np.cos(2 * math.pi * (t / (2 * beat)) % (2 * math.pi))
    ) * (t < QUIET_SECS)

    quiet = t < QUIET_SECS
    loud = ~quiet
    mix_l = (
        quiet * (0.22 * pad_quiet[0] + 0.10 * sub)
        + loud * (0.9 * kick + 0.35 * bass + 0.22 * hats + 0.30 * pad_bright[0])
    )
    mix_r = (
        quiet * (0.22 * pad_quiet[1] + 0.10 * sub)
        + loud * (0.9 * kick + 0.35 * bass + 0.22 * hats + 0.30 * pad_bright[1])
    )

    fade = np.minimum(1.0, np.minimum(t / 0.05, (TOTAL_SECS - t) / 0.05))
    stereo = np.stack([mix_l * fade, mix_r * fade], axis=1)
    stereo *= 0.9 / np.abs(stereo).max()
    pcm = (stereo * 32767).astype(np.int16)

    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(pcm.tobytes())


# ---------------------------------------------------------------- variants

def dump_schema() -> dict:
    CAPS.parent.mkdir(parents=True, exist_ok=True)
    r = subprocess.run(
        [str(BIN), "--dump-schema", "--out", str(CAPS)],
        cwd=REPO, capture_output=True, text=True,
    )
    if r.returncode != 0 or not CAPS.exists():
        sys.exit(f"--dump-schema failed:\n{r.stderr}")
    return json.loads(CAPS.read_text())


def pace_param(effect: dict) -> dict | None:
    """The one Float param whose name most says 'pace', token-matched so
    'saturate' never matches 'rate'."""
    floats = [i for i in effect["inputs"] if i["type"] == "Float"]
    best, best_rank = None, 99
    for inp in floats:
        tokens = re.split(r"[_\W]+", inp["name"].lower())
        if inp["name"].lower() == "speed":
            rank = 0
        elif "speed" in tokens:
            rank = 1
        elif any(tk in PACE_TOKENS for tk in tokens):
            rank = 2
        else:
            continue
        if rank < best_rank:
            best, best_rank = inp, rank
    return best


def variants_for(effect: dict) -> list[tuple[str, dict[str, float]]]:
    defaults = {
        i["name"]: float(i["default"])
        for i in effect["inputs"]
        if i["type"] == "Float"
    }
    out = [("default", defaults)]
    p = pace_param(effect)
    if p is not None:
        lo = p["min"] + 0.15 * (p["max"] - p["min"])
        hi = p["min"] + 0.90 * (p["max"] - p["min"])
        span = p["max"] - p["min"]
        for tag, v in ((f"{p['name']}_lo", lo), (f"{p['name']}_hi", hi)):
            if span > 0 and abs(v - defaults[p["name"]]) > 0.05 * span:
                out.append((tag, {**defaults, p["name"]: v}))
    return out


def write_scene_dir(dir: Path, effect_name: str, params: dict[str, float]) -> None:
    if dir.exists():
        shutil.rmtree(dir)
    dir.mkdir(parents=True)
    preset = {
        "layers": [{
            "effect_name": effect_name,
            "params": {k: {"Float": v} for k, v in params.items()},
            "blend_mode": "Normal",
            "opacity": 1.0,
        }],
        "active_layer": 0,
    }
    stem = effect_name.replace("/", "-")
    (dir / f"{stem}.json").write_text(json.dumps(preset, indent=2) + "\n")
    scene = {
        "version": 1,
        "name": f"catalog: {effect_name}",
        "cues": [{
            "preset_name": stem,
            "transition": "Cut",
            "transition_secs": 0.0,
            "hold_secs": 9999.0,
            "label": "catalog",
        }],
    }
    (dir / "_scene.json").write_text(json.dumps(scene, indent=2) + "\n")


# ---------------------------------------------------------------- motion

def motion_stats(clip: Path) -> dict:
    """Mean inter-frame absolute difference (0-1) on 320x180 grayscale — the
    objective 'how fast does this read' number — plus luma exposure stats."""
    r = subprocess.run(
        ["ffmpeg", "-v", "error", "-i", str(clip),
         "-f", "rawvideo", "-pix_fmt", "gray", "-s", "320x180", "pipe:1"],
        capture_output=True,
    )
    if r.returncode != 0 or not r.stdout:
        return {"error": r.stderr.decode(errors="replace")[-300:]}
    frames = np.frombuffer(r.stdout, dtype=np.uint8)
    count = len(frames) // (320 * 180)
    frames = frames[: count * 320 * 180].reshape(count, 180, 320).astype(np.float32) / 255.0
    if count < 2:
        return {"frames": count}
    d = np.abs(np.diff(frames, axis=0)).mean(axis=(1, 2))
    return {
        "frames": count,
        "motion_mean": round(float(d.mean()), 5),
        "motion_p95": round(float(np.percentile(d, 95)), 5),
        "luma_mean": round(float(frames.mean()), 4),
        "luma_std": round(float(frames.std()), 4),
    }


# ---------------------------------------------------------------- main

def render_variant(scene_dir: Path, out_dir: Path) -> tuple[bool, str]:
    r = subprocess.run(
        [str(BIN), "--render-scene", str(scene_dir), "--song", str(TRACK),
         "--out", str(out_dir), "--res", "640x360"],
        cwd=REPO, capture_output=True, text=True, timeout=600,
    )
    return r.returncode == 0, (r.stderr or "")[-500:]


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--effects", help="comma-separated effect-name filter")
    ap.add_argument("--limit", type=int, help="stop after N effects")
    ap.add_argument("--force", action="store_true",
                    help="re-render variants that already have a run.json")
    args = ap.parse_args()

    if not BIN.exists():
        sys.exit(f"{BIN} missing — cargo build --release --features analyze first")
    if not TRACK.exists():
        print("synthesizing test track...")
        synth_test_track(TRACK)

    caps = dump_schema()
    effects = [e for e in caps["effects"] if not e.get("hidden")]
    if args.effects:
        wanted = {w.strip().lower() for w in args.effects.split(",")}
        effects = [e for e in effects if e["name"].lower() in wanted]
    if args.limit:
        effects = effects[: args.limit]

    summary_path = RENDERS / "summary.json"
    summary: dict = {}
    if summary_path.exists():
        summary = json.loads(summary_path.read_text())

    scenes = RENDERS / "_scenes"
    for ei, effect in enumerate(effects):
        name = effect["name"]
        entry = summary.setdefault(name, {
            "effect_type": effect.get("effect_type"),
            "description": effect.get("description"),
            "variants": {},
        })
        for tag, params in variants_for(effect):
            out_dir = RENDERS / name.replace("/", "-") / tag
            if not args.force and (out_dir / "run.json").exists():
                continue
            scene_dir = scenes / f"{name.replace('/', '-')}__{tag}"
            write_scene_dir(scene_dir, name, params)
            t0 = time.time()
            ok, err = render_variant(scene_dir, out_dir)
            var: dict = {"ok": ok, "secs": round(time.time() - t0, 1)}
            if not ok:
                var["error"] = err
            else:
                run = json.loads((out_dir / "run.json").read_text())
                var["warnings"] = run.get("warnings", [])
                clips = sorted((out_dir / "clips").glob("*.mp4"))
                var["clips"] = {c.name: motion_stats(c) for c in clips}
                (out_dir / "motion.json").write_text(
                    json.dumps(var["clips"], indent=2) + "\n")
            entry["variants"][tag] = var
            state = "ok" if ok else "FAILED"
            print(f"[{ei + 1}/{len(effects)}] {name}/{tag}: {state} "
                  f"({var['secs']}s)", flush=True)
        summary_path.parent.mkdir(parents=True, exist_ok=True)
        summary_path.write_text(json.dumps(summary, indent=2) + "\n")

    done = sum(1 for e in summary.values() for v in e["variants"].values() if v["ok"])
    failed = sum(1 for e in summary.values() for v in e["variants"].values() if not v["ok"])
    print(f"catalog renders: {done} ok, {failed} failed → {summary_path}")


if __name__ == "__main__":
    main()
