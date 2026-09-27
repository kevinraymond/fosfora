// S5 per-eye billboard draw of the world-space particles.

// ---- per-eye billboard draw -------------------------------------------------

struct Eye {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
}

@group(1) @binding(0) var<uniform> eye: Eye;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec3<f32>,
}

// One camera-facing quad per particle: the corner offset is applied in view
// space so the sprite keeps a size in meters and perspective sets its
// footprint on screen (nearer = bigger), which is what depth cues need.
@vertex
fn vs_particle(
    @builtin(vertex_index) vi: u32,
    @builtin(instance_index) ii: u32,
) -> VsOut {
    // A quad (6 vertices) or, with 3 vertices per instance, one triangle
    // that circumscribes the unit disc (the fragment stage discards outside
    // it either way). The Rust side picks the vertex count.
    var corners = array<vec2<f32>, 9>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.7320508, -1.0), vec2<f32>(1.7320508, -1.0), vec2<f32>(0.0, 2.0),
    );
    var particle = ii;
    var corner_index = vi;
    if sim.pull != 0u {
        particle = vi / sim.verts_per_sprite;
        corner_index = vi % sim.verts_per_sprite;
    }
    if sim.verts_per_sprite == 3u {
        corner_index += 6u;
    }
    let p = particles[particle];
    let corner = corners[corner_index];
    let age = 1.0 - clamp(p.life / sim.lifetime, 0.0, 1.0);
    // Fade in and out over the life so respawns don't pop, and toward the
    // cube faces so the sim's bounds never read as hard edges in a room.
    let d = abs(p.pos - sim.cube_center) / sim.cube_half;
    let edge = max(d.x, max(d.y, d.z));
    let fade = smoothstep(0.0, 0.1, age) * (1.0 - smoothstep(0.85, 1.0, age))
        * (1.0 - smoothstep(0.7, 1.0, edge));
    let radius = sim.base_size * sim.size * (0.6 + 0.8 * p.seed);
    let view_pos = eye.view * vec4<f32>(p.pos, 1.0);
    let offset = vec4<f32>(corner * radius, 0.0, 0.0);
    var out: VsOut;
    out.pos = eye.proj * (view_pos + offset);
    if -view_pos.z < sim.near_cull {
        // Behind the near plane: the whole sprite is clipped away.
        out.pos = vec4<f32>(0.0, 0.0, 2.0, 1.0);
    }
    out.uv = corner;
    // Hue by seed, brightness by speed and life.
    let speed = length(p.vel);
    let hue = fract(p.seed + sim.time * 0.02);
    let base = 0.5 + 0.5 * cos(6.2831 * (hue + vec3<f32>(0.0, 0.33, 0.67)));
    // Dim: hundreds of thousands of additive sprites saturate fast.
    out.color = base * (0.5 + 2.0 * speed) * fade * 0.12;
    return out;
}

@fragment
fn fs_particle(in: VsOut) -> @location(0) vec4<f32> {
    let r2 = dot(in.uv, in.uv);
    if r2 > 1.0 {
        discard;
    }
    let a = (1.0 - r2) * (1.0 - r2);
    return vec4<f32>(in.color * a, a);
}
