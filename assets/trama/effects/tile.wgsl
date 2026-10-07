/*! trama
{
  "name": "Tile",
  "id": "tile",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "count",    "default": 3.0, "min": 1.0,  "max": 16.0 },
    { "type": "Bool",  "name": "mirror",   "default": false },
    { "type": "Float", "name": "scroll_x", "default": 0.0, "min": -2.0, "max": 2.0 },
    { "type": "Float", "name": "scroll_y", "default": 0.0, "min": -2.0, "max": 2.0 }
  ],
  "rates": ["scroll_x", "scroll_y"],
  "integers": ["count"]
}
*/
// Tile — repeats the whole picture in a grid, `count` copies across and
// down. `mirror` flips every other tile so neighbors meet edge to edge with
// no seam, which turns any picture into a pattern. `scroll_x` / `scroll_y`
// slide the grid, in tiles per second.
//
// The scroll parameters are rates, so the shader is handed their running
// phase and modulating the speed never makes the grid jump (see `rates` in
// trama/effect.rs).

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let n = max(round(param(0u)), 1.0);
    let p = uv * n - vec2f(param(2u), param(3u));

    var f = fract(p);
    if param(1u) > 0.5 {
        // Odd tiles run backwards: 0→1 then 1→0, a triangle wave.
        let odd = abs(floor(p) % 2.0);
        f = mix(f, 1.0 - f, odd);
    }
    // Half a texel in from the edge, so linear filtering at a tile seam reads
    // this tile's border instead of blending in the opposite edge.
    let half_texel = 0.5 / u.resolution;
    return input0(clamp(f, half_texel, vec2f(1.0) - half_texel));
}
