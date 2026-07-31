# Iris

> Spinning dot with fading feedback trails — circular pattern resembles an iris

- type: shader
- pace param: none (only `default` rendered)
- motion (loud half, mean Δ): default=0.0106
- luma (quiet/loud at default): 0.050 / 0.086
- headless caveats: none

## Imagery
A single ring of glowing beads centered in a pure black void — discrete lozenges of light arranged like a pearl necklace or a loading spinner, each bead a smeared echo of the orbiting dot. Quiet half: a small ring of magenta-pink and ice-white segments. Loud half: a wider ring of ice-blue and mint beads with thin magenta slivers between them.

## Motion character
Slow at default: 0.0058 quiet, 0.0106 loud. The dot orbits while its trail repaints the ring — perceptually a steady rotation and bead-shimmer, not displacement across the frame. Very high contrast (luma_std 0.16–0.19) makes even this small motion read cleanly.

## Energy response
The sharpest quiet/loud split in this set relative to its base: luma +72% (0.050 → 0.086), motion +81%. Visibly the ring widens (mid drives orbit radius), the beads swell (rms drives dot size), and the hue shifts from magenta-white to blue-cyan (centroid); onsets flash brightness, bass speeds the orbit.

## Palette
Jewel accents on absolute black: magenta, hot pink, ice blue, mint. Tiny lit area, huge value range — one glowing object in darkness.

## Casting notes
An eye, a clock face, a portal, a halo, a radar sweep, a mandala seed. Graphic and minimal — a centered focal accent over a darker texture layer, or the lone element of a sparse scene. Cannot fill the frame or do landscape; it is always one circular object at screen center.
