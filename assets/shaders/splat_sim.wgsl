// Splat — audio-reactive 3D Gaussian-splat playback (#1800).
//
// Persistent particles: slot i ↔ splat i of the scene in `splat_static`
// (group 2 binding 1, uploaded once per scene; zeroed slots unpack opacity 0
// and park dead — the buffer's zero-fill IS the count mechanism). Nothing is
// emitted or dies; every frame recomputes screen state from the static home
// plus a world-space displacement offset — the only true state, persisted in
// vel_size.xyz even for culled splats.
//
// Camera: scalar orbit camera (u.cam_yaw/pitch/distance/focal, driven CPU-side
// by SplatDriver) using the volumetric ray-marcher basis convention, inverted:
// world → view → perspective divide → NDC in pos_life.xy, view depth in
// pos_life.z (free in the raster path), projected radius in vel_size.w.
//
// Audio: u.splat_explode (drop envelope) throws splats radially outward and a
// spring reforms the scene; onsets scatter a hash-picked subset; u.centroid
// (via u.splat_focal_depth) pulls a depth-of-field focal plane — defocused
// splats render larger and dimmer (energy-conserving circle of confusion),
// so the DoF is drawn by the splatter itself, no post pass.
//
// Blend "oit": color.a carries α · depth-weight · OIT_ALPHA_SCALE; the
// compute raster accumulates Σc·w and Σw and the resolve divides (mode 2).
//
// param(0) audio_reactivity  param(1) explode_amount  param(2) scatter_amount
// param(3) focus_strength    param(4) splat_scale     param(5) opacity_gain
// param(6) idle_breathe      param(7) exposure
// (slots 8–11 are CPU-side camera params — not visible to the sim)

struct SplatStatic {
    pos: vec3f,      // world position, scene normalized (offset 0)
    color: u32,      // pack4x8unorm(r, g, b, opacity)      (offset 12)
    scale_xy: u32,   // pack2x16float(scale_x, scale_y)     (offset 16)
    scale_z_e: u32,  // pack2x16float(scale_z, spare)       (offset 20)
    rot_xy: u32,     // pack2x16float(quat.x, quat.y)       (offset 24)
    rot_zw: u32,     // pack2x16float(quat.z, quat.w)       (offset 28)
}
@group(2) @binding(1) var<storage, read> splat_static: array<SplatStatic>;

// Integer hash (lowbias32) — the lib's fract-sin hash() degrades on GPU for
// idx-scaled arguments (#1856): a band of indices rolls near-constant values
// that pass any threshold every frame. All per-index randomness here uses
// exact u32 mixing (copied from cleave_sim).
fn uhash(x: u32) -> u32 {
    var h = x;
    h = h ^ (h >> 16u);
    h = h * 0x7feb352du;
    h = h ^ (h >> 15u);
    h = h * 0x846ca68bu;
    h = h ^ (h >> 16u);
    return h;
}

fn uhash_f(x: u32) -> f32 {
    return f32(uhash(x)) / 4294967296.0;
}

// Uniform direction on the unit sphere from one seed.
fn rand_dir3(seed: u32) -> vec3f {
    let z = uhash_f(seed) * 2.0 - 1.0;
    let phi = uhash_f(seed ^ 0x9e3779b9u) * 6.2831853;
    let r = sqrt(max(0.0, 1.0 - z * z));
    return vec3f(r * cos(phi), z, r * sin(phi));
}

// Fold factor for every accumulated weight: bounds the worst case (3M splats
// collapsing onto one pixel) inside the i32/4096 fixed-point headroom
// (3M × 0.125 = 375k < 524k). The OIT resolve divides it back out; only
// coverage needs the matching COVERAGE_GAIN compensation (resolve shader).
const OIT_ALPHA_SCALE: f32 = 0.125;
// Reform time constant ≈ 0.45 s (matches the drop envelope decay).
const RETURN_RATE: f32 = 2.2;
// World-space displacement clamp — keeps exploded splats recoverable.
const MAX_OFF: f32 = 3.0;
// Final-alpha floor below which a splat cannot contribute a fixed-point
// quantum — skip mark_alive so the raster never sees it.
const ALPHA_CULL: f32 = 1.0 / 512.0;

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles {
        return;
    }

    let s = splat_static[idx];
    let base = unpack4x8unorm(s.color); // rgb + opacity
    if base.a < 0.004 {
        // Zero-padded slot past the scene (or a fully transparent splat):
        // park dead, offscreen (image_scatter pattern).
        var dead: Particle;
        dead.pos_life = vec4f(99.0, 99.0, 0.0, 0.0);
        dead.vel_size = vec4f(0.0);
        dead.color = vec4f(0.0);
        dead.flags = vec4f(0.0, u.lifetime, 0.0, 0.0);
        write_particle(idx, dead);
        return;
    }

    let react = param(0u);
    let scale3 = vec3f(
        unpack2x16float(s.scale_xy),
        unpack2x16float(s.scale_z_e).x
    ) * max(param(4u), 0.05);

    // ---- world-space audio physics (state = vel_size_in[idx].xyz) ----
    let pin = read_particle(idx);
    var off = pin.vel_size.xyz;
    let dir = normalize(s.pos + vec3f(1e-5, 0.0, 0.0)); // radial from scene center

    // Spring home — held open while the drop envelope is hot, so the scene
    // hangs shattered for the phrase and re-coalesces as the envelope decays.
    let env = u.splat_explode * react;
    off *= exp(-RETURN_RATE * u.delta_time * (1.0 - 0.7 * clamp(env, 0.0, 1.0)));
    // Radial throw, per-splat speed variance so the shell has depth.
    off += dir * env * param(1u) * 2.5
        * (0.6 + 0.8 * uhash_f(idx * 0x9e3779b9u)) * u.delta_time;

    // Onset scatter: a hash-picked percentage kicks in a random direction.
    if u.onset > 0.35 {
        let ev = uhash(u.frame_index);
        if uhash_f(idx ^ ev) < param(2u) * react * 0.3 {
            off += rand_dir3(uhash(idx * 747796405u) ^ ev) * 0.05 * (0.5 + u.onset);
        }
    }
    off = clamp(off, vec3f(-MAX_OFF), vec3f(MAX_OFF));

    // Silence idle: non-integrated breathing (never accumulates into state).
    let breath_amp = param(6u) * (0.010 * u.rms + 0.008 * u.beat * react);
    let breath = dir * breath_amp
        * sin(u.time * 1.7 + uhash_f(idx ^ 0x85ebca6bu) * 6.2831853);
    let world = s.pos + off + breath;

    // ---- orbit camera (volumetric basis convention, inverted) ----
    let cy = u.cam_yaw;
    let cp = u.cam_pitch;
    let ro = vec3f(cos(cy) * cos(cp), sin(cp), sin(cy) * cos(cp))
        * max(u.cam_distance, 0.2);
    let fwd = normalize(-ro);
    let right = normalize(cross(vec3f(0.0, 1.0, 0.0), fwd));
    let up = cross(fwd, right);
    let rel = world - ro;
    let t = vec3f(dot(rel, right), dot(rel, up), dot(rel, fwd)); // t.z = view depth

    var age = pin.flags.x + u.delta_time;
    if age > u.lifetime {
        age = 0.0;
    }

    if t.z < 0.05 {
        // Behind the camera: keep the displacement state, invisible to the
        // raster (life 0, not marked alive).
        var back: Particle;
        back.pos_life = vec4f(2.0, 2.0, t.z, 0.0);
        back.vel_size = vec4f(off, 0.0);
        back.color = vec4f(0.0);
        back.flags = vec4f(age, u.lifetime, 0.0, 0.0);
        write_particle(idx, back);
        return;
    }

    let focal = max(u.cam_focal, 0.1);
    let asp = aspect();
    let ndc = vec2f(focal * t.x / (t.z * asp), focal * t.y / t.z);

    // ---- isotropic footprint (stage 2 replaces with the EWA conic) ----
    // World σ = mean 3DGS scale; projected σ_px = focal_px·σ/z. The raster's
    // gaussian weight exp(−4·(d/r)²) has σ_px = r_px/(2√2) → r_px = 2√2·σ_px.
    let sigma_w = (scale3.x + scale3.y + scale3.z) / 3.0;
    let focal_px = focal * u.resolution.y * 0.5;
    let sigma2_core = pow(focal_px * sigma_w / t.z, 2.0);
    // DoF circle of confusion from the centroid-driven focal plane.
    let coc = param(3u) * 6.0 * abs(t.z - u.splat_focal_depth) / max(t.z, 0.1);
    // AA low-pass (0.3 px²) + defocus blur, energy-conserving: the peak dims
    // by the variance ratio so blur never brightens.
    let blur = 0.3 + coc * coc;
    let sigma2 = sigma2_core + blur;
    var alpha = base.a * param(5u) * (sigma2_core + 0.001) / (sigma2 + 0.001);
    let r_px = min(2.8284271 * sqrt(sigma2), 8.0); // raster radius cap (3×3-tile bound)
    let r_ndc = r_px * 2.0 / u.resolution.y;       // raster: radius_px = vel_size.w·H/2

    // ---- OIT weight + obstacle carve + cull ----
    // Bounded near-favoring depth weight (scene spans [dist−1, dist+1]).
    let wz = clamp((u.cam_distance + 1.0 - t.z) * 0.5, 0.0, 1.0) * 0.75 + 0.25;
    // Crowd silhouettes punch holes through the captured scene: alpha-carve
    // at the projected position (a 2D clip-space bounce is meaningless for a
    // re-projected 3D point). obstacle_alpha() is 0 when no obstacle is armed.
    let carve = 1.0 - smoothstep(
        u.obstacle_threshold - 0.1,
        u.obstacle_threshold + 0.1,
        obstacle_alpha(ndc)
    );
    let a_out = alpha * wz * carve * OIT_ALPHA_SCALE;
    let off_screen = any(abs(ndc) > vec2f(1.0 + r_ndc * 2.0 + 0.05, 1.0 + r_ndc * 2.0 + 0.05));
    let culled = a_out < ALPHA_CULL || off_screen;

    var p: Particle;
    p.pos_life = vec4f(ndc, t.z, select(1.0, 0.0, culled));
    p.vel_size = vec4f(off, r_ndc); // state ALWAYS persisted, even culled
    p.color = vec4f(
        clamp(base.rgb * (0.5 + param(7u) * 1.5), vec3f(0.0), vec3f(1.0)),
        a_out
    );
    p.flags = vec4f(age, u.lifetime, 0.0, 0.0); // .zw: stage-2 packed conic
    write_particle(idx, p);
    if !culled {
        mark_alive(idx);
    }
}
