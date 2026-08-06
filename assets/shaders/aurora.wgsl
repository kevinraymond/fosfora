// Aurora — Horizontal flowing curtain bands driven by 7 frequency bands
// A spectrogram disguised as northern lights.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let aspect = res.x / res.y;
    let p = (uv - 0.5) * vec2f(aspect, 1.0);
    let t = u.time;

    // param(0) = curtain_speed, param(1) = band_spread, param(2) = glow_width
    let curtain_speed = param(0u) * 2.0 + 0.3;
    let band_spread = param(1u) * 1.5 + 0.5;
    let glow_width = param(2u) * 0.04 + 0.008;

    // 7 frequency bands mapped to vertical positions
    let bands = array<f32, 7>(
        u.sub_bass,
        u.bass,
        u.low_mid,
        u.mid,
        u.upper_mid,
        u.presence,
        u.brilliance
    );

    // Band colors: warm (low) to cool (high)
    let band_colors = array<vec3f, 7>(
        vec3f(0.8, 0.1, 0.2),   // sub_bass: deep red
        vec3f(1.0, 0.4, 0.1),   // bass: orange
        vec3f(1.0, 0.8, 0.2),   // low_mid: gold
        vec3f(0.2, 1.0, 0.3),   // mid: green
        vec3f(0.1, 0.7, 1.0),   // upper_mid: cyan
        vec3f(0.3, 0.3, 1.0),   // presence: blue
        vec3f(0.7, 0.2, 1.0)    // brilliance: violet
    );

    var col = vec3f(0.0);

    for (var i = 0; i < 7; i++) {
        let fi = f32(i);
        let band_val = bands[i];

        // Vertical position for this band
        let band_y = (fi - 3.0) / 7.0 * band_spread;

        // Horizontal wave distortion — each band has different flow
        let wave_freq = 2.0 + fi * 0.5;
        let wave_phase = t * curtain_speed * (0.8 + fi * 0.1);
        let wave = sin(p.x * wave_freq + wave_phase) * 0.05
                 + sin(p.x * wave_freq * 2.3 + wave_phase * 0.7) * 0.03;

        // Noise-based curtain ripple
        let noise_p = vec2f(p.x * 3.0 + wave_phase * 0.5, fi * 2.0 + t * 0.1);
        let ripple = (fosfora_noise2(noise_p) - 0.5) * 0.08;

        // Distance from this band's center line
        let dy = p.y - band_y - wave - ripple;
        let width = glow_width * (1.0 + band_val * 1.5);

        // Soft gaussian glow
        let glow = exp(-dy * dy / (width * width));

        // Band brightness from audio
        let brightness = band_val * (1.5 + u.rms * 0.5);

        // Color for this band
        let band_col = band_colors[i] * brightness * glow;
        col += band_col;
    }

    // Subtle vertical shimmer
    let shimmer = fosfora_noise2(vec2f(p.x * 10.0 + t, p.y * 30.0)) * 0.05 * u.presence;
    col += vec3f(shimmer);

    // Onset brightens everything
    col *= (1.0 + u.onset * 0.8);

    // Beat phase subtle pulse
    let beat_bright = 1.0 + sin(u.beat_phase * 6.28318) * 0.1 * u.beat_strength;
    col *= beat_bright;

    return vec4f(col, 1.0);
}
