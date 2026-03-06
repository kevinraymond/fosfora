// Raster background shader — optional motion blur trails from particle scatter.
// trail_decay=0: pure black (particles are the entire visual).
// trail_decay>0: feedback trails create motion blur during audio displacement.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;

    let decay = param(0u); // trail_decay

    // When decay is 0, just output black — particles are the whole show
    if decay < 0.01 {
        return vec4f(0.0, 0.0, 0.0, 1.0);
    }

    // Feedback trails with decay
    let prev = feedback(uv);
    let trail = clamp(prev.rgb, vec3f(0.0), vec3f(1.0)) * decay;

    // HDR clamp for safety
    let result = min(trail, vec3f(1.2));

    // Alpha from brightness for compositing
    let alpha = clamp(max(result.r, max(result.g, result.b)) * 2.0, 0.0, 1.0);
    return vec4f(result, alpha);
}
