#!/usr/bin/env python3
"""List the wrong answers of a cascade result, step by step, with the model's
probability for its answer and for the expected one.

    scripts/xr/voice-eval/wrong.py out.json
"""

import ast
import json
import sys

POINTED = "table 3"


def p_of(probs, want):
    """The probability of the expected answer (the best one when several are accepted)."""
    wants = sorted(want) if isinstance(want, (list, set)) else [want]
    wants = [POINTED if w == "pointed" else w for w in wants] + (["pointed"] if POINTED in wants or "pointed" in wants else [])
    return max((probs.get(w, 0.0), w) for w in wants)


d = json.load(open(sys.argv[1]))
s = d["summary"]
print(f"### {s['model']} on {s['set']} ({s['variant']}): wrong answers\n")
print("| sentence | step | answer (p) | expected (p) |")
print("|---|---|---|---|")
for r in d["rows"]:
    alts = ast.literal_eval(r["expect"])
    if alts is None:
        if not r["a_ok"]:
            print(f"| {r['sentence']} | (a) gate | request ({r['p_request']:.2f}) | not a request ({1 - r['p_request']:.2f}) |")
        continue
    if not r["a_ok"]:
        print(f"| {r['sentence']} | (a) gate | not a request ({1 - r['p_request']:.2f}) | request ({r['p_request']:.2f}) |")
    if not r["b_ok"]:
        pk, k = max((r["probs_kind"].get(a[0], 0.0), a[0]) for a in alts)
        print(f"| {r['sentence']} | (b) kind | {r['kind']} ({r['p_kind']:.2f}) | {k} ({pk:.2f}) |")
    ref = next((a for a in alts if a[0] == r.get("kind")), alts[0])
    if r.get("c_ok") is False:
        p, w = p_of(r["probs_target"], ref[1])
        print(f"| {r['sentence']} | (c) target, true kind | {r['target_tf']} ({r['p_target_tf']:.2f}) | {w} ({p:.2f}) |")
    if r.get("d_ok") is False:
        p, w = p_of(r["probs_value"], ref[2])
        print(f"| {r['sentence']} | (d) value, true kind and target | {r['value_tf']} ({r['p_value_tf']:.2f}) | {w} ({p:.2f}) |")
