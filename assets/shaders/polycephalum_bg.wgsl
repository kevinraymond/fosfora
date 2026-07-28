// Polycephalum background — feedback decay for the species-tinted trail networks.
//
// The visible veins are the additive compute-raster render of the agents accumulating in the
// feedback target; this pass decays that accumulation each frame so paths fade unless kept alive
// by agent traffic. The beat gives the whole network a subtle breathing pulse.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;

    // trail_decay param, dipped briefly on the beat so the network "breathes".
    let decay = clamp(param(4u) * (1.0 - u.beat * 0.12), 0.5, 0.995);

    let prev = feedback(uv).rgb;
    let trail = prev * decay;

    // HDR clamp to keep additive accumulation from blowing out.
    let result = min(trail, vec3f(1.6));
    let alpha = clamp(max(result.r, max(result.g, result.b)) * 2.0, 0.0, 1.0);
    return vec4f(result, alpha);
}
