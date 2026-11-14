// Fireflies, the Quest build's one world effect (hidden preset
// assets/xr/effects/fireflies_xr_world.pfx; board #5470). Two kinds of
// particle share one pool: the fireflies, a sparse layer of soft points of
// light around the wearer, and the hand particles a throw or the pitcher
// makes. Drawn by the world-space sprite path (ParticleSystem::render_world,
// builtin/particle_render_world.wgsl) once per eye, additive. Structs,
// bindings and helpers are in particle_lib.wgsl (auto-prepended).
//
// World layout (particle_lib.wgsl, "World layout"):
//   pos_life = xyz position in meters relative to the effect anchor
//              (+Y up, -Z forward), w life: 0 dead, XR_FLY a firefly,
//              XR_FREE and above a hand particle (below)
//   vel_size = xyz velocity in m/s, w sprite radius in meters
//   color    = rgb straight color, a opacity; render_world multiplies rgb
//              by a (additive blend)
//   flags    = a firefly: x presence 0..1 (its fade in and out), the rest
//              unused. A hand particle: x age, y lifetime, z initial radius
//              in meters, w base opacity
//
// The pool and its caps. The particle count (the preset's max_count, which
// the XR app holds at or under fireflies::POOL_CAP) is every slot there is,
// so nothing can be alive past it. A firefly is only ever born in a slot
// whose index is under the fireflies' count (aux[1238].x, taken as at most
// XR_FLY_CAP), so no more fireflies than that are alive whatever the room,
// the song, the theme or the hands do. A hand particle is only ever born in
// a slot at or past that count. When the count goes down the fireflies past
// it fade out and die, and their slots go to the hands. When it goes up a
// hand particle in a slot the fireflies now own lives out its life first.
//
// Fireflies. One spawns in a dead slot of its own within about a second
// (XR_FLY_SPAWN_HZ), at a point uniform in the volume: XR_FLY_SPAN of the
// half extent u.emitter_radius around the anchor across, and the slab from
// XR_FLY_Y_LOW to XR_FLY_Y_HIGH of that half extent in height, which at the
// preset's 3 m is the floor to a ceiling of 2.65 m around an anchor 1 m up.
// It never dies of age. Everything that tells one firefly from another
// (its pace, its path, its rhythm, its size, its tint) comes from a hash of
// its slot index, so nothing about it changes from frame to frame.
//   Motion: a velocity that is a sum of slow sines of time, each firefly on
//   its own rates and phases, times the drift speed (aux[1238].w): a circle
//   of XR_FLY_TURN_S or longer, a second, slower one on top of it and a
//   slow rise and fall. Its speed moves between 0.6 and 1.3 of its own
//   drift speed and its direction turns under 40 degrees a second, half a
//   degree a frame. At the default drift its loop is about a meter across.
//   No flow field, no noise, no collide: a firefly inside a scanned box or
//   behind the depth map is hidden by the occluders. It fades toward the
//   volume's bounds and is unseen from the half extent on. One that gets
//   XR_FLY_OUT of the half extent out (the space size shrank, or the drift
//   knob is set high and its loop is wider than the volume) is born again
//   inside.
//   Light: each glows and fades on its own period, XR_FLY_PERIOD_S, from
//   XR_FLY_GLOW_LOW of its peak to the peak. The music lifts the light with
//   the loudness (u.rms as the XR app's flash limit lets it through; the
//   sim adds no envelope of its own and reads no beat phase, so the light
//   only ever moves the way the limit moves the level) and swells the
//   radius with the loudness as the limit glides it (aux[1239].w), each
//   scaled by the music knob (aux[1239].x).
//
// Hand particles. A burst (the instrument rows) or a pour (the pour row)
// claims dead hand slots, and when few are dead takes living hand particles,
// the old ones first. A hand particle is loose in the room until it dies of
// age or flies XR_FREE_REACH_M from the anchor. It is born looking as the
// cloud's bursts did, and reddens with age as they did.
//   Motion: gravity as an acceleration (aux[1239].y, m/s^2) and a light air
//   drag (u.drag), so it arcs and falls. Nothing else pushes it: no flow, no
//   turbulence, no kick on the beat. It does not bounce: where it meets a
//   box or the depth map the speed into the surface is taken away
//   (XR_HAND_RESTITUTION), and on a face that holds it up it slides, slows
//   (XR_HAND_FRICTION) and ages XR_REST_AGING times faster, so it fades
//   where it lies.
//   Edge flow (aux[1239].z, the strength, 0 = off). An edge is where a
//   face that holds a particle up meets a second surface: the floor and a
//   wall, the floor and a box standing on it, a table's top and its rim, a
//   table's top and the wall it stands against. Each frame the collide
//   notes every box face the particle is on or within the band of
//   (XR_EDGE_BAND_M times the strength), and the rims within the band of a
//   face that holds it up. With a face under it and a second surface
//   across it within the band, the particle is on a level edge: its speed
//   along the face is steered (XR_EDGE_STEER_HZ times the strength) to the
//   edge's direction at XR_EDGE_SPEED and in toward the edge, a rim brakes
//   it and stops it short of sliding off, and it does not age faster, so
//   it runs along the edge until it fades. Which way it runs is settled the
//   first time, from the way it was moving (or its slot index when it was
//   not), and kept in its life lane (XR_FREE_CW, XR_FREE_CCW). Where the
//   next edge comes within the band ahead it turns onto that one, so it
//   rounds a table's corner or a room's and goes on. With no face under it
//   and two upright surfaces of two boxes across each other within the
//   band (two walls, a wall and a cabinet) it slides down the edge at that
//   speed.
//   Where it cannot work. A box's own upright edges are not edges here: a
//   particle falling past a cabinet's corner drops to the floor and runs
//   along the cabinet's foot. The depth map gives a surface and its normal
//   but no edge of its own: a particle on something only the depth map
//   knows lies there and fades, unless a box is within the band (the floor
//   under a chair's leg). A sloping face and a level one do not make an
//   edge. With no room there is the stage floor alone, no second surface,
//   and the particles arc, fall and fade.
//   A hand still pushes the particles it touches (the spheres, their pad
//   and kick), but not in a particle's first XR_HAND_GRACE_S: a burst is
//   born at the pinch point, inside the hand's pad, and would jump out.
//
// Per-frame XR inputs ride the aux buffer, written by the XR app with
// ParticleSystem::update_aux_in_place, all in anchor-relative meters. The
// rows up to 1238 are flux_xr_sim.wgsl's (the layout mirrors ObstacleSet in
// crates/fosfora-xr/src/particles3d.rs), so one writer serves both:
//   aux[0]          head position xyz; w = near-fade radius (0 = off)
//   aux[1]          sphere count (u32 bits), box count (u32 bits),
//                   restitution (unused here), margin
//   aux[2]          x = the sim-cut mask (u32 bits; below), y = hand kick
//                   (m/s), z and w unused here
//   aux[3..67]      spheres: xyz center, w radius (hand joints)
//   aux[67..99]     box centers (xyz); w unused here
//   aux[99..131]    box rotations, quaternion box -> world (x, y, z, w)
//   aux[131..163]   box half extents (xyz); w unused here
//   aux[163..170]   unused here
//   aux[170..173]   the instrument rows (flux_xr_sim.wgsl, "Instrument
//                   rows"): the burst's count, center, radius and
//                   brightness, the lift, the steal fraction
//   aux[173..180]   the live depth rows (flux_xr_sim.wgsl, "Live depth
//                   rows")
//   aux[180]        the pour row: xyz direction, w speed (0 = no pour). At
//                   speed 0 a nonzero xyz is a throw's streak: the stretch
//                   from the burst's center back to the last frame's
//   aux[181..213]   unused here
//   aux[213..1238]  the box cell list (flux_xr_sim.wgsl, "Box cell list");
//                   the XR app dilates the masks by the edge band so a
//                   face within the band is in the particle's cell
//   aux[1238]       the fireflies: x = their count (u32 bits), y = sprite
//                   radius (m), z = brightness, the peak opacity, w = drift
//                   speed (m/s)
//   aux[1239]       x = how strongly the fireflies answer the music (0 =
//                   not at all), y = the hand particles' gravity (m/s^2),
//                   z = their edge-flow strength, w = the loudness gliding,
//                   0..1, for the fireflies' size
// All zero (nothing written yet) means no obstacles, no fireflies, no
// gravity and no edge flow.
//
// Sim-cut mask (crates/fosfora-xr/src/sim_cut.rs, `debug.fosfora.simcut`):
// aux[2].x as u32 bits, 0 in production. The bits are flux_xr_sim.wgsl's.
// This sim has no flow and no turbulence, so those two cut nothing.

const XR_AUX_HEAD: u32 = 0u;
const XR_AUX_HEADER: u32 = 1u;
const XR_MAX_SPHERES: u32 = 64u;
const XR_MAX_BOXES: u32 = 32u;
const XR_AUX_SPHERES: u32 = 3u;
const XR_AUX_BOX_CENTER: u32 = 67u;   // XR_AUX_SPHERES + XR_MAX_SPHERES
const XR_AUX_BOX_ROT: u32 = 99u;      // + XR_MAX_BOXES
const XR_AUX_BOX_HALF: u32 = 131u;    // + XR_MAX_BOXES
const XR_AUX_INSTRUMENTS: u32 = 170u;
const XR_AUX_INSTRUMENT_ROWS: u32 = 3u;
const XR_AUX_DEPTH: u32 = 173u;
const XR_AUX_DEPTH_ROWS: u32 = 7u;
const XR_AUX_POUR: u32 = 180u;
const XR_AUX_CELLS: u32 = 213u;
const XR_CELLS_MAX: u32 = 16u;
const XR_AUX_CELL_ROWS: u32 = 1024u;   // XR_CELLS_MAX^3 / 4
// After the box cell list; the XR app uploads 1240 rows.
const XR_AUX_FIREFLIES: u32 = 1238u;
const XR_AUX_FIREFLY_ROWS: u32 = 2u;

// The most fireflies alive at once (fireflies::FIREFLY_CAP).
const XR_FLY_CAP: u32 = 3000u;

// The life lane: a firefly, a hand particle whose way along an edge is not
// settled yet, and one that runs each way.
const XR_FLY: f32 = 1.0;
const XR_FREE: f32 = 2.0;
const XR_FREE_CW: f32 = 3.0;
const XR_FREE_CCW: f32 = 4.0;
// How far from the anchor (m) a hand particle dies.
const XR_FREE_REACH_M: f32 = 13.0;

// Fireflies. The slab they live in, as fractions of the half extent below
// and above the anchor. How far across the half extent they are born, the
// share of it over which they fade toward the bounds, and how far across it
// one is taken as out: it is unseen from 1 on, and at the default drift its
// loop carries it 1.2 m from where it was born at most. How fast a dead
// slot fills (per second) and how long a firefly takes to fade in or out
// (s). The shortest period of the circle its path is drawn on (s), and the
// size of the second, slower circle added to it, which makes the path a
// loop that never closes. The shortest and longest glow period (s) and the
// dimmest it gets against its peak. How much the music lifts the light and
// the radius at full loudness with the music knob at 1.
const XR_FLY_Y_LOW: f32 = -0.35;
const XR_FLY_Y_HIGH: f32 = 0.55;
const XR_FLY_SPAN: f32 = 0.9;
const XR_FLY_EDGE_FADE: f32 = 0.15;
const XR_FLY_OUT: f32 = 1.4;
const XR_FLY_SPAWN_HZ: f32 = 1.5;
const XR_FLY_FADE_S: f32 = 1.5;
const XR_FLY_TURN_S: f32 = 15.0;
const XR_FLY_LOOP: f32 = 0.35;
const XR_FLY_PERIOD_S: vec2f = vec2f(4.0, 9.0);
const XR_FLY_GLOW_LOW: f32 = 0.12;
const XR_FLY_MUSIC_LIGHT: f32 = 0.6;
const XR_FLY_MUSIC_SIZE: f32 = 0.35;
// Their two tints (linear RGB): a warm yellow and a yellow green.
const XR_FLY_WARM: vec3f = vec3f(1.0, 0.8, 0.32);
const XR_FLY_GREEN: vec3f = vec3f(0.7, 1.0, 0.35);

// Bursts: the radius the 0.3..1.2 m/s speeds are for, a streak's sprite and
// opacity against an impact's, an impact's opacity, how far toward white the
// color goes, and a pour's life against a throw's.
const XR_BURST_REF_RADIUS: f32 = 0.12;
const XR_BURST_DIM_SIZE: f32 = 0.5;
const XR_BURST_DIM_ALPHA: f32 = 0.1;
const XR_BURST_ALPHA: f32 = 0.35;
const XR_BURST_WHITE: f32 = 0.5;
const XR_POUR_LIFE: f32 = 2.0;
// The pour's cone: half-angle around its direction (radians; 6 degrees,
// instruments::POUR_SPREAD_DEG).
const XR_POUR_SPREAD: f32 = 0.10471976;
// The lift's acceleration at strength 1 (m/s^2) and its lateral damping
// (per second).
const XR_LIFT_ACCEL: f32 = 4.0;
const XR_LIFT_DAMP: f32 = 3.0;

// Hand particles: the share of the speed into a surface they keep (none:
// they do not bounce), how fast one slows on a face that holds it up (per
// second), the extra aging per second while it rests there, and how long
// after its birth the hands leave it alone (s).
const XR_HAND_RESTITUTION: f32 = 0.0;
const XR_HAND_FRICTION: f32 = 1.5;
const XR_REST_AGING: f32 = 2.0;
const XR_HAND_GRACE_S: f32 = 0.5;
// Edge flow at strength 1: how near a second surface must be (m,
// fireflies::EDGE_BAND_M), how fast the velocity turns to the edge (per
// second), the speed along it (m/s; each particle 0.6 to 1.4 of it), the
// share of that speed it closes on the edge with from the band's far side,
// how fast it may close on a rim per meter left to it (per second), how
// near a rim it is stopped (m), and the speed along an edge (m/s) from
// which the way it was moving decides the way it runs.
const XR_EDGE_BAND_M: f32 = 0.05;
const XR_EDGE_STEER_HZ: f32 = 6.0;
const XR_EDGE_SPEED: f32 = 0.3;
const XR_EDGE_PULL: f32 = 0.5;
const XR_EDGE_BRAKE_HZ: f32 = 20.0;
const XR_EDGE_STOP_M: f32 = 0.002;
const XR_EDGE_DECIDE: f32 = 0.03;
// A normal this far up is a face that holds a particle up; two normals
// closer than this (the cosine) are the same surface.
const XR_UP: f32 = 0.5;
const XR_SAME: f32 = 0.8;

// The depth collide's tangential damping per colliding frame, and the
// depth jump between neighboring texels (m) that reads as a silhouette.
const XR_DEPTH_COLLIDE_FRICTION: f32 = 0.9;
const XR_DEPTH_SILHOUETTE_M: f32 = 0.5;

// The sim-cut mask's bits (aux[2].x; sim_cut.rs `PARTS` in order).
const XR_CUT_FLOW: u32 = 1u;
const XR_CUT_TURB: u32 = 2u;
const XR_CUT_BOXES: u32 = 4u;
const XR_CUT_SPHERES: u32 = 8u;
const XR_CUT_DEPTH: u32 = 16u;
const XR_CUT_LIFT: u32 = 32u;
const XR_CUT_EDGE: u32 = 64u;
const XR_CUT_ATOMICS: u32 = 128u;
const XR_CUT_PATH: u32 = 256u;

const XR_TAU: f32 = 6.2831853;

// ---- random -------------------------------------------------------------------

// Three uniform [0, 1) values per (particle, salt), new every frame (the
// seed moves with time): where a particle is born.
fn xr_rand3(idx: u32, salt: u32) -> vec3f {
    let s = uhash(idx ^ uhash(bitcast<u32>(u.seed) + salt));
    return vec3f(uhash_f(s), uhash_f(s ^ 0x9e3779b9u), uhash_f(s ^ 0x85ebca6bu));
}

// Four uniform [0, 1) values per (slot, salt), the same every frame: what a
// firefly is.
fn xr_own4(idx: u32, salt: u32) -> vec4f {
    let s = uhash((idx + 1u) * 0x9e3779b1u ^ uhash(salt + 0x51ed270bu));
    return vec4f(
        uhash_f(s),
        uhash_f(s ^ 0x9e3779b9u),
        uhash_f(s ^ 0x85ebca6bu),
        uhash_f(s ^ 0xc2b2ae35u)
    );
}

// ---- obstacles ----------------------------------------------------------------

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

// The unit vector along box axis a (0 x, 1 y, 2 z). The collide picks its
// axes at run time and reads and writes them through this, with a dot
// product, so no vector is indexed by a variable.
fn xr_axis(a: u32) -> vec3f {
    return select(
        select(vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0), a == 2u),
        vec3f(1.0, 0.0, 0.0),
        a == 0u
    );
}

// The mask of the boxes that can touch a particle at p (the box cell list):
// bit k for box k. Every bit with no grid or outside it.
fn xr_box_mask(p: vec3f) -> u32 {
    let head = aux[XR_AUX_CELLS].home;
    let n = bitcast<u32>(head.x);
    let half = head.y;
    if n == 0u || n > XR_CELLS_MAX || !(half > 0.0) || !all(abs(p) <= vec3f(half)) {
        return 0xffffffffu;
    }
    let t = max(floor((p + half) / (2.0 * half) * f32(n)), vec3f(0.0));
    let i = min(vec3u(t), vec3u(n - 1u));
    let c = (i.x * n + i.y) * n + i.z;
    let row = bitcast<vec4u>(aux[XR_AUX_CELLS + 1u + c / 4u].home);
    let lane = c % 4u;
    return select(select(row.x, row.y, lane == 1u), select(row.z, row.w, lane == 3u), lane >= 2u);
}

// What a hand particle is on or beside this frame, for the edge flow: the
// face that holds it up, if any, and the nearest surfaces across it. A
// surface across is kept as the level direction from the particle toward
// it, so a table's rim and the wall the table stands against, which lie the
// same way, give the same edge.
struct Contact {
    // The face that holds it up: its normal, how far over it the particle
    // is (m), and whether there is one.
    up: vec3f,
    up_m: f32,
    held: bool,
    // The nearest surface across: the direction toward it and its distance.
    side: vec3f,
    side_m: f32,
    // The nearest one after it that lies another way, for an upright edge.
    other: vec3f,
    other_m: f32,
}

// Note a face with normal n that the particle is d meters over: it holds
// the particle up. The nearer of two wins.
fn xr_note_up(c: ptr<function, Contact>, n: vec3f, d: f32) {
    if !(*c).held || d < (*c).up_m {
        (*c).up = n;
        (*c).up_m = d;
        (*c).held = true;
    }
}

// Note a surface across, d meters away in the direction `toward`. A level
// surface (a ceiling, the underside of a shelf) is across nothing.
fn xr_note_side(c: ptr<function, Contact>, toward: vec3f, d: f32) {
    if abs(toward.y) >= XR_SAME {
        return;
    }
    if d < (*c).side_m {
        if (*c).side_m < 1e8 && abs(dot((*c).side, toward)) < XR_SAME {
            (*c).other = (*c).side;
            (*c).other_m = (*c).side_m;
        }
        (*c).side = toward;
        (*c).side_m = d;
    } else if d < (*c).other_m && abs(dot((*c).side, toward)) < XR_SAME {
        (*c).other = toward;
        (*c).other_m = d;
    }
}

// The distance (m) atlas texel t of layer k holds, or -1 without data: a
// 16-bit fraction of the range, high byte in R, low byte in B.
fn xr_depth_at(t: vec2i, k: u32, side: i32, range: f32) -> f32 {
    let c = clamp(t, vec2i(0), vec2i(side - 1));
    let s = textureLoad(obstacle_tex, vec2i(c.x, c.y + i32(k) * side), 0);
    let v = round(s.r * 255.0) * 256.0 + round(s.b * 255.0);
    return select(-1.0, v / 65535.0 * range, s.g > 0.5);
}

// The point at distance d (along -Z) on the ray through the middle of
// texel t, in the depth camera's frame. `tan` is the fov's tangents.
fn xr_depth_point(t: vec2i, d: f32, tan: vec4f, side: f32) -> vec3f {
    let uv = (vec2f(t) + 0.5) / side;
    return vec3f(mix(tan.x, tan.y, uv.x), mix(tan.w, tan.z, uv.y), -1.0) * d;
}

// The collide with the live depth map (flux_xr_sim.wgsl's, with a hand
// particle's restitution) for particle idx. Returns the surface's normal
// with w = 1 when the particle was pushed out of it, w = 0 otherwise.
fn xr_depth_collide(pos: ptr<function, vec3f>, vel: ptr<function, vec3f>, idx: u32) -> vec4f {
    let head = aux[XR_AUX_DEPTH].home;
    let stride = bitcast<u32>(head.x);
    let every = stride & 0xffffu;
    // No atlas, or not this particle's frame: one particle in `every` is
    // tested per frame, each on its own phase.
    if every == 0u || (idx + (stride >> 16u)) % every != 0u {
        return vec4f(0.0);
    }
    let side = i32(head.w + 0.5);
    if side < 2 {
        return vec4f(0.0);
    }
    let range = head.z;
    for (var k = 0u; k < 2u; k++) {
        let row = XR_AUX_DEPTH + 1u + 3u * k;
        let origin = aux[row].home.xyz;
        let q = aux[row + 1u].home;
        let pc = xr_quat_rotate(xr_quat_conj(q), *pos - origin);
        if pc.z > -1e-3 {
            continue;
        }
        let tan = aux[row + 2u].home;
        let depth = -pc.z;
        let t = pc.xy / depth;
        let uv = vec2f((t.x - tan.x) / (tan.y - tan.x), (t.y - tan.w) / (tan.z - tan.w));
        if any(uv < vec2f(0.0)) || any(uv >= vec2f(1.0)) {
            continue;
        }
        // The first layer whose view holds the particle decides, with data
        // or without.
        let texel = min(vec2i(uv * f32(side)), vec2i(side - 1));
        let surface = xr_depth_at(texel, k, side, range);
        let margin = aux[XR_AUX_HEADER].home.w;
        if surface < 0.0 || depth <= surface - margin || depth >= surface + head.y {
            return vec4f(0.0);
        }
        // The surface normal from the depth gradient, facing the camera; at
        // a silhouette, the ray toward the camera.
        var n_cam = -pc / length(pc);
        let last = vec2i(side - 1);
        let xp = min(texel + vec2i(1, 0), last);
        let xm = max(texel - vec2i(1, 0), vec2i(0));
        let yp = min(texel + vec2i(0, 1), last);
        let ym = max(texel - vec2i(0, 1), vec2i(0));
        let d = vec4f(
            xr_depth_at(xp, k, side, range),
            xr_depth_at(xm, k, side, range),
            xr_depth_at(yp, k, side, range),
            xr_depth_at(ym, k, side, range)
        );
        if all(d >= vec4f(0.0)) && all(abs(d - surface) <= vec4f(XR_DEPTH_SILHOUETTE_M)) {
            let fs = f32(side);
            let m = cross(
                xr_depth_point(xp, d.x, tan, fs) - xr_depth_point(xm, d.y, tan, fs),
                xr_depth_point(yp, d.z, tan, fs) - xr_depth_point(ym, d.w, tan, fs)
            );
            let len = length(m);
            if len > 1e-10 {
                n_cam = m / len;
                if dot(n_cam, pc) > 0.0 {
                    n_cam = -n_cam;
                }
            }
        }
        // Out along the normal, onto the plane through the point margin in
        // front of the surface on the particle's ray.
        let aim = pc * ((surface - margin) / depth);
        let out_cam = pc + n_cam * max(dot(aim - pc, n_cam), 0.0);
        *pos = origin + xr_quat_rotate(q, out_cam);
        let n = xr_quat_rotate(q, n_cam);
        let vn = dot(*vel, n);
        let tangential = *vel - vn * n;
        *vel = select(vn, -XR_HAND_RESTITUTION * vn, vn < 0.0) * n
            + tangential * XR_DEPTH_COLLIDE_FRICTION;
        return vec4f(n, 1.0);
    }
    return vec4f(0.0);
}

// Push a hand particle out of every obstacle it is inside, take away its
// speed into each, and note what it is on or beside (Contact): every box
// face within `band` of it, and the rims within the band of a face that
// holds it up. `band` is the edge band (0: no edge flow, a rim holds
// nothing back, and only a face it was pushed out of holds it up), `hands`
// whether the hands' spheres push it, `cut` the sim-cut mask.
fn xr_collide(
    pos: ptr<function, vec3f>,
    vel: ptr<function, vec3f>,
    idx: u32,
    band: f32,
    hands: bool,
    cut: u32
) -> Contact {
    var c: Contact;
    c.up = vec3f(0.0, 1.0, 0.0);
    c.up_m = 1e9;
    c.held = false;
    c.side = vec3f(0.0);
    c.side_m = 1e9;
    c.other = vec3f(0.0);
    c.other_m = 1e9;
    let header = aux[XR_AUX_HEADER].home;
    let sphere_count = select(
        min(bitcast<u32>(header.x), XR_MAX_SPHERES),
        0u,
        (cut & XR_CUT_SPHERES) != 0u || !hands
    );
    let box_count = select(min(bitcast<u32>(header.y), XR_MAX_BOXES), 0u, (cut & XR_CUT_BOXES) != 0u);
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
                *vel -= vn * n;
            }
            // Bring the outward speed up to the kick, never beyond it.
            let outward = dot(*vel, n);
            *vel += n * max(sphere_kick - outward, 0.0);
        }
    }
    // The boxes that can touch the particle where it is now, re-read after
    // each push.
    var mask = 0u;
    if box_count > 0u {
        mask = xr_box_mask(*pos);
    }
    for (var k = 0u; k < box_count; k++) {
        if ((mask >> k) & 1u) == 0u {
            continue;
        }
        let center = aux[XR_AUX_BOX_CENTER + k].home.xyz;
        let q = aux[XR_AUX_BOX_ROT + k].home;
        let h = aux[XR_AUX_BOX_HALF + k].home.xyz + vec3f(margin);
        var l = xr_quat_rotate(xr_quat_conj(q), *pos - center);
        // How far inside each pair of faces; negative is outside.
        let pen = h - abs(l);
        if !(pen.x > -band && pen.y > -band && pen.z > -band) {
            continue;
        }
        // The face it is on or nearest: the axis it is least inside.
        var a = 1u;
        if pen.z <= pen.x && pen.z <= pen.y {
            a = 2u;
        } else if pen.x <= pen.y {
            a = 0u;
        }
        let ea = xr_axis(a);
        let n_local = ea * xr_side(dot(l, ea));
        let n = xr_quat_rotate(q, n_local);
        let inside = pen.x > 0.0 && pen.y > 0.0 && pen.z > 0.0;
        if inside {
            // Onto the face it is nearest.
            l += n_local * dot(h, ea) - ea * dot(l, ea);
            let vn = dot(*vel, n);
            if vn < 0.0 {
                *vel -= (1.0 + XR_HAND_RESTITUTION) * vn * n;
            }
        }
        let over = max(-dot(pen, ea), 0.0);
        if n.y > XR_UP {
            if band > 0.0 || inside {
                xr_note_up(&c, n, over);
            }
            // The rims of a face that holds it up, within the band: the
            // edges it could slide off.
            for (var b = 0u; b < 3u; b++) {
                let eb = xr_axis(b);
                let room = dot(pen, eb);
                if b == a || room >= band {
                    continue;
                }
                let r_local = eb * xr_side(dot(l, eb));
                let rim = xr_quat_rotate(q, r_local);
                xr_note_side(&c, rim, max(room, 0.0));
                // The rim stops it: it does not slide off. Lying on the
                // face, the nearer the rim the slower it may close on it,
                // so it brakes over a few frames and comes no nearer than
                // the stop, however fast it slid.
                if inside {
                    if room < XR_EDGE_STOP_M {
                        l += r_local * (dot(h, eb) - XR_EDGE_STOP_M) - eb * dot(l, eb);
                    }
                    let most = max(room - XR_EDGE_STOP_M, 0.0)
                        * min(XR_EDGE_BRAKE_HZ, 1.0 / max(u.delta_time, 1e-4));
                    let vr = dot(*vel, rim);
                    if vr > most {
                        *vel -= (vr - most) * rim;
                    }
                }
            }
        } else if band > 0.0 {
            xr_note_side(&c, -n, over);
        }
        if inside {
            *pos = center + xr_quat_rotate(q, l);
            mask = xr_box_mask(*pos);
        }
    }
    if (cut & XR_CUT_DEPTH) == 0u {
        let hit = xr_depth_collide(pos, vel, idx);
        if hit.w > 0.5 {
            if hit.y > XR_UP {
                xr_note_up(&c, hit.xyz, 0.0);
            } else if band > 0.0 {
                xr_note_side(&c, -hit.xyz, 0.0);
            }
        }
    }
    return c;
}

// ---- hand particles -------------------------------------------------------------

// A pour particle's direction: within XR_POUR_SPREAD of `axis` (unit),
// uniform over the cone's cap.
fn xr_pour_dir(idx: u32, axis: vec3f) -> vec3f {
    let c = xr_rand3(idx, 9u);
    let cos_t = 1.0 - c.x * (1.0 - cos(XR_POUR_SPREAD));
    let sin_t = sqrt(max(1.0 - cos_t * cos_t, 0.0));
    let phi = c.y * XR_TAU;
    // Any vector not along the axis gives the cap's frame.
    let helper = select(vec3f(1.0, 0.0, 0.0), vec3f(0.0, 0.0, 1.0), abs(axis.x) > 0.9);
    let t1 = normalize(cross(axis, helper));
    let t2 = cross(axis, t1);
    return axis * cos_t + (t1 * cos(phi) + t2 * sin(phi)) * sin_t;
}

// A burst particle for slot idx (the instrument rows): born within the
// burst's radius, flying out from its center, opaque at once. A pour's (the
// pour row) flies along the pour instead and lives XR_POUR_LIFE as long.
// Its look is the one the cloud's bursts had: a dim hue anchored to the
// musical key, pulled halfway to a warm white, at the cloud's burst size
// and opacity. Only the flash a burst born on a beat had is left out.
fn xr_burst(idx: u32) -> Particle {
    let b = aux[XR_AUX_INSTRUMENTS + 1u].home;
    let given = aux[XR_AUX_INSTRUMENTS + 2u].home.w;
    let bright = select(clamp(given, 0.0, 1.0), 1.0, given <= 0.0);
    let r = xr_rand3(idx, 6u);
    let s = xr_rand3(idx, 7u);
    let t = xr_rand3(idx, 2u);
    let z = r.x * 2.0 - 1.0;
    let ring = sqrt(max(1.0 - z * z, 0.0));
    let angle = r.y * XR_TAU;
    let dir = vec3f(ring * cos(angle), z, ring * sin(angle));
    let radius = max(b.w, 0.0);
    // Uniform in the ball: the radius goes as the cube root.
    var at = dir * radius * pow(r.z, 1.0 / 3.0);
    var vel = dir * (mix(0.3, 1.2, s.x) * radius / XR_BURST_REF_RADIUS);
    var life = u.lifetime;
    let pour = aux[XR_AUX_POUR].home;
    let axis_len = length(pour.xyz);
    if pour.w > 0.0 && axis_len > 1e-4 {
        vel = xr_pour_dir(idx, pour.xyz / axis_len) * pour.w;
        life = u.lifetime * XR_POUR_LIFE;
    } else if axis_len > 1e-4 {
        // A throw's streak: born along the stretch flown since the last
        // frame's, so the frames run together in one line. Nothing here
        // stirs the particles, so a clump a frame would stay a clump.
        at += pour.xyz * s.y;
    }
    let hue = fract(t.x * 0.3 + u.dominant_chroma + u.centroid * 0.15);
    let key = clamp(
        vec3f(abs(hue * 6.0 - 3.0) - 1.0, 2.0 - abs(hue * 6.0 - 2.0), 2.0 - abs(hue * 6.0 - 4.0)),
        vec3f(0.0),
        vec3f(1.0)
    );
    let col = mix(
        key * (0.04 + u.rms * 0.03),
        vec3f(1.0, 0.95, 0.85),
        XR_BURST_WHITE * bright
    );
    // A dimmed streak is smaller and fainter; the impact is full.
    let size = u.initial_size * (0.7 + t.z * 0.6) * mix(XR_BURST_DIM_SIZE, 1.0, bright);
    let alpha = mix(XR_BURST_DIM_ALPHA, XR_BURST_ALPHA, bright);
    var p: Particle;
    p.pos_life = vec4f(b.xyz + at, XR_FREE);
    p.vel_size = vec4f(vel, size);
    p.color = vec4f(col, alpha);
    p.flags = vec4f(0.0, life, size, alpha);
    return p;
}

// One frame of a living hand particle.
fn xr_hand_step(idx: u32, p_in: Particle, cut: u32) {
    var p = p_in;
    let dt = u.delta_time;
    let knobs = aux[XR_AUX_FIREFLIES + 1u].home;
    let age = p.flags.x;
    let max_life = p.flags.y;
    let new_age = age + dt;
    if new_age >= max_life {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }
    var vel = p.vel_size.xyz;
    var pos = p.pos_life.xyz;

    // Gravity and a light air drag: an arc, then a fall.
    vel.y -= max(knobs.y, 0.0) * dt;
    vel *= max(1.0 - (1.0 - u.drag) * dt * 60.0, 0.0);

    // The lift: under the palm, within its radius, up toward it.
    let instruments = aux[XR_AUX_INSTRUMENTS].home;
    var lifted = false;
    if instruments.y > 0.0 && (cut & XR_CUT_LIFT) == 0u {
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

    pos += vel * dt;
    let strength = max(knobs.z, 0.0);
    let band = XR_EDGE_BAND_M * strength;
    let c = xr_collide(&pos, &vel, idx, band, new_age > XR_HAND_GRACE_S, cut);

    // Too far to be a throw's: gone.
    if (cut & XR_CUT_EDGE) == 0u && length(pos) > XR_FREE_REACH_M {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }

    // The edge flow.
    var life = p.pos_life.w;
    var on_edge = false;
    let steer = 1.0 - exp(-XR_EDGE_STEER_HZ * strength * dt);
    let own = xr_own4(idx, 3u);
    let speed = XR_EDGE_SPEED * (0.6 + 0.8 * own.x);
    if strength > 0.0 && !lifted {
        if c.held && c.side_m < band && abs(dot(c.up, c.side)) < XR_SAME {
            // A level edge: along the face that holds it up, across the
            // surface beside it.
            let along = normalize(cross(c.up, c.side));
            if life < XR_FREE_CW - 0.5 {
                let going = dot(vel, along);
                var cw = (idx & 1u) == 0u;
                if abs(going) > XR_EDGE_DECIDE {
                    cw = going > 0.0;
                }
                life = select(XR_FREE_CCW, XR_FREE_CW, cw);
            }
            let way = select(-1.0, 1.0, life < XR_FREE_CCW - 0.5);
            var toward = c.side;
            var toward_m = c.side_m;
            // A corner ahead, where the next edge is within the band: turn
            // onto it now, so the corner is rounded and not a stop.
            if c.other_m < band && abs(dot(c.up, c.other)) < XR_SAME
                && dot(c.other, along * way) > 0.5 {
                toward = c.other;
                toward_m = c.other_m;
            }
            let dir = normalize(cross(c.up, toward)) * way;
            // Along the edge, and in toward it while it is still off it.
            let level = toward - dot(toward, c.up) * c.up;
            let pull = XR_EDGE_PULL * clamp(toward_m / band, 0.0, 1.0);
            let vn = dot(vel, c.up);
            let flat = vel - vn * c.up;
            vel = mix(flat, (dir + level * pull) * speed, steer) + vn * c.up;
            on_edge = true;
        } else if !c.held && c.side_m < band && c.other_m < band
            && abs(c.side.y) < XR_UP && abs(c.other.y) < XR_UP {
            // An upright edge: down it.
            var down = normalize(cross(c.side, c.other));
            down *= select(1.0, -1.0, down.y > 0.0);
            vel = mix(vel, down * speed, steer);
            on_edge = true;
        }
    }
    // Lying on a face away from any edge: it slows and fades.
    let rested = c.held && c.up_m <= XR_EDGE_STOP_M && !on_edge && !lifted;
    if rested {
        let vn = dot(vel, c.up);
        let flat = vel - vn * c.up;
        vel = flat * max(1.0 - XR_HAND_FRICTION * dt, 0.0) + vn * c.up;
    }

    // Size: a gentle shrink over life, a little larger with the loudness.
    let aged = new_age + select(0.0, dt * XR_REST_AGING, rested);
    let life_frac = clamp(aged / max_life, 0.0, 1.0);
    var size = mix(p.flags.z, u.size_end, life_frac * life_frac) * (1.0 + u.rms * 0.3);
    var alpha = p.flags.w * (1.0 - smoothstep(0.7, 1.0, life_frac));

    // Near the head a sprite fills the view: fade it out and shrink it.
    let head = aux[XR_AUX_HEAD].home;
    if head.w > 0.0 {
        let near = smoothstep(head.w, head.w * 1.5, distance(pos, head.xyz));
        alpha *= near;
        size *= near;
    }

    // Color: the cloud's warm shift with age, so a burst cools to embers
    // (at its per-frame rate at 60 fps, whatever the display rate).
    let warm = (life_frac * 0.3 + u.mid * 0.1) * dt * 60.0;
    let col = clamp(
        vec3f(p.color.r + warm * 0.2, p.color.g, p.color.b - warm * 0.1),
        vec3f(0.0),
        vec3f(1.0)
    );

    p.pos_life = vec4f(pos, life);
    p.vel_size = vec4f(vel, size);
    p.color = vec4f(col, alpha);
    p.flags.x = aged;
    write_particle(idx, p);
    if (cut & XR_CUT_ATOMICS) != 0u {
        alive_indices_out[idx] = idx;
    } else {
        mark_alive(idx);
    }
}

// ---- fireflies ------------------------------------------------------------------

// A firefly for slot idx: a point uniform in the volume, unseen until its
// first update fades it in.
fn xr_fly_spawn(idx: u32, half: f32) -> Particle {
    let r = xr_rand3(idx, 0u);
    let own = xr_own4(idx, 2u);
    var p: Particle;
    p.pos_life = vec4f(
        (r.x * 2.0 - 1.0) * half * XR_FLY_SPAN,
        mix(XR_FLY_Y_LOW, XR_FLY_Y_HIGH, r.y) * half,
        (r.z * 2.0 - 1.0) * half * XR_FLY_SPAN,
        XR_FLY
    );
    p.vel_size = vec4f(0.0);
    p.color = vec4f(mix(XR_FLY_WARM, XR_FLY_GREEN, own.x), 0.0);
    p.flags = vec4f(0.0);
    return p;
}

// One frame of a living firefly. `count` is how many the knob asks for.
fn xr_fly_step(idx: u32, p_in: Particle, half: f32, count: u32, cut: u32) {
    var p = p_in;
    let dt = u.delta_time;
    let fly = aux[XR_AUX_FIREFLIES].home;
    let knobs = aux[XR_AUX_FIREFLIES + 1u].home;
    // Past the count it fades out and dies; under it, it fades in.
    let wanted = idx < count;
    let presence = p.flags.x + select(-1.0, 1.0, wanted) * dt / XR_FLY_FADE_S;
    if presence <= 0.0 && !wanted {
        p.pos_life.w = 0.0;
        write_particle(idx, p);
        return;
    }
    let own = xr_own4(idx, 1u);
    let look = xr_own4(idx, 2u);

    // The drift: slow sines of time, on this firefly's own rates and phases.
    let rate = XR_TAU / XR_FLY_TURN_S * (0.5 + 0.5 * own.x);
    let a = u.time * rate + own.y * XR_TAU;
    let b = u.time * rate * 0.618 + own.z * XR_TAU;
    let c = u.time * rate * 0.414 + own.w * XR_TAU;
    let dir = vec3f(cos(a) + XR_FLY_LOOP * cos(b), 0.5 * sin(c), sin(a) + XR_FLY_LOOP * sin(b));
    let vel = dir * (max(fly.w, 0.0) * (0.6 + 0.8 * look.y) / 1.1);
    let pos = p.pos_life.xyz + vel * dt;

    // Out of the volume: born again inside it.
    let across = max(abs(pos.x), abs(pos.z)) / half;
    let up = pos.y / half;
    if (cut & XR_CUT_EDGE) == 0u
        && (across > XR_FLY_OUT
            || up < XR_FLY_Y_LOW - (XR_FLY_OUT - 1.0)
            || up > XR_FLY_Y_HIGH + (XR_FLY_OUT - 1.0)) {
        write_particle(idx, xr_fly_spawn(idx, half));
        mark_alive(idx);
        return;
    }

    // Its own slow rhythm, lifted by the loudness.
    let period = mix(XR_FLY_PERIOD_S.x, XR_FLY_PERIOD_S.y, look.z);
    let wave = 0.5 - 0.5 * cos(fract(u.time / period + look.w) * XR_TAU);
    let glow = mix(XR_FLY_GLOW_LOW, 1.0, wave * wave);
    let music = max(knobs.x, 0.0);
    let light = 1.0 + music * XR_FLY_MUSIC_LIGHT * clamp(u.rms, 0.0, 1.0);
    let swell = 1.0 + music * XR_FLY_MUSIC_SIZE * clamp(knobs.w, 0.0, 1.0);
    let bound = (1.0 - smoothstep(1.0 - XR_FLY_EDGE_FADE, 1.0, across))
        * (1.0 - smoothstep(0.0, XR_FLY_EDGE_FADE, max(XR_FLY_Y_LOW - up, up - XR_FLY_Y_HIGH)));
    var alpha = clamp(max(fly.z, 0.0) * glow * light, 0.0, 1.0)
        * clamp(presence, 0.0, 1.0) * bound;
    var size = max(fly.y, 0.0) * (0.7 + 0.6 * own.x) * (0.75 + 0.25 * glow) * swell;

    // Near the head a sprite fills the view: fade it out and shrink it,
    // from farther out than a hand particle, which is born at the hand.
    let head = aux[XR_AUX_HEAD].home;
    if head.w > 0.0 {
        let near = smoothstep(head.w * 2.0, head.w * 3.0, distance(pos, head.xyz));
        alpha *= near;
        size *= near;
    }

    p.pos_life = vec4f(pos, XR_FLY);
    p.vel_size = vec4f(vel, size);
    p.color.a = alpha;
    p.flags.x = clamp(presence, 0.0, 1.0);
    write_particle(idx, p);
    if (cut & XR_CUT_ATOMICS) != 0u {
        alive_indices_out[idx] = idx;
    } else {
        mark_alive(idx);
    }
}

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles {
        return;
    }
    let half = max(u.emitter_radius, 0.05);
    // The sim-cut mask (aux[2].x): 0 runs every part.
    let cut = bitcast<u32>(aux[XR_AUX_HEADER + 1u].home.x);
    // The fireflies' slots are the first `count`; the hands' are the rest.
    let count = min(bitcast<u32>(aux[XR_AUX_FIREFLIES].home.x), XR_FLY_CAP);
    let instruments = aux[XR_AUX_INSTRUMENTS].home;
    let burst_count = bitcast<u32>(instruments.x);

    let p = read_particle(idx);
    let life = p.pos_life.w;
    if life <= 0.0 {
        if idx < count {
            // A firefly's slot: filled within about a second, not all on
            // one frame.
            if xr_rand3(idx, 11u).x < XR_FLY_SPAWN_HZ * u.delta_time {
                write_particle(idx, xr_fly_spawn(idx, half));
                mark_alive(idx);
            } else {
                write_particle(idx, p);
            }
        } else if emit_claim() < burst_count {
            write_particle(idx, xr_burst(idx));
            mark_alive(idx);
        } else {
            write_particle(idx, p);
        }
        return;
    }

    // Sim cut: the floor, the read, the write-back and the atomic alone.
    if (cut & XR_CUT_PATH) != 0u {
        write_particle(idx, p);
        mark_alive(idx);
        return;
    }

    if life < XR_FREE - 0.5 {
        xr_fly_step(idx, p, half, count, cut);
        return;
    }

    // A pool with few dead hand slots takes the rest of a burst from the
    // living hand particles, the old ones first.
    if burst_count > 0u && instruments.w > 0.0 {
        let spent = clamp(p.flags.x / max(p.flags.y, 1e-3), 0.0, 1.0);
        if xr_rand3(idx, 8u).x < instruments.w * 2.0 * spent {
            write_particle(idx, xr_burst(idx));
            mark_alive(idx);
            return;
        }
    }
    xr_hand_step(idx, p, cut);
}
