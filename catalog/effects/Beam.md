# Beam

> Vector-CRT oscilloscope — draws the audio waveform as a glowing, over-focused beam with phosphor persistence. Scope and radial modes.

- type: shader
- pace param: none
- motion (loud half, mean Δ): default=0.020
- luma (quiet/loud at default): 0.035 / 0.11
- headless caveats: none

## Imagery
Quiet half: a single soft horizontal ribbon of light across mid-frame — a near-flatline scope trace with a gentle wobble, pale cyan at the left warming to cream-white at the right, glowing against total black. Loud half: a tangle of jagged zigzag waveform traces stacked and overlapping through the middle band of the frame, with dimmer ghost copies of earlier sweeps hanging behind them — rose-pink and magenta on the left blending through violet to electric blue on the right.

## Motion character
Reads as redraw and flicker, not displacement: each sweep lays a new jagged line while phosphor ghosts of the previous ones fade in place. Motion jumps six-fold from quiet (0.0033 — a nearly still line) to loud (0.020, p95 0.040), so the pace of the picture is entirely the music's waveform; there is no pace parameter.

## Energy response
The most literal effect in the batch: quiet is one calm bright line (luma 0.035), loud triples brightness (0.11) and fills the mid-band with overlapping spiky traces and persistence trails. Transients read instantly as new jags; the top and bottom thirds of the frame stay black throughout.

## Palette
A two-tone gradient along the trace — warm pink/cream at one end, cool cyan/blue at the other (colour temperature follows spectral centroid). Saturated glowing line work on black; high local contrast, low overall luma.

## Casting notes
Oscilloscope, EKG, seismograph, laser show, retro CRT lab gear. A midline horizontal band — it will never fill the frame, which makes it a natural foreground line layer over any full-frame backdrop (Aurora, Sumi). Only the scope mode appears in these renders; the radial mode is documented but not captured. Cannot do imagery beyond line work, and quiet music leaves it a plain flat line.
