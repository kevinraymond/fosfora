// Bloom extract — thresholds bright pixels with soft knee.
// Audio-driven: RMS lowers the threshold for more reactive bloom.

@group(0) @binding(0) var src_texture: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

struct BloomParams {
    threshold: f32,
    soft_knee: f32,
    rms: f32,
    _pad: f32,
}
@group(0) @binding(2) var<uniform> bloom: BloomParams;

@fragment
fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
    let color = textureSample(src_texture, src_sampler, uv);
    let luminance = dot(color.rgb, vec3f(0.2126, 0.7152, 0.0722));

    // Audio-reactive threshold: RMS lowers it
    let threshold = bloom.threshold - bloom.rms * 0.3;
    let knee = bloom.soft_knee;

    // Soft knee curve
    let soft = luminance - threshold + knee;
    let soft_clamped = clamp(soft, 0.0, 2.0 * knee);
    let contribution = soft_clamped * soft_clamped / (4.0 * knee + 0.0001);

    let brightness = max(luminance - threshold, contribution);
    let factor = brightness / (luminance + 0.0001);

    return vec4f(color.rgb * factor, color.a);
}
