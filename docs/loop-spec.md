# The `.loop.json` spec

A loop render described in a few hundred bytes of JSON — the durable, shareable
artifact behind `--render-loop` (and Phase 3's wizard). Versioned; unknown
versions are rejected with a clear message. All fields except `version`,
`effect`, `bpm` and `bars` have defaults.

```jsonc
{
  "version": 1,
  "effect": "Tessera",            // effect name as shown in the browser
  "params": { "seed": 42.0 },     // param overrides; absent params keep .pfx defaults
  "bpm": 174.0,                   // requested; the EFFECTIVE bpm is derived, never stored
  "bars": 8,                      // loop length in bars (4/4 fixed in v1)
  "fps": 60,                      // 24 | 25 | 30 | 50 | 60 | 120
  "resolution": [1920, 1080],
  "codec": "hap_alpha",           // hap | hap_alpha | prores4444 | h264 | hevc
  "audio": "none",                // v1: "none" — the pinned neutral feature vector
  "background": "transparent"     // transparent | opaque
}
```

## BPM ↔ frame snapping

A loop is seamless only if it spans an integer number of frames, so the
requested BPM snaps to the nearest closing tempo:
`frames = round(bars·4·60/bpm · fps)`, `effective_bpm = bars·4·60·fps / frames`.
The CLI reports both: `requested 174.00 → effective 174.02 BPM (662 frames @ 60fps)`.
120 BPM at 30/60/120 fps snaps losslessly — one reason the market clusters there.

## Synthesized uniforms

Frames are driven by pure arithmetic — no audio device, no PCM, no wall clock:
`beat_phase`/`bar_phase` are exact sawtooths at the effective BPM (no PLL
smoothing, unlike live); `beat`/`downbeat` are one-frame pulses on the frame
where the monotonic beat/bar index steps (frame 0 is the loop's "one");
`beat_in_bar` steps through {0, ¼, ½, ¾}. Audio feature slots carry a pinned
neutral vector (mid-scale energies, stable key) — the same vector the
phase-locked determinism probe renders, so golden loops are golden against
exactly what ships.

## What can be rendered

- `loop: "phase_locked"` effects — the guarantee: **frame 0 ≡ frame N,
  bit-exact** (CI-linted at the source level; pixel-proven by the dev-run
  golden-loop probe on the reference adapter).
- One contract rule keeps that true: phase-locked effects consume
  `bar_index`/`beat_index` only through cycle arithmetic
  (`fract((bar_index + bar_phase) / bars_per_cycle)` or modulo) — raw
  monotonic counters make the visual period infinite. Variety belongs to the
  `seed` param.
- A loop closes over whole effect cycles: set `bars` to a multiple of the
  effect's `bars_per_cycle` (defaults: 4; Astrolabe 8).
- Backdrop-reactive effects (Limn, Intarsia) render nothing solo and cannot be
  exported as single-effect loops.
- Reproducibility: same spec + same Fosfora version + same GPU/driver ⇒
  identical frames; across machines, perceptually identical but not
  guaranteed bit-exact.
