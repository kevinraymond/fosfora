# Example loop specs

One curated `.loop.json` per phase-locked overlay effect — each renders a
seamless, beat-locked HAP Alpha loop with one command from the repo root (or
an installed build):

```
fosfora --render-loop examples/loops/tessera.loop.json
```

| spec | effect | bpm | bars | the look |
|---|---|---|---|---|
| `tessera.loop.json` | Tessera | 120 | 8 | center-out tile breath, punching through the scrim |
| `fenestra.loop.json` | Fenestra | 124 | 4 | GUI panels snapping in and releasing |
| `reticle.loop.json` | Reticle | 128 | 4 | four crosshairs rotating targets each bar |
| `bezel.loop.json` | Bezel | 120 | 4 | border chrome with drifting scanlines |
| `astrolabe.loop.json` | Astrolabe | 120 | 8 | the full targeting instrument, assembling and retracting |

All use `audio: "synthetic"` (beat-locked accent envelopes, still bit-exact —
see [docs/loop-spec.md](../../docs/loop-spec.md)), transparent backgrounds,
1080p60. Edit freely: `seed` re-rolls an effect's whole constellation; `bars`
must stay a multiple of the effect's `bars_per_cycle` for the loop to close
over whole cycles.

A spec is a few hundred bytes that reproduces a full clip — sharing one IS
sharing the loop. PRs with curated specs are welcome.
