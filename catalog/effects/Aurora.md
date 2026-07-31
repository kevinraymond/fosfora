# Aurora

> Horizontal flowing curtain bands driven by 7 frequency bands — a spectrogram disguised as northern lights

- type: shader
- pace param: curtain_speed (lo=0.15 default=0.5 hi=0.9)
- motion (loud half, mean Δ): lo=0.036 default=0.034 hi=0.037
- luma (quiet/loud at default): 0.15 / 0.31
- headless caveats: none

## Imagery
Wavy horizontal neon ribbons stacked down a black frame, soft-focus like an out-of-focus neon sign: crimson-pink along the top edge, then amber-orange, pale butter-yellow, a mint-green band mid-frame, and a violet crest peeking up from the bottom edge. Each ribbon is a thick, blurred sine-wave stripe with a glowing core. Quiet half: only the top two or three bands glow dimly, the lower two-thirds black. Loud half: five-plus bright bands spread the full height.

## Motion character
Bands undulate sideways like slow flags while their brightness pumps with their frequency band. The measured motion is dominated by that pumping — mean 0.034 with p95 at 0.080–0.086, spiky onset flashes on the beat — so it reads as pulse-flicker plus lateral drift, not displacement. The curtain_speed sweep is imperceptible: lo/default/hi sit within 0.034–0.037 and the stills are near-identical.

## Energy response
Strong and legible: luma doubles (0.15 → 0.31) and the band count visibly grows from a few dim top ribbons to a full stack. High luma_std (0.26 quiet, 0.36 loud) — bright saturated bands against true black. Each instrument owns a stripe, so the mix is readable at a glance.

## Palette
Candy neon — pink, orange, yellow, green, violet — fully saturated on pure black. High contrast, glowing cores with soft falloff.

## Casting notes
Northern lights, neon signage, silk ribbons, a luminous spectrogram. Its geometry is emphatically horizontal — pairs well with vertical or radial layers (Tunnel, Beam in radial mode) and works as a full-frame backdrop that still leaves black between bands. Everything is soft-focus: it cannot render fine detail or hard edges, and the pace param is not worth sweeping.
