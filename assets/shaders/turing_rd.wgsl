// Turing — Gray-Scott reaction-diffusion compute shader.
//
// Runs N steps per frame on a ping-pong texture pair.
// Wrapping boundaries (toroidal domain) via modulo arithmetic.
// Audio onset injects B chemical drops at random positions.

struct RDUniforms {
    feed_rate: f32,
    kill_rate: f32,
    diffuse_a: f32,
    diffuse_b: f32,
    time: f32,
    onset: f32,
    drop_radius: f32,
    _pad: f32,
}

@group(0) @binding(0) var<uniform> rd: RDUniforms;
@group(0) @binding(1) var rd_src: texture_2d<f32>;
@group(0) @binding(2) var rd_dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(rd_src);
    if gid.x >= dims.x || gid.y >= dims.y { return; }

    let c = vec2i(gid.xy);
    let w = i32(dims.x);
    let h = i32(dims.y);

    // Read center + 4 neighbors (wrapping boundaries)
    let center = textureLoad(rd_src, c, 0);
    let left   = textureLoad(rd_src, vec2i((c.x - 1 + w) % w, c.y), 0);
    let right  = textureLoad(rd_src, vec2i((c.x + 1) % w,     c.y), 0);
    let up     = textureLoad(rd_src, vec2i(c.x, (c.y - 1 + h) % h), 0);
    let down   = textureLoad(rd_src, vec2i(c.x, (c.y + 1) % h),     0);

    var A = center.r;
    var B = center.g;

    // 5-point Laplacian
    let lapA = (left.r + right.r + up.r + down.r) - 4.0 * A;
    let lapB = (left.g + right.g + up.g + down.g) - 4.0 * B;

    // Gray-Scott step (dt=1.0)
    let react = A * B * B;
    A = clamp(A + rd.diffuse_a * lapA - react + rd.feed_rate * (1.0 - A), 0.0, 1.0);
    B = clamp(B + rd.diffuse_b * lapB + react - (rd.kill_rate + rd.feed_rate) * B, 0.0, 1.0);

    // Onset drop injection — seed B chemical at random positions on beat
    // Multiple drops spread across the screen for even coverage
    if rd.onset > 0.5 {
        let uv = vec2f(gid.xy) / vec2f(dims);
        let radius = max(rd.drop_radius, 0.01);
        // Integer hash for stable randomness regardless of time magnitude
        let tick = u32(rd.time * 10.0);

        // Inject 4 drops at different positions using integer hash (pcg-style)
        var total_strength = 0.0;
        for (var i = 0u; i < 4u; i++) {
            var h = tick * 747796405u + i * 2891336453u + 1u;
            h = ((h >> 16u) ^ h) * 2654435769u;
            h = ((h >> 16u) ^ h);
            let hx = h & 0xFFFFu;
            let hy = (h >> 16u) & 0xFFFFu;
            let dp = vec2f(f32(hx) / 65535.0, f32(hy) / 65535.0);
            let d = length(uv - dp);
            total_strength += exp(-d * d / (radius * radius));
        }
        let strength = total_strength * min(rd.onset, 1.0) * 0.35;
        B = clamp(B + strength, 0.0, 1.0);
        A = clamp(A - strength * 0.7, 0.0, 1.0);
    }

    textureStore(rd_dst, c, vec4f(A, B, 0.0, 1.0));
}
