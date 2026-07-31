# Cleave

> The HPSS split made visible: crystalline shards stab outward from the fracture point on every drum hit, threaded through slow luminous ribbons that swell with melody. Drag the emitter to move the fracture; Balance forces either voice by hand.

- type: particle
- pace param: ribbon_drift (lo=0.15 default=0.5 hi=0.9)
- motion (loud half, mean Δ): lo=0.020 default=0.023 hi=0.025
- luma (quiet/loud at default): 0.165 / 0.229
- headless caveats: none

## Imagery
A full-frame churn of dark steel-teal grain — sea spray or TV static rendered in blue — with a white sea-urchin starburst at center: hundreds of hair-fine needles radiating from a small black pupil. Pale ice-cyan filament ribbons snake and loop through the grain like current lines in dark water. One glowing spiky organism suspended in a granular ocean.

## Motion character
Constant fine boil of the grain field, slow serpentine drift of the ribbon arcs, and the needle burst pulsing at the hub. Reads as texture churn with a breathing focal point, not flicker. ribbon_drift is a gentle dial: loud motion 0.020 → 0.025 lo-to-hi, with the hi variant visibly loopier ribbons and a brighter field (luma 0.213 → 0.252). Perceptible side by side, not a regime change.

## Energy response
Energy arrives as size and brightness more than speed: loud half lifts luma 0.165 → 0.229 and the starburst swells to double its quiet diameter, needles spiking on each kick, while mean motion rises only 0.0198 → 0.0225. The quiet half keeps a smaller, softer urchin with subtler ribbons — the effect never empties out.

## Palette
Near-monochrome cold blue: black-teal ground, steel and ice-cyan grain, white-hot needle core with faint violet fringing. Low saturation, dark overall, high local contrast at the burst. A hue param exists (default 0.62 = this blue).

## Casting notes
Deep-sea bioluminescence, frost breath, a supernova urchin, analog-noise seascapes — moody, percussive, monochrome. Its drum-gated shard bursts (percussive_energy per the docs) make it a natural for kick-led material; melodic pads feed the ribbons instead. Always a centered focal explosion in a full-frame texture — it cannot do color variety, geometry, or a clean background. Pairs well over or under other dark-field effects; too busy behind fine text.
