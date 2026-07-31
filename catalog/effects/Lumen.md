# Lumen

> Real-time global illumination by radiance cascades — a swarm of coloured fireflies lights a soft breathing silhouette, and every light casts long soft-edged shadows with physically plausible penumbras. Each firefly takes the hue of a pitch class and flares with that note's energy, kicks pulse the whole room, bass swells the dancer, and tonal passages thicken the air into volumetric god-rays.

- type: shader
- pace param: none swept (params exist — motion, fog, bounce etc. — but no lo/hi renders)
- motion (loud half, mean Δ): default=0.05223
- luma (quiet/loud at default): 0.1582 / 0.3126
- headless caveats: none

## Imagery
Two dozen soft bokeh orbs — magenta, cyan, yellow, lime, violet, white —
drifting through hazy colored fog, like out-of-focus fairy lights or paper
lanterns in mist. Between them float dark, soft-edged silhouette blobs that
block the light and cast wide penumbral shadows. The fog itself takes the
lights' colors: violet-pink washes on the left, olive-green toward the
upper right. Everything is defocused; there is not a hard edge in the frame.

## Motion character
The liveliest of this batch. At default the loud half measures 0.052 mean /
0.153 p95 — reads as continuous drift of the orbs plus pulsing flares on
hits, i.e. displacement and flicker together, never churn. The quiet half
still drifts gently at 0.014. No pace sweep was rendered; the `motion` param
(0–1, default 0.5) exists but is unmeasured here.

## Energy response
Strong and legible: luma doubles from quiet to loud (0.158 → 0.313) and
motion nearly quadruples (0.014 → 0.052). Quiet = a few dim lanterns
breathing in murk; loud = the whole room lit, more orbs burning, fog glowing
in saturated color, brightness pumping with the kick.

## Palette
Full-spectrum candy bokeh — hot magenta, cyan, lemon, lime, lavender — over
warm murky fog and black silhouettes. Mid brightness overall, high local
contrast, saturated highlights with soft pastel falloff.

## Casting notes
The atmospheric romantic of the set: casts as fairy lights, fireflies at
dusk, a rain-blurred city bokeh, a slow-dance club haze. Excellent solo
full-frame bed; also a natural warm base under sharp foreground layers.
Responds visibly to both kicks (room pulse) and melody (per-note colors).
Cannot do hard geometry, text-like structure, or aggressive strobe — it is
soft by construction. Avoid pairing with other soft-blur beds (mud risk).
