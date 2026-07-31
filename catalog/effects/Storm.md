# Storm

> Billowing dark clouds lit from within by flashes of lightning

- type: shader
- pace param: flow_speed (lo=0.15 default=0.5 hi=0.9)
- motion (loud half, mean Δ): lo=0.0489 default=0.0478 hi=0.0480
- luma (quiet/loud at default): 0.174 / 0.183
- headless caveats: none

## Imagery
A full-frame night cloudscape: mottled indigo cloud banks filling the whole screen,
no horizon, no ground. Ragged black cloud clumps — like ink blots or torn wool —
float in front of softer slate-blue billows. Paler patches glow from behind the
cover where a flash lights the interior; in the quiet half a broad silver-blue
sheet-lightning bloom washes through the center. Everything is soft-edged fog;
there are no bolts, particles, or hard lines.

## Motion character
Constant slow boiling — the clumps advect and deform rather than travel, so pace
reads as churn, not displacement. Flashes add flicker on top (loud p95 hits 0.17
against a 0.048 mean: sudden frame-wide brightness jumps). The flow_speed sweep
barely registers in the numbers (loud mean 0.0489 / 0.0478 / 0.0480 for lo/default/hi)
and the stills look alike; the audio-driven warp and flash timing dominate whatever
the dial changes.

## Energy response
Strong. Quiet half simmers (motion 0.0148, gentle interior glow); the loud half
triples the churn (0.0478) and turns strobic — beat-timed flashes light the cloud
from within while bass warps the billows. Mean luma barely moves (0.174 → 0.183)
because flashes are brief; the change reads as agitation and flicker, not sustained
brightening.

## Palette
Monochrome blue: indigo and slate through near-black cloud shadow, with cold
silver-white flash cores. Low-to-mid value, very low saturation spread — one hue,
many depths.

## Casting notes
Brooding weather: tension, dread, gathering pressure, night-sky backdrops. The
beat-locked flashes make it a natural strobe layer for four-on-floor material. It
is texture, not figure — no shapes, symbols, or focal objects — so it plays best
as a full-bleed background under a foreground effect (particles, silhouettes,
lattices). Cannot do calm daylight, color variety, or crisp geometry; the palette
is locked to blue.
