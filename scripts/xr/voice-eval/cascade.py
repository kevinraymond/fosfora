"""The voice path's intent cascade as four typed decisions, with its room, its
option lists, its test sets and its scorer. Model-agnostic: a driver (see
`decima_cascade.py`) answers each decision and this module says what to ask
and whether the answer was right.

For each sentence: (a) yes/no, "is this a request to change the room or the
effect"; (b) choice, the action kind (the fifteen `intent::Intent` variants);
(c) choice, the target (a surface by name, every surface of a kind, or the
pointed one); (d) choice, the value from the list the kind allows. The room
is the replayed room of `crates/fosfora-xr/src/intent.rs` (5 tables, 5
shelves, 4 walls, a floor, a ceiling, a window), table 3 pointed at.

Steps (c) and (d) are scored teacher-forced: the true kind (and target) from
the test set is supplied, so a wrong kind does not hide how the later steps
do. End to end chains the model's own answers. The three subsets are the 25
grammar-shaped sentences, the 10 indirect ones the hand-written grammar
misses, and the 5 non-requests.
"""

import json

# ---------------------------------------------------------------- the room
# Names as in the replayed room fixture's indices; storage is called "shelf".
ROOM = {
    0: ("table", "table 0"), 1: ("shelf", "shelf 1"), 2: ("wall", "wall 2"),
    3: ("table", "table 3"), 4: ("shelf", "shelf 4"), 5: ("wall", "wall 5"),
    6: ("table", "table 6"), 7: ("floor", "floor"), 8: ("window", "window"),
    9: ("shelf", "shelf 9"), 10: ("ceiling", "ceiling"), 11: ("shelf", "shelf 11"),
    12: ("table", "table 12"), 13: ("shelf", "shelf 13"), 14: ("table", "table 14"),
    15: ("wall", "wall 15"), 16: ("wall", "wall 16"),
}
POINTED = "table 3"
KIND_OF = {name: kind for kind, name in ROOM.values()}
EFFECTS = ["Flux Cloud", "Embers", "Flock"]
PLURAL = {"table": "tables", "shelf": "shelves", "wall": "walls"}

# What the model sees as the state. "mixed" is the reference layout: the gate
# and the kind see only the sentence (and what is pointed at); the target and
# the value see the room as well. "bare": only the sentence, always. "state":
# the room, always. "instr": the room, with the sentence moved into the
# question. "lean": like mixed, but the value sees only the sentence too, and
# the target candidates are bare names.
LAYOUTS = ("mixed", "bare", "state", "instr", "lean")
VARIANT = "mixed"


def state(sentence, did=None):
    if VARIANT == "bare" or (VARIANT == "mixed" and did in ("request", "kind")) \
            or (VARIANT == "lean" and did != "target"):
        return json.dumps({"sentence": sentence, "pointing_at": POINTED})
    by_kind = {}
    for kind, name in ROOM.values():
        by_kind.setdefault(kind, []).append(name)
    return json.dumps({
        **({} if VARIANT == "instr" else {"sentence": sentence}),
        "pointing_at": POINTED,
        "current_effect": "Flux Cloud",
        "effects": EFFECTS,
        "surfaces": {PLURAL.get(k, k): v if len(v) > 1 else v[0] for k, v in by_kind.items()},
    })


# ------------------------------------------------------------ the decisions
REQUEST_Q = "Is the sentence a request to change, switch, show or describe the visuals in the room or the current effect?"
REQUEST_YES = "Yes: the speaker asks the system to change, switch, turn on or off, clear, recolor or describe the room's visuals, a surface, the music or the effect."
REQUEST_NO = "No: the sentence is a greeting, small talk, a remark or a question that asks nothing of the room or the effect."

KIND_Q = "Which action does the sentence ask for?"
KINDS = [
    ("next_effect", "Switch to the next world effect."),
    ("previous_effect", "Switch back to the previous world effect."),
    ("effect", "Switch to a world effect by its name: Flux Cloud, Embers or Flock."),
    ("edit_room", "Turn the room editor on or off."),
    ("particles", "Turn the floating particle cloud on or off."),
    ("pitcher", "Turn the pitcher on or off."),
    ("music", "Play or stop the music."),
    ("rescan", "Scan the room's surfaces again."),
    ("recenter", "Move the particle cloud back to the center."),
    ("all_none", "Clear every surface in the room so nothing shows on any of them."),
    ("behavior", "Put a visual behavior on a surface, or clear one surface."),
    ("color", "Change the color of a surface's visual."),
    ("band", "Make a surface follow a band of the music: bass, mids, highs or the overall level."),
    ("strength", "Make a surface's visual brighter, dimmer, stronger or weaker."),
    ("describe", "Tell what a surface is showing now."),
]
NEEDS_TARGET = {"behavior", "color", "band", "strength", "describe"}

TARGET_Q = "Which surface does the sentence mean?"


def targets():
    if VARIANT == "lean":
        out = [(name, name) for _, name in ROOM.values()]
        out += [(f"all {p}", f"all the {p}") for p in PLURAL.values()]
        out.append(("pointed", "this one, the one pointed at"))
        return out
    out = [(name, f"The {kind} called {name}.") for kind, name in ROOM.values()]
    out += [(f"all {p}", f"Every {k} in the room.") for k, p in PLURAL.items()]
    out.append(("pointed", "The surface the speaker is pointing at, when the sentence says this, that, here or it, or names no surface."))
    return out


BEHAVIOR_TEXT = {
    "none": "Nothing: the surface shows no visual.",
    "embers": "Glowing embers shed off the top on the beat.",
    "sparks": "Sparks shooting up with the bass.",
    "spectrum": "The music's spectrum drawn as bars.",
    "rings": "Rings of light spreading on the beat.",
    "streamlines": "Lines of light flowing across the face.",
    "curls": "Swirls: tight curling lines with a faint fill.",
    "pulse": "A whole-face glow that jumps on the downbeat and breathes with the bar.",
    "aurora": "Flowing horizontal curtains of light, like the northern lights.",
    "prism": "A kaleidoscope turning about the center.",
    "shards": "Cells with glowing edges.",
    "astrolabe": "A dial assembling ring by ring.",
    "bezel": "Border chrome hugging the edges.",
    "fenestra": "Panels snapping into place across the bar.",
    "reticle": "Crosshairs taking a new target every bar.",
    "tessera": "A grid of tiles revealing in waves.",
}
# surfaces::SurfaceBehavior::catalogue; shelves are the "other" kind.
CATALOGUE = {
    "table": ["none", "streamlines", "curls", "pulse", "embers", "sparks", "shards", "prism"],
    "floor": ["none", "rings", "streamlines", "curls", "sparks", "tessera", "prism", "astrolabe"],
    "wall": ["none", "spectrum", "streamlines", "rings", "pulse", "aurora", "fenestra", "bezel"],
    "ceiling": ["none", "rings", "pulse", "streamlines", "aurora", "astrolabe"],
    "window": ["none", "pulse", "rings", "streamlines", "bezel", "reticle", "fenestra"],
    "shelf": ["none", "curls", "streamlines", "pulse", "embers", "shards", "reticle", "tessera"],
}
COLORS = [
    ("own color", "The surface's own default color."),
    ("blue", "Blue."), ("violet", "Violet, purple."), ("warm white", "Warm white."),
    ("amber", "Amber, a warm orange."), ("green", "Green."), ("teal", "Teal, blue green."),
    ("rose", "Rose, pink."),
]
BANDS = [
    ("level", "The overall loudness of the music."),
    ("bass", "The bass, the low notes and the kick drum."),
    ("mid", "The mids, voices and most instruments."),
    ("high", "The highs, cymbals and hi-hats, the treble."),
]
STRENGTHS = [
    ("brighter", "One step brighter or stronger."),
    ("dimmer", "One step dimmer or weaker."),
    ("half", "Half strength."),
    ("full", "Full strength, the brightest."),
    ("off", "Strength zero, dark."),
]
ON_OFF = [("on", "Turn it on, show it, play it, start it."), ("off", "Turn it off, hide it, stop it.")]
VALUE_Q = {
    "behavior": "Which visual should the surface show?",
    "color": "Which color does the sentence ask for?",
    "band": "Which part of the music should the surface follow?",
    "strength": "How should the surface's strength change?",
    "effect": "Which world effect does the sentence ask for?",
    "edit_room": "Should the room editor be on or off?",
    "particles": "Should the particle cloud be on or off?",
    "pitcher": "Should the pitcher be on or off?",
    "music": "Should the music play or stop?",
}


def target_kind(target):
    if target == "pointed":
        return KIND_OF[POINTED]
    if target.startswith("all "):
        return {v: k for k, v in PLURAL.items()}[target[4:]]
    return KIND_OF[target]


def values(kind, target):
    """The (id, description) options of the value step, or None when the kind has no value."""
    if kind == "behavior":
        return [(b, BEHAVIOR_TEXT[b]) for b in CATALOGUE[target_kind(target)]]
    return {
        "color": COLORS, "band": BANDS, "strength": STRENGTHS,
        "effect": [(e, f"The {e} effect.") for e in EFFECTS],
        "edit_room": ON_OFF, "particles": ON_OFF, "pitcher": ON_OFF, "music": ON_OFF,
    }.get(kind)


SENTENCE = ""


def decision(did, typ, question, criteria):
    """A decision in the TypeSafe `/v1/systemone` shape: `typ` is "noul" (yes/no) or "choice"."""
    if VARIANT == "instr":
        question = f'The speaker said: "{SENTENCE}" {question}'
    return {
        "id": did, "kind": "judgment", "type": typ,
        "instructions_json": json.dumps(question), "system_prompt": "",
        "criteria": [{"id": i, "description_json": json.dumps(d), "value": None} for i, d in criteria],
        "documents": [], "scoring": None,
    }


def request_decision():
    return decision("request", "noul", REQUEST_Q, [("false", REQUEST_NO), ("true", REQUEST_YES)])


# ------------------------------------------------------------- the test sets
GRAMMAR, MISS, NONE = "grammar", "miss", "none"


def load_tests(path):
    """`test-set-N.json` -> [(sentence, group, expect)], expect None for a non-request, else a
    list of accepted alternatives (kind, target, value); a target or value may be a set.
    Every id is checked against the option lists above, so a typo fails here, not in a score."""
    out = []
    for t in json.load(open(path))["tests"]:
        if t["expect"] is None:
            out.append((t["sentence"], t["group"], None))
            continue
        alts = []
        for a in t["expect"]:
            kind, target, value = a["kind"], a["target"], a["value"]
            target = set(target) if isinstance(target, list) else target
            value = set(value) if isinstance(value, list) else value
            assert kind in dict(KINDS), (t["sentence"], kind)
            for tg in (target if isinstance(target, set) else [target]):
                assert tg is None or tg in dict(targets()), (t["sentence"], tg)
                vals = values(kind, tg or "pointed")
                for v in (value if isinstance(value, set) else [value]):
                    assert (v is None) == (vals is None) and (v is None or v in dict(vals)), (t["sentence"], v)
            alts.append((kind, target, value))
        out.append((t["sentence"], t["group"], alts))
    return out


def same(want, got):
    if want is None:
        return got is None
    accepted = want if isinstance(want, set) else {want}
    norm = lambda t: POINTED if t == "pointed" else t  # noqa: E731
    return norm(got) in {norm(a) for a in accepted}


def summarize(rows):
    def acc(key, sel=lambda r: True):
        rs = [r for r in rows if sel(r) and key in r]
        return f"{sum(r[key] for r in rs)}/{len(rs)}"
    s = {}
    for name, sel in [("all", lambda r: True), ("grammar", lambda r: r["group"] == GRAMMAR),
                      ("miss", lambda r: r["group"] == MISS), ("none", lambda r: r["group"] == NONE)]:
        s[name] = {k: acc(k, sel) for k in ["a_ok", "b_ok", "c_ok", "d_ok", "e2e_ok"]}
    return s


STEPS = [("a_ok", "(a) gate"), ("b_ok", "(b) kind"), ("c_ok", "(c) target*"), ("d_ok", "(d) value*"), ("e2e_ok", "end to end")]


def table(summaries, header=True):
    """Markdown rows like the ones in docs/xr/MEASURED.md, one per result summary."""
    lines = []
    if header:
        lines.append("| model | set | layout | " + " | ".join(n for _, n in STEPS) + " | e2e grammar 25 | e2e miss 10 | e2e none 5 |")
        lines.append("|---|---|---|" + "---|" * (len(STEPS) + 3))
    for s in summaries:
        lines.append(f"| {s['model']} | {s['set']} | {s['variant']} | " + " | ".join(s["all"][k] for k, _ in STEPS)
                     + f" | {s['grammar']['e2e_ok']} | {s['miss']['e2e_ok']} | {s['none']['e2e_ok']} |")
    return "\n".join(lines)
