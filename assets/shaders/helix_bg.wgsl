// Helix — background pass. A near-black backdrop for the ribbon volume, which the
// R3 ray marcher composites over (premultiplied, LoadOp::Load). Every particle
// effect needs at least one pass; this is Helix's.
//
// Very slightly darker toward the edges: the flythrough's own far end already
// falls to black down the middle of frame, so a corner vignette keeps the tunnel
// mouth reading as the brightest thing without tinting the ribbon itself.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let d = length(uv - vec2f(0.5)) * 1.414;
    let base = vec3f(0.012, 0.014, 0.028);
    return vec4f(base * (1.0 - 0.55 * d * d), 1.0);
}
