# Drift

> Fluid smoke via triple domain-warped FBM noise with advected feedback

- type: shader
- pace param: flow_speed (lo=0.15 default=0.5 hi=0.9)
- motion (loud half, mean Δ): lo=0.022 default=0.025 hi=0.025
- luma (quiet/loud at default): 0.407 / 0.477
- headless caveats: none

## Imagery
An edge-to-edge ink-wash: soft marbled smoke with no figures, no lines, no center. Quiet half — hot magenta-pink plumes bleeding through slate-blue and teal, like ink dropped in dusk water. Loud half — a different painting entirely: dark rust and umber smoke billows rolling across a scorched cream, near-white sky, like sepia storm clouds lit from behind.

## Motion character
Slow, continuous rolling of the whole field — plumes fold, curl, and bleed into each other. Pure displacement/advection; zero flicker, no beat articulation. The flow_speed sweep barely registers at this clip length (loud motion 0.022 lo, 0.025 default and hi) — treat pace as effectively fixed and languid.

## Energy response
The music changes the painting's palette and light more than its speed: loud half lifts luma 0.41 → 0.48 and flips the color world from cool blue/magenta to hot cream/umber (docs map spectral centroid to palette), with motion up modestly (0.015 → 0.025, p95 0.066). Quiet = cool nocturne, loud = bright scorched sky.

## Palette
Mid-value and painterly throughout. Quiet: slate blue, teal, saturated magenta. Loud: cream, tan, chocolate umber. Soft gradients only — no hard edges anywhere in either half.

## Casting notes
The workhorse background: dream sequences, weather, ink-in-water titles, color-field ambience. Full-frame, opaque, never dark, never sharp — the natural base layer under particle or line effects (Cymatics, Cleave) and behind text. It cannot do accents, beats, or focal subjects, and the dramatic quiet-vs-loud palette flip means a scene cast on its quiet look will read completely different when the track kicks in.
