# Cascade

> Screen edges emit audio-segmented particle streams inward — bass from bottom, mids from sides, highs from top, creating visual interference where streams overlap

- type: particle
- pace param: inward_speed (lo=0.15 default=0.5 hi=0.9)
- motion (loud half, mean Δ): lo=0.0089 default=0.0090 hi=0.0092
- luma (quiet/loud at default): 0.024 / 0.065
- headless caveats: none

## Imagery
A black field hemmed by glowing coloured edges: an amber-orange floor glow like a bed of embers across the bottom, an emerald strip up the left edge, an indigo-violet strip up the right, and a thin pale fuzz of fine bristles along the top. Faint grain specks drift over the darkness just inside each edge. In these stills the streams read as shallow fringes — no visible streams reach the centre and no interference pattern appears; the middle of the frame stays empty black.

## Motion character
Quiet shimmer at the fringes — motion 0.009 at default, well below "moderate," reading as edge-grain flicker with a slow inward creep rather than travelling streams. The inward_speed sweep changes nothing measurable (0.0089/0.0090/0.0092) and the lo/hi stills are near-identical to default.

## Energy response
Legible but dim: loud nearly triples luma (0.024 → 0.065) and doubles motion (0.0042 → 0.0090). Quiet shows only the faint top fuzz and a dark-red floor line; loud lights all four edges in their band colours — orange floor for bass, green and violet walls for mids, pale top for highs — so the frame becomes a glowing border around darkness.

## Palette
Amber/ember orange, emerald green, indigo-violet, pale white fuzz; saturated glows fading quickly to a black centre. Low overall value (luma ≤0.07).

## Casting notes
A vignette or frame device: embers along a floor, light leaking in at the edges of a dark room, a coloured border that breathes with the mix. Its empty centre is the point — cast it as an edge layer around a centred effect (Tunnel, Beam, a splat model). Flag: it cannot carry a scene alone at these settings — the centre is black, the promised mid-frame interference is not visible in the renders, and the pace param reads as a no-op.
