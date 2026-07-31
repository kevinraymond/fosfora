# Lattice Pyroclastic

> 3D cellular automata — explosive shells that expand and fade (S4-7/B6-8/10/M). Bass drives the evolution; the grid freezes in silence.

- type: particle
- pace param: none
- motion (loud half, mean Δ): default=0.0048
- luma (quiet/loud at default): 0.0861 / 0.1066
- headless caveats: none

## Imagery
A single volumetric cloud-ball centered on a midnight-navy void: a puffy,
cauliflower-textured mass in dusty rose and mauve, like a pink smoke bomb or a
detonation frozen mid-bloom. Quiet half: an irregular, lumpy pink puff with
craggy edges. Loud half: it has swollen into a full round sphere — a pink
moon of tightly packed cumulus lobes with darker crevices between them.

## Motion character
Slow-growth character, honestly near-static frame to frame: quiet motion is
0.00055 (frozen) and loud only 0.0048. What moves is the surface — cells
churning and re-boiling across the ball like slow-motion smoke — not the ball
traveling anywhere. No pace param; the tempo of the churn is set by the music.

## Energy response
Bass literally inflates it. From quiet to loud the puff grows from a lumpy
off-center mass to a large full sphere: luma rises 0.086 → 0.107 and contrast
doubles (luma_std 0.053 → 0.106), while motion increases ~9x. In silence the
cloud freezes solid.

## Palette
Dusty pink, rose and mauve on a nearly black navy-violet field. Low overall
brightness but decent internal contrast in the loud half — the ball reads as
a soft, matte, unsaturated pastel object against darkness.

## Casting notes
A single centered hero object — casts as a slow explosion, a blooming
nebula, a pink brain/coral, a smoke-bomb held in time. Suits ominous-pretty,
volcanic, cosmic or organic-growth moods at slow builds. It cannot fill the
frame edge-to-edge, cannot strobe, and reads meditative even on the loud
half; use it as the centerpiece of a dark scene or behind faster foreground
layers. Background stays pure black-navy, so it composites cleanly.
