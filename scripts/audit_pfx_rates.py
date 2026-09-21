#!/usr/bin/env python3
"""Census: which .pfx layer effects compute `u.time * <param>` (board #2984).

Usage:  scripts/audit_pfx_rates.py

Not a fixer and not a verdict — a candidate list to REPRODUCE, in priority
order. A shipped .pfx effect that multiplies ABSOLUTE time by a value derived
from an effect param jumps by (change x uptime) on any frame that param moves,
and the binding matrix can drive any Float param from audio.

Two distinctions the flat grep gets wrong, both found by over-flagging:

  * Only a ROOT product counts. Downstream users of an already-contaminated
    value are consequences of the same defect, not separate ones.

  * Only UNBOUNDED time counts. `sin(t)` is bounded, so `param * sin(t)` is a
    gain that moves smoothly when the param moves. `param * t` is a phase, and
    that is the one that jumps by (change x uptime). A product inside a
    bounding call still counts — sin(u.time * speed) IS the bug — so the walk
    recurses into call arguments while refusing to treat their RESULT as
    unbounded.

Taint follows `let`/`var` bindings, because the pattern hides behind
intermediates (drift.wgsl: `let t = u.time;` then `t * flow_speed`), and is
scoped PER FUNCTION — strata.wgsl was a false positive because a raymarch
`var t = 0.06` in one function collided with `let t = u.time` in another.

Verified against the GPU by `pfx_rate_params_strobe_at_a_large_clock`
(effect/loader.rs): on the effects this flags, a 0.01 param nudge costs
884x-12,000x more at a 300 s clock than at a fresh one.

The blind spot, checked and empty when this was written: taint does not cross a
function call, so a helper that both reads `param()` and receives a time-derived
argument would be missed. Re-check that if this ever reports fewer than it should.
"""

import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
EFFECTS = REPO / "assets/effects"
SHADERS = REPO / "assets/shaders"

LET = re.compile(r"^\s*(?:let|var)\s+([A-Za-z_]\w*)\s*(?::[^=]+)?=\s*(.+?);\s*$")
IDENT = re.compile(r"[A-Za-z_][\w.]*")
TIME_RE = re.compile(r"\bu\.time\b")
PARAM_RE = re.compile(r"\bparam\(\s*(\d+)u?\s*\)")
CALL = re.compile(r"\b([A-Za-z_]\w*)\s*\(")

# Calls whose RESULT is bounded regardless of how large the argument grows.
# A param multiplied by one of these is a gain, not a phase.
BOUNDED_FNS = {
    "sin", "cos", "fract", "step", "smoothstep", "clamp", "saturate",
    "sign", "mod", "normalize", "hash", "hash2", "hash3", "hash21",
    "fosfora_noise2", "fosfora_noise3", "fosfora_fbm2", "fosfora_fbm3",
    "fosfora_hash", "fosfora_hash2", "fosfora_hash21", "fosfora_voronoi",
    "fosfora_worley", "noise2", "noise3", "fbm", "fbm2", "fbm3", "voronoi",
    "worley", "curl", "simplex", "perlin",
    "fosfora_palette", "fosfora_audio_palette", "fosfora_bioluminescent",
    "fosfora_hue_shift", "fosfora_key_hue", "fosfora_grad2", "fosfora_grad3",
    "fosfora_ihash", "fosfora_hash3", "ovl_hash01", "ovl_cell_hash", "ovl_stagger",
    "spectrogram", "atan2", "atan", "tanh", "exp2",
}
# phosphor_* are verbatim aliases of the fosfora_* helpers (lib/palette.wgsl).
BOUNDED_FNS |= {n.replace("fosfora_", "phosphor_") for n in BOUNDED_FNS if n.startswith("fosfora_")}


def strip_comment(line):
    i = line.find("//")
    return line if i < 0 else line[:i]


def split_top(expr, ops):
    """Split on `ops` at paren/bracket depth 0."""
    out, depth, cur = [], 0, ""
    for ch in expr:
        if ch in "([":
            depth += 1
        elif ch in ")]":
            depth -= 1
        if depth == 0 and ch in ops:
            out.append(cur)
            cur = ""
        else:
            cur += ch
    out.append(cur)
    return out


def call_args(expr, fn_filter=None):
    """Yield (fn_name, argument_string) for every call in expr."""
    for m in CALL.finditer(expr):
        fn = m.group(1)
        if fn_filter and fn not in fn_filter:
            continue
        depth, start = 0, m.end() - 1
        for i in range(start, len(expr)):
            if expr[i] == "(":
                depth += 1
            elif expr[i] == ")":
                depth -= 1
                if depth == 0:
                    yield fn, expr[start + 1:i]
                    break


def blank_bounded(expr):
    """Replace every bounded call with a neutral token, innermost first."""
    prev = None
    cur = expr
    while cur != prev:
        prev = cur
        for fn, arg in call_args(cur, BOUNDED_FNS):
            whole = f"{fn}({arg})"
            if whole in cur:
                cur = cur.replace(whole, " BOUNDED ")
                break
    return cur


def has_unbounded_time(expr, utime_names):
    """Does an unbounded, monotonically growing clock survive in this value?"""
    stripped = blank_bounded(expr)
    if TIME_RE.search(stripped):
        return True
    return any(n in utime_names for n in IDENT.findall(stripped))


def has_param(expr, param_names):
    if PARAM_RE.search(expr):
        return True
    return any(n in param_names for n in IDENT.findall(expr))


def slots_of(expr, param_slots):
    s = set(int(n) for n in PARAM_RE.findall(expr))
    for name in IDENT.findall(expr):
        s |= param_slots.get(name, set())
    return s


def find_products(expr, utime_names, param_names, param_slots, depth=0):
    """Products where unbounded time multiplies a param-derived value.

    Recurses into additive terms and into every call's arguments (a phase
    inside sin() is still a phase), but a bounded call's RESULT is not time.
    """
    if depth > 12:
        return set()
    found = set()
    for term in split_top(expr, "+-"):
        factors = split_top(term, "*/")
        if len(factors) > 1:
            t_side, p_side = [], []
            for f in factors:
                is_t = has_unbounded_time(f, utime_names)
                is_p = has_param(f, param_names)
                # A factor that is BOTH carries the product already — its own
                # defining line is the root, not this one.
                if is_t and not is_p:
                    t_side.append(f)
                elif is_p and not is_t:
                    p_side.append(f)
            if t_side and p_side:
                for f in p_side:
                    found |= slots_of(f, param_slots) or {-1}
        for f in factors:
            for _fn, arg in call_args(f):
                for a in split_top(arg, ","):
                    found |= find_products(a, utime_names, param_names, param_slots, depth + 1)
            # A bare parenthesized group, not a call.
            for grp in re.findall(r"(?<![A-Za-z_])\(([^()]*(?:\([^()]*\)[^()]*)*)\)", f):
                found |= find_products(grp, utime_names, param_names, param_slots, depth + 1)
    return found


FN_START = re.compile(r"^\s*fn\s+([A-Za-z_]\w*)\s*\(")


def function_spans(lines):
    """[(first_line_index, last_line_index)] for each top-level fn body."""
    spans, start, depth, inside = [], None, 0, False
    for i, line in enumerate(lines):
        if not inside and FN_START.match(line):
            start, inside, depth = i, True, 0
        if inside:
            depth += line.count("{") - line.count("}")
            if depth <= 0 and "{" in "".join(lines[start:i + 1]):
                spans.append((start, i))
                inside = False
    return spans


def scan_shader(path):
    src = path.read_text()
    raw = src.splitlines()
    all_lines = [strip_comment(l) for l in raw]
    findings = []
    for lo, hi in function_spans(all_lines):
        findings += scan_body(raw, all_lines, lo, hi)
    return findings


def scan_body(raw, all_lines, lo, hi):
    lines = all_lines[lo:hi + 1]

    utime_names, param_names = set(), set()
    param_slots = {}
    for _ in range(4):
        for line in lines:
            m = LET.match(line)
            if not m:
                continue
            name, expr = m.group(1), m.group(2)
            if has_unbounded_time(expr, utime_names):
                utime_names.add(name)
            if has_param(expr, param_names):
                param_names.add(name)
                param_slots.setdefault(name, set())
                param_slots[name] |= slots_of(expr, param_slots)

    findings = []
    for off, line in enumerate(lines):
        i = lo + off + 1
        m = LET.match(line)
        rhs = m.group(2) if m else line
        if "*" not in rhs:
            continue
        slots = find_products(rhs, utime_names, param_names, param_slots)
        if slots:
            findings.append((i, raw[i - 1].strip(), slots))
    return findings


def main():
    all_pfx = sorted(EFFECTS.glob("*.pfx"))
    rows, clean = [], []
    for pfx in all_pfx:
        doc = json.loads(pfx.read_text())
        inputs = doc.get("inputs", [])
        names = [i["name"] for i in inputs]
        kinds = [i.get("type") for i in inputs]

        shaders = []
        if doc.get("shader"):
            shaders.append(doc["shader"])
        for p in doc.get("passes", []):
            if p.get("shader"):
                shaders.append(p["shader"])

        hits = []
        for s in dict.fromkeys(shaders):
            path = SHADERS / s
            if not path.exists():
                print(f"  !! {pfx.name}: missing shader {s}", file=sys.stderr)
                continue
            for ln, text, slots in scan_shader(path):
                hits.append((s, ln, text, slots))

        if not hits:
            clean.append(pfx.stem)
            continue
        slots = set()
        for _, _, _, sl in hits:
            slots |= sl
        targets = [(i, names[i], kinds[i]) for i in sorted(slots) if 0 <= i < len(names)]
        rows.append((pfx.stem, doc.get("name", pfx.stem), hits, targets, -1 in slots))

    bindable = [r for r in rows if any(t[2] == "Float" for t in r[3])]
    print(f"# .pfx files scanned:                  {len(all_pfx)}")
    print(f"# with unbounded time x param:         {len(rows)}")
    print(f"# of those with a Float slot resolved: {len(bindable)}")
    print(f"# clean:                               {len(clean)}\n")

    for stem, name, hits, targets, unresolved in sorted(rows, key=lambda r: (-len(r[3]), r[0])):
        tag = "BINDABLE" if any(t[2] == "Float" for t in targets) else "slot unresolved"
        print(f"## {name} ({stem}.pfx) — {tag}")
        for i, pname, kind in targets:
            print(f"     param({i}) {pname}  [{kind}]")
        if unresolved:
            print("     (a product resolved to no explicit slot)")
        for s, ln, text, sl in hits:
            ss = ",".join(str(x) for x in sorted(x for x in sl if x >= 0)) or "?"
            print(f"     {s}:{ln}  slots {ss}\n        {text}")
        print()

    print("## clean (no unbounded time x param found)")
    print("   " + ", ".join(clean))


if __name__ == "__main__":
    main()
