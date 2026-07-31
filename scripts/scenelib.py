"""Shared machinery for the scene-authoring scripts (#2027, #2040).

Used by `generate_scene.py` (the one-stage generator), `write_screenplay.py`
(Stage A: analysis -> markdown screenplay) and `realize_screenplay.py`
(Stage B: screenplay + casting catalog -> scene dir). Nothing here ships to
users; it exists so the three scripts agree on the intermediate schema, the
emitted file shapes, and the prompt-side rules that mirror the Rust types.

The load-bearing constraints documented in generate_scene.py's docstring
(externally tagged ParamValue, enum-closed vocabularies, structural binding
targets assembled Python-side) all live here now.
"""

from __future__ import annotations

import json
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
# encode this (see schema_blocks), so it is enforced here and stated in the prompt.
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


# ---------------------------------------------------------- the schema blocks
def schema_blocks(caps: dict[str, Any], all_sources: bool = False) -> dict[str, Any]:
    """The reusable pieces of an intermediate-scene JSON Schema, with enums
    filled from this build's actual vocabulary. Each script assembles its own
    top level from these.

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
    # (TRANSFORM_FIELDS / TARGET_FIELDS above), is stated in the system prompt so
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

    override_group = {
        "type": "object",
        "properties": {
            "layer": {"type": "integer"},
            "params": param_list,
        },
        "required": ["layer", "params"],
        "additionalProperties": False,
    }

    return {
        "effect_names": effect_names,
        "param_list": param_list,
        "binding_target": binding_target,
        "transform": transform,
        "binding": binding,
        "layer": layer,
        "override_group": override_group,
    }


# ------------------------------------------------------------- prompt helpers
def cached_system(blocks: list[str]) -> list[dict[str, Any]]:
    """Text blocks -> system blocks, with the cache breakpoint on the last one.

    Render order is tools -> system -> messages, so putting the whole stable
    corpus here and the per-song material in the user turn means the cached
    prefix is identical for every song and every repair attempt. Nothing
    volatile (no timestamps, no song name) may appear before the breakpoint.
    1h rather than the 5m default: the corpus is byte-stable across songs, so a
    session of several generations reads it repeatedly.
    """
    out: list[dict[str, Any]] = [{"type": "text", "text": t} for t in blocks]
    out[-1]["cache_control"] = {"type": "ephemeral", "ttl": "1h"}
    return out


def compact(obj: Any) -> str:
    """Minified JSON. The corpus is ~107KB pretty-printed and is sent on every
    request, so the whitespace is worth removing."""
    return json.dumps(obj, separators=(",", ":"), ensure_ascii=False)


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


def analysis_digest(analysis: dict[str, Any]) -> str:
    """The analysis minus its bulk: fingerprints and percentile tables dropped,
    per-beat timestamps reduced to counts. What both stages actually read —
    the full event lists are token noise next to BPM and the drop times."""
    sections = [
        {k: s[k] for k in (
            "index", "start_secs", "end_secs", "duration_secs", "label",
            "cluster", "energy", "energy_rank", "descriptors",
        )}
        for s in analysis["sections"]
    ]
    ev = analysis["events"]
    return compact({
        "global": analysis["global"],
        "sections": sections,
        "events": {
            "beat_count": len(ev["beats_secs"]),
            "downbeat_count": len(ev["downbeats_secs"]),
            "drops_secs": ev["drops_secs"],
            "boundaries_secs": ev["boundaries_secs"],
        },
    })


# ------------------------------------------------------- intermediate -> real
def check_variants(result: dict[str, Any]) -> list[str]:
    """Fields the schema could not require, checked before emitting.

    The flat schema lets a variant arrive incomplete (see schema_blocks). Without
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


# ------------------------------------------------------------------ API calls
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


def call_model(
    client,
    system,
    messages,
    schema=None,
    stream_to_stderr: bool = True,
    model: str = MODEL,
    max_tokens: int = MAX_TOKENS,
    effort: str = EFFORT,
):
    """One request. Streamed because max_tokens is well past the SDK's
    non-streaming timeout guard. With `schema` the output is structured JSON;
    without, plain text (Stage A's screenplay is prose)."""
    output_config: dict[str, Any] = {"effort": effort}
    if schema is not None:
        output_config["format"] = {"type": "json_schema", "schema": schema}
    with client.messages.stream(
        model=model,
        max_tokens=max_tokens,
        system=system,
        messages=messages,
        output_config=output_config,
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
            f"hit max_tokens ({max_tokens}) — output truncated, raise max_tokens"
        )
    return message


def usage_line(message) -> str:
    u = message.usage
    return (
        f"in {u.input_tokens} | cache write {u.cache_creation_input_tokens} "
        f"| cache read {u.cache_read_input_tokens} | out {u.output_tokens}"
    )


# ============================================================= the screenplay
# The seam of the two-stage pipeline (#2040). Prose is the document; the machine
# contract is a handful of backticked bracket-lines Kevin can see while editing.
# Scenes map 1:1 to analyzer sections (coverage is checkable against
# analysis.json); beats subdivide a scene and are the cue-granularity unit —
# subdividing is how a dominant cluster gets varied instead of running one look
# for three minutes (#2038). Everything outside the grammar below is free text.

# The closed pace vocabulary — the screenplay's one measurable promise per scene,
# anchored to the catalog's measured motion scale (mean inter-frame |delta| at
# 320x180 gray: 0.001 static, 0.03 moderate, 0.10 frantic).
PACE_WORDS = ("still", "drifting", "pulsing", "driving", "frantic")

# Seconds. Bracket-line numbers are normalized from analysis.json after
# generation, so the tolerance only has to absorb Kevin's hand-edits.
TIME_TOL = 0.15
MIN_BEAT_SECS = 2.0

_DASH = r"[–—-]"  # writers and models disagree; accept en, em, hyphen
_NUM = r"(\d+(?:\.\d+)?)"

# The head is canonically `song`, but writers put the title there; any bracket
# line with pipe-separated fields that is not a section line reads as the song
# line, and normalization rewrites it canonically.
SONG_RE = re.compile(r"`\[(?:song|[^|\]]+?)\s*\|(?P<body>[^\]]*)\]`")
SECTION_RE = re.compile(
    rf"`\[section\s+(?P<index>\d+)\s*\|\s*(?P<start>{_NUM})\s*s?\s*{_DASH}\s*(?P<end>{_NUM})\s*s?"
    rf"\s*\|\s*{_NUM}\s*s?\s*\|\s*identity\s+(?P<label>[A-Z]+)\s*\|\s*energy\s+(?P<energy>{_NUM})\s*,"
    rf"\s*rank\s+(?P<rank>\d+)\s*/\s*(?P<of>\d+)\s*\|\s*drops:\s*(?P<drops>[^\]]*)\]`"
)
SIGNALS_RE = re.compile(r"`\[signals:\s*(?P<body>[^\]]*)\]`")
BEAT_RE = re.compile(
    rf"^\s*[-*]\s+\*\*(?P<id>\d+[a-z])\s*\(\s*(?P<start>{_NUM})\s*s?\s*{_DASH}\s*(?P<end>{_NUM})\s*s?\s*\)"
    rf"\s*{_DASH}\s*(?P<title>.+?)\*\*\s*(?P<prose>.*)$"
)
DIRECTION_RE = re.compile(rf"^\s*\*\*Direction\*\*\s*{_DASH}\s*(?P<body>.+)$")

KEY_NAMES = ("C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B")


class Beat:
    def __init__(self, id: str, start: float, end: float, title: str, prose: str,
                 line_no: int, implicit: bool = False):
        self.id = id
        self.start = start
        self.end = end
        self.title = title
        self.prose = prose
        self.line_no = line_no
        self.implicit = implicit


class SceneBlock:
    def __init__(self, index: int, start: float, end: float, label: str,
                 energy: float, rank: int, rank_of: int, drops: list[float],
                 line_no: int):
        self.index = index
        self.start = start
        self.end = end
        self.label = label
        self.energy = energy
        self.rank = rank
        self.rank_of = rank_of
        self.drops = drops
        self.line_no = line_no
        self.signals_line_no: int | None = None
        self.beats: list[Beat] = []
        self.direction: dict[str, str] | None = None
        self.direction_line_no: int | None = None
        self.prose: list[str] = []

    def effective_beats(self) -> list[Beat]:
        """The cue-granularity spans: explicit beats, or the whole scene as one
        implicit beat when no Beats block was written."""
        if self.beats:
            return self.beats
        return [Beat(f"{self.index}a", self.start, self.end, "", "",
                     self.line_no, implicit=True)]


class Screenplay:
    def __init__(self, text: str):
        self.text = text
        self.song_line_no: int | None = None
        self.song: dict[str, Any] | None = None
        self.scenes: list[SceneBlock] = []
        self.parse_problems: list[str] = []

    def all_beats(self) -> list[tuple[SceneBlock, Beat]]:
        return [(sc, b) for sc in self.scenes for b in sc.effective_beats()]


def _parse_drops(text: str) -> list[float]:
    text = text.strip()
    if not text or text == "none":
        return []
    out = []
    for m in re.finditer(_NUM, text):
        out.append(float(m.group(1)))
    return out


def _parse_song_line(body: str) -> dict[str, Any]:
    """Loose field scan of the `[song | …]` body; absent fields are absent keys."""
    song: dict[str, Any] = {}
    for part in body.split("|"):
        part = part.strip()
        if m := re.fullmatch(rf"{_NUM}\s*s", part):
            song["duration_secs"] = float(m.group(1))
        elif m := re.fullmatch(rf"{_NUM}\s*BPM", part, re.IGNORECASE):
            song["bpm"] = float(m.group(1))
        elif m := re.match(r"(\d+)\s*sections?\s*/\s*(\d+)\s*identit", part):
            song["sections"] = int(m.group(1))
            song["identities"] = int(m.group(2))
        elif part.startswith("drops:"):
            song["drops"] = _parse_drops(part[len("drops:"):])
        elif part.startswith("key "):
            song["key"] = part[4:].strip()
    return song


def parse_screenplay(text: str) -> Screenplay:
    """Tolerant of surrounding prose, strict about the bracket-line grammar.

    A line that clearly *tries* to be a grammar line but fails its regex is a
    parse problem, not prose — silently demoting it to prose is how a mangled
    timecode would slip past every later check.
    """
    sp = Screenplay(text)
    scene: SceneBlock | None = None

    for i, line in enumerate(text.splitlines()):
        if m := SECTION_RE.search(line):
            scene = SceneBlock(
                index=int(m.group("index")),
                start=float(m.group("start")),
                end=float(m.group("end")),
                label=m.group("label"),
                energy=float(m.group("energy")),
                rank=int(m.group("rank")),
                rank_of=int(m.group("of")),
                drops=_parse_drops(m.group("drops")),
                line_no=i,
            )
            sp.scenes.append(scene)
            continue
        if "[section" in line:
            # Checked before the song line's loose head-pattern can eat it.
            sp.parse_problems.append(
                f"line {i + 1}: malformed section line (expected "
                "`[section N | 0.0s – 10.0s | 10.0s | identity A | energy 0.50, rank 1/9 | drops: none]`): "
                f"{line.strip()}"
            )
            continue
        if m := SIGNALS_RE.search(line):
            if scene is None:
                sp.parse_problems.append(f"line {i + 1}: signals line before any section line")
            else:
                scene.signals_line_no = i
            continue
        if "[signals" in line:
            sp.parse_problems.append(f"line {i + 1}: malformed signals line: {line.strip()}")
            continue
        if m := SONG_RE.search(line):
            if sp.song is not None:
                sp.parse_problems.append(f"line {i + 1}: second `[song |…]` line")
            sp.song = _parse_song_line(m.group("body"))
            sp.song_line_no = i
            continue
        if m := BEAT_RE.match(line):
            if scene is None:
                sp.parse_problems.append(f"line {i + 1}: beat line before any section line")
            else:
                scene.beats.append(Beat(
                    id=m.group("id"),
                    start=float(m.group("start")),
                    end=float(m.group("end")),
                    title=m.group("title").strip().rstrip("."),
                    prose=m.group("prose").strip(),
                    line_no=i,
                ))
            continue
        if re.match(r"^\s*[-*]\s+\*\*\d+[a-z]", line):
            sp.parse_problems.append(
                f"line {i + 1}: malformed beat line (expected "
                "`- **5a (118.6 – 135.0) — Title.** prose`): {0}".format(line.strip())
            )
            continue
        if m := DIRECTION_RE.match(line):
            if scene is None:
                sp.parse_problems.append(f"line {i + 1}: Direction line before any section line")
            else:
                if scene.direction is not None:
                    sp.parse_problems.append(
                        f"line {i + 1}: second Direction line for section {scene.index}"
                    )
                fields: dict[str, str] = {}
                for part in re.split(r"[·|]", m.group("body")):
                    if ":" in part:
                        k, v = part.split(":", 1)
                        fields[k.strip().lower()] = v.strip()
                scene.direction = fields
                scene.direction_line_no = i
            continue
        if scene is not None and line.strip():
            scene.prose.append(line)

    return sp


def check_screenplay(
    sp: Screenplay, analysis: dict[str, Any]
) -> tuple[list[str], list[str]]:
    """(problems, warnings). analysis.json is the source of truth: sections are
    the analyzer's and are not Kevin's to retime — his edit surface is prose,
    beats, direction and act structure."""
    problems = list(sp.parse_problems)
    warnings: list[str] = []
    sections = analysis["sections"]

    # Coverage: every analyzer section, exactly once, in order.
    got = [sc.index for sc in sp.scenes]
    want = [s["index"] for s in sections]
    if got != want:
        problems.append(
            f"screenplay covers sections {got}, analysis has {want} — every "
            "section must appear exactly once, in order"
        )
        return problems, warnings  # everything below assumes alignment

    if sp.song is None:
        warnings.append("no `[song |…]` line")
    else:
        dur = analysis["source"]["duration_secs"]
        if "duration_secs" in sp.song and abs(sp.song["duration_secs"] - dur) > 1.0:
            problems.append(
                f"song line says {sp.song['duration_secs']}s, analysis says {dur:.1f}s"
            )
        bpm = analysis["global"]["bpm"]
        if "bpm" in sp.song and abs(sp.song["bpm"] - bpm) > 1.0:
            problems.append(f"song line says {sp.song['bpm']} BPM, analysis says {bpm:.1f}")

    for sc, s in zip(sp.scenes, sections):
        where = f"section {sc.index} ({sc.label})"
        if abs(sc.start - s["start_secs"]) > TIME_TOL or abs(sc.end - s["end_secs"]) > TIME_TOL:
            problems.append(
                f"{where}: timecodes {sc.start:.1f}–{sc.end:.1f} do not match the "
                f"analysis ({s['start_secs']:.1f}–{s['end_secs']:.1f}); section "
                "boundaries are the analyzer's and cannot be retimed here"
            )
        if sc.label != s["label"]:
            problems.append(f"{where}: identity {sc.label!r}, analysis says {s['label']!r}")
        if sc.signals_line_no is None:
            warnings.append(f"{where}: no signals line")

        if sc.direction is None:
            problems.append(f"{where}: no `**Direction**` line")
        elif "pace" not in sc.direction:
            problems.append(f"{where}: Direction line has no `pace:`")
        else:
            pace = sc.direction["pace"].split()[0].strip(".,") if sc.direction["pace"] else ""
            if pace not in PACE_WORDS:
                problems.append(
                    f"{where}: pace {pace!r} is not one of {'/'.join(PACE_WORDS)}"
                )

        # Beats: tile the section, contiguous, each long enough to be a cue.
        if sc.beats:
            ids = [b.id for b in sc.beats]
            wanted = [f"{sc.index}{chr(ord('a') + j)}" for j in range(len(ids))]
            if ids != wanted:
                problems.append(f"{where}: beat ids {ids} should be {wanted}")
            if abs(sc.beats[0].start - s["start_secs"]) > TIME_TOL:
                problems.append(
                    f"{where}: first beat starts at {sc.beats[0].start:.1f}, "
                    f"section starts at {s['start_secs']:.1f}"
                )
            if abs(sc.beats[-1].end - s["end_secs"]) > TIME_TOL:
                problems.append(
                    f"{where}: last beat ends at {sc.beats[-1].end:.1f}, "
                    f"section ends at {s['end_secs']:.1f}"
                )
            for a, b in zip(sc.beats, sc.beats[1:]):
                if abs(a.end - b.start) > TIME_TOL:
                    problems.append(
                        f"{where}: beat {a.id} ends at {a.end:.1f} but {b.id} "
                        f"starts at {b.start:.1f} — beats must tile the section"
                    )
            for b in sc.beats:
                if b.end - b.start < MIN_BEAT_SECS - TIME_TOL:
                    problems.append(
                        f"{where}: beat {b.id} is {b.end - b.start:.1f}s; "
                        f"a cue needs at least {MIN_BEAT_SECS:.0f}s"
                    )

    return problems, warnings


def _fmt_drops(drops: list[float]) -> str:
    return ", ".join(f"{d:.1f}s" for d in drops) if drops else "none"


def _rank_str(s: dict[str, Any], n: int) -> str:
    return f"{round(s['energy_rank'] * (n - 1)) + 1}/{n}"


def canonical_song_line(analysis: dict[str, Any]) -> str:
    g, src = analysis["global"], analysis["source"]
    key = f"{KEY_NAMES[g['key_class'] % 12]} {'minor' if g['key_is_minor'] else 'major'}"
    return (
        f"`[song | {src['duration_secs']:.1f}s | {g['bpm']:.1f} BPM | key {key} | "
        f"{len(analysis['sections'])} sections / {g['cluster_count']} identities | "
        f"drops: {_fmt_drops(analysis['events']['drops_secs'])}]`"
    )


def canonical_section_line(s: dict[str, Any], analysis: dict[str, Any]) -> str:
    n = len(analysis["sections"])
    drops = [d for d in analysis["events"]["drops_secs"]
             if s["start_secs"] <= d < s["end_secs"]]
    return (
        f"`[section {s['index']} | {s['start_secs']:.1f}s – {s['end_secs']:.1f}s | "
        f"{s['duration_secs']:.1f}s | identity {s['label']} | "
        f"energy {s['energy']:.2f}, rank {_rank_str(s, n)} | drops: {_fmt_drops(drops)}]`"
    )


def canonical_signals_line(s: dict[str, Any]) -> str:
    d = s["descriptors"]
    return (
        f"`[signals: rms {d['rms']:.2f} · centroid {d['centroid']:.2f} · "
        f"percussive {d['percussive_energy']:.2f} · onsets {d['onset_density']:.1f}/s · "
        f"harmonic {d['harmonic_ratio']:.2f} · width {d['stereo_width']:.2f} · "
        f"buildup {d['buildup']:.2f}]`"
    )


def normalize_screenplay(sp: Screenplay, analysis: dict[str, Any]) -> str:
    """Rewrite the bracket-line numbers from analysis.json, preserving all prose
    byte-for-byte. Run after a clean check, so model (or hand) rounding never
    drifts the contract. Beat endpoints are snapped to the section bounds and to
    each other; interior boundaries stay where the author put them."""
    lines = sp.text.splitlines()

    if sp.song_line_no is not None:
        lines[sp.song_line_no] = canonical_song_line(analysis)

    for sc, s in zip(sp.scenes, analysis["sections"]):
        lines[sc.line_no] = canonical_section_line(s, analysis)
        if sc.signals_line_no is not None:
            lines[sc.signals_line_no] = canonical_signals_line(s)
        if not sc.beats:
            continue
        sc.beats[0].start = s["start_secs"]
        sc.beats[-1].end = s["end_secs"]
        for a, b in zip(sc.beats, sc.beats[1:]):
            b.start = a.end
        for b in sc.beats:
            title = f"{b.title}." if b.title and not b.title.endswith((".", "!", "?")) else b.title
            lines[b.line_no] = (
                f"- **{b.id} ({b.start:.1f} – {b.end:.1f}) — {title}** {b.prose}".rstrip()
            )

    return "\n".join(lines) + "\n"


# ----------------------------------------------- deterministic realizer passes
# Fraction of a beat span a transition may eat. The incoming transition of cue N
# completes exactly on beat N's boundary, so it plays over the tail of beat N-1;
# capping at a quarter of either adjacent span keeps both beats recognizable.
MAX_TRANSITION_FRACTION = 0.25


def plan_cue_timing(
    spans: list[tuple[float, float]], requested: list[dict[str, Any]]
) -> list[dict[str, Any]]:
    """Beat spans + the model's requested transitions -> per-cue transition and
    hold_secs, all arithmetic here rather than in the model.

    `hold_secs` counts AFTER the incoming transition completes (scene/types.rs),
    so hold_i = span_i - transition_{i+1}: cue i+1's transition starts before its
    boundary and completes exactly on it, and the sum telescopes to the song.
    """
    n = len(spans)
    trans: list[float] = [0.0] * n
    kinds: list[str] = ["Cut"] * n
    for i in range(1, n):
        kind = requested[i].get("transition", "Cut")
        kinds[i] = kind
        if kind == "Cut":
            continue
        want = float(requested[i].get("transition_secs", 1.0))
        prev_span = spans[i - 1][1] - spans[i - 1][0]
        this_span = spans[i][1] - spans[i][0]
        trans[i] = round(
            max(0.0, min(want, MAX_TRANSITION_FRACTION * prev_span,
                         MAX_TRANSITION_FRACTION * this_span)),
            2,
        )

    out = []
    for i in range(n):
        span = spans[i][1] - spans[i][0]
        nxt = trans[i + 1] if i + 1 < n else 0.0
        out.append({
            "transition": kinds[i],
            "transition_secs": trans[i],
            "hold_secs": round(span - nxt, 2),
        })
    return out


# audio.band.N -> the Nth band feature; everything else strips the audio. prefix.
BAND_FEATURES = ("sub_bass", "bass", "low_mid", "mid", "upper_mid", "presence", "brilliance")

# Sources where ranging a remap by observed percentiles is wrong: triggers are
# 0-or-1, phases are sawtooths, and the categoricals encode identity, not level.
# `key_hue` is derived in the bus and has no feature slot at all.
NO_CALIBRATE = {
    "beat", "downbeat", "drop", "beat_phase", "bar_phase", "beat_in_bar",
    "key_class", "key_is_minor", "dominant_chroma", "key_hue", "bpm",
}

MIN_CALIBRATED_WIDTH = 0.05


def source_feature(source: str) -> str | None:
    """Binding source key -> percentile-table feature name, or None when
    calibration does not apply."""
    if not source.startswith("audio."):
        return None
    key = source[len("audio."):]
    if key.startswith("band."):
        try:
            return BAND_FEATURES[int(key[len("band."):])]
        except (ValueError, IndexError):
            return None
    if key in NO_CALIBRATE:
        return None
    return key


def calibrate_remaps(
    result: dict[str, Any],
    analysis: dict[str, Any],
    preset_sections: dict[str, list[int]],
) -> tuple[list[str], list[str]]:
    """Set each leading remap's input range to the song's measured per-section
    distribution (#2037). The model owns the transform chain's shape and the
    output range; the input range is a measurement, not a choice — an authored
    [0.3, 0.8] pinned at max on a compressed master is the failure this replaces.

    Mutates `result` in place; returns (log, warnings).
    """
    log: list[str] = []
    warnings: list[str] = []
    by_index = {s["index"]: s for s in analysis["sections"]}
    have_percentiles = any(s.get("percentiles") for s in analysis["sections"])
    if not have_percentiles:
        warnings.append(
            "analysis.json has no per-section percentiles — re-run --analyze with a "
            "current build; remap input ranges are left as authored"
        )
        return log, warnings

    for preset in result["presets"]:
        covered = [by_index[i] for i in preset_sections.get(preset["name"], []) if i in by_index]
        for b in preset.get("bindings", []):
            transforms = b.get("transforms", [])
            remap_at = next(
                (i for i, t in enumerate(transforms) if t.get("type") == "remap"), None
            )
            if remap_at is None:
                continue
            if remap_at != 0:
                warnings.append(
                    f"preset {preset['name']!r} binding {b['name']!r}: remap is "
                    f"transforms[{remap_at}], not first — input range left as authored "
                    "(calibration only trusts a remap that sees the raw source)"
                )
                continue
            feature = source_feature(b["source"])
            if feature is None:
                continue
            sections = covered or list(by_index.values())
            p10s = [s["percentiles"][feature][0] for s in sections
                    if feature in s.get("percentiles", {})]
            p90s = [s["percentiles"][feature][2] for s in sections
                    if feature in s.get("percentiles", {})]
            if not p10s:
                warnings.append(
                    f"preset {preset['name']!r} binding {b['name']!r}: no percentiles "
                    f"for {b['source']!r}; input range left as authored"
                )
                continue
            lo, hi = min(p10s), max(p90s)
            if hi - lo < MIN_CALIBRATED_WIDTH:
                mid = (lo + hi) / 2.0
                lo = max(0.0, mid - MIN_CALIBRATED_WIDTH / 2.0)
                hi = min(1.0, lo + MIN_CALIBRATED_WIDTH)
                lo = max(0.0, hi - MIN_CALIBRATED_WIDTH)
            remap = transforms[0]
            log.append(
                f"{preset['name']} / {b['name']}: {b['source']} remap in "
                f"[{remap.get('in_lo')}, {remap.get('in_hi')}] -> [{lo:.3f}, {hi:.3f}] "
                f"(p10/p90 over sections {[s['index'] for s in sections]})"
            )
            remap["in_lo"] = round(lo, 3)
            remap["in_hi"] = round(hi, 3)

    return log, warnings


def check_run(
    run: dict[str, Any], scene: dict[str, Any], sp: Screenplay
) -> list[str]:
    """run.json's cue_spans are the ground truth of which cue executed when; the
    screenplay's beats are the promise. Each cue's transition should complete on
    its beat boundary (the seed of the #2039 conformance metrics)."""
    problems: list[str] = []
    beats = [b for _, b in sp.all_beats()]
    cues = scene["cues"]
    if len(cues) != len(beats):
        problems.append(f"scene has {len(cues)} cues but the screenplay has {len(beats)} beats")
        return problems
    spans = run.get("cue_spans", [])
    if len(spans) != len(cues):
        problems.append(
            f"run executed {len(spans)} cue spans, scene has {len(cues)} cues — "
            "the timeline stalled or skipped"
        )
    for span in spans:
        i = span["cue"]
        if i >= len(beats):
            problems.append(f"run executed cue {i}, which no beat maps to")
            continue
        expected = beats[i].start - cues[i].get("transition_secs", 0.0) if i else 0.0
        got = span["start_secs"]
        if abs(got - expected) > 0.5:
            problems.append(
                f"cue {i} (beat {beats[i].id}) started at {got:.2f}s, expected "
                f"{expected:.2f}s (beat start {beats[i].start:.1f} minus its transition)"
            )
    return problems
