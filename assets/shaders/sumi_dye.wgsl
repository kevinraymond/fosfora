// Sumi — the coloured ink field (the rendered pass).
//   feedback() = own dye, previous frame
//   input0     = velocity, this frame (divergence-free)
// Advects the dye by the fresh velocity and injects colour on onsets: one splat per
// pitch class at its circle-of-fifths ring site, its brightness the class energy, plus
// a central drop tinted by the song's detected key. Runs at full resolution; the sim
// fields are half-res, so velocity is converted using the sim grid's texel size.

fn sumi_hsv2rgb(h: f32, s: f32, v: f32) -> vec3f {
    let k = vec3f(1.0, 0.6666667, 0.3333333);
    let p = abs(fract(vec3f(h) + k) * 6.0 - 3.0);
    return v * mix(vec3f(1.0), clamp(p - 1.0, vec3f(0.0), vec3f(1.0)), s);
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(prev_frame));       // dye (full res)
    let uv = frag_coord.xy / dims;
    let aspect = dims.x / dims.y;
    let sim_texel = 1.0 / vec2f(textureDimensions(input0_tex)); // velocity grid

    let flow_speed = 0.4 + param(0u) * 1.2;   // p0
    // Persistence, not accumulation: ink must survive long enough to be carried across the
    // frame (fill), while injection stays low so the steady state (≈ inject/(1−decay)) is
    // moderate, not a white wash.
    let dye_decay = mix(0.93, 0.995, param(4u)); // p4
    let splat_rad = param(5u) * 0.04 + 0.025;    // p5  (crisp drops, slight overlap)
    let inject = 0.05 + param(6u) * 0.35;        // p6
    let sat = 0.55 + param(8u) * 0.45;            // p8
    let bright = 0.6 + param(9u) * 0.9;           // p9

    let dt = clamp(u.delta_time, 0.0, 0.05) * 60.0 * flow_speed;
    let texel = 1.0 / dims;

    // Advect the dye by the velocity field, then dissipate. A small isotropic diffusion
    // (ink bleeding into still water) lets colour creep into the gaps between drops and
    // fill the frame without inflating the splats into a blurry wash.
    let vel = input0(uv).xy;
    let uv_back = uv - dt * vel * sim_texel;
    let blur = 0.25
        * (feedback(uv_back + vec2f(texel.x, 0.0)).rgb
            + feedback(uv_back - vec2f(texel.x, 0.0)).rgb
            + feedback(uv_back + vec2f(0.0, texel.y)).rgb
            + feedback(uv_back - vec2f(0.0, texel.y)).rgb);
    // The 0.12 weights the 4-neighbour mean of the SAME tap — spatial diffusion,
    // not a temporal weight, so it takes frame_diffuse rather than frame_decay
    // (#2350). It is still a per-frame rate: the deviation from the neighbour mean
    // retains (1 - 0.12) each frame, so the dye blurred to a different radius on
    // every frame rate. #1986 correctly ruled it out as a decay and left it.
    var col = mix(feedback(uv_back).rgb, blur, frame_diffuse(0.12)) * frame_decay(dye_decay);

    // coloured injection on a screen-filling circle-of-fifths ring (wide ellipse, matching
    // sumi_velocity so each colour is born where its velocity splat pushes)
    let center = vec2f(0.5, 0.5);
    let ring = vec2f(0.46, 0.38);
    for (var i = 0u; i < 12u; i = i + 1u) {
        let fifth = f32((i * 7u) % 12u);
        let ang = 6.28318 * fifth / 12.0;
        let site = center + ring * vec2f(cos(ang), sin(ang));
        let d = (uv - site) * vec2f(aspect, 1.0);
        let g = exp(-dot(d, d) / (splat_rad * splat_rad));
        let hue = fifth / 12.0;
        col += sumi_hsv2rgb(hue, sat, bright) * g * u.onset * chroma_val(i) * inject;
    }

    // central drop tinted by the detected key (dominant_chroma = pitch class / 11)
    let dc = (uv - center) * vec2f(aspect, 1.0);
    let gd = exp(-dot(dc, dc) / (splat_rad * splat_rad * 2.5));
    let key_hue = fosfora_key_hue(u.dominant_chroma, 0.0);
    col += sumi_hsv2rgb(key_hue, sat, bright) * gd * u.onset * inject * 0.6;

    col = min(col, vec3f(4.0)); // clamp to keep the feedback loop from blowing out
    return vec4f(col, 1.0);
}
