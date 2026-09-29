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
// acceleration did not read, the flow blend damped it. A particle resting
// on a horizontal surface ages XR_REST_AGING times faster: with a uniform
// drift nearly every particle reaches the floor within its life and the
// cloud sank there (wearer, Sep 27); now it lands, slides for a few
// seconds, fades and respawns up in the volume.
//
// Per-frame XR inputs ride the aux buffer, which Flux does not otherwise use.
// The XR app writes them with ParticleSystem::update_aux_in_place, all in
// anchor-relative meters (layout mirrors ObstacleSet in
// crates/fosfora-xr/src/particles3d.rs, one vec4 per aux entry):
//   aux[0]          head position xyz; w = near-fade radius (0 = off)
//   aux[1]          sphere count (u32 bits), box count (u32 bits),
//                   restitution, margin
//   aux[2]          x = occluder shrink (unused here), y = hand kick (m/s),
//                   z = settle drift (m/s, downward; 0 = none),
//                   w = hand calm, 0..1: 1 - Murmur's hand scare, so an
//                   unwritten 0 keeps the full scare (unused here)
//   aux[3..67]      spheres: xyz center, w radius (hand joints)
//   aux[67..99]     box centers (xyz); w = surface kind (surface lanes)
//   aux[99..131]    box rotations, quaternion box -> world (x, y, z, w)
//   aux[131..163]   box half extents (xyz); w = emitter weight 0..1
//   aux[163..170]   Murmur's per-hand behavior lanes (murmur_xr_sim.wgsl;
//                   unused here)
//   aux[170..173]   the instrument rows (below; Murmur ignores them)
// All zero (nothing written yet, or a desktop test) means no obstacles, no
// near fade and no instruments.
//
// Instrument rows (board #3327; crates/fosfora-xr/src/instruments.rs, the
// hands as instruments):
//   aux[170]   x = burst count (u32 bits): of this frame's claimed dead
//              slots, the first x spawn at the burst instead of the
//              emitter; y = lift strength 0..1 (0 = no lift); z = lift
//              radius (m); w = steal fraction: a living particle respawns
//              at the burst with this probability, for a sim whose
//              particles are nearly all alive (few dead slots to claim)
//   aux[171]   burst center xyz (anchor-relative), w = burst radius (m)
//   aux[172]   lift point xyz (anchor-relative: the far palm), w = the
//              burst's brightness 0..1 (0 reads as 1): the streak shrinks
//              and dims as the throw recedes, the impact is 1
// A burst particle is born within the radius of the center, flying
// radially outward (0.3..1.2 m/s at a 0.12 m burst, slower for the
// throw's thinner streak), with half the lifetime, and drawn to be seen
// against the cloud: XR_BURST_SIZE times the sprite, XR_BURST_ALPHA
// opaque, its color pulled toward white (a 6000 ball drawn like the cloud
// is 1.5% of a 400K cloud and reads as nothing; board #3329), all three
// scaled by the brightness in aux[172].w. It
// is not bound to the volume: its life lane reads XR_FREE, it never
// respawns at the bounds and has no edge fade (a throw lands on walls
// outside the volume), and it dies XR_FREE_REACH_M from the anchor (past
// the 12 m the throw's ray looks; the old 3 half extents, 4.5 m, killed a
// burst on a wall 5 m ahead on its first frame). The
// lift pulls the particles under the palm, within its radius
// horizontally, up toward it (strength x XR_LIFT_ACCEL, fading to the
// rim), damps their lateral speed so they gather under the palm, and
// keeps them from resting (no XR_REST_AGING); above the palm, nothing.
//
// Surface lanes (board #3317; crates/fosfora-xr/src/surfaces.rs): the kind
// is 0 none, 1 table (DESK or TABLE), 2 floor, 3 wall, 4 ceiling, 5 door or
// window frame, 6 other, as a float; the weight is 0 for a box that emits
// nothing (every kind but tables and floors in the first pass, a surface
// out of the volume, the stage floor when the room has its own). A preset
// with param(6) > 0.5 (Flux XR Room's `surface_emit`) spawns on the top
// faces of the weighted boxes instead of in the volume; with no weight
// anywhere, or param(6) = 0 (every other preset: they have six inputs),
// the volume path runs unchanged.

const XR_AUX_HEAD: u32 = 0u;
const XR_AUX_HEADER: u32 = 1u;
const XR_MAX_SPHERES: u32 = 64u;
const XR_MAX_BOXES: u32 = 32u;
const XR_AUX_SPHERES: u32 = 3u;
const XR_AUX_BOX_CENTER: u32 = 67u;   // XR_AUX_SPHERES + XR_MAX_SPHERES
const XR_AUX_BOX_ROT: u32 = 99u;      // + XR_MAX_BOXES
const XR_AUX_BOX_HALF: u32 = 131u;    // + XR_MAX_BOXES
// After Murmur's per-hand lanes (163..170, XR_AUX_END in murmur_xr_sim.wgsl).
const XR_AUX_INSTRUMENTS: u32 = 170u;
const XR_AUX_INSTRUMENT_ROWS: u32 = 3u;

// Bursts: the radius the 0.3..1.2 m/s speeds are for, the lifetime
// fraction, the sprite size and base opacity against the cloud's and how
// far toward white the color goes, the life lane of a particle not bound
// to the volume, and how far from the anchor (m) it dies.
const XR_BURST_REF_RADIUS: f32 = 0.12;
const XR_BURST_LIFE: f32 = 0.5;
const XR_BURST_SIZE: f32 = 2.0;
const XR_BURST_ALPHA: f32 = 0.35;
const XR_BURST_WHITE: f32 = 0.5;
const XR_FREE: f32 = 2.0;
const XR_FREE_REACH_M: f32 = 13.0;
// The lift's acceleration at strength 1 (m/s^2) and its lateral damping
// (per second).
const XR_LIFT_ACCEL: f32 = 4.0;
const XR_LIFT_DAMP: f32 = 3.0;

// Surface emission: the param slot that turns it on, how far above the face
// a particle is born (m), and the table embers' lateral speed (m/s).
const XR_SURFACE_PARAM: u32 = 6u;
const XR_SURFACE_LIFT: f32 = 0.01;
const XR_EMBER_SPEED: f32 = 0.03;
const XR_KIND_TABLE: u32 = 1u;
const XR_KIND_FLOOR: u32 = 2u;
// Beat envelope decay per unit of beat phase for the table gate.
const XR_BEAT_DECAY: f32 = 6.0;

// Fraction of the half extent over which opacity fades out toward the bounds.
const XR_EDGE_FADE: f32 = 0.3;
// Extra aging per second for a particle resting on a horizontal surface.
const XR_REST_AGING: f32 = 2.0;

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
// Returns true when the particle was pushed out through an upward face: it
// is resting on a table or the floor.
fn xr_collide(pos: ptr<function, vec3f>, vel: ptr<function, vec3f>) -> bool {
    var rested = false;
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
            rested = rested || n.y > 0.7;
        }
    }
    return rested;
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

// How open a surface kind's emission is this frame, 0..1: tables shed
// embers on the beat, floors spark with the bass.
fn xr_kind_gate(kind: u32) -> f32 {
    if kind == XR_KIND_TABLE {
        // u.beat is a one-frame pulse; the envelope over beat_phase (a 0..1
        // sawtooth at the tempo) keeps the desk shedding for about a sixth
        // of a beat after each one, so the burst reads.
        return 0.15 + 0.85 * max(u.beat, exp(-XR_BEAT_DECAY * u.beat_phase));
    }
    if kind == XR_KIND_FLOOR {
        return 0.1 + 0.9 * u.bass;
    }
    return 0.3;
}

fn xr_box_kind(k: u32) -> u32 {
    return u32(max(aux[XR_AUX_BOX_CENTER + k].home.w, 0.0) + 0.5);
}

// A spawn on box k's upward face: the local axis closest to vertical,
// signed to point up (surfaces.rs `TopFace` picks it the same way), the
// point uniform over the part of the face under the volume (a 20 m floor
// is sampled only within the half extent of the anchor), XR_SURFACE_LIFT
// above it. `r` is three uniform draws.
fn xr_surface_point(k: u32, half: f32, r: vec3f) -> vec3f {
    let c = aux[XR_AUX_BOX_CENTER + k].home.xyz;
    let q = aux[XR_AUX_BOX_ROT + k].home;
    let h = aux[XR_AUX_BOX_HALF + k].home.xyz;
    let ax = xr_quat_rotate(q, vec3f(1.0, 0.0, 0.0));
    let ay = xr_quat_rotate(q, vec3f(0.0, 1.0, 0.0));
    let az = xr_quat_rotate(q, vec3f(0.0, 0.0, 1.0));
    // Normal axis n (half extent hn) and the in-plane axes t1, t2.
    var n = ax;
    var hn = h.x;
    var t1 = ay;
    var t2 = az;
    var h1 = h.y;
    var h2 = h.z;
    if abs(ay.y) > abs(n.y) {
        n = ay;
        hn = h.y;
        t1 = az;
        t2 = ax;
        h1 = h.z;
        h2 = h.x;
    }
    if abs(az.y) > abs(n.y) {
        n = az;
        hn = h.z;
        t1 = ax;
        t2 = ay;
        h1 = h.x;
        h2 = h.y;
    }
    n *= xr_side(n.y);
    let face = c + n * hn;
    // The anchor (the origin) in face coordinates, and the face's range
    // within the volume's half extent of it.
    let u0 = dot(-face, t1);
    let v0 = dot(-face, t2);
    let lo = vec2f(max(-h1, u0 - half), max(-h2, v0 - half));
    let hi = max(vec2f(min(h1, u0 + half), min(h2, v0 + half)), lo);
    let uv = mix(lo, hi, r.xy);
    return face + t1 * uv.x + t2 * uv.y + n * XR_SURFACE_LIFT;
}

// Spawn a particle for slot idx: on a surface when this preset asks for it
// and some box carries weight, otherwise in the volume (emit_particle,
// unchanged). Surface spawns draw over the boxes' cumulative weight times
// their kind's gate against the ungated total, so a draw past the gated sum
// spawns nothing: returns false and the slot stays dead this frame, and the
// room's emission breathes with the music.
fn xr_emit(idx: u32, half: f32, out: ptr<function, Particle>) -> bool {
    var total = 0.0;
    var box_count = 0u;
    if param(XR_SURFACE_PARAM) > 0.5 {
        box_count = min(bitcast<u32>(aux[XR_AUX_HEADER].home.y), XR_MAX_BOXES);
        for (var k = 0u; k < box_count; k++) {
            total += max(aux[XR_AUX_BOX_HALF + k].home.w, 0.0);
        }
    }
    if total <= 0.0 {
        *out = emit_particle(idx, half);
        return true;
    }
    let pick = xr_rand3(idx, 3u);
    let draw = pick.x * total;
    var acc = 0.0;
    var chosen = XR_MAX_BOXES;
    for (var k = 0u; k < box_count; k++) {
        let w = max(aux[XR_AUX_BOX_HALF + k].home.w, 0.0);
        acc += w * xr_kind_gate(xr_box_kind(k));
        if draw < acc {
            chosen = k;
            break;
        }
    }
    if chosen == XR_MAX_BOXES {
        return false;
    }
    // Life, color, size and opacity as the volume path gives them; the
    // position and velocity are the surface's.
    var p = emit_particle(idx, half);
    let at = xr_rand3(idx, 4u);
    let pos = xr_surface_point(chosen, half, at);
    let jitter = xr_rand3(idx, 5u) * 2.0 - 1.0;
    var vel: vec3f;
    if xr_box_kind(chosen) == XR_KIND_FLOOR {
        // Sparks: up with the bass, a little sideways.
        vel = vec3f(jitter.x * 0.1, 0.3 + 1.2 * u.bass, jitter.z * 0.1);
    } else {
        // Embers: a slow slide across the top; the flow and the settle
        // drift carry them to the edge and off it.
        vel = vec3f(jitter.x, 0.0, jitter.z) * XR_EMBER_SPEED;
    }
    p.pos_life = vec4f(pos, 1.0);
    p.vel_size = vec4f(vel, p.vel_size.w);
    *out = p;
    return true;
}

// A burst particle for slot idx (the instrument rows, above): the volume
// path's color, size and opacity, born within the burst's radius, flying
// out from its center, fully opaque at once and with a shorter life.
fn xr_burst(idx: u32, half: f32) -> Particle {
    let b = aux[XR_AUX_INSTRUMENTS + 1u].home;
    let given = aux[XR_AUX_INSTRUMENTS + 2u].home.w;
    let bright = select(clamp(given, 0.0, 1.0), 1.0, given <= 0.0);
    var p = emit_particle(idx, half);
    let r = xr_rand3(idx, 6u);
    let s = xr_rand3(idx, 7u);
    let z = r.x * 2.0 - 1.0;
    let ring = sqrt(max(1.0 - z * z, 0.0));
    let angle = r.y * 6.2831853;
    let dir = vec3f(ring * cos(angle), z, ring * sin(angle));
    let radius = max(b.w, 0.0);
    // Uniform in the ball: the radius goes as the cube root.
    let at = dir * radius * pow(r.z, 1.0 / 3.0);
    let speed = mix(0.3, 1.2, s.x) * radius / XR_BURST_REF_RADIUS;
    let life = u.lifetime * XR_BURST_LIFE;
    p.pos_life = vec4f(b.xyz + at, XR_FREE);
    // Bigger, brighter and more opaque than the cloud, so the streak and
    // the ball read through it; a dimmed streak falls back toward the
    // cloud's sprite, opacity and color.
    let size = p.flags.z * mix(1.0, XR_BURST_SIZE, bright);
    let alpha = mix(p.flags.w, XR_BURST_ALPHA, bright);
    p.vel_size = vec4f(dir * speed, size);
    p.color = vec4f(
        mix(p.color.rgb, vec3f(1.0, 0.95, 0.85), XR_BURST_WHITE * bright),
        alpha
    );
    // Past the fade-in, at its base opacity: a burst shows at once (the
    // surface and volume spawns start invisible for particles born inside
    // a box; this one is born in front of the surface).
    p.flags = vec4f(life * 0.05, life, size, alpha);
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

    let instruments = aux[XR_AUX_INSTRUMENTS].home;
    let burst_count = bitcast<u32>(instruments.x);
    if life <= 0.0 {
        let slot = emit_claim();
        var born: Particle;
        if slot < burst_count {
            write_particle(idx, xr_burst(idx, half));
            mark_alive(idx);
        } else if slot < u.emit_count && xr_emit(idx, half, &born) {
            write_particle(idx, born);
            mark_alive(idx);
        } else {
            write_particle(idx, p);
        }
        return;
    }

    // A sim with few dead slots takes the rest of a burst from the living.
    if burst_count > 0u && instruments.w > 0.0 && xr_rand3(idx, 8u).x < instruments.w {
        write_particle(idx, xr_burst(idx, half));
        mark_alive(idx);
        return;
    }
    let free = life > 1.5;

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

    // The lift: under the palm, within its radius, up toward it.
    var lifted = false;
    if instruments.y > 0.0 {
        let palm = aux[XR_AUX_INSTRUMENTS + 2u].home.xyz;
        let radius = max(instruments.z, 0.01);
        let to = palm - pos;
        let across = length(to.xz) / radius;
        if across < 1.0 && to.y > 0.0 {
            let pull = instruments.y * (1.0 - across * across);
            vel += normalize(to) * (XR_LIFT_ACCEL * pull * dt);
            let damp = max(1.0 - XR_LIFT_DAMP * pull * dt, 0.0);
            vel.x *= damp;
            vel.z *= damp;
            lifted = true;
        }
    }

    // Integrate with the settle drift, then push out of hands, furniture and
    // the floor.
    let settle = aux[XR_AUX_HEADER + 1u].home.z;
    pos += (vel - vec3f(0.0, settle, 0.0)) * dt;
    let rested = xr_collide(&pos, &vel) && !lifted;

    // Leaving the volume: respawn at a new point inside it (the 2D sim wraps).
    // Still alive, so the alive count and the density stay steady. In
    // surface mode the respawn is a surface spawn, and a closed gate lets
    // the particle die instead.
    let edge = max(abs(pos.x), max(abs(pos.y), abs(pos.z))) / half;
    if free {
        // A burst particle: loose in the room until it dies, or when it
        // flies farther than a throw can reach.
        if length(pos) > XR_FREE_REACH_M {
            p.pos_life.w = 0.0;
            write_particle(idx, p);
            return;
        }
    } else if edge > 1.0 {
        var born: Particle;
        if xr_emit(idx, half, &born) {
            write_particle(idx, born);
            mark_alive(idx);
        } else {
            p.pos_life.w = 0.0;
            write_particle(idx, p);
        }
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
    let edge_fade = select(1.0 - smoothstep(1.0 - XR_EDGE_FADE, 1.0, edge), 1.0, free);
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

    p.pos_life = vec4f(pos, select(1.0, XR_FREE, free));
    p.vel_size = vec4f(vel, size);
    p.color = vec4f(col, alpha);
    // Resting particles age faster, so surfaces turn over instead of
    // collecting the whole cloud.
    p.flags.x = new_age + select(0.0, dt * XR_REST_AGING, rested);

    write_particle(idx, p);
    mark_alive(idx);
}
