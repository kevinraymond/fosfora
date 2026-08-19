// Sumi — project + advect + forces, the one pass that owns the velocity field.
//   feedback() = own velocity, previous frame (V_prev)
//   input0     = pressure, this frame (solved from div(V_prev))
//   input1     = dye, previous frame (buoyancy source; a prev_input)
//
// Projected (divergence-free) field is Ṽ = V_prev − ∇P, evaluated on the fly by
// sampling both textures. We self-advect Ṽ (semi-Lagrangian), then add vorticity
// confinement, buoyancy, and onset velocity splats. Next frame's divergence pass
// reads this output and the cycle re-projects it, so the field stays incompressible.

fn samp_vel(uv: vec2f) -> vec2f { return feedback(uv).xy; }
fn samp_p(uv: vec2f) -> f32 { return input0(uv).x; }

// Projected velocity at uv: V_prev(uv) − grad(pressure)(uv).
fn proj_vel(uv: vec2f, texel: vec2f) -> vec2f {
    let pl = samp_p(uv - vec2f(texel.x, 0.0));
    let pr = samp_p(uv + vec2f(texel.x, 0.0));
    let pb = samp_p(uv - vec2f(0.0, texel.y));
    let pt = samp_p(uv + vec2f(0.0, texel.y));
    return samp_vel(uv) - 0.5 * vec2f(pr - pl, pt - pb);
}

// Scalar curl of V_prev at uv (∂vy/∂x − ∂vx/∂y). ∇P adds no curl, so V_prev is fine.
fn curl_at(uv: vec2f, texel: vec2f) -> f32 {
    let vl = samp_vel(uv - vec2f(texel.x, 0.0));
    let vr = samp_vel(uv + vec2f(texel.x, 0.0));
    let vb = samp_vel(uv - vec2f(0.0, texel.y));
    let vt = samp_vel(uv + vec2f(0.0, texel.y));
    return 0.5 * ((vr.y - vl.y) - (vt.x - vb.x));
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(prev_frame));
    let texel = 1.0 / dims;
    let uv = frag_coord.xy / dims;
    let aspect = dims.x / dims.y;

    let flow_speed = 0.4 + param(0u) * 1.2;   // p0
    let viscosity = param(1u);                // p1  velocity retention
    let vort_amt = param(2u) * 0.6;           // p2  vorticity confinement
    let buoy_amt = param(3u) * 0.9;           // p3  buoyancy
    let splat_rad = param(5u) * 0.06 + 0.035; // p5  splat radius (crisp = turbulent detail)
    let impulse = param(7u) * 3.0;            // p7  onset impulse

    let dt = clamp(u.delta_time, 0.0, 0.05) * 60.0 * flow_speed;

    // 1) self-advect the projected field
    let v_here = proj_vel(uv, texel);
    let uv_back = uv - dt * v_here * texel;
    var vel = proj_vel(uv_back, texel);

    // 2) vorticity confinement — restores the swirl the grid dissipates, scaled by flux
    let w = curl_at(uv, texel);
    let wl = abs(curl_at(uv - vec2f(texel.x, 0.0), texel));
    let wr = abs(curl_at(uv + vec2f(texel.x, 0.0), texel));
    let wb = abs(curl_at(uv - vec2f(0.0, texel.y), texel));
    let wt = abs(curl_at(uv + vec2f(0.0, texel.y), texel));
    let eta = 0.5 * vec2f(wr - wl, wt - wb);
    let n = eta / (length(eta) + 1e-5);
    vel += dt * vort_amt * (0.3 + u.flux) * w * vec2f(n.y, -n.x);

    // 3) buoyancy — a gentle upward drift where there is ink (−y is up on screen). The
    // bass response is floored AND capped: a loud passage must not spike it into a jet that
    // sends the lower ring colours surging up over the upper ones (the bottom row would
    // "take over" the top). It stays a drift at every level, not a fountain.
    let dye_lum = dot(input1(uv).rgb, vec3f(0.299, 0.587, 0.114));
    let buoy_gate = 0.25 + 0.35 * u.bass; // 0.25..0.60, never spikes on loud bass
    vel += dt * vec2f(0.0, -1.0) * buoy_amt * buoy_gate * dye_lum;

    // 4) onset velocity splats on a screen-filling circle-of-fifths ring, each pushing
    // outward, + a wide beat pulse that expands the whole cloud toward the edges. The
    // ring is a wide ellipse (not aspect-corrected to a circle) so on a 16:9 window the
    // drops are born across the full frame rather than in a narrow central band.
    if (u.onset > 0.01 || u.beat > 0.01) {
        let center = vec2f(0.5, 0.5);
        let ring = vec2f(0.46, 0.38);
        for (var i = 0u; i < 12u; i = i + 1u) {
            let fifth = f32((i * 7u) % 12u);
            let ang = 6.28318 * fifth / 12.0;
            let site = center + ring * vec2f(cos(ang), sin(ang));
            let d = (uv - site) * vec2f(aspect, 1.0);
            let g = exp(-dot(d, d) / (splat_rad * splat_rad));
            let dir = d / (length(d) + 1e-4);
            vel += dir * g * u.onset * chroma_val(i) * impulse;
        }
        let dc = (uv - center) * vec2f(aspect, 1.0);
        let gd = exp(-dot(dc, dc) / 0.35);
        vel += (dc / (length(dc) + 1e-4)) * gd * u.beat * impulse * 0.7;
    }

    // 5) viscosity damping + magnitude clamp (feedback-loop safety)
    vel *= frame_decay(mix(0.985, 0.999, viscosity));
    let sp = length(vel);
    if (sp > 6.0) { vel *= 6.0 / sp; }

    return vec4f(vel, 0.0, 1.0);
}
