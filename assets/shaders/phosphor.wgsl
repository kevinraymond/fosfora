// Phosphor background — pure feedback decay.
// All visuals come from the particle tubes.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let prev = feedback(uv);
    let decay = param(0u); // trail_decay
    var col = prev.rgb * decay;
    col = min(col, vec3f(1.5));
    let alpha = clamp(max(col.r, max(col.g, col.b)) * 2.0, 0.0, 1.0);
    return vec4f(col, alpha);
}
