# Strata

> Spectral canyon — a heightfield flythrough over the last ~8 seconds of the mel-spectrogram. Ridges are loud moments, chasms are quiet ones; the terrain scrolls with the music.

- type: shader
- pace param: none
- motion (loud half, mean Δ): default=0.0312
- luma (quiet/loud at default): 0.282 / 0.228
- headless caveats: none

## Imagery
A desert flyover under a deep navy dusk sky. In the quiet half the ground is an
almost featureless tan plain — smooth sand stretching to a flat horizon, with only
a small cluster of rounded knolls at one edge. In the loud half the same ground
erupts into rolling dunes: smooth wind-carved ridges of sand and taupe, deep
black chasms between them, and along the far crest a frayed, brush-like fringe of
fine spikes — loud transients rendered as scrubby growth on the ridgeline. Ridge
edges carry a faint rainbow fringe. Unmistakably landscape: horizon, sky, terrain.

## Motion character
The terrain scrolls toward the camera — pace reads as displacement, a steady
forward flight. No pace sweep was rendered. Quiet half is nearly frozen (motion
0.0009: a static plain); the loud half moves at a moderate, even glide (mean
0.0312, p95 0.0402 — low flicker, the motion is smooth travel rather than churn).

## Energy response
The most literal music-to-image mapping in the catalog: quiet music IS flat land,
loud music IS mountains. Motion jumps 34x from quiet to loud (0.0009 → 0.0312).
Mean luma actually drops (0.282 → 0.228) because the dunes cast deep chasm
shadows, while contrast doubles (luma_std 0.062 → 0.114) — the loud half trades
flat brightness for sculpted relief.

## Palette
Two-tone: warm sand/tan/taupe terrain under a cold navy-slate sky, black shadow
valleys, thin prismatic fringes on crests. Mid value, low saturation — earth
tones, not neon.

## Casting notes
Journey and landscape moods: desert crossings, alien terrain, road-trip momentum,
"the music made this ground" literalism. One of the few effects with a horizon
and a sky, so it establishes a world rather than a texture — strong as a base
layer or a solo shot. It cannot sit still interestingly during quiet passages
(the plain is genuinely empty), and it cannot abstract away from being terrain.
Pairs well with a sparse particle layer above the horizon; avoid stacking it
under other full-frame landscapes.
