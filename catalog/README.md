# Casting catalog

What every shipped effect actually looks like, measured and described — the
vocabulary the screenplay realizer (board #2040) casts effects from, built
because the first generated scene proved that authoring by param *names* picks
the wrong look and the wrong knobs.

- `effects/<Name>.md` — one entry per effect (checked in): imagery vocabulary,
  motion character grounded in measured numbers, energy response across a
  quiet/loud test track, palette, casting notes. Written by inspecting real
  headless renders, never from the effect's description alone.
- `renders/` — the renders themselves (gitignored, regenerable):
  `scripts/build_catalog.py` drives the app's own `--render-scene` per effect
  against a deterministic synthesized test track (quiet pad half → loud
  four-on-floor half), default params plus lo/hi sweeps of each effect's pace
  param. `renders/summary.json` holds per-clip motion/luma stats.

Motion scale (mean inter-frame Δ, grayscale 0-1): 0.001 essentially static ·
~0.03 moderate · 0.10 frantic. Regenerate everything with
`uv run scripts/build_catalog.py` (needs `target/release/phosphor-app` built
with `--features analyze`, plus ffmpeg).

Known catalog-wide truths (2026-07-31 sweep): several effects are near-black or
degenerate at shipped defaults (Array, Lattice 445, Lattice Pulse, Accretion,
Vessel, Polycephalum); most "speed"-named params are perceptually inert while
audio-driven motion dominates (only Sumi and Tide sweep cleanly; Tunnel's
speed=hi reads as washout, not velocity); several effects invert expectations
under loud music (Chaos collapses near-black, Strata darkens, Protea floods
and flattens). Per-effect detail lives in each entry.
