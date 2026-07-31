# Ascend

> A spectral mountain range that rises with the brightness of the sound rather than its volume — the seven frequency bands raise their own peaks across the width, cymbal swells send the range towering up the frame, and sub-heavy passages sink it to a low ridge. A pure tone draws a taut crest while noise inflates it into a massif.

- type: particle
- pace param: flow (lo=0.15 default=0.4 hi=0.9)
- motion (loud half, mean Δ): lo=0.0045 default=0.0046 hi=0.0047
- luma (quiet/loud at default): 0.033 / 0.028
- headless caveats: none

## Imagery
A grainy particle ridge line hugging the bottom third of an otherwise black frame. Quiet half: a low, near-white static-noise ridge sloping down from the left corner to a flat glowing floor line — like TV snow settled into a hillside. Loud half: an undulating indigo-to-violet mountain silhouette with a magenta valley on the right, fine vertical streaks rising off the crest like spray or rain falling upward. The upper two-thirds of the frame stays empty.

## Energy response
Brightness barely moves (0.033 → 0.028 — the quiet half is actually marginally brighter, its sharp white grains giving luma_std 0.116 vs 0.062). What the music changes is shape and hue: quiet is a flat pale floor line, loud a rolling violet ridge with crest spray. This test track's loud half is kick/bass-heavy, which by the documented mapping sinks the range low — the towering-cymbal behaviour is not exercised in these renders.

## Motion character
Near-static (0.004–0.005 everywhere): a slow swell of the ridge silhouette plus constant fine grain-flicker on its surface — texture shimmer, not displacement. The flow sweep is inert here: lo/default/hi are indistinguishable in both the stills and the numbers (0.0045/0.0046/0.0047).

## Palette
Black sky over a desaturated sparkle-white ridge (quiet) or an indigo-violet-magenta one (loud). Dim overall; the ridge reads as glowing grain, not solid ground.

## Casting notes
A horizon: landscape silhouette, a spectrum-as-terrain lower third, a quiet ground plane under any taller effect. Natural bedfellow for full-frame effects that leave the bottom edge free. Cannot fill a frame, flash, or strobe; treat the pace param as a no-op. Flag: most of the frame is black, and brightness is essentially constant between quiet and loud — cast it for shape, not energy.
