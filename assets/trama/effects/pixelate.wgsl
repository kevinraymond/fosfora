/*! trama
{
  "name": "Pixelate",
  "id": "pixelate",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "size", "default": 32.0, "min": 2.0, "max": 256.0 },
    { "type": "Float", "name": "gap",  "default": 0.0,  "min": 0.0, "max": 0.5 }
  ]
}
*/
// Pixelate — quantizes the picture to a grid of cells, each the color of its
// own center. `size` is roughly how many cells fit down the frame.
//
// `gap` clears a transparent border inside every cell, which turns the blocks
// into a tile wall with the layers beneath showing through the grout. At 0 it
// is a plain mosaic.
//
// THE CELL IS AN INTEGER NUMBER OF PIXELS, and that is the whole trick. Laying
// the grid out in UV space gives cells of, say, 6.4 px: cell boundaries then
// land at different sub-pixel offsets all the way across the frame, so some
// gaps round to one pixel and some to two. The eye reads that alternation as
// major and minor rules and the whole thing looks like graph paper. Snapping
// the cell to whole pixels makes every cell identical, which also makes cells
// square without an aspect-ratio correction.
//
// The gap is measured in PIXELS from the cell edge and feathered across one,
// so a thin gap fades instead of dropping in and out between cells. At gap 0
// no pixel center is within half a pixel of an edge, so the mosaic is solid.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;

    let cell_px = max(floor(res.y / max(param(0u), 2.0)), 1.0);
    let cell = floor(frag_coord.xy / cell_px);

    // Distance in pixels from this pixel's center to the nearest cell edge,
    // per axis; the tighter axis is the one that decides. Worked out by
    // SUBTRACTING whole pixels rather than taking a fraction of the division:
    // `cell * cell_px` and `frag_coord.xy` are both exact in f32, so this is
    // exact, and every cell gets a bit-identical gap. Via `x / cell_px -
    // cell` the same position rounds differently across the frame, which puts
    // a one-ULP wobble into the gap's edge.
    let within_px = frag_coord.xy - cell * cell_px;
    let edge_px = min(within_px, vec2f(cell_px) - within_px);
    let gap_px = param(1u) * cell_px;
    let k = smoothstep(gap_px - 0.5, gap_px + 0.5, min(edge_px.x, edge_px.y));

    let c = input0(clamp((cell + 0.5) * cell_px / res, vec2f(0.0), vec2f(1.0)));
    return c * k;
}
