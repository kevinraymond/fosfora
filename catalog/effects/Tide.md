# Tide

> Luminous waterfall that parts, pools, and eddies around silhouettes — enable Obstacle (webcam/depth) with Flow Around mode. Drums make the water break; pads make it glide.

- type: particle
- pace param: flow_speed (lo=0.15 default=0.5 hi=0.9)
- motion (loud half, mean Δ): lo=0.0499 default=0.0621 hi=0.0709
- luma (quiet/loud at default): 0.405 / 0.416
- headless caveats: none

## Imagery
A wall of falling water filling the entire frame, seen close: dense vertical
streaks of spray like a long-exposure waterfall or heavy TV-static rain. Bright
horizontal foam bands cross the falls — pulse fronts traveling down with the
beat, two or three visible at once like whitewater ledges. In the quiet half the
falls are silver-gray, almost monochrome; in the loud half the whole sheet turns
blue-violet with brighter, harder bands. No silhouette appears in these renders
(no Obstacle in the headless rig), so it is pure curtain, edge to edge.

## Motion character
The busiest mover in this set — constant heavy downward churn (mean ~0.05–0.07
in BOTH halves; even "quiet" runs 0.0615). Pace reads as displacement (falling
streaks) plus fine grain shimmer. The flow_speed sweep registers cleanly on the
loud half: 0.0499 → 0.0621 → 0.0709 lo→hi, with hi showing longer streaks and
bolder bands, and hi also darker (luma 0.348 vs 0.416) as the water thins out.

## Energy response
Motion barely changes with the music at default (0.0615 quiet vs 0.0621 loud —
the falls never stop); the audible difference shows as color and structure
instead: the quiet half is soft gray laminar spray, the loud half saturates blue
and organizes into beat-spaced surge bands. Luma stays high throughout
(0.405 / 0.416), the brightest effect in this group.

## Palette
Quiet: silver, white, pale gray — near-monochrome. Loud: cornflower and violet
blue with white foam bands. High value (luma ~0.3–0.42), low-to-medium
saturation; it reads as light water, not neon.

## Casting notes
Water, rain, cleansing, relentless downpour, static-noise texture. Its constant
high brightness and full-frame coverage make it a dominant base layer — it will
wash out anything dim layered under it, so put darker effects on top or use it
solo. It cannot hold still, go dark, or show figures without an Obstacle source;
the parting-around-silhouettes behavior needs webcam/depth and is absent here.
Strong pairing: a silhouette obstacle, or an energetic high-tempo backdrop.
