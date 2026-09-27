#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy", "pillow"]
# ///
"""Render the catalog's pictures (board #3124): one still per shipped effect,
written to assets/thumbs/<pfx stem>.webp, which the v2 catalog shows, and a
2.5 s loop beside it (<pfx stem>.anim.webp) that plays while it is hovered.

Each effect renders through the app's own `--render-scene` (real analysis,
real shaders, real post-processing) against the casting catalog's test track,
so a picture is what the effect looks like in the app — not the raw shader a
probe sees. The scene preset carries the effect's own `postprocess` block,
because the app adopts it when an effect is alone on the stack.

Overlays render over black, which shows their chrome plainly — except the
ones that only trace or tile what is beneath them, which solo render nothing
and get a dimmed base layer (NEEDS_BASE).

From the loud half of the track, the still with the most spread in luma is
kept (a flat frame makes a poor picture). OVERRIDES handles effects whose
default render does not show what they are.

Needs `target/release/fosfora` built with `--features release,analyze`.
    uv run scripts/build_thumbnails.py                 # every effect
    uv run scripts/build_thumbnails.py --effects sumi,tide
    uv run scripts/build_thumbnails.py --anim-only     # previews, stills kept
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image

REPO = Path(__file__).resolve().parent.parent
BIN = REPO / "target" / "release" / "fosfora"
EFFECTS = REPO / "assets" / "effects"
OUT = REPO / "assets" / "thumbs"
TRACK = REPO / "catalog" / "test_track.wav"

# Twice the catalog's 176x99 tile, for HiDPI screens.
SIZE = (352, 198)
QUALITY = 82

# The hover preview: <stem>.anim.webp, a short loop at the same size. Kept
# short and low-rate so the whole set stays a few MB.
ANIM_SECS = 2.5
ANIM_FPS = 12
ANIM_QUALITY = 50

# Overlays that draw from the layers beneath, and what they are shown over:
# dimmed, so the overlay reads as the subject rather than the base.
NEEDS_BASE = {"limn", "intarsia"}
OVERLAY_BASE = "Sumi"
OVERLAY_BASE_OPACITY = 0.5

# Per .pfx stem. "params": Float overrides by param name. "still": take the
# still at this index (0-3: two quiet, two loud) instead of choosing.
# "video": a capture of the effect as the app showed it, and the second to
# take the picture at; "tile": a frame of one of the README's animated tiles
# (assets/media/tiles), by index. For effects the headless renderer cannot
# show (Fluvid needs a camera, Splat's point cloud does not load headless) or
# shows poorly (Accretion is near-black at defaults on the test track). Used
# only when the file is present; otherwise the effect renders headless.
OVERRIDES: dict[str, dict] = {
    "splat": {"video": ("capture-out/splat.mp4", 6.0)},
    # Not the README tile: that is the Videezy dancer, whose license needs a
    # credit beside every showing, and a catalog picture has nowhere to put
    # one. This is Fluvid over Panorama, stirred by a live camera.
    "fluvid": {"video": ("capture-out-preset/Flovid_1_layers_0-1.mp4", 16.0)},
    "accretion": {"tile": ("accretion.webp", 60)},
    "intarsia": {"tile": ("intarsia.webp", 40)},
}


def load_catalog_module():
    spec = importlib.util.spec_from_file_location(
        "build_catalog", REPO / "scripts" / "build_catalog.py"
    )
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def shipped_effects() -> list[tuple[str, dict]]:
    out = []
    for p in sorted(EFFECTS.glob("*.pfx")):
        d = json.loads(p.read_text())
        if d.get("hidden") or d.get("author") != "Fosfora":
            continue
        out.append((p.stem, d))
    return out


def layer(name: str, params: dict[str, float] | None = None, opacity: float = 1.0) -> dict:
    return {
        "effect_name": name,
        "params": {k: {"Float": v} for k, v in (params or {}).items()},
        "blend_mode": "Normal",
        "opacity": opacity,
    }


def write_scene(dir: Path, stem: str, pfx: dict, params: dict[str, float]) -> None:
    dir.mkdir(parents=True)
    layers = [layer(pfx["name"], params)]
    if stem in NEEDS_BASE:
        layers.append(layer(OVERLAY_BASE, opacity=OVERLAY_BASE_OPACITY))
    preset = {"layers": layers, "active_layer": 0}
    if pfx.get("postprocess"):
        preset["postprocess"] = pfx["postprocess"]
    (dir / "thumb.json").write_text(json.dumps(preset, indent=2) + "\n")
    scene = {
        "version": 1,
        "name": f"thumbnail: {pfx['name']}",
        "cues": [{
            "preset_name": "thumb",
            "transition": "Cut",
            "transition_secs": 0.0,
            "hold_secs": 9999.0,
            "label": "thumb",
        }],
    }
    (dir / "_scene.json").write_text(json.dumps(scene, indent=2) + "\n")


def spread(img: Image.Image) -> float:
    return float(np.asarray(img.convert("L"), dtype=np.float32).std())


def frames_of_video(clip: Path, start: float, work: Path, tag: str) -> list[Image.Image]:
    """ANIM_SECS of `clip` from `start`, at ANIM_FPS, as preview-sized frames."""
    d = work / f"frames_{tag}"
    d.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["ffmpeg", "-v", "error", "-y", "-ss", f"{max(start, 0.0):.2f}",
         "-t", str(ANIM_SECS), "-i", str(clip),
         "-vf", f"fps={ANIM_FPS},scale={SIZE[0]}:{SIZE[1]}:flags=lanczos",
         str(d / "%03d.png")],
        check=True,
    )
    return [Image.open(f).convert("RGB") for f in sorted(d.glob("*.png"))]


def frames_of_tile(tile: Path, middle: int) -> list[Image.Image]:
    """About ANIM_SECS of an animated README tile, around frame `middle`."""
    im = Image.open(tile)
    n = getattr(im, "n_frames", 1)
    # The tiles run at their own rate; sample them at ANIM_FPS.
    ms = im.info.get("duration", 1000 // ANIM_FPS) or 1000 // ANIM_FPS
    step = max(1, round((1000 / ANIM_FPS) / ms))
    count = int(ANIM_SECS * ANIM_FPS)
    first = max(0, min(middle - count * step // 2, n - count * step))
    out = []
    for k in range(count):
        im.seek(min(first + k * step, n - 1))
        out.append(im.convert("RGB").resize(SIZE, Image.LANCZOS))
    return out


def render(stem: str, pfx: dict, work: Path) -> tuple[Image.Image, list[Image.Image]] | None:
    """The still, and the frames of the hover preview."""
    ov = OVERRIDES.get(stem, {})
    if "video" in ov:
        clip, at = ov["video"]
        clip = REPO / clip
        if clip.exists():
            frame = work / f"{stem}_frame.png"
            subprocess.run(
                ["ffmpeg", "-v", "error", "-y", "-ss", str(at), "-i", str(clip),
                 "-frames:v", "1", str(frame)],
                check=True,
            )
            anim = frames_of_video(clip, at - ANIM_SECS / 2, work, stem)
            return Image.open(frame).convert("RGB"), anim
        print(f"  {stem}: {clip} missing, rendering headless instead")
    if "tile" in ov:
        name, index = ov["tile"]
        tile = REPO / "assets" / "media" / "tiles" / name
        if tile.exists():
            im = Image.open(tile)
            im.seek(min(index, getattr(im, "n_frames", 1) - 1))
            return im.convert("RGB"), frames_of_tile(tile, index)
        print(f"  {stem}: {tile} missing, rendering headless instead")

    scene = work / f"scene_{stem}"
    out = work / f"out_{stem}"
    write_scene(scene, stem, pfx, ov.get("params", {}))
    r = subprocess.run(
        [str(BIN), "--render-scene", str(scene), "--song", str(TRACK),
         "--out", str(out), "--res", "640x360"],
        cwd=REPO, capture_output=True, text=True, timeout=600,
    )
    # Success is run.json, not the exit code: heavy effects can panic in
    # device teardown after every output is written (see build_catalog.py).
    if not (out / "run.json").exists():
        print(f"  {stem}: render failed\n{r.stderr[-600:]}")
        return None
    stills = sorted((out / "frames").glob("*.png"))
    if not stills:
        print(f"  {stem}: no stills")
        return None
    if "still" in ov:
        pick = stills[ov["still"]]
    else:
        loud = stills[len(stills) // 2:] or stills
        pick = max(loud, key=lambda p: spread(Image.open(p)))
    # The preview comes from the loud section's clip, past its first second
    # (the section boundary is a hard cut in the test track).
    clips = sorted((out / "clips").glob("*.mp4"))
    if not clips:
        print(f"  {stem}: no clips")
        return None
    return Image.open(pick).convert("RGB"), frames_of_video(clips[-1], 1.5, work, stem)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--effects", help="comma-separated .pfx stems")
    ap.add_argument("--anim-only", action="store_true",
                    help="write only the hover previews, leaving the stills as they are")
    args = ap.parse_args()

    if not BIN.exists():
        sys.exit(f"{BIN} missing: cargo build --release --features release,analyze")
    if not TRACK.exists():
        load_catalog_module().synth_test_track(TRACK)

    wanted = set(args.effects.split(",")) if args.effects else None
    OUT.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="fosfora_thumbs_"))
    failed = []
    try:
        for stem, pfx in shipped_effects():
            if wanted and stem not in wanted:
                continue
            print(f"{stem} ({pfx['name']})", flush=True)
            got = render(stem, pfx, work)
            if got is None or not got[1]:
                failed.append(stem)
                continue
            img, anim = got
            if not args.anim_only:
                img = img.resize(SIZE, Image.LANCZOS)
                img.save(OUT / f"{stem}.webp", quality=QUALITY, method=6)
                print(f"  spread {spread(img):.1f}", flush=True)
            path = OUT / f"{stem}.anim.webp"
            anim[0].save(path, save_all=True, append_images=anim[1:],
                         duration=round(1000 / ANIM_FPS), loop=0,
                         quality=ANIM_QUALITY, method=6)
            print(f"  preview {len(anim)} frames, {path.stat().st_size // 1024} KB", flush=True)
    finally:
        shutil.rmtree(work, ignore_errors=True)
    if failed:
        sys.exit(f"failed: {', '.join(failed)}")


if __name__ == "__main__":
    main()
