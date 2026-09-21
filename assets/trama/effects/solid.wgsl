/*! trama
{
  "name": "Solid",
  "id": "solid",
  "kind": "source",
  "inputs": 0,
  "params": [
    { "type": "Color", "name": "color", "default": [1.0, 1.0, 1.0, 1.0] }
  ]
}
*/
// Solid — a flat color, a picture from nothing. Dull on its own and useful
// everywhere else: it is the second input a Mix needs to tint or fade a
// chain, the backdrop a keyed layer sits on, and the quickest way to see what
// a node downstream actually does to a known input.
//
// A Color param takes four scalar slots, so `color` is param(0u)..param(3u):
// R, G, B, A. The alpha is real coverage — drop it and the whole frame goes
// transparent, not black.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let a = param(3u);
    return vec4f(vec3f(param(0u), param(1u), param(2u)) * a, a);
}
