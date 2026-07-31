# Reliquary

> A form that holds light — the surface stays put while the light escaping it streams outward, fades and returns. Point it at a model lit from inside and the shafts leaving the openings are made of moving particles

- type: particle
- pace param: stream_speed (lo=0.3 default=1.05 hi=1.8)
- motion (loud half, mean Δ): lo=0.014 default=0.015 hi=0.015
- luma (quiet/loud at default): 0.173 / 0.194
- headless caveats: none

## Imagery
A white eagle, wings spread wide, built from a fine halftone stipple of particles on
black — like a silver engraving or a chapel icon rendered in dust. Sparks and short
streaks radiate off the body and trail below it, and cross-shaped star glints hang
scattered across the black like motes in a light shaft. Faint pink-green fringing gives
the white a spectral edge.

## Motion character
Gentle (~0.015): the figure holds absolutely still while the surrounding spray drifts and
the glints twinkle — reads as flicker and slow shed, never displacement. The stream_speed
sweep is nearly invisible: 0.0139 / 0.0152 / 0.0149 loud, and the lo/hi stills are
practically identical. Do not cast this param expecting a visible change.

## Energy response
Subtle. Loud brightens 0.173 → 0.194 with a denser spark-spray below the body; the figure
itself does not change. This is a presence, not a reactor — energy shifts read as a
slightly heavier shimmer.

## Palette
Monochrome white/silver on black with prismatic RGB fringe; high contrast (luma_std 0.25).
Any other color must come from the model it is pointed at.

## Casting notes
BYO-model effect (catalog uses a stock eagle). Sacred, monumental, memorial,
engraved-icon moods — a statue that sheds light. Its role is a still foreground subject
over a moving background; it cannot carry rhythm, fill a frame with motion, or change
shape with the music. Renders are dark outside the figure, so it composites cleanly over
almost anything.
