/*! trama
{
  "name": "Glitch",
  "id": "glitch",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "intensity", "default": 0.5,  "min": 0.0, "max": 1.0 },
    { "type": "Float", "name": "blocks",    "default": 24.0, "min": 2.0, "max": 128.0 },
    { "type": "Float", "name": "shift",     "default": 0.1,  "min": 0.0, "max": 0.5 },
    { "type": "Float", "name": "rgb_split", "default": 0.01, "min": 0.0, "max": 0.05 },
    { "type": "Float", "name": "speed",     "default": 8.0,  "min": 0.0, "max": 30.0 }
  ],
  "rates": ["speed"],
  "integers": ["blocks"]
}
*/
// Glitch — tears the picture into displaced strips and blocks with the color
// channels pulled apart, the broken-signal look. Put `onset` on `intensity`'s
// mod row and it breaks up on every hit and snaps back between them; at 0 the
// picture passes through untouched.
//
// `blocks` is how many horizontal strips the frame is cut into; the blocks
// are squares the height of one strip. `intensity` sets how many strips and
// blocks break loose as well as how hard the channels split. A loose strip
// slides sideways up to `shift` of the frame's width and wraps around; a
// loose block also jumps vertically. `rgb_split` is the red/blue separation
// at full intensity, larger inside the broken regions.
//
// `speed` is how many times a second a new pattern is drawn. It is a RATE
// (see "rates" above): param(4u) arrives as the count of patterns so far, so
// turning it up or modulating it never skips the pattern backwards. At 0 the
// current pattern holds.
//
// Param slots: intensity 0, blocks 1, shift 2, rgb_split 3, speed 4.

// PCG3D (Jarzynski & Olano 2020): three well-mixed u32s from three inputs.
// Integer hashing, not a sin() hash, so a pattern looks the same on every GPU.
fn glitch_pcg3d(v_in: vec3u) -> vec3u {
    var v = v_in * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v = v ^ (v >> vec3u(16u));
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return v;
}

// Three uniform values in [0, 1) for one cell of one pattern.
fn glitch_rand(cell: vec2f, seed: u32, salt: u32) -> vec3f {
    let c = vec2u(vec2i(cell) + vec2i(65536));
    let h = glitch_pcg3d(vec3u(c.x ^ (salt * 0x9e3779b9u), c.y, seed));
    return vec3f(h >> vec3u(8u)) * (1.0 / 16777216.0);
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let intensity = clamp(param(0u), 0.0, 1.0);
    if intensity <= 0.0 {
        return input0(uv);
    }

    let rows = max(round(param(1u)), 1.0);
    let shift = param(2u);
    let seed = u32(max(floor(param(4u)), 0.0));

    // Strips: a share of them, growing with intensity, slide sideways.
    let strip = glitch_rand(vec2f(0.0, floor(uv.y * rows)), seed, 1u);
    let strip_on = select(0.0, 1.0, strip.x < intensity * 0.5);
    var offset = vec2f((strip.y * 2.0 - 1.0) * shift * strip_on, 0.0);

    // Blocks: square cells the height of a strip; fewer break loose, but they
    // also jump vertically, which reads as a corrupted macroblock.
    let cols = max(round(rows * res.x / res.y), 1.0);
    let cell = floor(uv * vec2f(cols, rows));
    let block = glitch_rand(cell, seed, 2u);
    let block_on = select(0.0, 1.0, block.x < intensity * 0.2);
    offset += (block.yz * 2.0 - 1.0) * vec2f(shift, shift * 0.5) * block_on;

    // Wrap sideways like a lost horizontal sync; clamp vertically.
    let p = vec2f(fract(uv.x + offset.x), clamp(uv.y + offset.y, 0.0, 1.0));
    let broken = max(strip_on, block_on);
    let split = vec2f(param(3u) * intensity * (1.0 + 2.0 * broken), 0.0);

    let r = input0(vec2f(fract(p.x + split.x), p.y));
    let g = input0(p);
    let b = input0(vec2f(fract(p.x - split.x), p.y));

    // Widest alpha of the three, as Chromatic Aberration does: keeping
    // green's would erase the fringe wherever green lands on transparency.
    return vec4f(r.r, g.g, b.b, max(g.a, max(r.a, b.a)));
}
