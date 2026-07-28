// Tesla background shader — feedback trails with electric shimmer and dim field line hints.

fn bg_dipole(p: vec2f, d: vec2f, moment: f32) -> f32 {
    let r = p - d;
    let r2 = dot(r, r) + 0.05;
    let r_mag = sqrt(r2);
    return moment / (r2 * r_mag);
}

fn bg_total_field(p: vec2f, t: f32, mode: f32, rotation: f32) -> f32 {
    let angle = t * rotation * 0.5;
    let ca = cos(angle);
    let sa = sin(angle);
    let moment = 0.3;

    var B = 0.0;
    if mode < 0.25 {
        let d1 = vec2f(ca * 0.4, sa * 0.4);
        let d2 = vec2f(-ca * 0.4, -sa * 0.4);
        B = bg_dipole(p, d1, moment) + bg_dipole(p, d2, moment);
    } else if mode < 0.5 {
        let d1 = vec2f(ca * 0.4, sa * 0.4);
        let d2 = vec2f(-ca * 0.4, -sa * 0.4);
        B = bg_dipole(p, d1, moment) + bg_dipole(p, d2, -moment);
    } else if mode < 0.75 {
        for (var i = 0; i < 4; i++) {
            let a = angle + f32(i) * 1.5707963;
            let d = vec2f(cos(a), sin(a)) * 0.35;
            let s = select(-1.0, 1.0, i % 2 == 0);
            B += bg_dipole(p, d, moment * s);
        }
    } else {
        for (var i = 0; i < 4; i++) {
            let a = angle + f32(i) * 1.5707963;
            let d = vec2f(cos(a), sin(a)) * 0.35;
            let s = select(-1.0, 1.0, (i / 2) % 2 == 0);
            B += bg_dipole(p, d, moment * s);
        }
        B += bg_dipole(p, vec2f(0.0), sin(t * 1.5) * 0.15);
    }
    return B;
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let p = uv * 2.0 - 1.0;

    let decay = param(0u);
    let prev = feedback(uv);

    let shimmer_str = 0.001 + u.onset * 0.003;
    let shimmer_x = phosphor_noise2(p * 8.0 + vec2f(u.time * 0.8, 0.0)) - 0.5;
    let shimmer_y = phosphor_noise2(p * 8.0 + vec2f(0.0, u.time * 0.7)) - 0.5;
    let shimmer_uv = uv + vec2f(shimmer_x, shimmer_y) * shimmer_str;
    let shimmer_prev = feedback(clamp(shimmer_uv, vec2f(0.001), vec2f(0.999)));

    let trail = mix(prev.rgb, shimmer_prev.rgb, 0.4) * decay;

    let mode = param(3u);
    let rotation = param(4u);
    let B = bg_total_field(p, u.time, mode, rotation);
    let field_vis = abs(B) * 0.005 * u.rms;
    let field_color = vec3f(0.15, 0.25, 0.5) * field_vis;

    let flip_sens = param(7u);
    let beat_flash = u.beat * flip_sens * 0.04;
    let flash_color = vec3f(0.5, 0.6, 0.8) * beat_flash;

    let result = min(trail + field_color + flash_color, vec3f(1.1));
    let alpha = clamp(max(result.r, max(result.g, result.b)) * 2.0, 0.0, 1.0);
    return vec4f(result, alpha);
}
