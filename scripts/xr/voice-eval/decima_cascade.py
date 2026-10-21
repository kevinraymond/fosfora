#!/usr/bin/env -S uv run --quiet --with decima-ai python
"""Run the voice path's intent cascade through a Decima model, zero-shot.

    scripts/xr/voice-eval/decima_cascade.py amyrmahdy/decima-small out.json
    scripts/xr/voice-eval/decima_cascade.py amyrmahdy/decima-base-clean out.json --set test-set-2.json
    scripts/xr/voice-eval/decima_cascade.py <export dir> out.json --layout bare --phrasing room --gate f

The model is a Hugging Face id or a local export directory, as
`decima.Decima.from_pretrained` takes it (the int8 ONNX export). Each decision
of `cascade.py` goes through Decima's own TypeSafe mapping
(`decima.systemone.to_question`: choice -> choose, noul -> verify) and
`Decima.decide`, so the model is asked exactly as its runtime asks it.

Options:
  --set PATH        test-set-1.json (default) or test-set-2.json, next to this script
  --layout L        mixed (default, the reference) | bare | state | instr | lean; see cascade.py
  --phrasing P      plain (default): the question texts as written; room: each question is
                    preceded by a sentence saying the speaker is in a mixed-reality room
  --gate G          s1 (default): the yes/no gate as a noul with true/false criteria;
                    q: the same question with no criteria; f: a two-option choice
                    ("command" / "chat") that was reworded after looking at test set 1, so
                    its numbers are an upper bound on that set, not a zero-shot score
  --threads N       ONNX Runtime intra-op threads (default 4)

Prints the summary and a Markdown table row; writes every row with its probabilities
to out.json (`wrong.py` lists the misses from it).
"""

import argparse
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cascade as c  # noqa: E402
from decima import Decima  # noqa: E402
from decima.systemone import to_question  # noqa: E402

ROOM_LEAD = ("The speaker is in a mixed-reality room whose tables, shelves, walls, floor, window and ceiling "
             "show visuals that follow the music, and a world effect plays around them. ")
GATE_F = {"type": "choice", "instructions": "Is the speaker asking the room's visuals to change?",
          "criteria": {"command": "a command to the visuals, the music or the room",
                       "chat": "small talk, a remark or an unrelated question"}}


def spec(d, phrasing, gate):
    """A cascade decision dict -> the TypeSafe question spec Decima's /v1/systemone takes."""
    q = json.loads(d["instructions_json"])
    if d["id"] == "request" and gate == "f":
        return {**GATE_F, "instructions": (ROOM_LEAD if phrasing == "room" else "") + GATE_F["instructions"]}
    if phrasing == "room":
        q = ROOM_LEAD + q
    crit = {x["id"]: json.loads(x["description_json"]) for x in d["criteria"]}
    if d["type"] == "noul":
        strip = lambda t: t.split(": ", 1)[1] if t.startswith(("Yes: ", "No: ")) else t  # noqa: E731
        if gate == "q":
            return {"type": "noul", "instructions": q}
        return {"type": "noul", "instructions": q, "criteria": {"true": strip(crit["true"]), "false": strip(crit["false"])}}
    return {"type": "choice", "instructions": q, "criteria": crit}


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("model")
    ap.add_argument("out")
    ap.add_argument("--set", default=os.path.join(here, "test-set-1.json"))
    ap.add_argument("--layout", default="mixed", choices=c.LAYOUTS)
    ap.add_argument("--phrasing", default="plain", choices=("plain", "room"))
    ap.add_argument("--gate", default="s1", choices=("s1", "q", "f"))
    ap.add_argument("--threads", type=int, default=4)
    a = ap.parse_args()
    c.VARIANT = a.layout
    tests = c.load_tests(a.set)

    t0 = time.time()
    model = Decima.from_pretrained(a.model, threads=a.threads)
    load_s = time.time() - t0

    def ask(sentence, d):
        _, q, keys = to_question(d["id"], spec(d, a.phrasing, a.gate))
        dec = model.decide(c.state(sentence, d["id"]), q)
        if d["id"] == "request":  # verify: P(yes) first; gate f: a choice whose first option is "command"
            p = float(dec.probs[0])
            return {"probability_yes": p, "probabilities": {"true": p, "false": 1 - p}}
        probs = {k: float(p) for k, p in zip(keys, dec.probs)}
        return {"selected_id": max(probs, key=probs.get), "probabilities": probs}

    rows = []
    t0 = time.time()
    for sentence, group, alts in tests:
        c.SENTENCE = sentence
        row = {"sentence": sentence, "group": group, "expect": repr(alts)}
        r = ask(sentence, c.request_decision())
        row["p_request"] = r["probability_yes"]
        is_req = r["probability_yes"] >= 0.5
        row["a_ok"] = is_req == (alts is not None)
        if alts is None:
            row["e2e_ok"] = not is_req
            rows.append(row)
            continue
        r = ask(sentence, c.decision("kind", "choice", c.KIND_Q, c.KINDS))
        kind = r["selected_id"]
        row["kind"] = kind
        row["probs_kind"] = r["probabilities"]
        row["p_kind"] = r["probabilities"][kind]
        row["p_kind_true"] = max(r["probabilities"][k] for k, _, _ in alts)
        row["b_ok"] = any(kind == k for k, _, _ in alts)
        # Teacher forcing: the later steps follow the accepted alternative the model's
        # kind matches, or the first one when the kind was wrong.
        tk, tt, tv = next((x for x in alts if x[0] == kind), alts[0])
        tt1 = sorted(tt)[0] if isinstance(tt, set) else tt
        if tk in c.NEEDS_TARGET:
            r = ask(sentence, c.decision("target", "choice", c.TARGET_Q, c.targets()))
            row["target_tf"] = r["selected_id"]
            row["probs_target"] = r["probabilities"]
            row["p_target_tf"] = r["probabilities"][r["selected_id"]]
            row["c_ok"] = c.same(tt, r["selected_id"])
        vals = c.values(tk, tt1 or "pointed")
        if vals:
            r = ask(sentence, c.decision("value", "choice", c.VALUE_Q[tk], vals))
            row["value_tf"] = r["selected_id"]
            row["probs_value"] = r["probabilities"]
            row["p_value_tf"] = r["probabilities"][r["selected_id"]]
            row["d_ok"] = c.same(tv, r["selected_id"])
        ok = is_req and row["b_ok"]
        target = None
        if ok and kind in c.NEEDS_TARGET:
            target = row["target_tf"] if tk == kind else None
            ok = c.same(tt, target)
        if ok and c.values(kind, target or "pointed"):
            ok = row.get("d_ok", False)
        row["e2e_ok"] = ok
        rows.append(row)
    run_s = time.time() - t0

    summary = c.summarize(rows)
    summary.update(model=a.model, set=os.path.basename(a.set),
                   variant=f"{a.layout}/{a.phrasing}" + ("" if a.gate == "s1" else f"/gate-{a.gate}"),
                   load_s=round(load_s, 2), run_s=round(run_s, 2))
    with open(a.out, "w") as f:
        json.dump({"summary": summary, "rows": rows}, f, indent=1)
    print(json.dumps(summary, indent=1))
    print(c.table([summary]))


if __name__ == "__main__":
    main()
