#!/usr/bin/env python3
"""Validate scripts/capture/demos/ before anything is filmed.

Almost every way a demo preset can be wrong is SILENT at runtime, which is what makes
this worth having as a separate, app-free step:

  * an unknown param key is dropped by `if layer.param_store.values.contains_key(name)`
    (app.rs) with no log line at all;
  * a param outside its declared range is applied anyway — `ParamStore::set` does not
    clamp — and reaches the shader out of bounds;
  * a missing obstacle image is guarded by a bare `if path.exists()` with no else;
  * a binding whose source key does not exist in the snapshot simply never fires, and
    one whose target does not parse is dropped by `apply_binding_target`;
  * a cue naming a preset that is not installed only warns.

So a bad demo does not fail the capture run — it produces a clip that looks like the
default effect and quietly lies about what it is showing. Catch it here instead.

Usage:  scripts/capture/check_demos.py [--quiet]
Exit:   0 clean, 1 problems found (printed to stderr)
"""

import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
DEMOS = REPO / "scripts" / "capture" / "demos"
EFFECTS = REPO / "assets" / "effects"
SOURCES_RS = REPO / "crates" / "fosfora-app" / "src" / "bindings" / "sources.rs"

# Mirrors apply_preset_immediately's cap (app.rs).
MAX_LAYERS = 8

# Loop geometry, kept in step with capture.sh / build_media.sh.
LOOP_BARS, BPM, SETTLE_FRAC = 20, 124, 0.5
LOOP_SECS = LOOP_BARS * 4 * 60 / BPM
TILE_AT, TILE_BARS = 9.75, 2
TILE_DUR = TILE_BARS * 4 * 60 / BPM

problems: list[str] = []
notes: list[str] = []


def bad(msg: str) -> None:
    problems.append(msg)


# --------------------------------------------------------------------- inputs
def load_effects() -> dict:
    """name -> {param name: (kind, min, max)} for every shipped .pfx."""
    out = {}
    for p in sorted(EFFECTS.glob("*.pfx")):
        e = json.loads(p.read_text())
        out[e["name"]] = {
            i["name"]: (i["type"], i.get("min"), i.get("max"))
            for i in (e.get("inputs") or [])
        }
    return out


def load_source_keys() -> set:
    """Every audio source key `collect_audio` can emit, read from sources.rs.

    Parsed rather than hardcoded so a renamed feature shows up here as a broken
    binding instead of as a tile where nothing moves.
    """
    src = SOURCES_RS.read_text()
    keys = set(re.findall(r'"(audio\.[a-z0-9_.]+)"', src))
    # The indexed families are built with format!(), so expand them by hand.
    for n in range(7):
        keys.add(f"audio.band.{n}")
    for n in range(13):
        keys.add(f"audio.mfcc.{n}")
        keys.add(f"audio.dmfcc.{n}")
    for n in range(12):
        keys.add(f"audio.chroma.{n}")
    for n in range(64):
        keys.add(f"audio.mel.{n}")
    return keys


# --------------------------------------------------------------------- checks
def check_target(target: str, layers: list, effects: dict, where: str) -> None:
    """Validate against apply_binding_target's grammar (app.rs)."""
    head, _, rest = target.partition(".")
    if head == "param":
        segs = rest.split(".", 2)
        if len(segs) != 3:
            bad(f"{where}: target '{target}' is not the 4-part param.{{layer}}.{{effect}}.{{name}} form")
            return
        idx, effect, name = segs
        if not idx.isdigit() or int(idx) >= len(layers):
            bad(f"{where}: target '{target}' names layer {idx}, but the preset has {len(layers)}")
            return
        lp = layers[int(idx)]
        if lp["effect_name"] != effect:
            bad(f"{where}: target '{target}' says layer {idx} is '{effect}', preset says '{lp['effect_name']}'")
            return
        params = effects.get(effect, {})
        if name not in params:
            bad(f"{where}: target '{target}' — '{effect}' has no param '{name}'")
        elif params[name][0] not in ("Float", "Bool"):
            bad(f"{where}: target '{target}' — '{name}' is {params[name][0]}; only Float and Bool are bindable")
    elif head == "layer":
        segs = rest.split(".")
        if len(segs) != 2 or not segs[0].isdigit() or segs[1] not in ("opacity", "blend", "enabled"):
            bad(f"{where}: target '{target}' is not layer.{{n}}.{{opacity|blend|enabled}}")
        elif int(segs[0]) >= len(layers):
            bad(f"{where}: target '{target}' names layer {segs[0]}, but the preset has {len(layers)}")
    elif head == "postfx":
        known = {"bloom_threshold", "bloom_intensity", "vignette", "ca_intensity", "grain_intensity"}
        if rest not in known:
            bad(f"{where}: unknown postfx target '{target}'")
    elif head in ("particle", "uniform", "global", "scene"):
        pass  # accepted by apply_binding_target; not worth mirroring in full here
    else:
        bad(f"{where}: unknown target category '{head}' in '{target}'")


def check_preset(path: Path, effects: dict) -> list:
    data = json.loads(path.read_text())
    layers = data.get("layers", [])
    name = path.stem
    if not layers:
        bad(f"{name}: no layers")
    if len(layers) > MAX_LAYERS:
        bad(f"{name}: {len(layers)} layers, the app caps at {MAX_LAYERS}")

    for i, lp in enumerate(layers):
        eff = lp.get("effect_name", "")
        is_media = bool(lp.get("media_path")) or bool(lp.get("webcam_device"))
        if not eff and not is_media:
            bad(f"{name} layer {i}: empty effect_name and no media source")
            continue
        if is_media:
            if eff:
                notes.append(
                    f"{name} layer {i}: has both media_path and effect_name '{eff}'; "
                    "apply_preset_immediately dispatches on media_path first"
                )
        elif eff not in effects:
            bad(f"{name} layer {i}: effect '{eff}' does not ship — the layer would keep whatever was loaded before")
            continue

        for pname, pval in (lp.get("params") or {}).items():
            defs = effects.get(eff, {})
            if pname not in defs:
                bad(f"{name} layer {i}: '{eff}' has no param '{pname}' — it would be silently dropped")
                continue
            kind, lo, hi = defs[pname]
            if kind not in pval:
                bad(f"{name} layer {i}: param '{pname}' is {kind}, preset gives {list(pval)[0]}")
                continue
            if kind == "Float":
                v = pval["Float"]
                if lo is not None and hi is not None and not (lo <= v <= hi):
                    bad(f"{name} layer {i}: {pname} = {v} is outside {eff}'s range {lo}..{hi} (ParamStore::set does not clamp)")

        # Every one of these is guarded by a bare `if path.exists()` with no else
        # branch, so a wrong path produces no log line and no visible error — the
        # effect just renders without it.
        for key in (
            "obstacle_image_path",
            "media_path",
            "splat_scene_path",
            "particle_image_path",
            "particle_video_path",
        ):
            raw = lp.get(key)
            if not raw:
                continue
            if "@REPO@" in raw:
                resolved = Path(raw.replace("@REPO@", str(REPO)))
                if not resolved.exists():
                    bad(f"{name} layer {i}: {key} -> {resolved} does not exist (the app logs nothing for this)")
            elif "@WORK@" not in raw and not Path(raw).exists():
                bad(f"{name} layer {i}: {key} -> {raw} does not exist and is not a @WORK@/@REPO@ placeholder")

        if lp.get("obstacle_image_path") and lp.get("obstacle_fit") is None:
            notes.append(f"{name} layer {i}: no obstacle_fit; restores as Cover, which crops a 1:1 image on a 16:9 canvas")

    if data.get("volumetric", {}).get("enabled"):
        active = data.get("active_layer", 0)
        if active >= len(layers):
            bad(f"{name}: volumetric enabled but active_layer {active} is out of range")
        else:
            eff = layers[active].get("effect_name", "")
            if eff.startswith("Lattice"):
                bad(f"{name}: volumetric enabled on '{eff}' — lattice_enabled short-circuits the volumetric branch")
    return layers


def check_bindings(path: Path, layers: list, effects: dict, source_keys: set) -> int:
    data = json.loads(path.read_text())
    binds = data.get("bindings", [])
    name = path.name
    if data.get("version") != 1:
        bad(f"{name}: version is {data.get('version')}, the loader expects 1")
    for b in binds:
        where = f"{name} [{b.get('id', '?')}]"
        if b.get("scope") != "Preset":
            bad(f"{where}: scope is '{b.get('scope')}'; a sidecar is only loaded for Preset scope")
        src = b.get("source", "")
        if src.startswith("audio.") and src not in source_keys:
            bad(f"{where}: source '{src}' is not collected by sources.rs — it can never fire")
        check_target(b.get("target", ""), layers, effects, where)
    return len(binds)


def main() -> int:
    quiet = "--quiet" in sys.argv
    effects = load_effects()
    source_keys = load_source_keys()

    scene = json.loads((DEMOS / "_scene.json").read_text())
    cues = scene["cues"]

    manifest = []
    for line in (DEMOS / "_manifest.tsv").read_text().splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        parts = line.split("\t")
        manifest.append((parts[0], int(parts[1]), parts[2], parts[3] if len(parts) > 3 else ""))

    preset_files = sorted(p for p in DEMOS.glob("*.json") if not p.name.startswith("_") and not p.name.endswith(".bindings.json"))
    layers_by_name = {}
    for p in preset_files:
        layers_by_name[p.stem] = check_preset(p, effects)

    for p in sorted(DEMOS.glob("*.bindings.json")):
        stem = p.name[: -len(".bindings.json")]
        if stem not in layers_by_name:
            bad(f"{p.name}: no preset named '{stem}' to attach to")
            continue
        n = check_bindings(p, layers_by_name[stem], effects, source_keys)
        for slug, _, pname, want in manifest:
            if pname == stem and want.startswith("Loaded ") and f"Loaded {n} bindings" != want:
                bad(f"_manifest.tsv [{slug}]: expects '{want}' but the sidecar has {n} bindings")

    # Cue <-> preset agreement, both directions.
    cue_names = [c["preset_name"] for c in cues]
    for i, cn in enumerate(cue_names):
        if cn not in layers_by_name:
            bad(f"_scene.json cue {i}: no preset file named '{cn}'")
    for pn in layers_by_name:
        if pn not in cue_names:
            bad(f"'{pn}.json' is not referenced by any cue — it would never be reached")

    # Manifest <-> scene agreement.
    for slug, cue, pname, _ in manifest:
        if cue >= len(cues):
            bad(f"_manifest.tsv [{slug}]: cue {cue} but the scene has {len(cues)}")
        elif cues[cue]["preset_name"] != pname:
            bad(f"_manifest.tsv [{slug}]: cue {cue} is '{cues[cue]['preset_name']}', not '{pname}'")

    # Adjacent cues must not put the same effect on the same layer index: the
    # `already_loaded` morph-safe skip (app.rs) then leaves the particle system alive,
    # carrying obstacle state and unlisted params over from the previous demo.
    for i in range(len(cue_names) - 1):
        a, b = layers_by_name.get(cue_names[i], []), layers_by_name.get(cue_names[i + 1], [])
        for j in range(min(len(a), len(b))):
            ea, eb = a[j].get("effect_name", ""), b[j].get("effect_name", "")
            if ea and ea == eb:
                bad(
                    f"cues {i} and {i + 1} both put '{ea}' on layer {j} — the morph-safe skip "
                    "would carry obstacle state and unlisted params across the cut"
                )

    if not quiet:
        print(f"{len(preset_files)} presets, {len(cues)} cues, {len(manifest)} clips")
        print(f"loop {LOOP_SECS:.3f}s  record opens at {LOOP_SECS * SETTLE_FRAC:.3f}s  "
              f"tile [{TILE_AT:.2f}, {TILE_AT + TILE_DUR:.2f}]s into each clip")
        print(f"total run ~{len(manifest) * LOOP_SECS / 60:.1f} min plus setup\n")
        for slug, cue, pname, want in manifest:
            n = len(layers_by_name.get(pname, []))
            print(f"  cue {cue}  {slug:16s} {pname:22s} {n} layer(s)" + (f"   expect: {want}" if want else ""))
        print()

    for n in notes:
        print(f"note: {n}", file=sys.stderr)
    for p in problems:
        print(f"FAIL: {p}", file=sys.stderr)
    if problems:
        print(f"\n{len(problems)} problem(s)", file=sys.stderr)
        return 1
    print("demos OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
