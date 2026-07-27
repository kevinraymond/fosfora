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
// by SplatDriver): world → view → perspective divide → NDC in pos_life.xy, view
// depth in pos_life.z (free in the raster path), projected radius in vel_size.w.
// This deliberately does NOT share the volumetric ray-marcher's basis, which
// negates `right` and so renders mirrored — harmless for a procedural volume,
// wrong for a capture of something real. See the basis construction below.
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
    pos: vec3f,        // world position, scene normalized  (offset 0)
    color: u32,        // pack4x8unorm(r, g, b, opacity)    (offset 12)
    rot_a: u32,        // pack2x16float(qx, qy)             (offset 16)
    rot_b: u32,        // pack2x16float(qz, qw)             (offset 20)
    log_scale: u32,    // pack2x16float(ln σx, ln σy)       (offset 24)
    log_scale_z: u32,  // pack2x16float(ln σz, 0)           (offset 28)
}
@group(2) @binding(1) var<storage, read> splat_static: array<SplatStatic>;

// View-dependent spherical harmonics, bands 1–3 (#1862). 45 f16 coefficients
// per splat (15 bands × RGB, channel-major with a 15-wide stride at every
// degree) packed into 24 u32 words; the last 3 halves are padding.
//
// RECORD 0 IS A HEADER, NOT A SPLAT — splat i lives at index i+1. Words 0–8
// hold the inverse scene rotation as a row-major 3×3 of raw f32 bits: SH lobes
// are defined in the source frame, but the loader rotates geometry into the
// render frame by the .pfx `rotation_degrees`, so the view direction is rotated
// BACK through this before evaluation (rotating the coefficients themselves
// would need Wigner-D matrices for bands 2–3).
//
// DC-only scenes bind a single zeroed dummy record and set u.splat_sh_degree
// to 0, which skips every load below.
struct SplatSh {
    data: array<u32, 24>,
}
@group(2) @binding(2) var<storage, read> splat_sh: array<SplatSh>;

// Coefficient `slot` (0..44) of SH record `rec`, unpacked from its f16 half.
fn sh_coeff(rec: u32, slot: u32) -> f32 {
    let pair = unpack2x16float(splat_sh[rec].data[slot >> 1u]);
    return select(pair.x, pair.y, (slot & 1u) == 1u);
}

// Real SH basis of the 3DGS convention — identical constants and sign pattern
// to INRIA's reference rasterizer (computeColorFromSH) and gsplat/SuperSplat's
// evalSH, so a capture shades the same here as in the viewer it was authored
// in. `d` is the SOURCE-frame direction from camera to splat, normalized.
// Returns the view-dependent delta only; the DC term is already in base.rgb.
fn splat_sh_color(rec: u32, degree: u32, d: vec3f) -> vec3f {
    let x = d.x;
    let y = d.y;
    let z = d.z;
    var basis: array<f32, 15>;
    basis[0] = -0.4886025119 * y;
    basis[1] = 0.4886025119 * z;
    basis[2] = -0.4886025119 * x;
    var n = 3u;
    if degree >= 2u {
        let xx = x * x;
        let yy = y * y;
        let zz = z * z;
        basis[3] = 1.0925484306 * (x * y);
        basis[4] = -1.0925484306 * (y * z);
        basis[5] = 0.3153915653 * (2.0 * zz - xx - yy);
        basis[6] = -1.0925484306 * (x * z);
        basis[7] = 0.5462742153 * (xx - yy);
        n = 8u;
        if degree >= 3u {
            basis[8] = -0.5900435899 * y * (3.0 * xx - yy);
            basis[9] = 2.8906114426 * (x * y) * z;
            basis[10] = -0.4570457995 * y * (4.0 * zz - xx - yy);
            basis[11] = 0.3731763326 * z * (2.0 * zz - 3.0 * xx - 3.0 * yy);
            basis[12] = -0.4570457995 * x * (4.0 * zz - xx - yy);
            basis[13] = 1.4453057213 * z * (xx - yy);
            basis[14] = -0.5900435899 * x * (xx - 3.0 * yy);
            n = 15u;
        }
    }
    var rgb = vec3f(0.0);
    for (var k = 0u; k < n; k = k + 1u) {
        rgb += basis[k] * vec3f(
            sh_coeff(rec, k),
            sh_coeff(rec, 15u + k),
            sh_coeff(rec, 30u + k),
        );
    }
    return rgb;
}

// Build the 3D covariance Σ3 = M·Mᵀ, M = R(q)·diag(σ), in f32 from the stored
// quaternion and LOG scales. Returned as (diagonal, off-diagonal) in the packing
// sym3_mul expects: d = (Σxx, Σyy, Σzz), o = (Σxy, Σxz, Σyz).
//
// This used to be precomputed CPU-side and stored as six f16 halves ×1024. That
// format cannot hold a real capture: σ² spans ~15 decades against f16's ~12, so
// the thin axis of most surfels flushed to zero and `det_core` below — a
// cancelling difference — came out as much as 187× too large on near-rank-1
// splats, painting them as bright shimmering needles. σ in log space costs 3
// exp() per splat per frame and is exact. Mirrors gsplat's computeCovariance.
fn build_sigma3(
    s: SplatStatic,
    roundness: f32,
    d_out: ptr<function, vec3f>,
    o_out: ptr<function, vec3f>,
) {
    let qa = unpack2x16float(s.rot_a);
    let qb = unpack2x16float(s.rot_b);
    let q = normalize(vec4f(qa.x, qa.y, qb.x, qb.y)); // xyzw
    let ls = unpack2x16float(s.log_scale);
    var lsc = vec3f(ls.x, ls.y, unpack2x16float(s.log_scale_z).x);
    // Shard → sphere morph. A trained splat is a flat sliver — 60% of this
    // capture are needles, 40% flakes, 0.1% round — which is why an exploded
    // scene reads as fur. Blending the three log-scales toward their mean pulls
    // each Gaussian toward a sphere; doing it in LOG space makes it a geometric
    // mean, so the ellipsoid's VOLUME is preserved and the scene neither swells
    // nor thins as it rounds off. Free because the scales are stored as logs.
    lsc = mix(lsc, vec3f((lsc.x + lsc.y + lsc.z) / 3.0), clamp(roundness, 0.0, 1.0));
    let sc = exp(lsc);

    // Rotation matrix columns, each already scaled — M = R·diag(σ).
    let x = q.x; let y = q.y; let z = q.z; let w = q.w;
    let c0 = vec3f(1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w)) * sc.x;
    let c1 = vec3f(2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w)) * sc.y;
    let c2 = vec3f(2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y)) * sc.z;

    // Σ3 = M·Mᵀ = Σ_k c_k ⊗ c_k (columns of M are the scaled rotation axes).
    *d_out = c0 * c0 + c1 * c1 + c2 * c2;
    *o_out = vec3f(
        c0.x * c0.y + c1.x * c1.y + c2.x * c2.y, // Σxy
        c0.x * c0.z + c1.x * c1.z + c2.x * c2.z, // Σxz
        c0.y * c0.z + c1.y * c1.z + c2.y * c2.z, // Σyz
    );
}

// Symmetric 3×3 (6 unique entries) times a vector.
fn sym3_mul(d: vec3f, o: vec3f, v: vec3f) -> vec3f {
    // d = (xx, yy, zz), o = (xy, xz, yz)
    return vec3f(
        d.x * v.x + o.x * v.y + o.y * v.z,
        o.x * v.x + d.y * v.y + o.z * v.z,
        o.y * v.x + o.z * v.y + d.z * v.z,
    );
}

// All per-index randomness here uses the lib's uhash/uhash_f (integer mixing).
// fract-sin hash() bands idx-scaled arguments onto near-constant values that
// pass any threshold every frame.

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
// OIT-only opacity accumulation boost. Weighted-average OIT must lean on the
// splats' own weight to build a solid surface, and real captures are very
// low-opacity (median sigmoid ≈ 0.03); the fallback was tuned around an
// opacity_gain of 4.0. The sorted path now owns the shared `opacity_gain`
// default at 1.0 (raw, SuperSplat-faithful), so the OIT branch reintroduces the
// 4× accumulation here — one .pfx default serves both paths without regressing
// the fallback. Applied only when !splat_sorted.
const OIT_OPACITY_BOOST: f32 = 4.0;
// Reform time constant ≈ 0.45 s (matches the drop envelope decay).
const RETURN_RATE: f32 = 2.2;
// World-space displacement clamp — keeps exploded splats recoverable.
const MAX_OFF: f32 = 3.0;
// Final-alpha floor below which a splat cannot contribute a fixed-point
// quantum — skip mark_alive so the raster never sees it.
const ALPHA_CULL: f32 = 1.0 / 512.0;
// Standard 3DGS antialiasing dilation (Kerbl et al.): a fixed px² added to the
// 2D covariance diagonal so every splat — including the degenerate thin
// needles/surfels typical of real captures (anisotropy ~1000:1, sub-pixel
// minor axis) — covers at least ~1px and merges into a surface instead of
// aliasing into speckle. 0.3 is the canonical value; do NOT drop it, it is
// load-bearing for surface smoothness, not fog.
const AA_FLOOR: f32 = 0.3;
// Front-favouring OIT depth weight. Since the resolve averages Σc·w/Σw with no
// sorting, a near-bias lets the nearest surface lead the colour → less
// front+back grey blend. FLOORED (WZ_FLOOR) so far splats still register in the
// framebuffer and contribute coverage/opacity — the weight only reshades the
// average, it must never gate visibility (that is the intrinsic-alpha cull
// below). K≈4 over the [dist−1, dist+1] span gives a ~6× front:back lean; too
// steep risks nearest-splat flicker as the camera orbits.
const OIT_DEPTH_K: f32 = 4.0;
const WZ_FLOOR: f32 = 0.15;

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

    // ---- orbit camera ----
    let cy = u.cam_yaw;
    let cp = u.cam_pitch;
    let ro = vec3f(cos(cy) * cos(cp), sin(cp), sin(cy) * cos(cp))
        * max(u.cam_distance, 0.2);
    let fwd = normalize(-ro);
    // right = fwd × worldUp. The ray-marchers this camera was modelled on use
    // cross(worldUp, fwd) — the NEGATION — which mirrors the image horizontally.
    // On a procedural volume that is invisible (there is no handedness to get
    // wrong), so it went unnoticed there and was inherited here, where it made
    // a photogrammetric capture render as its own mirror image. Sanity check:
    // fwd = (0,0,−1), worldUp = +Y must give right = +X.
    // `up` must flip with it (cross(fwd, right) was cancelling the same sign
    // error, which is why the old basis was mirrored yet upright).
    let right = normalize(cross(fwd, vec3f(0.0, 1.0, 0.0)));
    let up = cross(right, fwd);
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

    // ---- anisotropic EWA footprint (3DGS): Σ2 = (J·W)·Σ3·(J·W)ᵀ ----
    // Σ3 comes precomputed from the upload; the J·W rows are the world-space
    // gradients of the two screen-pixel axes (aspect-symmetric because size
    // is height-relative; the y row is negated for the raster's y-flip).
    // Roundness = the manual slider PLUS the drop envelope, so the two use cases
    // compose instead of excluding each other: at rest the slider alone holds
    // (0 = the capture's true shard shape, higher = a static "galaxy" look), and
    // a drop rounds the scene off on top of it and decays back. `env` already
    // carries audio_reactivity, so setting that to 0 disables the automatic half.
    let round_amt = clamp(u.splat_roundness + env, 0.0, 1.0);
    var cov_d: vec3f;
    var cov_o: vec3f;
    build_sigma3(s, round_amt, &cov_d, &cov_o); // Σxx,Σyy,Σzz / Σxy,Σxz,Σyz — f32
    let focal_px = focal * u.resolution.y * 0.5;
    let iz = 1.0 / t.z;
    let jw0 = (right - fwd * (t.x * iz)) * (focal_px * iz);
    let jw1 = (fwd * (t.y * iz) - up) * (focal_px * iz);
    // splat_scale scales σ linearly → covariance by s².
    let ps4 = max(param(4u), 0.05);
    let s2 = ps4 * ps4;
    let sv0 = sym3_mul(cov_d, cov_o, jw0);
    var caa = dot(jw0, sv0) * s2; // Σ2 in px²
    var cbb = dot(jw1, sv0) * s2;
    var ccc = dot(jw1, sym3_mul(cov_d, cov_o, jw1)) * s2;
    // det of a projected surfel is legitimately ~0 (that is what "flat" means),
    // so this floors at 0 rather than clamping up: the alpha factor below must
    // be allowed to go to zero, which is what kills edge-on needles.
    let det_core = max(caa * ccc - cbb * cbb, 0.0);

    // DoF circle of confusion from the centroid-driven focal plane + AA
    // low-pass, both isotropic covariance adds; energy-conserving via the
    // √det ratio (peak of a 2D gaussian ∝ 1/√det), so blur never brightens.
    // The VISUAL blur is capped: beyond it, heavy defocus keeps dimming
    // (energy comp uses the uncapped determinant) while the footprint stays
    // bounded — far-defocus reads as fade at a fraction of the raster cost.
    let coc = param(3u) * 6.0 * abs(t.z - u.splat_focal_depth) / max(t.z, 0.1);
    let blur = AA_FLOOR + coc * coc;
    let det_full = max((caa + blur) * (ccc + blur) - cbb * cbb, 1e-8);
    let blur_vis = min(blur, 3.0);
    caa += blur_vis;
    ccc += blur_vis;
    var det = max(caa * ccc - cbb * cbb, 1e-8);
    var alpha = base.a * param(5u) * sqrt(det_core / det_full);

    // Footprint radius from the major eigenvalue. The fragment cuts at q = 8, so
    // sqrt(8·λmax) is the exact extent — the old sqrt(12·λmax) drew a quad 22%
    // wider in each axis (~1.5× the fragment area) for nothing.
    let lmax = 0.5 * (caa + ccc) + sqrt(max(0.25 * (caa - ccc) * (caa - ccc) + cbb * cbb, 0.0));
    var r_px = sqrt(8.0 * lmax);
    // OIT keeps its 8px 3×3-tile scatter bound (load-bearing for compute_raster)
    // and pays for it with a uniform shrink of the covariance. The SORTED path
    // does NOT touch the covariance: gsplat bounds the QUAD per axis and leaves
    // the conic exact, so an oversized splat gets clipped instead of shrunk —
    // see splat_render.wgsl. Shrinking it here is what let huge far-field splats
    // wash a scene capture to white.
    if u.splat_sorted < 0.5 && r_px > 8.0 {
        let shrink = (8.0 / r_px) * (8.0 / r_px);
        caa *= shrink;
        cbb *= shrink;
        ccc *= shrink;
        r_px = 8.0;
    }
    // Keep the conic f16-safe: a thin-needle conic can overflow f16 and
    // NaN-poison the sorted blend (the old black-square artifact). No-op for
    // normal splats — the AA floor already puts both diagonals ≥ 0.3.
    caa = max(caa, 0.05);
    ccc = max(ccc, 0.05);
    det = max(caa * ccc - cbb * cbb, 1e-8);

    // ---- flags.zw payload: quad axes (sorted) OR the conic (OIT) ----
    // The two consumers are mutually exclusive — system.rs gates use_compute_raster
    // off whenever the sorted path is active — so the same two words carry whichever
    // format that path needs.
    //
    // The sorted renderer gets the QUAD AXES, computed here in f32 where Σ2 is
    // exact. It must NOT be handed the conic and left to invert it: the conic is
    // f16-packed, and inverting a near-singular 2×2 from f16 inputs is the same
    // catastrophic cancellation that the covariance format change fixed. Measured
    // on trooper.ply at cam_distance 0.6, that inversion produced a NEGATIVE
    // determinant for 197 splats — which clamps, explodes Σ2, and turns ordinary
    // blobs into 280 hard-edged screen-crossing slivers.
    var pack_z: u32;
    var pack_w: u32;
    if u.splat_sorted > 0.5 {
        // gsplat initCornerCov: eigen-decompose Σ2, clamp each half-extent
        // independently at vmin, and orient the quad along the eigenvectors.
        let mid = 0.5 * (caa + ccc);
        let rad = length(vec2f(0.5 * (caa - ccc), cbb));
        let lam1 = mid + rad;
        let lam2 = max(mid - rad, 0.1); // gsplat's minor-eigenvalue floor
        let cap = min(1024.0, min(u.resolution.x, u.resolution.y));
        let e1 = 2.0 * min(sqrt(2.0 * lam1), cap); // half-extent, q = 8 at the edge
        let e2 = 2.0 * min(sqrt(2.0 * lam2), cap);
        // Major-axis direction. Degenerates to (0,0) for a perfectly axis-aligned
        // splat (cbb == 0, lam1 == caa), which normalize() would turn into NaN.
        let ev = vec2f(cbb, lam1 - caa);
        let evl = length(ev);
        let axis1 = select(vec2f(1.0, 0.0), ev / evl, evl > 1e-9);
        pack_z = pack2x16float(e1 * axis1);
        pack_w = pack2x16float(e2 * vec2f(axis1.y, -axis1.x));
    } else {
        // Conic = inverse covariance for the compute raster's anisotropic branch.
        let inv_det = 1.0 / det;
        pack_z = pack2x16float(vec2f(ccc * inv_det, caa * inv_det));
        pack_w = pack2x16float(vec2f(-cbb * inv_det, 0.0));
    }

    // Bounding radius for the frustum cull and the OIT raster's scatter extent.
    // Clamped to gsplat's vmin on the sorted path so one enormous splat cannot
    // defeat the offscreen test and drag the whole scene through the sort; the
    // COVARIANCE is untouched, only this bound.
    let vmin = min(1024.0, min(u.resolution.x, u.resolution.y));
    let r_ndc = min(r_px, vmin) * 2.0 / u.resolution.y; // raster: radius_px = vel_size.w·H/2

    // ---- OIT weight + obstacle carve + cull ----
    // Front-favouring depth weight (scene spans [dist−1, dist+1]), floored so
    // the far half of the figure still registers coverage instead of averaging
    // toward the background. Near splats lead the OIT colour average.
    let dnorm = clamp((t.z - (u.cam_distance - 1.0)) * 0.5, 0.0, 1.0);
    let wz = WZ_FLOOR + (1.0 - WZ_FLOOR) * exp(-OIT_DEPTH_K * dnorm);
    // Crowd silhouettes punch holes through the captured scene: alpha-carve
    // at the projected position (a 2D clip-space bounce is meaningless for a
    // re-projected 3D point). obstacle_alpha() is 0 when no obstacle is armed.
    let carve = 1.0 - smoothstep(
        u.obstacle_threshold - 0.1,
        u.obstacle_threshold + 0.1,
        obstacle_alpha(ndc)
    );
    // Intrinsic (depth-INDEPENDENT) visibility gates the cull, so the depth
    // weight only reshades the average and can never delete the far half of
    // the figure — that regression came from culling on the weighted a_out.
    // OIT accumulates the 4×-boosted alpha; sorted composites raw opacity.
    let alpha_oit = alpha * OIT_OPACITY_BOOST;
    let vis = alpha_oit * carve * OIT_ALPHA_SCALE;
    let a_out = vis * wz;
    let off_screen = any(abs(ndc) > vec2f(1.0 + r_ndc * 2.0 + 0.05, 1.0 + r_ndc * 2.0 + 0.05));
    // Sorted path composites raw intrinsic alpha, so it must cull on that (not the
    // boosted, ×0.125-folded `vis` the OIT accumulation uses).
    let cull_alpha = select(vis, alpha * carve, u.splat_sorted > 0.5);
    let culled = cull_alpha < ALPHA_CULL || off_screen;

    var p: Particle;
    p.pos_life = vec4f(ndc, t.z, select(1.0, 0.0, culled));
    p.vel_size = vec4f(off, r_ndc); // state ALWAYS persisted, even culled
    // Sorted path composites front-to-back with hardware alpha-over, so it wants
    // the RAW intrinsic alpha (opacity·falloff·carve) — no OIT depth-weight wz
    // and no OIT_ALPHA_SCALE fold. The OIT path keeps a_out.
    let a_sorted = clamp(alpha * carve, 0.0, 1.0);
    // View-dependent colour: rotate the camera→splat direction back into the
    // source frame (header record 0) and add the SH bands to the DC term.
    // NOTE the DC term arrived through pack4x8unorm, so it is already clamped
    // to [0,1] — a capture whose DC exceeds 1 has lost the headroom a negative
    // view term would have used. Pre-existing to the 8-bit colour packing, not
    // introduced by SH.
    var rgb = base.rgb;
    let sh_degree = u32(u.splat_sh_degree);
    if sh_degree > 0u {
        let dw = normalize(world - ro);
        let r0 = vec3f(bitcast<f32>(splat_sh[0].data[0]),
                       bitcast<f32>(splat_sh[0].data[1]),
                       bitcast<f32>(splat_sh[0].data[2]));
        let r1 = vec3f(bitcast<f32>(splat_sh[0].data[3]),
                       bitcast<f32>(splat_sh[0].data[4]),
                       bitcast<f32>(splat_sh[0].data[5]));
        let r2 = vec3f(bitcast<f32>(splat_sh[0].data[6]),
                       bitcast<f32>(splat_sh[0].data[7]),
                       bitcast<f32>(splat_sh[0].data[8]));
        let d = vec3f(dot(r0, dw), dot(r1, dw), dot(r2, dw));
        rgb = max(rgb + splat_sh_color(idx + 1u, sh_degree, d), vec3f(0.0));
    }
    p.color = vec4f(
        clamp(rgb * (0.5 + param(7u) * 1.5), vec3f(0.0), vec3f(1.0)),
        select(a_out, a_sorted, u.splat_sorted > 0.5)
    );
    // .zw = quad axes (sorted) or the screen conic (OIT raster) — see above.
    p.flags = vec4f(age, u.lifetime, bitcast<f32>(pack_z), bitcast<f32>(pack_w));
    write_particle(idx, p);
    if !culled {
        mark_alive(idx);
    }
}
