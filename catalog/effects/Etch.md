# Etch

> Your image scratched into aluminium powder one stroke at a time — the stylus zigzags along a handful of scan lines, swinging wide where the picture is dark, then the board is shaken clean and it starts over. Pull Clear cycle to zero to hold the board blank

- type: particle
- pace param: draw_rate (default=0.22 hi=1.082; no lo variant rendered)
- motion (loud half, mean Δ): default=0.0011 hi=0.0011
- luma (quiet/loud at default): 0.816 / 0.817
- headless caveats: none

## Imagery
A pale warm-grey board — the brightest frame in this batch — carrying one faint pencil-weight drawing built from rows of tiny zigzag scribbles: in these renders, a small horned cartoon face (the catalog's test image), its hatching dense in the dark features and absent in the lights. Tiny rainbow flecks glint along the strokes. Contrast is whisper-low (luma_std 0.03); from across a room the frame reads as a blank sheet of paper.

## Motion character
Essentially static to the eye — mean Δ 0.0011, two orders below the catalog's moderate band. The only motion is the crawling stylus line and the periodic board-clear. The draw_rate sweep (0.22 → 1.08) is invisible in the numbers (0.00105 vs 0.00107); do not cast this param expecting a visible pace change.

## Energy response
Nearly none visible: quiet vs loud is 0.0008 vs 0.0011 motion and 0.816 vs 0.817 luma. The documented response (RMS drives stylus speed, kick rattles the pen, drops shake the board clean) operates at a scale these stills and stats barely register.

## Palette
Whites and pale warm greys with graphite-grey strokes and pin-point RGB speckle. High key, near-zero saturation, paper-like.

## Casting notes
BYO-media effect: it draws a supplied image, and everything on this board depends on that image — these renders show only the built-in horned-face doodle, so judge composition, not content. Cast for sketch reveals, hand-drawn credits, lo-fi zine interludes, an Etch-a-Sketch conceit. It is a bright, still, full-frame surface: it cannot carry beat energy or motion, and its near-white ground will dominate or blow out additive/screen layer mixes. Best solo, or as a top layer with deliberate opacity.
