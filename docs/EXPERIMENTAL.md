# Experimental: the screenplay pipeline

> **Status: experimental.** Nothing on this page ships in release builds — it all
> lives behind the `analyze` cargo feature, so you need a source build. The
> two middle stages call the Claude API (paid, ~$1–2 per full run). Interfaces,
> file formats and flags here may change without notice and without changelog
> coverage.

Turn a song into a scene in five steps, with a human checkpoint in the middle:

```
song.mp3
  │  --analyze                    (offline DSP: sections, beats, drops, per-section stats)
  ▼
song.analysis.json
  │  write_screenplay.py          (Claude writes the song's story as pure imagery)
  ▼
song.screenplay.md   ◄── YOU EDIT THIS ──►  --check-only lint
  │  realize_screenplay.py        (Claude casts it from the effect catalog)
  ▼
scene dir: presets + bindings + cues (+ gaps.md)
  │  --render-scene               (headless render, audio-muxed clips)
  ▼
clips/*.mp4 + frames/*.png + run.json
```

The screenwriter knows nothing about Fosfora — it writes light, motion,
material and palette against the song's measured structure. The realizer knows
nothing about your taste — it casts whatever the (possibly edited) screenplay
says, from what the effect library has measurably been shown to do
(`catalog/effects/`). The markdown file between them is the contract, and
editing it is the intended way to steer the result.

## Prerequisites

- A source build with the feature: `cargo build --release --features analyze`
- `ffmpeg` on `PATH` (clip encoding)
- [`uv`](https://docs.astral.sh/uv/) (runs the Python stages, no venv setup)
- `ANTHROPIC_API_KEY` exported in the shell that runs the two model stages

## The five commands

```bash
SONG="/path/to/song.mp3"

# 1. Analyze (offline, no API, ~7x realtime)
./target/release/phosphor-app --analyze "$SONG"
# -> /path/to/song.analysis.json

# 2. Write the screenplay (Claude; --style is an optional vibe prompt)
uv run scripts/write_screenplay.py --analysis "/path/to/song.analysis.json" \
    --style "dark tones, no warm colors"
# -> /path/to/song.screenplay.md

# 3. Edit the screenplay, then lint your edits (no API, instant)
uv run scripts/realize_screenplay.py --check-only \
    --screenplay "/path/to/song.screenplay.md" \
    --analysis  "/path/to/song.analysis.json"

# 4. Realize it (Claude + the casting catalog; validates and self-repairs)
uv run scripts/realize_screenplay.py \
    --screenplay "/path/to/song.screenplay.md" \
    --analysis  "/path/to/song.analysis.json" \
    --out gen/
# -> gen/*.json (presets, bindings, _scene.json), gen/gaps.md

# 5. Render it headless (no API; ~3x realtime at the default 640x360)
./target/release/phosphor-app --render-scene gen/ --song "$SONG"
# -> gen/_render/clips/*.mp4, frames/*.png, run.json
```

Every artifact is a plain file: the scene dir loads in the app like any other
preset/scene JSON, and you can re-run any stage in isolation.

## Editing the screenplay

The prose is yours to rewrite freely — it is the brief the realizer casts
from, and concrete imagery ("a seam of white opens in the ash") realizes far
better than mood words. The structure has a few rules, all enforced by
`--check-only`:

- **Backticked bracket lines are the machine contract.** The `[song |…]`,
  `[section N |…]` and `[signals: …]` lines carry the timecodes and measured
  audio numbers. Don't retime section boundaries — they are the analyzer's,
  and the check will refuse a screenplay whose sections disagree with the
  analysis. (The numbers are auto-normalized after generation, so don't worry
  about rounding.)
- **Beats are the cue grid.** A `**Beats**` list subdivides a scene into
  visual movements; each beat becomes one cue. Beats must tile their section
  (first starts at section start, last ends at section end, contiguous), each
  at least 2 seconds, ids numbered `<section><letter>` (`5a`, `5b`, …). Split,
  merge and retime beats as you like within those rules; a scene with no
  Beats block is a single beat.
- **`pace:` is a closed vocabulary** on each scene's `**Direction**` line:
  `still · drifting · pulsing · driving · frantic`. It maps to the catalog's
  measured motion scale, so it is a promise the render can be checked against.
  The other Direction fields (light, palette, stillness) are free text.
- Acts, headings and titles are entirely free — reshape at will.

## What the realizer decides for you

Two things are deliberately *not* the model's (or your) job in the scene JSON:

- **Cue timing.** `hold_secs` in the app counts *after* a transition
  completes, so the pipeline plans transition lengths and holds such that
  every cue's incoming transition finishes exactly on its beat boundary.
  Steer timing by editing beat boundaries in the screenplay, not `_scene.json`.
- **Remap input ranges.** Every leading `remap` on an audio source gets its
  input range set to the song's measured per-section p10–p90 (from
  `analysis.json`), so bindings ride the range the song actually occupies
  instead of an authored guess that pins on a compressed master. The model
  (and you, editing bindings JSON afterward) own the *output* range and any
  further shaping.

## gaps.md

Where the screenplay asked for imagery no effect can currently play, the
realizer casts the nearest achievable thing anyway and files the difference in
`<out>/gaps.md`: what was asked, what was cast, why it falls short, and what
capability would close the gap. An ambitious screenplay *should* produce gaps
— they are the demand signal for new effects, not failures.

## Render outputs

By default `--render-scene` captures 3 stills per section plus one 6-second
audio-muxed clip per section (centered on its midpoint, plus a window around
each detected drop; overlapping windows merge). `run.json` records what
actually executed — `cue_spans` is the ground truth for whether the cues
landed where the screenplay promised.

**To render the whole song as one watchable clip**, ask for a window longer
than the song; all windows merge into one:

```bash
./target/release/phosphor-app --render-scene gen/ --song "$SONG" \
    --res 1280x720 --quality high --window-secs 600 \
    --out full_render/
# -> full_render/clips/s00_*.mp4  (the entire song, audio-muxed)
```

Headless caveat: `Dissolve` transitions currently play as `Cut` in the
headless renderer (`run.json` carries a warning); they dissolve normally in
the app.

## The one-stage baseline

`scripts/generate_scene.py` is the older, single-hop generator (analysis →
scene directly, no screenplay, no catalog, no checkpoint). It remains as a
quick smoke test and A/B baseline; the screenplay pipeline supersedes it for
real authoring.

## Troubleshooting

- **`Could not resolve authentication method`** — `ANTHROPIC_API_KEY` isn't
  visible to the script. Shell rc files often skip exports in non-interactive
  shells; `export ANTHROPIC_API_KEY=…` in the same shell you run from.
- **`capabilities.json expects analysis version N`** — the checked-in
  `catalog/capabilities.json` and your `analysis.json` came from different
  builds. Re-run `--analyze` (and `--dump-schema` if you changed effects) with
  the current build.
- **Realization stops after N attempts** — the files in `--out` are the last
  *invalid* draft, and the printed problems say exactly why the validator
  refused them. Usually a re-run fixes it; the system prompt is cached for an
  hour, so retries are cheap.
- **Remap warnings about missing percentiles** — your `analysis.json` predates
  per-section percentiles. Re-run `--analyze` with a current build, or the
  remap input ranges stay as authored.

Related reading: [AUDIO-FEATURES.md](AUDIO-FEATURES.md) for what the binding
sources mean, [TECHNICAL.md](TECHNICAL.md) for the `.pfx` effect format the
presets reference, and `catalog/README.md` for how the casting catalog was
measured.
