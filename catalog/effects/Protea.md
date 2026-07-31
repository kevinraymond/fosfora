# Protea

> A mass-conserving Flow Lenia ecosystem — three species of amoebae with membranes and organelles that hunt, merge, and divide, fed by the music itself. Loudness is food (silence starves them until they shrink), bass swells their sensing kernels, onsets rain nutrient droplets, key changes mutate regional species, and strong beats thin the medium so the whole ecosystem surges.

- type: shader
- pace param: sim_speed (lo=0.15 default=0.5 hi=0.9)
- motion (loud half, mean Δ): lo=0.0077 default=0.0113 hi=0.0146
- luma (quiet/loud at default): 0.480 / 0.700
- headless caveats: none

## Imagery
A microscope slide: amoeba bodies with peach-salmon membranes, lavender interiors, and
mint-green organelle blobs, all rendered in a fine stippled halftone weave like cells
under a lab scope. The quiet half keeps one large creature with black margins around it;
the loud half is pale dotted tissue wall-to-wall, ringed droplets and green pockets
floating in it.

## Motion character
Slow protoplasm drift. sim_speed genuinely doubles the crawl lo → hi (0.0077 → 0.0146
loud) and changes texture with it: lo renders smoother, softer blobs with clean gradients;
hi a finer, busier dotted mesh. Reads as churn — internal flowing mass — never flicker.

## Energy response
Loud music floods the frame with mass: luma jumps 0.48 → 0.70 while luma_std collapses
0.28 → 0.05 — the black disappears and the whole frame becomes bright, even tissue.
Contrast dies at full energy; the creatures are most legible in the quiet half. Documented
response: RMS is food, bass swells the creatures, onsets rain nutrient droplets.

## Palette
Peach, salmon, lavender, mint green — milky pastels at mid-to-high value, modest
saturation. A biology-textbook plate, not a neon rig.

## Casting notes
Petri-dish, biology, inner-body, dream-tissue moods; slow breathing sections. Casting
caution: sustained loud music fills and flattens it into a bright pastel field — cast it
for quiet-to-mid passages or as a soft organic background, not for drops. Cannot do sharp
geometry, beat-locked hits, or darkness while the music is loud.
