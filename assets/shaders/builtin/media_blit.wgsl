// Media blit shader — samples Rgba8UnormSrgb texture with letterbox transform,
// outputs to Rgba16Float HDR target.

struct MediaUniforms {
    scale: vec2f,
    offset: vec2f,
    mirror: u32,
}

@group(0) @binding(0) var media_texture: texture_2d<f32>;
@group(0) @binding(1) var media_sampler: sampler;
@group(0) @binding(2) var<uniform> mu: MediaUniforms;

@fragment
fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
    var media_uv = (uv - mu.offset) / mu.scale;
    if media_uv.x < 0.0 || media_uv.x > 1.0 || media_uv.y < 0.0 || media_uv.y > 1.0 {
        return vec4f(0.0);
    }
    if mu.mirror != 0u {
        media_uv.x = 1.0 - media_uv.x;
    }
    return textureSample(media_texture, media_sampler, media_uv);
}
