// Reliquary — a form that holds light, and the light streaming out of it.
//
// Every other image-emitter effect treats the source as one population: a wall of
// pixels that all behave alike. This one splits it in two and gives each half its
// own physics.
//
//   BODY    — the surface. Springs to its home position and stays put, so the
//             form reads as a solid object.
//   ESCAPING — light leaving that surface. Streams outward, fades, recycles, and
//             is brightened several-fold on the way.
//
// The split is what makes it an effect rather than a setting. There is continuous
// motion with no audio at all, and a structural separation on screen between a
// still core and a moving halo — neither of which Raster can do at any knob
// position.
//
// Structs, bindings, and helpers are in particle_lib.wgsl (auto-prepended).

fn unpack_rgba(packed: f32) -> vec4f {
    let bits = bitcast<u32>(packed);
    return vec4f(
        f32(bits & 0xFFu) / 255.0,
        f32((bits >> 8u) & 0xFFu) / 255.0,
        f32((bits >> 16u) & 0xFFu) / 255.0,
        f32((bits >> 24u) & 0xFFu) / 255.0,
    );
}

fn luminance(c: vec3f) -> f32 {
    return dot(c, vec3f(0.299, 0.587, 0.114));
}

const PI = 3.14159265;

// Alpha below this marks a particle as escaping light rather than surface.
//
// #1996's god-ray pass scatters shafts into the TRANSPARENT background, so they
// arrive at roughly 0.10–0.30 alpha while every surface texel is a flat 1.0. That
// is a clean separation with a wide margin, and it costs nothing to read —
// ParticleAux has all four lanes spoken for (xy home, z packed RGBA, w gradient)
// and could not have carried a dedicated flag.
const SHAFT_ALPHA = 0.75;

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles {
        return;
    }

    let home = aux[idx].home;
    let home_pos = home.xy;
    let home_color = unpack_rgba(home.z);

    // Padding beyond the sampled source: park it offscreen and clear it.
    if home_color.a < 0.01 {
        var dead = read_particle(idx);
        dead.pos_life = vec4f(99.0, 99.0, 0.0, 0.0);
        dead.color = vec4f(0.0);
        write_particle(idx, dead);
        return;
    }

    var p = read_particle(idx);

    // Initial emit: everything starts at its home position.
    if p.pos_life.w <= 0.0 {
        let slot = emit_claim();
        if slot < u.emit_count {
            let seed_base = u.seed + f32(idx) * 7.31;
            p.pos_life = vec4f(home_pos, 0.0, 1.0);
            p.vel_size = vec4f(0.0, 0.0, 0.0, u.initial_size);
            p.color = home_color;
            // flags.z is the stream phase, seeded per particle. Without the
            // offset every shaft particle would set out together and the flow
            // would read as one pulsing ring instead of a continuous stream.
            p.flags = vec4f(
                hash(seed_base + 2.0) * u.lifetime * 0.5,
                u.lifetime,
                hash(seed_base + 3.0),
                0.0,
            );
            write_particle(idx, p);
            mark_alive(idx);
        } else {
            write_particle(idx, p);
        }
        return;
    }

    let dt = u.delta_time;

    // param(0) is trail_decay, read by the feedback background pass.
    let stream_speed  = param(1u);  // how fast light travels outward (0–2)
    let stream_length = param(2u);  // how far it gets before recycling (0–0.6)
    let shaft_gain    = param(3u);  // brightness of escaping light (0–4)
    let shed          = param(4u);  // luminance above which surface sheds (0–1)
    let body_spring   = param(5u);  // how firmly the form holds (2–30)
    let surge         = param(6u);  // onset kick to stream speed (0–2)
    let bass_swell    = param(7u);  // radial breathing of the form (0–0.6)

    let lum = luminance(home_color.rgb);

    // Two ways to be escaping light, and both are the same idea seen from
    // different sources. A god ray is literally light in the empty space around
    // the model, so it comes in translucent. On a source with no rays — an
    // ordinary picture, or a model lit by the default key light — the brightest
    // surfaces are the ones that would be shedding, so they stream instead.
    // Without that second test the effect would collapse to a still image for
    // every source except a model with its interior light switched on.
    let is_escaping = home_color.a < SHAFT_ALPHA || lum > shed;

    // Radially out from the frame centre. #1996 defaults its light to the model's
    // own centre, which projects to the middle of the frame, so the ray a shaft
    // particle already sits on points this way. An off-centre light skews it,
    // which reads as the light leaning rather than as an error.
    let r = length(home_pos);
    let dir = select(vec2f(0.0, 1.0), home_pos / max(r, 1e-4), r > 1e-4);

    var pos: vec2f;
    var vel = p.vel_size.xy;
    var color: vec4f;
    var size: f32;

    if is_escaping {
        // Advance a phase rather than integrating a velocity. Light has to flow
        // CONTINUOUSLY, and a particle that merely accelerated outward would
        // leave the frame once and never come back without a respawn — the halo
        // would empty out after a second or two.
        let speed = stream_speed * (0.35 + u.rms * 1.4) * (1.0 + u.onset * surge);
        p.flags.z = fract(p.flags.z + speed * dt);
        let phase = p.flags.z;

        pos = home_pos + dir * phase * stream_length;
        vel = vec2f(0.0);

        // Zero at both ends of the run, so nothing pops into or out of existence
        // at the moment the phase wraps.
        let fade = sin(phase * PI);

        // The gain is the load-bearing part of this effect. Shafts arrive DIM —
        // alpha 0.27 at the aperture, falling to nothing — and finding #2008
        // measured that no blend mode rescues them: the sampler lays down about
        // one particle per pixel, so there is nothing for additive to accumulate.
        // Brightening the escaping particles directly is the only lever that
        // works, which is exactly why this belongs in a sim and not in a preset.
        color = vec4f(home_color.rgb * shaft_gain * fade, home_color.a);
        size = u.initial_size * (0.8 + fade * 0.6);
    } else {
        // The form holds. A slow radial swell on bass so it breathes without
        // smearing the picture the lighting drew.
        // `rest`, not `target` — the latter is a reserved WGSL keyword.
        let rest = home_pos + dir * u.bass * bass_swell * 0.25;
        pos = p.pos_life.xy;
        vel += (rest - pos) * body_spring * dt;
        vel *= 0.90;

        let prev = pos;
        pos += vel * dt;
        let coll = apply_obstacle_collision(pos, vel, prev);
        pos = coll.xy;
        vel = coll.zw;

        color = home_color;
        // Brighter surface sits fractionally larger, which keeps the lit side of
        // a form from dissolving into the gaps between its own particles.
        size = u.initial_size * (1.0 + lum * 0.8);
    }

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, size);
    p.color = color;
    p.flags.x += dt;
    if p.flags.x >= p.flags.y {
        p.flags.x = 0.0;
    }

    write_particle(idx, p);
    mark_alive(idx);
}
