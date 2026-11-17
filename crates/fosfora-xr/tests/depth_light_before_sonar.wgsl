// fs_mesh_light as it was before the sweep (board #5511, at e9df95e), kept
// for tests/depth_light_gpu.rs: with the sweep's rows at 0 the depth light
// must draw what this drew, bit for bit. Appended after
// env_depth::ENV_DEPTH_WGSL, whose helpers it calls. Not used by the app.
@fragment
fn fs_mesh_light_before(in: LightOut) -> @location(0) vec4<f32> {
    let p = in.world.xyz;
    // Before any branch: how far the world moves across a pixel.
    let fw = fwidth(p);
    let keep = in.world.w;
    let weight = env.light.z * in.normal.w;
    var points = 0.0;
    var up = 0.0;
    if keep > 0.0 && weight > 0.0 {
        let nn = in.normal.xyz;
        let n = select(vec3<f32>(0.0, 0.0, 1.0), normalize(nn), dot(nn, nn) > 1e-12);
        up = clamp(n.y, 0.0, 1.0);
        let n2 = n * n;
        let tri = n2 * n2 / dot(n2, n2);
        let l = clamp(log2(max(length(p - env.eye.xyz), 1e-3)), OCTAVE_MIN, OCTAVE_MAX);
        let li = floor(l);
        let next = smoothstep(env.light_cell.y, 1.0, l - li);
        let radius_scale = env.light_point.x * env.light_point.w * sqrt(keep);
        var sum = 0.0;
        var used = 0.0;
        for (var j = 0; j < 2; j++) {
            let wo = select(1.0 - next, next, j == 1);
            if wo <= 0.0 {
                continue;
            }
            let o = i32(li) + j;
            let cell = ldexp(env.light_cell.x, o);
            for (var k = 0u; k < 3u; k++) {
                // Less the floor, so a plane fades out rather than drops.
                let wk = tri[k] - PLANE_MIN;
                if wk <= 0.0 {
                    continue;
                }
                let f = light_plane(fw, k);
                let aa = 1.0 - smoothstep(env.light_cell.z, env.light_cell.w, max(f.x, f.y) / cell);
                if aa > 0.0 {
                    sum += wo * wk * aa * plane_points(light_plane(p, k) / cell, k, o, radius_scale);
                }
                used += wo * wk;
            }
        }
        points = sum / max(used, 1e-6);
    }
    let wave = 1.0 - smoothstep(env.light_wave.x, env.light_wave.y, fw.y * env.light.x);
    let c = light_wave(p.y * env.light.x - env.light.y);
    let crest = mix(env.light_wave.z, c * c, wave);
    let band = env.light_side.w + (1.0 - env.light_side.w) * crest * env.light.w;
    let tint = clamp(up + env.light_top.w * crest, 0.0, 1.0);
    let a = clamp(weight * band * points, 0.0, 1.0);
    return vec4<f32>(mix(env.light_side.rgb, env.light_top.rgb, tint) * a, env.light_fade.w);
}
