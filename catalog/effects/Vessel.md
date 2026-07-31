# Vessel

> Your silhouette becomes a vessel: trapped light slowly fills it as the music builds, then bursts outward on the drop. Enable Obstacle (webcam/depth/image — any mode works); without one, a standing amphora takes your place. Liquidity morphs pooling liquid into drifting fireflies; Release fires the burst by hand.

- type: particle
- pace param: fill_rate (lo=0.15 default=0.6 hi=0.9)
- motion (loud half, mean Δ): lo=0.00228 default=0.00256 hi=0.00267
- luma (quiet/loud at default): 0.018 / 0.027
- headless caveats: none

## Imagery
One object on a black stage: a tall rounded-bottom vessel — a glass amphora /
test-tube silhouette, centered, drawn only by its glowing rim. Inside it, fine
motes drizzle downward like luminous rain trapped in the glass, and a bright
pool of light collects and glows at the curved bottom, haloing beneath the
vessel. Everything outside the outline is true black. This render uses the
no-Obstacle fallback amphora; with a webcam/depth source the vessel would be a
person's silhouette instead.

## Motion character
Nearly still — the darkest, quietest mover in this set (motion 0.0008 quiet,
0.0026 loud; catalog floor is 0.001). What moves is the fine interior drizzle
and the pool's simmer: pace reads as faint flicker inside a static shape. The
fill_rate sweep is real but subtle: loud motion 0.00228 / 0.00256 / 0.00267 and
luma 0.023 / 0.027 / 0.030 lo→hi — hi packs denser motes and a brighter pool,
visible side by side but not at a glance. No burst is captured in these stills.

## Energy response
Read as color and glow, not speed: the quiet half's vessel is cold — teal-cyan
rim, blue-green drizzle, white-cyan pool; the loud half turns warm gold/amber
with a brighter pool and denser motes (luma 0.018 → 0.027, motion ~3x from a
near-zero base). The drop-triggered outward burst in the description does not
appear in the sampled frames.

## Palette
True black field; quiet = cyan/teal glass-glow, loud = amber/gold candle-glow.
Extremely low mean value, one small hotspot; saturation lives in the rim and pool.

## Casting notes
Intimacy, containment, patience, a single held image: a lantern, an hourglass,
a body filling with light. Ideal as the lone figure in a sparse scene or a
focal point over a dark ambient layer — its near-total black composites cleanly.
It cannot carry energy sections on screen-filling terms: no frame-wide motion,
near-black luma, and the payoff burst only arrives via a build/drop (or the
Release param). Cast the Obstacle-on silhouette when a scene needs a human figure.
