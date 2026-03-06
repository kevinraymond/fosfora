// Array background shader — feedback trails with ring-shaped glow at emitter positions.

const TAU: f32 = 6.2831853;
const NUM_BANDS: u32 = 5u;

fn emitter_center(band: u32, arrangement: f32) -> vec2f {
    let stack_y = f32(band) / 4.0 * 1.4 - 0.7;
    let stack_pos = vec2f(0.0, stack_y);
    let concentric_pos = vec2f(0.0, 0.0);
    return mix(stack_pos, concentric_pos, arrangement);
}

fn emitter_ring_radius(band: u32, arrangement: f32, radius_param: f32) -> f32 {
    var band_scale: f32;
    switch band {
        case 0u: { band_scale = 1.0; }
        case 1u: { band_scale = 0.85; }
        case 2u: { band_scale = 0.7; }
        case 3u: { band_scale = 0.55; }
        default: { band_scale = 0.4; }
    }
    let stack_r = radius_param * 0.15 * band_scale;
    let concentric_r = radius_param * (0.1 + f32(band) * 0.15);
    return mix(stack_r, concentric_r, arrangement);
}

fn band_energy(band: u32) -> f32 {
    switch band {
        case 0u: { return max(u.sub_bass * 0.6 + u.bass * 0.4, 0.02); }
        case 1u: { return max(u.bass * 0.3 + u.low_mid * 0.7, 0.02); }
        case 2u: { return max(u.low_mid * 0.3 + u.mid * 0.7, 0.02); }
        case 3u: { return max(u.mid * 0.3 + u.upper_mid * 0.7, 0.02); }
        default: { return max(u.presence * 0.5 + u.brilliance * 0.5, 0.02); }
    }
}

fn band_color(band: u32) -> vec3f {
    switch band {
        case 0u: { return vec3f(1.0, 0.2, 0.1); }
        case 1u: { return vec3f(1.0, 0.5, 0.1); }
        case 2u: { return vec3f(0.2, 0.9, 0.4); }
        case 3u: { return vec3f(0.2, 0.5, 1.0); }
        default: { return vec3f(0.7, 0.8, 1.0); }
    }
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let p = uv * 2.0 - 1.0;

    // Feedback trail with slight center-pull warp
    let decay = param(0u);
    let warp = 0.002 + u.rms * 0.001;
    let warped_uv = clamp(uv + (vec2f(0.5) - uv) * warp, vec2f(0.001), vec2f(0.999));
    let trail = feedback(warped_uv).rgb * decay;

    // Ring-shaped glow at each emitter position
    let glow_param = param(7u);
    let arrangement = param(3u);
    let radius_param = param(1u);
    var glow = vec3f(0.0);
    for (var b = 0u; b < NUM_BANDS; b++) {
        let center = emitter_center(b, arrangement);
        let ring_r = emitter_ring_radius(b, arrangement, radius_param);
        let dist_to_ring = abs(length(p - center) - ring_r);
        let energy = band_energy(b);
        let width = 0.02 + energy * 0.06;
        let ring_glow = exp(-dist_to_ring * dist_to_ring / (width * width)) * energy;
        glow += band_color(b) * ring_glow;
    }
    glow *= glow_param * 0.1;

    let result = min(trail + glow, vec3f(1.5));
    let alpha = clamp(max(result.r, max(result.g, result.b)) * 2.0, 0.0, 1.0);
    return vec4f(result, alpha);
}
