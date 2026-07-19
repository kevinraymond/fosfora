// Volumetric ray march: fullscreen fragment pass that renders the 3D density
// texture as self-illuminated fog/nebula. An orbiting camera ray is built inline
// (no view/projection matrix — same technique as strata.wgsl), bounded to the
// unit cube [-1,1]^3 via a slab ray/box test, then Beer-Lambert transmittance is
// accumulated with FBM detail breakup and an emission palette. Output is
// premultiplied (composited over the scene with One / OneMinusSrcAlpha blend).

struct VolUniforms {
    grid_res: u32,
    march_steps: u32,
    res_x: f32,
    res_y: f32,
    time: f32,
    absorption: f32,
    detail_scale: f32,
    detail_strength: f32,
    density_threshold: f32,
    volume_depth: f32,
    density_scale: f32,
    cam_yaw: f32,
    cam_pitch: f32,
    cam_distance: f32,
    cam_orbit_speed: f32,
    fov: f32,
    palette_hue: f32,
    emission_gain: f32,
    beat: f32,
    kick: f32,
    rms: f32,
    beat_phase: f32,
    dominant_chroma: f32,
    density_gain: f32,
}

@group(0) @binding(0) var<uniform> u: VolUniforms;
@group(0) @binding(1) var density_tex: texture_3d<f32>;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    // Fullscreen triangle (matches compute_raster_resolve.wgsl).
    var out: VertexOutput;
    let x = f32(i32(vi & 1u)) * 4.0 - 1.0;
    let y = f32(i32(vi >> 1u)) * 4.0 - 1.0;
    out.position = vec4f(x, y, 0.0, 1.0);
    out.uv = vec2f((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

// --- Self-contained 3D value noise / FBM (builtin shaders get no lib preamble) ---
fn hash13(p_in: vec3f) -> f32 {
    var p3 = fract(p_in * 0.1031);
    p3 += dot(p3, p3.zyx + 31.32);
    return fract((p3.x + p3.y) * p3.z);
}

fn vnoise(p: vec3f) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    let n000 = hash13(i + vec3f(0.0, 0.0, 0.0));
    let n100 = hash13(i + vec3f(1.0, 0.0, 0.0));
    let n010 = hash13(i + vec3f(0.0, 1.0, 0.0));
    let n110 = hash13(i + vec3f(1.0, 1.0, 0.0));
    let n001 = hash13(i + vec3f(0.0, 0.0, 1.0));
    let n101 = hash13(i + vec3f(1.0, 0.0, 1.0));
    let n011 = hash13(i + vec3f(0.0, 1.0, 1.0));
    let n111 = hash13(i + vec3f(1.0, 1.0, 1.0));
    let x00 = mix(n000, n100, w.x);
    let x10 = mix(n010, n110, w.x);
    let x01 = mix(n001, n101, w.x);
    let x11 = mix(n011, n111, w.x);
    return mix(mix(x00, x10, w.y), mix(x01, x11, w.y), w.z);
}

fn fbm(p_in: vec3f) -> f32 {
    var p = p_in;
    var v = 0.0;
    var a = 0.5;
    for (var i = 0; i < 3; i++) {
        v += a * vnoise(p);
        p *= 2.0;
        a *= 0.5;
    }
    return v;
}

fn palette(t: f32) -> vec3f {
    return 0.5 + 0.5 * cos(6.28318 * (t + vec3f(0.0, 0.33, 0.67)));
}

// Slab ray/box intersection against the unit cube [-1,1]^3. Returns (t_near, t_far).
fn hit_box(ro: vec3f, rd: vec3f) -> vec2f {
    let inv = 1.0 / rd;
    let t0 = (vec3f(-1.0) - ro) * inv;
    let t1 = (vec3f(1.0) - ro) * inv;
    let tmin = min(t0, t1);
    let tmax = max(t0, t1);
    let tn = max(max(tmin.x, tmin.y), tmin.z);
    let tf = min(min(tmax.x, tmax.y), tmax.z);
    return vec2f(tn, tf);
}

fn tex_load(c: vec3i, gi: i32) -> f32 {
    let cc = clamp(c, vec3i(0), vec3i(gi - 1));
    return textureLoad(density_tex, cc, 0).r;
}

// Manual trilinear sample (r32float is non-filterable without FLOAT32_FILTERABLE,
// so a hardware Linear sampler is not usable here).
fn sample_density(p: vec3f) -> f32 {
    let g = f32(u.grid_res);
    let vc = (p * 0.5 + 0.5) * g - 0.5;
    let b = floor(vc);
    let f = vc - b;
    let bi = vec3i(b);
    let gi = i32(u.grid_res);

    let c000 = tex_load(bi + vec3i(0, 0, 0), gi);
    let c100 = tex_load(bi + vec3i(1, 0, 0), gi);
    let c010 = tex_load(bi + vec3i(0, 1, 0), gi);
    let c110 = tex_load(bi + vec3i(1, 1, 0), gi);
    let c001 = tex_load(bi + vec3i(0, 0, 1), gi);
    let c101 = tex_load(bi + vec3i(1, 0, 1), gi);
    let c011 = tex_load(bi + vec3i(0, 1, 1), gi);
    let c111 = tex_load(bi + vec3i(1, 1, 1), gi);

    let x00 = mix(c000, c100, f.x);
    let x10 = mix(c010, c110, f.x);
    let x01 = mix(c001, c101, f.x);
    let x11 = mix(c011, c111, f.x);
    return mix(mix(x00, x10, f.y), mix(x01, x11, f.y), f.z);
}

fn envelope(p: vec3f) -> f32 {
    // Soft boundary: fade density toward the unit-cube faces (no hard box) with a
    // gentle center bias so a uniformly-filled field reads as a cloud, not a lit box.
    let q = abs(p);
    let edge = smoothstep(1.0, 0.75, max(q.x, max(q.y, q.z)));
    let center = mix(0.5, 1.0, smoothstep(1.2, 0.0, length(p)));
    return edge * center;
}

// Cheap nearest-voxel density for the self-shadow march (trilinear quality is
// unnecessary there and the extra texture loads would cost too much).
fn sample_density_nearest(p: vec3f) -> f32 {
    let vc = (p * 0.5 + 0.5) * f32(u.grid_res);
    return tex_load(vec3i(floor(vc)), i32(u.grid_res));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // Orbiting camera basis (inline, no matrix).
    let yaw = u.cam_yaw + u.time * u.cam_orbit_speed;
    let pitch = u.cam_pitch;
    let ro = vec3f(cos(yaw) * cos(pitch), sin(pitch), sin(yaw) * cos(pitch)) * u.cam_distance;
    let fwd = normalize(-ro);
    let right = normalize(cross(vec3f(0.0, 1.0, 0.0), fwd));
    let up = cross(fwd, right);

    let aspect = u.res_x / max(u.res_y, 1.0);
    // Screen coords: x in [-aspect, aspect], y in [-1, 1] with +y up.
    let sc = vec2f((in.uv.x - 0.5) * 2.0 * aspect, (0.5 - in.uv.y) * 2.0);
    let rd = normalize(sc.x * right + sc.y * up + u.fov * fwd);

    let bounds = hit_box(ro, rd);
    let t_enter = max(bounds.x, 0.0);
    let t_exit = bounds.y;
    if t_exit <= t_enter {
        return vec4f(0.0);
    }

    let steps = max(u.march_steps, 1u);
    let dt = (t_exit - t_enter) / f32(steps);
    var t = t_enter + dt * 0.5;

    let pulse = 0.8 + 0.2 * sin(u.beat_phase * 6.28318);
    let absorption = u.absorption * (1.0 + u.rms * 0.5);
    let emission_gain = u.emission_gain * (1.0 + u.kick * 1.5);

    var transmittance = 1.0;
    var accum = vec3f(0.0);

    // Fixed key light for self-shadowing (unit-cube space).
    let light_dir = normalize(vec3f(0.4, 0.9, 0.35));
    let ls = 0.16; // self-shadow march step

    for (var i = 0u; i < steps; i++) {
        let p = ro + rd * t;
        // `envelope` gives soft cloud boundaries instead of hard cube faces; applied
        // here (not in resolve) so the stored density texture stays raw for a future
        // Lattice 3D source.
        var d = sample_density(p) * envelope(p);
        if d > u.density_threshold {
            let detail = fbm(p * u.detail_scale + vec3f(0.0, u.time * 0.15, 0.0));
            d *= mix(1.0, detail, u.detail_strength);
            d *= pulse;

            // Self-shadow: accumulate optical depth toward the light so dense regions
            // shade themselves. Without this, uniform self-emission floods the cloud to
            // a flat color and hides all internal structure at any emission > 0.
            var shadow_od = 0.0;
            for (var j = 1; j <= 5; j++) {
                let lp = p + light_dir * (f32(j) * ls);
                shadow_od += max(sample_density_nearest(lp) * envelope(lp), 0.0);
            }
            let light = exp(-shadow_od * ls * absorption);
            let lit = 0.2 + 0.8 * light; // ambient floor so shadows keep some color

            let a = 1.0 - exp(-d * absorption * dt);
            let hue = u.palette_hue + u.dominant_chroma / 12.0 + d * 0.15;
            let col = palette(hue) * emission_gain * lit;
            accum += transmittance * a * col;
            transmittance *= (1.0 - a);
            if transmittance < 0.01 {
                break;
            }
        }
        t += dt;
    }

    let alpha = 1.0 - transmittance;
    // Reinhard tone map: roll off highlights so accumulated emission can never clip
    // to flat white, preserving palette color at any density. Applied to the
    // (premultiplied) color; alpha is the coverage from Beer-Lambert transmittance.
    let mapped = accum / (1.0 + accum);
    return vec4f(mapped, alpha); // premultiplied
}
