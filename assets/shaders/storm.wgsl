// Storm — Volumetric dark clouds lit from within by lightning flashes
// FBM-Worley density for puffy cloud shapes, Beer-Lambert light march for
// volumetric self-shadowing, silver lining at cloud edges.

// Smooth Worley noise — log-sum-exp blends cell distances to eliminate
// hard gradient discontinuities at cell boundaries (Inigo Quilez technique)
fn storm_worley(p: vec2f) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let k = 20.0; // sharpness: higher = sharper cells, lower = smoother blend
    var res = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let neighbor = vec2f(f32(x), f32(y));
            let cell = i + neighbor;
            let point = vec2f(
                phosphor_hash2(cell),
                phosphor_hash2(cell + vec2f(57.0, 113.0))
            );
            let diff = neighbor + point - f;
            res += exp(-k * dot(diff, diff));
        }
    }
    return sqrt(max(0.0, -(1.0 / k) * log(res)));
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let aspect = res.x / res.y;
    let p = (uv - 0.5) * vec2f(aspect, 1.0);
    let t = u.time;

    // param(0) = turbulence, param(1) = flow_speed, param(2) = flash_power, param(3) = flash_spread
    let turb_param = param(0u) * 0.7 + 0.4;
    let flow_speed = param(1u) * 0.8 + 0.1;
    let flash_power = param(2u) * 2.0 + 0.2;
    let flash_spread = param(3u) * 2.0 + 0.5;

    let flow_t = t * flow_speed;
    let bass = (u.sub_bass + u.bass) * 0.5;

    // === DOMAIN WARP (organic billowing) ===
    let warp_str = turb_param * (1.0 + bass * 0.3);
    let wq = vec2f(
        phosphor_fbm2(p * 2.0 + vec2f(0.0, flow_t * 0.25), 4, 0.5),
        phosphor_fbm2(p * 2.0 + vec2f(5.2, flow_t * 0.3), 4, 0.5)
    );
    let qw = p + (wq - 0.5) * 0.3 * warp_str;
    let wr = vec2f(
        phosphor_fbm2(qw * 2.0 + vec2f(1.7, flow_t * 0.15), 3, 0.5),
        phosphor_fbm2(qw * 2.0 + vec2f(8.3, flow_t * 0.2), 3, 0.5)
    );
    let wp = p + (wr - 0.5) * 0.25 * warp_str;

    // === CLOUD DENSITY (FBM - Worley for puffy billow shapes) ===
    // Worley carves rounded voids at cell boundaries → clouds form as round blobs
    let time_off = vec2f(flow_t * 0.08);
    let fbm_val = phosphor_fbm2(wp * 2.5 + time_off, 5, 0.55);
    let worley_val = storm_worley(wp * 2.0 + time_off * 0.3);
    let density = fbm_val - worley_val * 0.22;
    let cloud_shape = smoothstep(0.18, 0.48, density);

    // === BEER-LAMBERT LIGHT MARCH (4 steps) ===
    // Accumulate extinction toward light → volumetric self-shadowing
    // Bright: thin edges facing light. Dark: deep cloud interior.
    let light_dir = normalize(vec2f(0.3, 0.5));
    let march_step = 0.03;
    let extinction_coeff = 4.5;
    var transmittance = 1.0;
    for (var i = 1; i <= 4; i++) {
        let sp = wp + light_dir * march_step * f32(i);
        // LOD: 3 octaves, no Worley (cheaper shadow approximation)
        let sd = phosphor_fbm2(sp * 2.5 + time_off, 3, 0.55);
        let sample_shape = smoothstep(0.2, 0.5, sd);
        transmittance *= exp(-sample_shape * extinction_coeff * march_step);
    }

    // Silver lining: bright halo at thin cloud edges facing the light
    let silver = cloud_shape * (1.0 - cloud_shape) * 4.0 * transmittance;

    // Combined: volumetric lit surface + silver lining + faint deep base
    let total_bright = cloud_shape * transmittance * 0.6 + silver * 0.5 + cloud_shape * 0.05;

    // === BEAT FLASH ===
    let flash_env = pow(1.0 - smoothstep(0.0, 0.3, u.beat_phase), 2.0);
    let bpm_hz = max(u.bpm * 300.0, 60.0) / 60.0;
    let beat_idx = floor(t * bpm_hz);
    let flash_x = phosphor_hash2(vec2f(beat_idx, 0.0));
    let flash_y = phosphor_hash2(vec2f(beat_idx, 1.0));
    let flash_center = (vec2f(flash_x, flash_y) - 0.5) * vec2f(aspect * 0.6, 0.4);
    let fd = length(p - flash_center);
    let flash_local = exp(-fd * fd / (flash_spread * 0.15)) * flash_env;

    // === COMPOSITE ===
    let void_col = vec3f(0.005, 0.008, 0.025);
    let shadow_col = vec3f(0.012, 0.018, 0.05);
    var col = mix(void_col, shadow_col, cloud_shape);

    // Ambient glow (always visible — reveals cloud structure between beats)
    let ambient = mix(vec3f(0.015, 0.03, 0.12), vec3f(0.1, 0.22, 0.6), transmittance);
    col += ambient * total_bright * 0.28;

    // Flash glow (beat-triggered, localized)
    let flash_col = mix(vec3f(0.08, 0.18, 0.5), vec3f(0.5, 0.65, 1.0), transmittance);
    col += flash_col * total_bright * flash_local * flash_power;

    // Onset micro-flickers
    col *= 1.0 + u.onset * 0.15 * total_bright;

    // === FEEDBACK ===
    let advect = (wq - 0.5) * 0.002 * (1.0 + bass);
    let prev = feedback(uv + advect);
    let decay = 0.72;
    let result = mix(col, prev.rgb * decay, 0.45);

    return vec4f(min(result, vec3f(1.2)), 1.0);
}
