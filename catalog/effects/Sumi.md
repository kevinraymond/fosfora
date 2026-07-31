# Sumi

> Ink drops bloom in water on every onset — a real incompressible fluid (advection, a Jacobi pressure solve, vorticity confinement) whose twelve dye colours are keyed to the twelve pitch classes. Splats land on a circle-of-fifths ring, bass makes the ink rise, spectral flux sharpens the swirl, and the central drop takes the hue of the detected key.

- type: shader
- pace param: flow_speed (lo=0.15 default=0.5 hi=0.9)
- motion (loud half, mean Δ): lo=0.016 default=0.038 hi=0.053
- luma (quiet/loud at default): 0.11 / 0.31
- headless caveats: none

## Imagery
Wispy ink plumes hanging in dark water, each its own dye colour, arranged in a loose ring — cyan and blue on the left, greens lower-left, a bright yellow-gold column at centre, violet and magenta up the right. Every plume has a glowing round bulb at its base and a curling, feathered tail. In the loud half the frame fills: rising green and yellow ink columns with mushroom-cap heads, and the separate drops marble together into an interlocked smoke of teal, olive, orange, and pink over black.

## Motion character
Buoyant, liquid churn — plumes rise and curl like incense smoke. flow_speed reads as churn speed and spread: lo (0.016) keeps the drops as separate slow blooms with lots of black water between them; default (0.038) is a steady simmer; hi (0.053) roils, filling more of the frame and knotting the colours together. Displacement is upward and swirling, never linear.

## Energy response
The music nearly triples both brightness and motion: luma 0.11 → 0.31, motion 0.012 → 0.038. Quiet is sparse, isolated drops on black; loud is frame-filling marbled smoke with strong rising jets on the kick. The ring layout stays legible in quiet passages and dissolves into full-frame marbling in loud ones.

## Palette
Full twelve-hue rainbow of dyes — saturated but milky, mid-value plumes with bright bulb cores on a true-black ground. High luma_std (0.19–0.27): strong lights against deep darks.

## Casting notes
Ink in water, incense smoke, nebulae, lava-lamp psychedelia, watercolour bleeding — anything organic and liquid. Cannot do geometry, hard edges, or stillness-with-detail. Carries a scene solo full-frame; also a rich background under a hard-edged geometric overlay. The colour set is dictated by the music's pitch content, not a parameter.
