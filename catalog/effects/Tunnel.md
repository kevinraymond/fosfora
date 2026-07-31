# Tunnel

> Raymarched infinite cylindrical flythrough with twist, ribs, and glow

- type: shader
- pace param: speed (lo=0.15 default=0.4 hi=0.9)
- motion (loud half, mean Δ): lo=0.024 default=0.028 hi=0.028
- luma (quiet/loud at default): 0.14 / 0.25
- headless caveats: none

## Imagery
A head-on view straight down a ribbed bore: concentric rings receding to a small dark pupil at dead centre, the walls tiled in checkerboard quadrants of brick red, slate blue-teal, and tan. Soft white radial spokes of light cut across the rings like an asterisk. In the quiet half a bright white glow haloes the vanishing point; in the loud half the panels themselves light up warm.

## Motion character
Pure forward displacement — rings swell toward the viewer, panels slide past. The quiet half shows the throttle cleanly (motion 0.010 → 0.014 → 0.021 across lo/default/hi); in the loud half the mean plateaus (~0.028) because flashes dominate. At hi the loud half goes bright and washed rather than visibly faster: luma jumps to 0.40 and contrast drops (luma_std 0.15 → 0.10), a hazy tan-and-blue blur. lo keeps it dark, contrasty, blue-dominant with red panels.

## Energy response
Loud half brightens the walls (luma 0.14 → 0.25) and adds hard kick-flash frames (motion p95 0.064 vs mean 0.028 — spiky, on-the-beat). Quiet is a dim, steady glide with the white centre glow; loud is a lit corridor pulsing on the four-on-floor.

## Palette
Muted industrial: brick red, slate blue, teal-navy, tan/beige, white light spokes. Low saturation, mid values — the one effect in this batch that is not neon-on-black.

## Casting notes
Sci-fi flythrough, warp corridor, subway bore, hyperspace. Strong single focal point at frame centre with full radial symmetry — good under a centred subject, or solo as a driving-motion shot. Cannot do organic shapes or asymmetry. Avoid speed=hi for dark moods: it washes out to a bright haze.
