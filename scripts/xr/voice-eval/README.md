# Voice intent cascade: test sets and zero-shot driver

The two frozen 40-sentence test sets behind the decision-model rows in
`docs/xr/MEASURED.md`, the cascade they are scored on, and a driver that runs
it through a Decima model. Everything a reader needs to reproduce or extend
those rows.

## What is here

| file | what |
|---|---|
| `test-set-1.json` | the first set: 25 grammar-shaped sentences, 10 indirect ones the hand-written grammar misses, 5 non-requests. Used while developing our own model, never trained on. sha256 `be2f9da1…` |
| `test-set-2.json` | the second set, same shape, written and frozen before the training data it checks and never looked at during development. sha256 `4f8f4294…` |
| `cascade.py` | the room (17 surfaces, table 3 pointed), the four decisions with their option lists, the state layouts, the test-set loader and the scorer |
| `decima_cascade.py` | runs the cascade through `decima-ai` (`Decima.from_pretrained` + its TypeSafe mapping) and prints a table row |
| `wrong.py` | the misses of a result, step by step, with probabilities |

The sentences are spoken commands to a room whose surfaces show music-driven
visuals: "Aurora on wall 5.", "Kill the music.", "What's showing on the window
right now?", and small talk that should be left alone. `expect` is `null` for a
non-request, else a list of accepted alternatives `{kind, target, value}`; a
list in `target` or `value` means any of them is right. `"pointed"` and
`"table 3"` are the same surface.

## The cascade

Four typed decisions per sentence, each a (state, question, options) call:

- (a) gate, yes/no: is the sentence a request to change the room or the effect?
- (b) kind, choice of 15: which action?
- (c) target, choice of 21: which surface (by name, every one of a kind, or the pointed one)?
- (d) value, choice: the behavior, color, band, strength, effect or on/off the kind allows.

(c) and (d) are scored teacher-forced (the true kind, and target, supplied),
so a wrong kind does not hide how the later steps do. End to end chains the
model's own answers. The `mixed` layout is the reference: the gate and the
kind see only the sentence and what is pointed at, the target and the value
also see the room.

## Run it

```sh
scripts/xr/voice-eval/decima_cascade.py amyrmahdy/decima-small out.json
scripts/xr/voice-eval/decima_cascade.py amyrmahdy/decima-base-clean out2.json --set scripts/xr/voice-eval/test-set-2.json
scripts/xr/voice-eval/wrong.py out.json
```

The shebang runs it with `uv` and the `decima-ai` package (torch-free: ONNX
Runtime and a tokenizer). `--layout`, `--phrasing`, `--gate` and `--threads`
are described in the script's header. `--gate f` is a gate reworded after
looking at test set 1, so on that set it bounds what a better gate could do
and is not a zero-shot number.

For another model, write a driver like `decima_cascade.py`: ask
`cascade.decision(...)` dicts (TypeSafe `/v1/systemone` shape) however the
model takes them and keep the loop, so the scoring stays identical.

## Reference rows

From `docs/xr/MEASURED.md`, test set 1, `mixed/plain` unless noted, zero-shot:

| model | (a) gate | (b) kind | (c) target* | (d) value* | end to end |
|---|---|---|---|---|---|
| decima-small 1.1 (122M) | 5/40 | 14/35 | 3/19 | 14/26 | 5/40 |
| decima-small 1.1, `bare/plain --gate f` (upper bound) | 38/40 | 14/35 | 7/19 | 17/26 | 15/40 |
| decima-small 1.1, test set 2 | 5/40 | 13/35 | 4/19 | 15/26 | 5/40 |
| System One 68M, zero-shot | 32/40 | 18/35 | 13/19 | 22/26 | 18/40 |
| our 17M, trained on synthetic cases | | | | | 36/40 (set 2: 35/40) |

\* teacher-forced.
