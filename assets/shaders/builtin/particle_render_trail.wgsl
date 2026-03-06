// Trail renderer — draws ribbon strips from per-particle trail ring buffers.
// Each instance = one alive particle. Vertices per instance = 6 * (trail_length - 1).
// Each trail segment is a quad between two adjacent trail points.
// Trail points stored as vec4f(pos.x, pos.y, size, alpha) in ring buffer.

struct RenderUniforms {
    resolution: vec2f,
    time: f32,
    render_mode: u32,
    sprite_cols: u32,
    sprite_rows: u32,
    sprite_frames: u32,
    frame_index: u32,
    trail_length: u32,
    trail_width: f32,
    _pad: vec2f,
}

@group(0) @binding(0) var<storage, read> pos_life: array<vec4f>;
@group(0) @binding(1) var<storage, read> vel_size: array<vec4f>;
@group(0) @binding(2) var<storage, read> color: array<vec4f>;
@group(0) @binding(3) var<storage, read> flags: array<vec4f>;
@group(0) @binding(4) var<uniform> ru: RenderUniforms;
@group(0) @binding(5) var<storage, read> alive_indices: array<u32>;
@group(0) @binding(6) var<storage, read> trail_buffer: array<vec4f>;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) color: vec4f,
    @location(1) trail_frac: f32,  // 0 = head (newest), 1 = tail (oldest)
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    var out: VertexOutput;

    let particle_idx = alive_indices[instance_index];
    let p_color = color[particle_idx];

    let trail_len = ru.trail_length;
    if trail_len < 2u {
        // No trail — degenerate
        out.position = vec4f(0.0, 0.0, 0.0, 0.0);
        out.color = vec4f(0.0);
        out.trail_frac = 0.0;
        return out;
    }

    let segments = trail_len - 1u;

    // Which segment and which vertex within the segment quad
    let segment_id = vertex_index / 6u;
    let quad_vi = vertex_index % 6u;

    if segment_id >= segments {
        out.position = vec4f(0.0, 0.0, 0.0, 0.0);
        out.color = vec4f(0.0);
        out.trail_frac = 0.0;
        return out;
    }

    // Ring buffer head: current frame's write slot
    let frame = u32(ru.time * 60.0);
    let head_slot = frame % trail_len;

    // Trail points: head is newest, iterate backward for older points
    // Point A (newer) and Point B (older) for this segment
    // segment_id=0 → newest segment, segment_id=segments-1 → oldest
    let slot_a = (head_slot + trail_len - segment_id) % trail_len;
    let slot_b = (head_slot + trail_len - segment_id - 1u) % trail_len;

    let base = particle_idx * trail_len;
    let point_a = trail_buffer[base + slot_a];
    let point_b = trail_buffer[base + slot_b];

    // Skip if either point is at origin (not yet written)
    let a_valid = abs(point_a.x) + abs(point_a.y) > 0.0001;
    let b_valid = abs(point_b.x) + abs(point_b.y) > 0.0001;
    if !a_valid || !b_valid {
        out.position = vec4f(0.0, 0.0, 0.0, 0.0);
        out.color = vec4f(0.0);
        out.trail_frac = 0.0;
        return out;
    }

    // Ribbon direction: perpendicular to segment
    let dir = point_a.xy - point_b.xy;
    let seg_len = length(dir);
    let perp = select(
        vec2f(0.0, 1.0),
        normalize(vec2f(-dir.y, dir.x)),
        seg_len > 0.0001
    );

    // Width tapers from head (full) to tail (zero)
    let frac_a = f32(segment_id) / f32(segments);
    let frac_b = f32(segment_id + 1u) / f32(segments);
    let width_a = ru.trail_width * (1.0 - frac_a) * point_a.z / 0.01; // scale by particle size
    let width_b = ru.trail_width * (1.0 - frac_b) * point_b.z / 0.01;

    // Aspect ratio correction
    let aspect = ru.resolution.x / ru.resolution.y;
    let perp_corrected = perp * vec2f(1.0 / aspect, 1.0);

    // Quad vertices: 2 triangles
    // Vertices: A-left, A-right, B-right, B-right, B-left, A-left
    var pos: vec2f;
    var frac: f32;
    switch quad_vi {
        case 0u: { pos = point_a.xy - perp_corrected * width_a; frac = frac_a; }
        case 1u: { pos = point_a.xy + perp_corrected * width_a; frac = frac_a; }
        case 2u: { pos = point_b.xy + perp_corrected * width_b; frac = frac_b; }
        case 3u: { pos = point_b.xy + perp_corrected * width_b; frac = frac_b; }
        case 4u: { pos = point_b.xy - perp_corrected * width_b; frac = frac_b; }
        case 5u: { pos = point_a.xy - perp_corrected * width_a; frac = frac_a; }
        default: { pos = vec2f(0.0); frac = 0.0; }
    }

    out.position = vec4f(pos, 0.0, 1.0);
    out.trail_frac = frac;

    // Color: particle color with alpha tapering along trail
    let trail_alpha = (1.0 - frac) * (1.0 - frac); // quadratic falloff
    let avg_alpha = mix(point_a.w, point_b.w, 0.5);
    out.color = vec4f(p_color.rgb, avg_alpha * trail_alpha * 0.5);

    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    if in.color.a < 0.005 {
        discard;
    }
    return in.color;
}
