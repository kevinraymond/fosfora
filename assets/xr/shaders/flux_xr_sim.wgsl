// Flux, world-space variant for the Quest build (hidden preset
// assets/xr/effects/flux_xr_world.pfx). The same smoke as flux_sim.wgsl, but
// in meters around the wearer, drawn by the world-space sprite path
// (ParticleSystem::render_world, builtin/particle_render_world.wgsl) once per
// eye. Structs, bindings and helpers are in particle_lib.wgsl (auto-prepended).
//
// World layout (particle_lib.wgsl, "World layout"):
//   pos_life = xyz position in meters relative to the effect anchor
//              (+Y up, -Z forward), w life (1 alive, 0 dead)
//   vel_size = xyz velocity in m/s, w sprite radius in meters
//   color    = rgb straight color (the 2D sim's formulas), a opacity;
//              render_world multiplies rgb by a (additive blend), so the eye
//              target receives premultiplied color and passthrough shows
//              through where a is low
//   flags    = x age, y lifetime, z initial radius in meters (pos_life.z in
//              the 2D sim), w base opacity (compounded into color.a there)
//
// Volume: the cube within +-emitter.radius meters of the anchor (the preset's
// `emitter.radius`, read as u.emitter_radius). Particles spawn uniformly
// inside it; one that leaves it respawns at a new random point inside (the 2D
// sim wraps at the screen edge instead), and opacity fades out over the outer
// 30 % of the half extent so the bounds never read as a hard edge in a room.
// The preset's 1.5 m half extent puts the floor (1 m below the anchor, which
// sits at 1 m) inside the volume, so particles can settle on it and on
// tables instead of respawning at the boundary.
//
// Settle drift: a steady downward speed added at integration, not to the
// velocity, so the flow keeps its swirl and the collide step (which reflects
// the velocity) lets a particle rest on a table or the floor and slide with
// the flow's tangential part. The S7 test sim settled the same way; an
// acceleration did not read, the flow blend damped it.
//
// Per-frame XR inputs ride the aux buffer, which Flux does not otherwise use.
// The XR app writes them with ParticleSystem::update_aux_in_place, all in
// anchor-relative meters (layout mirrors ObstacleSet in
// crates/fosfora-xr/src/particles3d.rs, one vec4 per aux entry):
//   aux[0]          head position xyz; w = near-fade radius (0 = off)
//   aux[1]          sphere count (u32 bits), box count (u32 bits),
//                   restitution, margin
//   aux[2]          x = occluder shrink (unused here), y = hand kick (m/s),
//                   z = settle drift (m/s, downward; 0 = none)
//   aux[3..67]      spheres: xyz center, w radius (hand joints)
//   aux[67..99]     box centers (xyz)
//   aux[99..131]    box rotations, quaternion box -> world (x, y, z, w)
//   aux[131..163]   box half extents (xyz)
// All zero (nothing written yet, or a desktop test) means no obstacles and
// no near fade.

const XR_AUX_HEAD: u32 = 0u;
const XR_AUX_HEADER: u32 = 1u;
const XR_MAX_SPHERES: u32 = 64u;
const XR_MAX_BOXES: u32 = 32u;
const XR_AUX_SPHERES: u32 = 3u;
const XR_AUX_BOX_CENTER: u32 = 67u;   // XR_AUX_SPHERES + XR_MAX_SPHERES
const XR_AUX_BOX_ROT: u32 = 99u;      // + XR_MAX_BOXES
const XR_AUX_BOX_HALF: u32 = 131u;    // + XR_MAX_BOXES

// Fraction of the half extent over which opacity fades out toward the bounds.
const XR_EDGE_FADE: f32 = 0.3;

// ---- random -------------------------------------------------------------------

// Three uniform [0, 1) values per (particle, salt), from the integer hash. The
// 2D sim's fract-sin `rand_vec2` bands at large seeds (see its note in
// particle_lib.wgsl); in 3D the same defect lines the components up and
// spawns collapse onto a diagonal.
fn xr_rand3(idx: u32, salt: u32) -> vec3f {
    let s = uhash(idx ^ uhash(bitcast<u32>(u.seed) + salt));
    return vec3f(uhash_f(s), uhash_f(s ^ 0x9e3779b9u), uhash_f(s ^ 0x85ebca6bu));
}

// ---- obstacles (the S7 block and collide() from particles3d_sim.wgsl) ----------

fn xr_quat_rotate(q: vec4f, v: vec3f) -> vec3f {
    return v + 2.0 * cross(q.xyz, cross(q.xyz, v) + q.w * v);
}

fn xr_quat_conj(q: vec4f) -> vec4f {
    return vec4f(-q.xyz, q.w);
}

// sign() that never returns 0, so a particle exactly on a box's mid-plane
// still picks a side.
fn xr_side(x: f32) -> f32 {
    return select(-1.0, 1.0, x >= 0.0);
}

// Push the particle out of every obstacle it is inside and reflect the inward
// part of its velocity. One pass per frame is enough at these speeds; a
// particle deep inside a box (spawned there) exits through the nearest face.
fn xr_collide(pos: ptr<function, vec3f>, vel: ptr<function, vec3f>) {
    let header = aux[XR_AUX_HEADER].home;
    let sphere_count = min(bitcast<u32>(header.x), XR_MAX_SPHERES);
    let box_count = min(bitcast<u32>(header.y), XR_MAX_BOXES);
    let restitution = header.z;
    let margin = header.w;
    let sphere_kick = aux[XR_AUX_HEADER + 1u].home.y;
    for (var k = 0u; k < sphere_count; k++) {
        let s = aux[XR_AUX_SPHERES + k].home;
        let d = *pos - s.xyz;
        let r = s.w + margin;
        let dist2 = dot(d, d);
        if dist2 < r * r {
            let dist = max(sqrt(dist2), 1e-4);
            let n = d / dist;
            *pos = s.xyz + n * r;
            let vn = dot(*vel, n);
            if vn < 0.0 {
                *vel -= (1.0 + restitution) * vn * n;
            }
            // Bring the outward speed up to the kick, never beyond it: a
            // particle that stays inside the pad must not accumulate speed.
            let outward = dot(*vel, n);
            *vel += n * max(sphere_kick - outward, 0.0);
        }
    }
    for (var k = 0u; k < box_count; k++) {
        let c = aux[XR_AUX_BOX_CENTER + k].home.xyz;
        let q = aux[XR_AUX_BOX_ROT + k].home;
        let h = aux[XR_AUX_BOX_HALF + k].home.xyz + vec3f(margin);
        let l = xr_quat_rotate(xr_quat_conj(q), *pos - c);
        let pen = h - abs(l);
        if pen.x > 0.0 && pen.y > 0.0 && pen.z > 0.0 {
            var n_local = vec3f(0.0);
            var l2 = l;
            if pen.z <= pen.x && pen.z <= pen.y {
                n_local.z = xr_side(l.z);
                l2.z = n_local.z * h.z;
            } else if pen.x <= pen.y {
                n_local.x = xr_side(l.x);
                l2.x = n_local.x * h.x;
            } else {
                n_local.y = xr_side(l.y);
                l2.y = n_local.y * h.y;
            }
            *pos = c + xr_quat_rotate(q, l2);
            let n = xr_quat_rotate(q, n_local);
            let vn = dot(*vel, n);
            if vn < 0.0 {
                *vel -= (1.0 + restitution) * vn * n;
            }
        }
    }
}

// ---- flow ---------------------------------------------------------------------

// The volumetric flow field around the anchor. sample_flow_field_3d is fixed
// in space; the 2D sim animates by sweeping its slice through the texture's
// third axis at flow_speed. Here two samples slide through the field along
// different diagonals at that rate and are averaged, so the swirl evolves in
// place rather than translating as one block (the average of two curl fields
// is still divergence-free). sqrt(2) restores the amplitude one sample has.
fn xr_flow(pos: vec3f, half: f32) -> vec3f {
    // UV units per second (the 2D sim's time * flow_speed * 0.1), in meters.
    let drift = u.time * u.flow_speed * 0.1 * 2.0 * half / max(u.flow_scale, 1e-3);
    let a = sample_flow_field_3d(pos + vec3f(0.57735, 0.57735, 0.57735) * drift, half);
    let b = sample_flow_field_3d(pos + vec3f(-0.57735, 0.57735, -0.57735) * drift, half);
    return (a + b) * 0.70710678;
}

// ---- emission -----------------------------------------------------------------

fn emit_particle(idx: u32, half: f32) -> Particle {
    var p: Particle;
    let r = xr_rand3(idx, 0u);
    let s = xr_rand3(idx, 1u);

    // Uniform in the volume.
    let pos = (r * 2.0 - 1.0) * half;

    // Small random initial velocity, uniform direction (flow takes over).
    let z = s.x * 2.0 - 1.0;
    let ring = sqrt(max(1.0 - z * z, 0.0));
    let angle = s.y * 6.2831853;
    let speed = u.initial_speed * (0.3 + 0.7 * s.z);
    let vel = vec3f(ring * cos(angle), z, ring * sin(angle)) * speed;

    // Color: cool smoke tones, anchored to the musical key with centroid
    // warmth (the 2D sim's formulas; its warm shift below carries red to
    // saturation within a second or two, so the cloud reads as red embers
    // with key-tinted young particles and white bursts, as on desktop).
    let t = xr_rand3(idx, 2u);
    let hue = fract(t.x * 0.3 + u.dominant_chroma + u.centroid * 0.15);
    let r_c = abs(hue * 6.0 - 3.0) - 1.0;
    let g_c = 2.0 - abs(hue * 6.0 - 2.0);
    let b_c = 2.0 - abs(hue * 6.0 - 4.0);
    let brightness = 0.04 + u.rms * 0.03;
    // Burst particles flash bright near-white on beats/onsets.
    let hit = max(u.onset, u.beat);
    let base_col = clamp(vec3f(r_c, g_c, b_c), vec3f(0.0), vec3f(1.0)) * brightness;
    let col = mix(base_col, vec3f(0.6 + hit * 0.4), hit * hit);

    // Stagger initial age.
    let initial_age = t.y * u.lifetime * 0.3;

    let init_size = u.initial_size * (0.7 + t.z * 0.6);
    // Half the 2D sim's 0.15-0.25: over passthrough opacity also veils the
    // room, and a wearer inside the volume looks through far more sprites
    // per pixel than a screen shows.
    let base_alpha = 0.08 + s.z * 0.05;
    p.pos_life = vec4f(pos, 1.0);
    p.vel_size = vec4f(vel, init_size);
    // Invisible until its first update sets the opacity, by which time a
    // particle spawned inside an obstacle has been pushed out of it.
    p.color = vec4f(col, 0.0);
    p.flags = vec4f(initial_age, u.lifetime, init_size, base_alpha);
    return p;
}

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles {
        return;
    }
    let half = max(u.emitter_radius, 0.05);

    var p = read_particle(idx);
    let life = p.pos_life.w;
    let age = p.flags.x;
    let max_life = p.flags.y;

    if life <= 0.0 {
        let slot = emit_claim();
        if slot < u.emit_count {
            p = emit_particle(idx, half);
            write_particle(idx, p);
            mark_alive(idx);
        } else {
            write_particle(idx, p);
        }
        return;
    }

    let new_age = age + u.delta_time;
    if new_age >= max_life {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    let life_frac = new_age / max_life;
    let dt = u.delta_time;
    var vel = p.vel_size.xyz;
    var pos = p.pos_life.xyz;

    // --- Flow field: primary force ---
    let flow_vel = xr_flow(pos, half);
    // Audio modulation: bass increases flow strength.
    // MFCC(1) modulates curl tightness: bright timbre = tighter spirals, dark = loose.
    let mfcc1_curl = clamp(mfcc(1u) * 0.02, -0.5, 0.5);
    let audio_flow_mult = (1.0 + mfcc1_curl) * (1.0 + u.bass * 0.8 + u.mid * 0.3);
    vel += flow_vel * audio_flow_mult * dt;

    // Beat: brief speed boost in flow direction.
    if u.beat > 0.5 {
        vel += flow_vel * 0.5;
    }

    // Onset: radial push outward from the anchor.
    if u.onset > 0.3 {
        let dir = normalize(pos + vec3f(0.001, 0.001, 0.001));
        vel += dir * u.onset * 0.05 * dt;
    }

    // Gentle turbulence on top of flow. Spectral flux drives turbulence speed:
    // more timbral change = more spatial agitation. Two 2D noise planes give
    // a direction on the sphere (the 2D sim's one angle, plus an elevation).
    let flux_turb = 1.0 + u.flux * 2.0;
    let drift = vec2f(u.time * 0.3 * flux_turb, u.time * 0.25 * flux_turb);
    let turb_angle = fosfora_noise2(pos.xz * 5.0 + drift) * 6.28318;
    let turb_z = fosfora_noise2(pos.xy * 5.0 - drift) * 2.0 - 1.0;
    let turb_ring = sqrt(max(1.0 - turb_z * turb_z, 0.0));
    let turb_dir = vec3f(turb_ring * cos(turb_angle), turb_z, turb_ring * sin(turb_angle));
    vel += turb_dir * 0.003 * flux_turb * dt;

    // Drag.
    vel *= 1.0 - (1.0 - u.drag) * dt * 60.0;

    // Integrate with the settle drift, then push out of hands, furniture and
    // the floor.
    let settle = aux[XR_AUX_HEADER + 1u].home.z;
    pos += (vel - vec3f(0.0, settle, 0.0)) * dt;
    xr_collide(&pos, &vel);

    // Leaving the volume: respawn at a new point inside it (the 2D sim wraps).
    // Still alive, so the alive count and the density stay steady.
    let edge = max(abs(pos.x), max(abs(pos.y), abs(pos.z))) / half;
    if edge > 1.0 {
        p = emit_particle(idx, half);
        write_particle(idx, p);
        mark_alive(idx);
        return;
    }

    // Size: gentle shrink over life, audio reactive (rms, as in the 2D sim,
    // plus a pulse on the beat standing in for its trail shutter snap).
    let init_size = p.flags.z;
    let base_size = mix(init_size, u.size_end, life_frac * life_frac);
    let beat_env = exp(-u.beat_phase * 5.0);
    var size = base_size * (1.0 + u.rms * 0.3) * (1.0 + 0.25 * beat_env);

    // Opacity: fade in, fade out, toward the volume's bounds, and near the
    // wearer's eyes. Evaluated from the base opacity every frame rather than
    // compounded into color.a as the 2D sim does: the bounds and near fades
    // must recover when a particle moves back.
    let fade_in = smoothstep(0.0, 0.05, life_frac);
    let fade_out = 1.0 - smoothstep(0.7, 1.0, life_frac);
    let edge_fade = 1.0 - smoothstep(1.0 - XR_EDGE_FADE, 1.0, edge);
    var alpha = p.flags.w * fade_in * fade_out * edge_fade;

    // Near the head a sprite a few millimeters across fills the view: fade it
    // out and shrink it to nothing (zero radius rasterizes no fragments).
    let head = aux[XR_AUX_HEAD].home;
    if head.w > 0.0 {
        let near = smoothstep(head.w, head.w * 1.5, distance(pos, head.xyz));
        alpha *= near;
        size *= near;
    }

    // Color: warm shift with age and audio (at the 2D sim's per-frame rate
    // at 60 fps, independent of the display rate).
    var col = p.color.rgb;
    let warm_shift = (life_frac * 0.3 + u.mid * 0.1) * dt * 60.0;
    col = vec3f(col.r + warm_shift * 0.2, col.g, col.b - warm_shift * 0.1);
    col = clamp(col, vec3f(0.0), vec3f(1.0));

    p.pos_life = vec4f(pos, 1.0);
    p.vel_size = vec4f(vel, size);
    p.color = vec4f(col, alpha);
    p.flags.x = new_age;

    write_particle(idx, p);
    mark_alive(idx);
}
