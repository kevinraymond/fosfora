#!/usr/bin/env python3
"""Author a Fosfora scene from an offline song analysis (#2027, dev-side tool).

    ./target/release/phosphor-app --analyze song.mp3 --out song.analysis.json
    ./target/release/phosphor-app --dump-schema --out capabilities.json
    uv run --with anthropic scripts/generate_scene.py \
        --analysis song.analysis.json --capabilities capabilities.json --out gen/
    ./target/release/phosphor-app --validate gen/

Nothing here ships to users: it reads two JSON files the `analyze`-gated
subcommands produce and writes preset/bindings/scene JSON for Kevin to edit.

THREE DESIGN DECISIONS, because each replaces a class of failure:

1. The model never writes a `Preset`. Strict JSON Schema requires
   `additionalProperties: false` and cannot express `params: HashMap<String,
   ParamValue>` — arbitrary keys whose values are an *externally* tagged enum
   (`{"Float": 0.5}`). So the model emits a flat intermediate (params as a list
   of {name, type, value}) and this script transforms it into the real shapes.

2. Every closed set is an `enum` in the schema, filled in from
   capabilities.json at runtime: effect names, binding sources, blend modes,
   uniform/postfx/particle leaf names, curve types, transitions. Structured
   outputs guarantee the response validates, so a hallucinated effect name or
   dead source key becomes *unrepresentable* rather than something the validator
   catches afterwards.

3. Binding targets are emitted structurally ({kind, layer, param} …) and the
   dotted string is assembled here. The model never writes `param.0.Aurora.speed`
   by hand, so it cannot typo the grammar — which matters more than usual because
   `parse_target` is infallible and a malformed target loads silently inert.

What is left for `--validate` is what a schema cannot know: whether a param name
exists on the effect that layer actually runs, whether a value sits inside that
param's declared range, and whether the cue list stalls. Those come back as
repair feedback (--attempts).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

MODEL = "claude-opus-5"
# Streaming is required at this size; the SDK refuses long non-streaming requests.
MAX_TOKENS = 32000
EFFORT = "high"

# Only Float and Bool params are emitted. Across the 50 shipped .pfx files that
# is 304 of 305 params (the one Point2D is not worth a union in the schema), and
# they are also exactly the two types a binding can drive.
PARAM_TYPES = ["Float", "Bool"]

# Raw per-coefficient families are dropped from the source enum by default.
# `audio.mel.37` is essentially never the right thing to hang a look on, and
# offering 64 mel bands, 13 MFCCs and 13 delta-MFCCs (90 of the 159 keys) invites
# a bad pick as much as it enables a good one. It also matters mechanically: the
# enum is inlined into the compiled output grammar, and the full list overruns it.
# `audio.band.0..6` and the 12 chroma pitch classes stay — those are musical.
NOISY_SOURCE_FAMILIES = ("audio.mel.", "audio.mfcc.", "audio.dmfcc.")


# Which fields each transform type and target kind requires. The schema cannot
# encode this (see build_schema), so it is enforced here and stated in the prompt.
# Mirrors TransformDef in bindings/types.rs and parse_target's grammar.
TRANSFORM_FIELDS: dict[str, tuple[str, ...]] = {
    "remap": ("in_lo", "in_hi", "out_lo", "out_hi"),
    "smooth": ("factor",),
    "invert": (),
    "quantize": ("steps",),
    "deadzone": ("lo", "hi"),
    "curve": ("curve_type",),
    "gate": ("threshold",),
    "scale": ("factor",),
    "offset": ("value",),
    "clamp": ("lo", "hi"),
}

TARGET_FIELDS: dict[str, tuple[str, ...]] = {
    "param": ("layer", "param"),
    "layer": ("layer", "layer_field"),
    "postfx": ("postfx",),
    "particle": ("particle",),
    "uniform": ("uniform",),
    "global_master_opacity": (),
}


def field_rules() -> str:
    """The tables above, for the system prompt."""
    tf = "\n".join(
        f"  {t:10} {', '.join(f) if f else '(no other fields)'}"
        for t, f in TRANSFORM_FIELDS.items()
    )
    tk = "\n".join(
        f"  {k:22} {', '.join(f) if f else '(no other fields)'}"
        for k, f in TARGET_FIELDS.items()
    )
    return (
        "Each transform `type` requires exactly these fields, and omitting one "
        "makes the whole bindings file fail to load:\n" + tf +
        "\n\nEach target `kind` requires exactly these fields:\n" + tk
    )


def curate_sources(sources: list[str], keep_all: bool) -> list[str]:
    if keep_all:
        return sources
    return [k for k in sources if not k.startswith(NOISY_SOURCE_FAMILIES)]


# ----------------------------------------------------------------- the schema
def build_schema(caps: dict[str, Any], all_sources: bool = False) -> dict[str, Any]:
    """A JSON Schema whose enums are this build's actual vocabulary.

    NOTE ON `required`: structured outputs reject a schema with more than 24
    *optional* fields ("grammar compilation inefficient"). Container fields are
    therefore required-but-emptyable (`params: []`, `bindings: []`,
    `param_overrides: []`) rather than optional — that costs the model nothing and
    buys back the whole budget, which is spent on the two objects that genuinely
    vary by variant: `transform` and `binding_target`. Do not "tidy" these back to
    optional; it reintroduces the 400.

    Every `enum` below is read from capabilities.json rather than hardcoded, so
    the schema tracks the app: a new effect or audio feature widens it on the
    next --dump-schema with no change here.
    """
    effect_names = [e["name"] for e in caps["effects"] if not e["hidden"]]
    targets = caps["targets"]
    enums = caps["enums"]

    param_list = {
        "type": "array",
        "description": (
            "Params to set, by name. Only params the effect declares; the value "
            "must sit inside the declared min..max. Bool params take 0 or 1."
        ),
        "items": {
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "type": {"enum": PARAM_TYPES},
                "value": {"type": "number"},
            },
            "required": ["name", "type", "value"],
            "additionalProperties": False,
        },
    }

    # NOTE: `transform` and `binding_target` are flat objects with optional
    # fields, not `anyOf` over per-variant objects, and that is a forced choice
    # rather than a preference.
    #
    # As `anyOf` these would be strictly better: each variant makes its own fields
    # required, so `{"type": "deadzone"}` with no `lo`/`hi` — which the first real
    # run emitted, and which the Rust `TransformDef` then refused to deserialize —
    # becomes unrepresentable. But the compiled output grammar for that shape
    # overruns the API's size limit ("The compiled grammar is too large"), even
    # after curating the source enum from 159 keys to 69. Flat compiles; anyOf does
    # not.
    #
    # So the variant-completeness rule lives in Python instead
    # (TRANSFORM_FIELDS / TARGET_FIELDS below), is stated in the system prompt so
    # the model knows it up front, and is checked before anything is written. The
    # two limits pull against each other: fewer optional fields (limit 24) means
    # more required ones, and required fields are what inflate the grammar.
    binding_target = {
        "type": "object",
        "description": (
            "Where the binding sends its value. Set `kind`, then EXACTLY the fields "
            "that kind requires (see the system prompt); the dotted target string "
            "is assembled downstream."
        ),
        "properties": {
            "kind": {"enum": list(TARGET_FIELDS)},
            "layer": {
                "type": "integer",
                "description": "Layer index, bottom-first. For kind=param and kind=layer.",
            },
            "param": {
                "type": "string",
                "description": "A param declared by the effect on that layer. For kind=param.",
            },
            "layer_field": {"enum": list(targets["layer_fields"])},
            "postfx": {"enum": list(targets["postfx"])},
            "particle": {"enum": list(targets["particle"])},
            "uniform": {"enum": list(targets["uniform"])},
        },
        "required": ["kind"],
        "additionalProperties": False,
    }

    transform = {
        "type": "object",
        "description": (
            "Shaping applied in order between source and target. Set `type`, then "
            "EXACTLY the fields that type requires (see the system prompt) — a "
            "missing field makes the whole bindings file unloadable."
        ),
        "properties": {
            "type": {"enum": list(TRANSFORM_FIELDS)},
            "in_lo": {"type": "number"},
            "in_hi": {"type": "number"},
            "out_lo": {"type": "number"},
            "out_hi": {"type": "number"},
            "factor": {"type": "number"},
            "steps": {"type": "integer"},
            "lo": {"type": "number"},
            "hi": {"type": "number"},
            "threshold": {"type": "number"},
            "value": {"type": "number"},
            "curve_type": {"enum": list(enums["curve_types"])},
        },
        "required": ["type"],
        "additionalProperties": False,
    }

    binding = {
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Short human label, shown in the binding matrix.",
            },
            "source": {
                "enum": curate_sources(caps["sources"], all_sources),
                "description": "Audio feature driving this binding.",
            },
            "target": binding_target,
            "transforms": {"type": "array", "items": transform},
        },
        "required": ["name", "source", "target", "transforms"],
        "additionalProperties": False,
    }

    layer = {
        "type": "object",
        "properties": {
            "effect": {"enum": effect_names},
            "blend_mode": {"enum": list(enums["blend_modes"])},
            "opacity": {"type": "number"},
            "params": param_list,
        },
        "required": ["effect", "blend_mode", "opacity", "params"],
        "additionalProperties": False,
    }

    preset = {
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Preset name; cues reference it, and it becomes the filename.",
            },
            "for_section_label": {
                "type": "string",
                "description": "Which section identity (A, B, C…) this look is for.",
            },
            "rationale": {
                "type": "string",
                "description": (
                    "One or two sentences: which measured features of this section "
                    "drove this choice of effect, params and bindings."
                ),
            },
            "layers": {
                "type": "array",
                "description": f"1..{caps['limits']['max_layers']} layers, bottom-first.",
                "items": layer,
            },
            "bindings": {"type": "array", "items": binding},
        },
        "required": ["name", "for_section_label", "rationale", "layers", "bindings"],
        "additionalProperties": False,
    }

    override_group = {
        "type": "object",
        "properties": {
            "layer": {"type": "integer"},
            "params": param_list,
        },
        "required": ["layer", "params"],
        "additionalProperties": False,
    }

    cue = {
        "type": "object",
        "properties": {
            "preset": {"type": "string", "description": "Name of one of the presets above."},
            "label": {"type": "string"},
            "hold_secs": {
                "type": "number",
                "description": (
                    "Seconds to hold. REQUIRED on every cue: under Timer advance a "
                    "cue without it never advances. Use the section's duration."
                ),
            },
            "transition": {"enum": list(enums["transitions"])},
            "transition_secs": {"type": "number"},
            "param_overrides": {
                "type": "array",
                "description": (
                    "Per-layer param values for this cue only. This is how one "
                    "preset serves several sections of the same identity at "
                    "different intensities."
                ),
                "items": override_group,
            },
        },
        "required": [
            "preset",
            "label",
            "hold_secs",
            "transition",
            "transition_secs",
            "param_overrides",
        ],
        "additionalProperties": False,
    }

    return {
        "type": "object",
        "properties": {
            "scene_name": {"type": "string"},
            "reading": {
                "type": "string",
                "description": (
                    "A short paragraph on how you read this song's structure and "
                    "what visual arc you chose. Written for a human reviewer."
                ),
            },
            "presets": {"type": "array", "items": preset},
            "cues": {
                "type": "array",
                "description": "One cue per section, in order, covering the whole song.",
                "items": cue,
            },
        },
        "required": ["scene_name", "reading", "presets", "cues"],
        "additionalProperties": False,
    }


# ----------------------------------------------------------------- the prompt
SYSTEM_INSTRUCTIONS = """\
You are authoring a scene for Fosfora, an audio-reactive VJ engine, from an \
offline analysis of one song. You will be given the engine's full capabilities \
and the song's measured structure, and you emit presets, audio bindings and a \
timed cue list.

The capabilities document is authoritative and complete. It was dumped by the \
build this scene will run on, so every effect, param, param range and audio \
source in it exists, and anything not in it does not.

How to do this well:

READ THE SONG FIRST. Sections carry a `label` (A, B, C…): sections sharing a \
label are the same musical identity recurring, so they should share a look. \
Author one preset per distinct label, then vary it per cue with \
`param_overrides` where the measured descriptors differ — that is what makes a \
recurring chorus feel like the same chorus, only bigger the third time.

BIND TO FEATURES THAT ACTUALLY MOVE ON THIS SONG. Each section carries \
descriptors, and the request tells you their spread across the song. A feature \
with a wide spread is a real lever; one with a narrow spread is an honest \
ordering of an inaudible difference and will look like nothing. Prefer the wide \
ones. `energy_rank` is a rank across sections, useful for choosing per-cue \
intensity, not as a binding source.

BINDINGS ARE THE POINT. A scene with no bindings is a slideshow. Every preset \
should have several, and they should be legible: the thing the ear notices and \
the thing the eye notices should be the same thing. Bus values arrive \
normalized 0..1 and are scaled into each param's declared range, so bind to \
what should move, and use transforms to shape the response rather than to \
rescale it.

PREFER FEWER, BETTER LAYERS. Two or three layers that read clearly beat eight \
that fight. Blend modes matter: `Add` and `Screen` accumulate light, `Multiply` \
darkens, and the last three modes (Displace, Refract, Lens) make a layer warp \
what is beneath it rather than draw itself.

COVER THE WHOLE SONG. One cue per section, in order, with `hold_secs` set from \
that section's duration so the cue list tracks the music. Every cue needs \
`hold_secs`.

Explain your reasoning in `reading` and in each preset's `rationale`, in terms \
of the measured numbers you were given. A reviewer will read those to decide \
whether to keep the scene.
"""


def compact(obj: Any) -> str:
    """Minified JSON. The corpus is ~107KB pretty-printed and is sent on every
    request, so the whitespace is worth removing."""
    return json.dumps(obj, separators=(",", ":"), ensure_ascii=False)


def build_system(caps: dict[str, Any]) -> list[dict[str, Any]]:
    """System blocks, stable-first, with the cache breakpoint on the last one.

    Render order is tools -> system -> messages, so putting the whole corpus here
    and the per-song analysis in the user turn means the cached prefix is
    identical for every song and every repair attempt. Nothing volatile (no
    timestamps, no song name) may appear before the breakpoint.
    """
    return [
        {"type": "text", "text": SYSTEM_INSTRUCTIONS + "\n" + field_rules() + "\n"},
        {
            "type": "text",
            "text": "Engine capabilities (authoritative):\n" + compact(caps),
            # 1h rather than the 5m default: the corpus is byte-stable across
            # songs, so a session of several generations reads it repeatedly.
            "cache_control": {"type": "ephemeral", "ttl": "1h"},
        },
    ]


def spreads(analysis: dict[str, Any]) -> str:
    """Per-descriptor spread across sections, so the model can tell a real lever
    from an honest ordering of an inaudible difference."""
    sections = analysis["sections"]
    if not sections:
        return "(no sections)"
    keys = sorted(sections[0]["descriptors"])
    rows = []
    for k in keys:
        vals = [s["descriptors"][k] for s in sections]
        rows.append(f"  {k:20} spread {max(vals) - min(vals):.3f}   ({min(vals):.2f} .. {max(vals):.2f})")
    return "\n".join(rows)


def build_user(analysis: dict[str, Any], style: str | None) -> str:
    g, src = analysis["global"], analysis["source"]
    parts = [
        f"Song: {Path(src['path']).name}",
        f"{src['duration_secs']:.1f}s, {g['bpm']:.1f} BPM, "
        f"key class {g['key_class']} ({'minor' if g['key_is_minor'] else 'major'}, "
        f"agreement {g['key_agreement']:.2f}), "
        f"{len(analysis['sections'])} sections in {g['cluster_count']} identities.",
        "",
        "Descriptor spread across sections — wide spreads are the levers that will read:",
        spreads(analysis),
        "",
        "Full analysis:",
        compact(analysis),
    ]
    if style:
        parts += ["", f"Direction from the operator: {style}"]
    parts += [
        "",
        "Author the scene: one preset per section identity, bindings on each, and "
        "one cue per section in order covering the whole song.",
    ]
    return "\n".join(parts)


# ------------------------------------------------------- intermediate -> real
def check_variants(result: dict[str, Any]) -> list[str]:
    """Fields the schema could not require, checked before emitting.

    The flat schema lets a variant arrive incomplete (see build_schema). Without
    this, `{"type": "deadzone"}` reaches disk and surfaces as
    `missing field lo` from the Rust loader — true, but two steps from the
    cause and with no indication of which binding.
    """
    problems: list[str] = []
    for preset in result["presets"]:
        for b in preset.get("bindings", []):
            where = f"preset {preset['name']!r} binding {b['name']!r}"
            kind = b["target"].get("kind")
            for f in TARGET_FIELDS.get(kind, ()):
                if f not in b["target"]:
                    problems.append(f"{where}: target kind '{kind}' requires '{f}'")
            for i, t in enumerate(b.get("transforms", [])):
                ty = t.get("type")
                for f in TRANSFORM_FIELDS.get(ty, ()):
                    if f not in t:
                        problems.append(
                            f"{where}: transforms[{i}] type '{ty}' requires '{f}' "
                            f"(needs {', '.join(TRANSFORM_FIELDS[ty])})"
                        )
    return problems


def assemble_target(t: dict[str, Any]) -> str:
    """Structured target -> the dotted string the app parses.

    Doing this here rather than in the model is the point: `parse_target` is
    infallible, so a typo would become `Unknown(raw)` and load silently inert.
    """
    kind = t["kind"]
    if kind == "param":
        return f"param.{t['layer']}.{t['effect']}.{t['param']}"
    if kind == "layer":
        return f"layer.{t['layer']}.{t['layer_field']}"
    if kind == "global_master_opacity":
        return "global.master_opacity"
    if kind in ("postfx", "particle", "uniform"):
        return f"{kind}.{t[kind]}"
    raise ValueError(f"unknown target kind {kind!r}")


def param_value(p: dict[str, Any]) -> dict[str, Any]:
    """Flat {name,type,value} -> the externally tagged ParamValue the app reads."""
    if p["type"] == "Bool":
        return {"Bool": bool(p["value"])}
    return {"Float": float(p["value"])}


def sanitize(name: str) -> str:
    """Mirror PresetStore's filename sanitizer: `/`, `\\` and `.` become `_`, then
    trim and cap at 64 bytes. A name that survives this is one the app can save."""
    out = re.sub(r"[/\\.]", "_", name).strip()
    return out.encode("utf-8")[:64].decode("utf-8", "ignore")


def emit(result: dict[str, Any], outdir: Path) -> list[Path]:
    outdir.mkdir(parents=True, exist_ok=True)
    written: list[Path] = []

    # A layer's effect name is needed to assemble `param.{layer}.{effect}.{param}`
    # targets; the model gives us the layer index, and the effect comes from the
    # preset it is describing.
    for preset in result["presets"]:
        name = sanitize(preset["name"])
        layers = []
        for layer in preset["layers"]:
            lp: dict[str, Any] = {"effect_name": layer["effect"]}
            if layer.get("params"):
                lp["params"] = {p["name"]: param_value(p) for p in layer["params"]}
            if "blend_mode" in layer:
                lp["blend_mode"] = layer["blend_mode"]
            if "opacity" in layer:
                lp["opacity"] = layer["opacity"]
            layers.append(lp)

        preset_path = outdir / f"{name}.json"
        preset_path.write_text(
            json.dumps({"layers": layers, "active_layer": 0}, indent=2) + "\n"
        )
        written.append(preset_path)

        bindings = []
        for i, b in enumerate(preset.get("bindings", [])):
            target = dict(b["target"])
            if target["kind"] == "param":
                idx = target.get("layer", 0)
                if idx >= len(preset["layers"]):
                    # Out of range: --validate would report it, but the target
                    # string cannot even be assembled without an effect name.
                    raise ValueError(
                        f"preset {name!r} binding {b['name']!r} targets layer {idx}, "
                        f"but the preset has {len(preset['layers'])}"
                    )
                target["effect"] = preset["layers"][idx]["effect"]
            bindings.append(
                {
                    "id": f"b_{i:03}",
                    "name": b["name"],
                    "enabled": True,
                    # Preset scope, always: save_preset_bindings only writes
                    # Preset-scoped bindings into a sidecar, so a Global one here
                    # would never be applied with the preset.
                    "scope": "Preset",
                    "source": b["source"],
                    "target": assemble_target(target),
                    "transforms": b.get("transforms", []),
                }
            )
        if bindings:
            sidecar = outdir / f"{name}.bindings.json"
            sidecar.write_text(
                json.dumps({"version": 1, "bindings": bindings}, indent=2) + "\n"
            )
            written.append(sidecar)

    by_name = {p["name"]: sanitize(p["name"]) for p in result["presets"]}
    cues = []
    for c in result["cues"]:
        cue: dict[str, Any] = {
            "preset_name": by_name.get(c["preset"], sanitize(c["preset"])),
            "transition": c.get("transition", "Cut"),
            "transition_secs": c.get("transition_secs", 1.0),
            # Always Some: under Timer a None cue holds forever.
            "hold_secs": c["hold_secs"],
        }
        if c.get("label"):
            cue["label"] = c["label"]
        if c.get("param_overrides"):
            # Positional per-layer list, so it has to be densified: entry 0
            # overrides layer 0. The model addresses layers by index, which may
            # be sparse.
            top = max(g["layer"] for g in c["param_overrides"])
            dense: list[dict[str, Any]] = [{} for _ in range(top + 1)]
            for g in c["param_overrides"]:
                dense[g["layer"]] = {p["name"]: param_value(p) for p in g["params"]}
            cue["param_overrides"] = dense
        cues.append(cue)

    scene_path = outdir / "_scene.json"
    scene_path.write_text(
        json.dumps(
            {
                "version": 1,
                "name": result["scene_name"],
                "cues": cues,
                "loop_mode": False,
                "advance_mode": "Timer",
            },
            indent=2,
        )
        + "\n"
    )
    written.append(scene_path)
    return written


# -------------------------------------------------------------------- the run
def run_validator(binary: Path, outdir: Path) -> tuple[bool, str]:
    proc = subprocess.run(
        [str(binary), "--validate", str(outdir)],
        capture_output=True,
        text=True,
    )
    if proc.returncode == 2:
        raise SystemExit(f"validator usage error: {proc.stdout}{proc.stderr}")
    # Notes (template placeholders) are not faults; drop them from feedback.
    lines = [
        ln
        for ln in proc.stdout.splitlines()
        if ln.strip() and not ln.startswith("note:")
    ]
    return proc.returncode == 0, "\n".join(lines)


def call_model(client, system, messages, schema, stream_to_stderr: bool):
    """One request. Streamed because MAX_TOKENS is well past the SDK's
    non-streaming timeout guard."""
    with client.messages.stream(
        model=MODEL,
        max_tokens=MAX_TOKENS,
        system=system,
        messages=messages,
        output_config={
            "effort": EFFORT,
            "format": {"type": "json_schema", "schema": schema},
        },
    ) as stream:
        if stream_to_stderr:
            for text in stream.text_stream:
                sys.stderr.write(".")
                sys.stderr.flush()
                del text
            sys.stderr.write("\n")
        message = stream.get_final_message()

    if message.stop_reason == "refusal":
        raise SystemExit(f"model declined: {getattr(message, 'stop_details', None)}")
    if message.stop_reason == "max_tokens":
        raise SystemExit(
            f"hit max_tokens ({MAX_TOKENS}) — output truncated, raise MAX_TOKENS"
        )
    return message


def usage_line(message) -> str:
    u = message.usage
    return (
        f"in {u.input_tokens} | cache write {u.cache_creation_input_tokens} "
        f"| cache read {u.cache_read_input_tokens} | out {u.output_tokens}"
    )


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--analysis", required=True, type=Path)
    ap.add_argument("--capabilities", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--style", help="free-text direction, e.g. 'restrained, cold palette'")
    ap.add_argument(
        "--binary",
        type=Path,
        default=Path("target/release/phosphor-app"),
        help="phosphor-app built with --features analyze, for --validate",
    )
    ap.add_argument(
        "--attempts",
        type=int,
        default=3,
        help="validation repair rounds (1 = emit once, do not repair)",
    )
    ap.add_argument(
        "--all-sources",
        action="store_true",
        help=(
            "offer every source key including mel/mfcc/dmfcc coefficients "
            "(may overrun the output-grammar size limit)"
        ),
    )
    ap.add_argument(
        "--dry-run",
        action="store_true",
        help="assemble the prompt, count tokens, and stop without generating",
    )
    args = ap.parse_args()

    analysis = json.loads(args.analysis.read_text())
    caps = json.loads(args.capabilities.read_text())

    if caps.get("analysis_version") != analysis.get("version"):
        print(
            f"warning: capabilities.json expects analysis version "
            f"{caps.get('analysis_version')}, but this analysis is version "
            f"{analysis.get('version')} — regenerate both from the same build",
            file=sys.stderr,
        )

    schema = build_schema(caps, all_sources=args.all_sources)
    system = build_system(caps)
    user = build_user(analysis, args.style)

    import anthropic

    if args.dry_run:
        print(f"effects            {len(caps['effects'])}")
        print(f"sources in schema  {len(caps['sources'])}")
        print(f"sections           {len(analysis['sections'])}")
        print(f"system bytes       {sum(len(b['text']) for b in system):,}")
        print(f"user bytes         {len(user):,}")
        print(f"schema bytes       {len(compact(schema)):,}")
        # count_tokens is itself an API call, so it needs a credential. Report the
        # byte counts regardless — they are the part that tells you whether the
        # prompt is assembled sanely.
        try:
            counted = anthropic.Anthropic().messages.count_tokens(
                model=MODEL,
                system=system,
                messages=[{"role": "user", "content": user}],
            )
            print(f"input tokens       {counted.input_tokens:,}")
        except Exception as e:  # noqa: BLE001 - any auth/network failure is the same story here
            print(f"input tokens       unavailable ({type(e).__name__}: {e})")
        return 0

    client = anthropic.Anthropic()

    messages: list[dict[str, Any]] = [{"role": "user", "content": user}]

    for attempt in range(1, args.attempts + 1):
        print(f"[{attempt}/{args.attempts}] generating…", file=sys.stderr)
        message = call_model(client, system, messages, schema, stream_to_stderr=True)
        print(f"    usage: {usage_line(message)}", file=sys.stderr)

        text = next(b.text for b in message.content if b.type == "text")
        result = json.loads(text)

        # Cheap, local, and precise — catches the one class the schema cannot
        # express before anything touches disk.
        incomplete = check_variants(result)
        if incomplete:
            ok, problems = False, "\n".join(incomplete)
        else:
            emit(result, args.out)
            ok, problems = run_validator(args.binary, args.out)
        if ok:
            written = sorted(args.out.glob("*.json"))
            print(f"\n{result['scene_name']}\n", file=sys.stderr)
            print(result["reading"], file=sys.stderr)
            print(file=sys.stderr)
            for p in result["presets"]:
                print(f"  {p['name']}  (identity {p['for_section_label']})", file=sys.stderr)
                print(f"    {p['rationale']}", file=sys.stderr)
            print(file=sys.stderr)
            for path in written:
                print(path)
            print(f"\nvalidated clean — {len(result['cues'])} cues", file=sys.stderr)
            return 0

        print(f"    validator found problems:\n{problems}", file=sys.stderr)
        if attempt == args.attempts:
            print(
                f"\nstopping after {args.attempts} attempt(s); files in {args.out} "
                f"are the last (invalid) draft",
                file=sys.stderr,
            )
            return 1

        # Repair: keep the cached system prefix, append the rejected draft and
        # what was wrong with it. The corpus never moves, so this reads the cache.
        messages += [
            {"role": "assistant", "content": text},
            {
                "role": "user",
                "content": (
                    "The offline validator rejected that scene. It checks against the "
                    "same types the app loads, so each line is a real fault:\n\n"
                    f"{problems}\n\n"
                    "Emit the corrected scene in full. Keep everything that was not "
                    "faulted."
                ),
            },
        ]

    return 1


if __name__ == "__main__":
    sys.exit(main())
