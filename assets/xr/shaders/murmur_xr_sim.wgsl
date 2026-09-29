// Murmur, world-space variant for the Quest build (hidden preset
// assets/xr/effects/murmur_xr_world.pfx). The same murmuration as
// murmur_sim.wgsl (topological K nearest neighbors, Vicsek noise, adaptive
// separation, a predator, roost centering), but in meters around the wearer,
// with the wearer's hands as predators and the room as obstacles, drawn by
// the world-space sprite path (ParticleSystem::render_world,
// builtin/particle_render_world.wgsl) once per eye with the alpha pipeline:
// dark birds over passthrough read as silhouettes against the real room.
// Structs, bindings and helpers are in particle_lib.wgsl (auto-prepended).
//
// --- Param mapping (as murmur_sim.wgsl) ---
// param(0) = noise_eta     (Vicsek noise baseline: 0=order, 1=chaos)
// param(1) = cohesion      (cohesion weight)
// param(2) = color_mode    (0=silhouette, 1=rim lighting)
// param(3) = predator      (predator avoidance strength, hands and audio)
// param(4) = separation    (separation weight)
// param(5) = speed         (base flock cruise speed)
// param(6) = smoothing     (heading low-pass filter)
// param(7) = audio_drive   (master audio reactivity scaling)
//
// World layout (particle_lib.wgsl, "World layout"):
//   pos_life = xyz position in meters relative to the effect anchor
//              (+Y up, -Z forward), w life (1 alive, 0 dead)
//   vel_size = xyz velocity in m/s, w sprite radius in meters
//   color    = rgb straight color, a opacity (alpha blend: the eye target
//              receives it over alpha 0, a premultiplied layer over
//              passthrough)
//   flags    = x age, y lifetime, z initial radius in meters (pos_life.z in
//              the 2D sim), w birth time (as in the 2D sim)
//
// Heading is the velocity direction. The 2D sim stores a heading angle in
// flags.z; a direction in 3D needs two angles and a stored pair would drift
// from the velocity it describes, so every steering term (alignment,
// cohesion, separation, predators, roost, bounds) goes into target_vel, the
// Vicsek noise tilts its direction, and a per-frame low-pass (a normalized
// blend, at frame_diffuse's rate) turns the current direction toward it.
// Collisions change the velocity, and with it the heading the next frame
// starts from, so a bird that meets a table turns along it.
//
// Volume: the cube within +-emitter.radius meters of the anchor (the preset's
// `emitter.radius`, read as u.emitter_radius), the same 3 m cube as Flux XR
// World. Distances the 2D sim gives in clip units (its screen spans +-1) are
// scaled by the half extent here, so the flock keeps its proportions.
//
// Per-frame XR inputs ride the aux buffer, which Murmur does not otherwise
// use, in the layout flux_xr_sim.wgsl documents (head + near-fade radius,
// obstacle header, 64 hand-joint spheres, 32 room boxes). The settle drift
// lane (aux[2].z) is ignored: birds do not settle; aux[2].w calms the
// hands' scare (0 = full scare, 1 = the hands only push; stored as calm so
// a caller that never writes it keeps the hawk). All zero (nothing written
// yet, or a desktop test) means no obstacles and no near fade.
//
// Per-hand lanes follow the obstacle block (board #3314: the hand's pose
// picks its behavior; the app reads the pose). Every lane is stored so that
// 0 is the behavior before the lanes existed:
//   aux[163]            x = the left hand's sphere count (u32 bits): spheres
//                       [0, x) are the left hand's, the rest the right's
//   aux[164 + 3h]       hand h (0 left, 1 right): x = calm (1 - scare, times
//                       aux[2].w's), y = still (>= 0.5: its spheres do not
//                       push), z = kick calm (its kick is aux[2].y * (1 - z)),
//                       w = hold strength 0..1 (0 = no hold)
//   aux[165 + 3h]       the hold's center xyz, w radius (meters)
//   aux[166 + 3h]       the hold's velocity xyz (m/s): the birds it holds
//                       move with it; w = seconds since the hold began
//   aux[170..173]       Flux's instrument rows (the throw's burst and the
//                       palm lift, flux_xr_sim.wgsl); ignored here: the
//                       app uploads them for every world effect
// A hold latches the birds inside its radius during its first
// HOLD_CAPTURE_S, keeps them inside and carries them with the hand, and
// lets them go when it ends (or when one falls HOLD_LOSE radii behind). The
// heading is rebuilt every frame at cruise speed, so a contact alone only
// parts the flock (MEASURED.md, "Why the hands never scoop"). The latch is
// what makes it part of the flock: a hold open to every bird that crosses
// it is a trap that cohesion keeps filling (a test hold at the roost took
// 70-80 % of the flock in a second). A bird's holder rides in its life
// lane, which every other reader tests only against 0: pos_life.w = 1 for a
// free bird, 2 + h for one held by hand h.

const PI: f32 = 3.1415927;

// ---- Tuning (device sweep knobs) ----------------------------------------------

// Topological neighbors per bird (K=7 in the starling studies).
const K: u32 = 7u;
// Candidates read per cell. The scan covers up to 27 cells, so a bird reads
// at most 27 * MAX_PER_CELL neighbors per frame: the sim's main cost.
const MAX_PER_CELL: u32 = 16u;

// Roost: the flock's home, relative to the anchor, free within ROOST_FREE_M.
// The app centers the anchor on the wearer's head (x, z) at 1 m, and a
// pinch-drag moves it, so the roost sits 1 m ahead of the anchor (-Z, the
// stage's forward) and 1.4 m up: in front of a standing wearer, not at the
// eyes. A roost at the head packed the flock around the face (review of the
// port); the head is also a hawk below, so the flock keeps its distance.
const ROOST_OFFSET_M: vec3f = vec3f(0.0, 0.4, -1.0);
const ROOST_FREE_M: f32 = 0.5;
// Bounds: soft repulsion within BOUNDS_BAND_M of the cube's faces, rising to
// BOUNDS_WEIGHT (target direction units) at the face.
const BOUNDS_BAND_M: f32 = 0.3;
const BOUNDS_WEIGHT: f32 = 4.0;
// Audio predator reach in half extents (0.28 of the 2D sim's screen).
const PREDATOR_RADIUS: f32 = 0.28;
// Hand predator reach from each joint sphere's center.
const HAND_RADIUS_M: f32 = 0.4;
// The wearer's head (aux[0].xyz) is a hawk too, with a wider reach: birds
// must not fly through the face, and the head is not in the obstacle block.
const HEAD_RADIUS_M: f32 = 0.6;
// Holds: a hold latches the free birds inside its radius during its first
// HOLD_CAPTURE_S seconds; a held bird is steered back inward past HOLD_CORE
// of the radius (HOLD_STEER, in target direction units: alignment is 1),
// pulled in at HOLD_SPRING meters per second per meter off the core, carried
// with the hold's velocity, and let go HOLD_LOSE radii from the center.
const HOLD_CAPTURE_S: f32 = 0.15;
const HOLD_CORE: f32 = 0.4;
const HOLD_STEER: f32 = 4.0;
const HOLD_SPRING: f32 = 2.0;
const HOLD_LOSE: f32 = 2.0;

// ---- XR inputs (aux layout: flux_xr_sim.wgsl) ----------------------------------

const XR_AUX_HEAD: u32 = 0u;
const XR_AUX_HEADER: u32 = 1u;
const XR_MAX_SPHERES: u32 = 64u;
const XR_MAX_BOXES: u32 = 32u;
const XR_AUX_SPHERES: u32 = 3u;
const XR_AUX_BOX_CENTER: u32 = 67u;   // XR_AUX_SPHERES + XR_MAX_SPHERES
const XR_AUX_BOX_ROT: u32 = 99u;      // + XR_MAX_BOXES
const XR_AUX_BOX_HALF: u32 = 131u;    // + XR_MAX_BOXES
const XR_AUX_HANDS: u32 = 163u;       // + XR_MAX_BOXES
const XR_AUX_HAND_ROWS: u32 = 3u;
const XR_AUX_END: u32 = 170u;         // XR_AUX_HANDS + 1 + 2 * XR_AUX_HAND_ROWS
// (Flux's instrument rows follow, 170..173; unread here.)

// Fraction of the half extent over which opacity fades out toward the bounds.
const XR_EDGE_FADE: f32 = 0.3;

// The hand joint sphere k belongs to (0 left, 1 right).
fn xr_sphere_hand(k: u32) -> u32 {
    return select(1u, 0u, k < bitcast<u32>(aux[XR_AUX_HANDS].home.x));
}

// Hand h's behavior lanes: calm, still, kick calm, hold strength.
fn xr_hand_lanes(h: u32) -> vec4f {
    return aux[XR_AUX_HANDS + 1u + h * XR_AUX_HAND_ROWS].home;
}

// ---- random -------------------------------------------------------------------

// Three uniform [0, 1) values per (particle, salt), from the integer hash. The
// 2D sim's fract-sin `rand_vec2` bands at large seeds (see its note in
// particle_lib.wgsl); in 3D the same defect lines the components up and
// spawns collapse onto a diagonal.
fn xr_rand3(idx: u32, salt: u32) -> vec3f {
    let s = uhash(idx ^ uhash(bitcast<u32>(u.seed) + salt));
    return vec3f(uhash_f(s), uhash_f(s ^ 0x9e3779b9u), uhash_f(s ^ 0x85ebca6bu));
}

// Uniform direction on the unit sphere from two uniform [0, 1) values.
fn xr_sphere_dir(a: f32, b: f32) -> vec3f {
    let z = a * 2.0 - 1.0;
    let ring = sqrt(max(1.0 - z * z, 0.0));
    let angle = b * 6.2831853;
    return vec3f(ring * cos(angle), z, ring * sin(angle));
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
        let lanes = xr_hand_lanes(xr_sphere_hand(k));
        if lanes.y >= 0.5 {
            continue;
        }
        let kick = sphere_kick * (1.0 - lanes.z);
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
            *vel += n * max(kick - outward, 0.0);
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

// ============================================================
// Mapped params — all audio-modulated in cs_main
// ============================================================

fn noise_eta_base() -> f32 { return param(0u); }
// Alignment ~1.0 (normalized), cohesion ~0.3, separation ~1.0
// Adaptive topological separation prevents collapse at higher cohesion
fn cohesion_weight() -> f32 { return mix(0.1, 0.6, param(1u)); }
fn predator_strength() -> f32 { return mix(0.0, 5.0, param(3u)); }
fn separation_weight() -> f32 { return mix(0.3, 1.8, param(4u)); }
// Half extents per second (the 2D sim's clip units per second).
fn cruise_speed() -> f32 { return mix(0.04, 0.25, param(5u)); }
fn heading_smoothing() -> f32 { return mix(0.05, 0.35, param(6u)); }
fn audio_drive() -> f32 { return param(7u); }

// The unit vector along v, or `fallback` when v is (nearly) zero.
fn xr_dir_or(v: vec3f, fallback: vec3f) -> vec3f {
    let len2 = dot(v, v);
    return select(fallback, v * inverseSqrt(len2), len2 > 1e-12);
}

// ============================================================
// Audio predator — deterministic from uniforms
// ============================================================

// A slow 3D Lissajous around the roost; an onset or kick throws it to a
// strike point inside the flock, then it decays back to the Lissajous.
fn predator_pos(roost: vec3f, half: f32) -> vec3f {
    let liss = roost + vec3f(
        sin(u.time * 0.17) * 0.5,
        cos(u.time * 0.23) * 0.25,
        sin(u.time * 0.13 + 1.0) * 0.5
    ) * half;
    let strike_seed = u32(max(floor(u.time * 0.5 + u.onset * 3.0), 0.0));
    let s = uhash(strike_seed ^ 0x51ed270bu);
    let r = vec3f(uhash_f(s), uhash_f(s ^ 0x9e3779b9u), uhash_f(s ^ 0x85ebca6bu));
    let strike = roost + (r * 2.0 - 1.0) * vec3f(0.4, 0.2, 0.4) * half;
    let onset_hold = smoothstep(0.0, 0.4, u.onset) * 0.7 + smoothstep(0.0, 0.3, u.kick) * 0.3;
    return mix(liss, strike, onset_hold);
}

fn predator_intensity() -> f32 {
    // mfcc(3u) = formant structure: vocal-like sounds make predator more aggressive
    let formant_boost = 1.0 + max(mfcc(3u), 0.0) * 1.5;
    return max(u.onset * 2.0, u.kick * 1.5) * formant_boost;
}

// ============================================================
// Emission
// ============================================================

fn emit_particle(idx: u32, half: f32, roost: vec3f) -> Particle {
    var p: Particle;
    let r = xr_rand3(idx, 0u);
    let s = xr_rand3(idx, 1u);
    let t = xr_rand3(idx, 2u);
    let probe = xr_rand3(idx, 3u);
    let jitter = xr_rand3(idx, 4u);

    // Fallback: uniform in the roost's free ball, random heading.
    let fallback_dir = xr_sphere_dir(r.x, r.y);
    var spawn_pos = roost + fallback_dir * pow(r.z, 1.0 / 3.0) * ROOST_FREE_M;
    var heading = xr_sphere_dir(s.x, s.y);

    // Emit near an existing alive bird, so the flock grows where it is and a
    // new bird flies with it at once. The donor is a uniformly random alive
    // bird: the hash's sorted index list holds every alive bird packed from
    // 0 to the last cell's end, so one lookup finds one wherever the flock
    // went. A probe around the roost (the first port) refilled the roost
    // with a second flock whenever the first had fled a hand.
    let last = SH_GRID_D * SH_GRID_D * SH_GRID_D - 1u;
    let alive = sh_cell_offsets[last] + sh_cell_counts[last];
    if alive > 0u {
        // Integer hash: which bird a new one is seeded from decides where the
        // flock grows (see the 2D sim).
        let pick = uhash(idx + uhash(u32(u.seed * 4096.0) ^ bitcast<u32>(probe.x))) % alive;
        let donor = sh_sorted_indices[pick];
        let donor_pl = pos_life_in[donor];
        if donor_pl.w > 0.0 {
            spawn_pos = donor_pl.xyz + (jitter * 2.0 - 1.0) * 0.02 * half;
            heading = xr_dir_or(vel_size_in[donor].xyz, heading);
        }
    }

    // Small jitter on the inherited heading (the 2D sim's +-0.15 rad).
    heading = xr_dir_or(heading + (xr_rand3(idx, 5u) * 2.0 - 1.0) * 0.15, heading);
    let speed = cruise_speed() * half * (0.8 + 0.4 * s.z);

    // Dark bird silhouette colors via gradient or default
    var col: vec3f;
    if u.gradient_count > 0u {
        col = eval_color_gradient(t.x).rgb;
    } else {
        col = vec3f(0.04 + t.x * 0.03);
    }

    let initial_age = t.y * u.lifetime * 0.5;
    let init_size = u.initial_size * (0.7 + t.z * 0.6);

    p.pos_life = vec4f(spawn_pos, 1.0);
    p.vel_size = vec4f(heading * speed, init_size);
    // Invisible until its first update sets the opacity, by which time a bird
    // spawned inside an obstacle has been pushed out of it.
    p.color = vec4f(col, 0.0);
    p.flags = vec4f(initial_age, u.lifetime, init_size, u.time);  // w = birth time
    return p;
}

// ============================================================
// Main simulation
// ============================================================

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles {
        return;
    }
    let half = max(u.emitter_radius, 0.05);
    let roost = ROOST_OFFSET_M;

    var p = read_particle(idx);
    let life = p.pos_life.w;
    let age = p.flags.x;
    let max_life = p.flags.y;

    // --- Dead/emit preamble ---
    if life <= 0.0 {
        let slot = emit_claim();
        if slot < u.emit_count {
            p = emit_particle(idx, half, roost);
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
    var pos = p.pos_life.xyz;
    let heading = xr_dir_or(p.vel_size.xyz, vec3f(0.0, 0.0, -1.0));

    // --- Read all params + audio modulation ---
    let drive = audio_drive();
    let sep_w = separation_weight();
    // mfcc(1u) = spectral tilt: bright timbre tightens flock, dark loosens
    let timbre_coh = 1.0 + (mfcc(1u) * 0.5 + u.mid * 0.5) * drive * 1.5;
    let coh_w = cohesion_weight() * timbre_coh;
    let heading_smooth = heading_smoothing();

    // Vicsek noise: eta sweeps order→chaos with bass
    let eta = clamp(noise_eta_base() * PI + u.bass * drive * PI, 0.0, PI);

    let pred_str = predator_strength();

    // --- K nearest topological neighbors over the 27 cells around the bird ---
    // Center cell first (index 13 of the 3x3x3 block swaps with 0), so the
    // closest candidates fill the list early. A cell whose nearest point is
    // farther than the current K-th neighbor cannot improve the list and is
    // skipped: in a dense flock most of the 26 outer cells are.
    let my_cell = sh_pos_to_cell_3d(pos, u.emitter_radius);
    let extent = max(u.emitter_radius, 1e-3);
    let cell_m = 2.0 * extent / f32(SH_GRID_D);

    var knn_dist: array<f32, K>;
    var knn_idx: array<u32, K>;
    for (var k = 0u; k < K; k++) {
        knn_dist[k] = 999.0;
        knn_idx[k] = 0u;
    }
    var knn_count: u32 = 0u;

    for (var ci = 0u; ci < 27u; ci++) {
        let b = select(select(ci, 0u, ci == 13u), 13u, ci == 0u);
        let cell = my_cell + vec3i(i32(b % 3u), i32((b / 3u) % 3u), i32(b / 9u)) - vec3i(1);
        if knn_count == K {
            let lo = vec3f(cell) * cell_m - extent;
            let gap = max(max(lo - pos, pos - (lo + cell_m)), vec3f(0.0));
            let worst = knn_dist[K - 1u];
            if dot(gap, gap) >= worst * worst {
                continue;
            }
        }
        let range = sh_cell_range_3d(cell);
        let start = range.x;
        let total_in_cell = range.y;
        let count = min(total_in_cell, MAX_PER_CELL);

        // Randomized start within the cell, so the cap drops different
        // candidates each frame instead of the same ones (integer hash).
        let scan_offset = uhash(idx + uhash(ci * 0x9e3779b9u)) % max(total_in_cell, 1u);

        for (var i = 0u; i < count; i++) {
            let ni = sh_sorted_indices[start + ((i + scan_offset) % total_in_cell)];
            if ni == idx { continue; }

            let n_pl = pos_life_in[ni];
            if n_pl.w <= 0.0 { continue; }

            let diff = pos - n_pl.xyz;
            let dist_sq = dot(diff, diff);
            if dist_sq < 0.0000001 { continue; }

            let dist = sqrt(dist_sq);

            // K-nearest insertion sort
            let worst_k = select(K - 1u, knn_count, knn_count < K);
            if knn_count < K || dist < knn_dist[worst_k] {
                var ins = knn_count;
                if ins >= K { ins = K - 1u; }

                // Shift down: find where this distance slots in
                loop {
                    if ins < 1u { break; }
                    if knn_dist[ins - 1u] <= dist { break; }
                    if ins < K {
                        knn_dist[ins] = knn_dist[ins - 1u];
                        knn_idx[ins] = knn_idx[ins - 1u];
                    }
                    ins--;
                }

                if ins < K {
                    knn_dist[ins] = dist;
                    knn_idx[ins] = ni;
                }
                if knn_count < K {
                    knn_count++;
                }
            }
        }
    }

    // --- All three forces from the K neighbors (fully topological) ---

    // Alignment: normalized mean of the neighbors' flight directions and the
    // bird's own (Vicsek-style, magnitude 1.0).
    var align_vec = heading;
    for (var k = 0u; k < knn_count; k++) {
        align_vec += xr_dir_or(vel_size_in[knn_idx[k]].xyz, vec3f(0.0));
    }
    var target_vel = xr_dir_or(align_vec, heading);

    var edge_factor = 0.0;
    if knn_count > 0u {
        // Cohesion: steer toward the neighbors' center of mass
        var com = vec3f(0.0);
        for (var k = 0u; k < knn_count; k++) {
            com += pos_life_in[knn_idx[k]].xyz;
        }
        com /= f32(knn_count);
        let to_com = com - pos;
        let com_dist = length(to_com);
        if com_dist > 0.001 * half {
            target_vel += to_com / com_dist * coh_w * min(com_dist / half * 20.0, 1.0);
        }

        // Separation: adaptive from the K-th neighbor distance (density-invariant)
        let k_radius = knn_dist[knn_count - 1u];
        // Separation threshold: closest ~40% of KNN range
        let sep_threshold = k_radius * (0.4 + (u.presence + u.brilliance) * 0.15 * drive);
        var sep_force = vec3f(0.0);
        for (var k = 0u; k < knn_count; k++) {
            if knn_dist[k] < sep_threshold {
                let n_pos = pos_life_in[knn_idx[k]].xyz;
                let away = pos - n_pos;
                // Inverse-distance: stronger when closer (1/d scaling, normalized by threshold)
                sep_force += normalize(away) * (sep_threshold / knn_dist[k] - 1.0);
            }
        }
        let sep_len = length(sep_force);
        if sep_len > 0.001 {
            target_vel += sep_force / sep_len * sep_w;
        }

        // Edge detection: anisotropy of the KNN neighborhood
        // High com_dist/k_radius = neighbors only on one side = flock edge
        edge_factor = smoothstep(0.35, 0.75, com_dist / k_radius);
    }

    // --- Predators: the wearer's hands ---
    // Every hand-joint sphere repels within HAND_RADIUS_M of its center at a
    // steady intensity of its hand's scare (a fist is a hawk; an open or
    // holding hand is calm). The joints act as one predator: the flight
    // direction is the falloff-weighted sum of the directions away from each
    // joint, the strength the nearest joint's falloff, so a hand counts once
    // however many joints it tracks.
    let sphere_count = min(bitcast<u32>(aux[XR_AUX_HEADER].home.x), XR_MAX_SPHERES);
    let head_row = aux[XR_AUX_HEAD].home;
    // The app's hand calm (aux[2].w): 1 leaves the hands only their push
    // (collide), 0 the full scare; the head keeps its own scare.
    let hand_scare = 1.0 - clamp(aux[XR_AUX_HEADER + 1u].home.w, 0.0, 1.0);
    if pred_str > 0.01 {
        var flee = vec3f(0.0);
        var threat = 0.0;
        for (var k = 0u; k < sphere_count; k++) {
            let away = pos - aux[XR_AUX_SPHERES + k].home.xyz;
            let dist2 = dot(away, away);
            if dist2 < HAND_RADIUS_M * HAND_RADIUS_M && dist2 > 1e-8 {
                let dist = sqrt(dist2);
                let calm = clamp(xr_hand_lanes(xr_sphere_hand(k)).x, 0.0, 1.0);
                let falloff = (1.0 - smoothstep(0.0, HAND_RADIUS_M, dist)) * hand_scare * (1.0 - calm);
                flee += away / dist * falloff;
                threat = max(threat, falloff);
            }
        }
        // The head, when the app has written it (w = near-fade radius > 0).
        if head_row.w > 0.0 {
            let away = pos - head_row.xyz;
            let dist2 = dot(away, away);
            if dist2 < HEAD_RADIUS_M * HEAD_RADIUS_M && dist2 > 1e-8 {
                let dist = sqrt(dist2);
                let falloff = 1.0 - smoothstep(0.0, HEAD_RADIUS_M, dist);
                flee += away / dist * falloff;
                threat = max(threat, falloff);
            }
        }
        if threat > 0.0 {
            target_vel += xr_dir_or(flee, vec3f(0.0)) * pred_str * threat;
        }
    }

    // --- Predator: audio (onset / kick), as in the 2D sim ---
    let pred_intensity = predator_intensity();
    if pred_str > 0.01 && pred_intensity > 0.05 {
        let pred_radius = PREDATOR_RADIUS * half;
        let to_bird = pos - predator_pos(roost, half);
        let pred_dist = length(to_bird);
        if pred_dist < pred_radius && pred_dist > 0.001 {
            let falloff = exp(-pred_dist * pred_dist / (pred_radius * pred_radius * 0.15));
            target_vel += to_bird / pred_dist * pred_str * pred_intensity * falloff;
        }
    }

    // --- Holds: latch, contain and carry (see the aux layout above). The
    // steering keeps a held bird milling inside the radius with its
    // neighbors; the spring and the hold's velocity go straight into the
    // velocity, so the group moves with the hand however fast it goes.
    // `holder` is 0 for a free bird, 1 + h for one held by hand h.
    var holder = u32(clamp(life - 0.5, 0.0, 2.0));
    for (var h = 0u; h < 2u; h++) {
        let strength = clamp(xr_hand_lanes(h).w, 0.0, 1.0);
        let base = XR_AUX_HANDS + 1u + h * XR_AUX_HAND_ROWS;
        let hold = aux[base + 1u].home;
        let d = distance(hold.xyz, pos);
        let radius = max(hold.w, 0.02);
        if holder == 1u + h && (strength <= 0.0 || d > radius * HOLD_LOSE) {
            holder = 0u;
        } else if holder == 0u && strength > 0.0 && d < radius
            && aux[base + 2u].home.w < HOLD_CAPTURE_S {
            holder = 1u + h;
        }
    }
    var carry = vec3f(0.0);
    var held = 0.0;
    if holder > 0u {
        let h = holder - 1u;
        let base = XR_AUX_HANDS + 1u + h * XR_AUX_HAND_ROWS;
        let strength = clamp(xr_hand_lanes(h).w, 0.0, 1.0);
        let hold = aux[base + 1u].home;
        let radius = max(hold.w, 0.02);
        let to_c = hold.xyz - pos;
        let d = length(to_c);
        let rim = smoothstep(radius * HOLD_CORE, radius, d);
        let inward = to_c / max(d, 1e-4);
        target_vel += inward * HOLD_STEER * strength * rim;
        carry = (aux[base + 2u].home.xyz + inward * d * HOLD_SPRING * rim) * strength;
        held = strength;
    }

    // --- Roost centering: steers the heading home, quadratic beyond the free
    // radius (in half extents, as the 2D sim's clip units). In target_vel so
    // alignment consensus cannot overpower it. A held bird goes where the
    // hand takes it.
    let to_roost = roost - pos;
    let roost_dist = length(to_roost);
    if roost_dist > ROOST_FREE_M {
        let excess = (roost_dist - ROOST_FREE_M) / half;
        target_vel += to_roost / roost_dist * excess * excess * 3.0 * (1.0 - held);
    }

    // --- Bounds: soft repulsion from each face within BOUNDS_BAND_M (the 2D
    // edge term per axis), in target_vel so the flock turns before it.
    let over = max(abs(pos) - vec3f(half - BOUNDS_BAND_M), vec3f(0.0)) / BOUNDS_BAND_M;
    target_vel -= sign(pos) * over * BOUNDS_WEIGHT;

    // --- Vicsek noise: tilt the target direction by an angle up to eta about
    // a random axis (the 2D sim's uniform angle in [-eta, eta]).
    let target_dir = xr_dir_or(target_vel, heading);
    let n = xr_rand3(idx, 6u);
    let r = xr_sphere_dir(n.x, n.y);
    let side = xr_dir_or(r - dot(r, target_dir) * target_dir, vec3f(0.0));
    let tilt = n.z * eta;
    let noisy_dir = xr_dir_or(target_dir * cos(tilt) + side * sin(tilt), target_dir);

    // --- Heading low-pass on the unit vector: a normalized blend. As in the
    // 2D sim, heading_smooth is a per-FRAME weight, so frame_diffuse keeps
    // the turn rate per second the same at any display rate. An exact
    // reversal (a zero blend) keeps the current heading for this frame.
    let new_dir = xr_dir_or(mix(heading, noisy_dir, frame_diffuse(heading_smooth)), heading);

    // --- Speed: base * centroid_mod * flux agitation * per-bird * beat pulse ---
    // Min/max speed clamping prevents stalling (blob collapse) and runaway
    let base_spd = cruise_speed() * half;
    let centroid_mod = 1.0 + (u.centroid - 0.5) * drive * 0.8;
    // Spectral flux = timbral change rate → flock agitation
    let flux_mod = 1.0 + u.flux * drive * 0.6;
    let per_bird = 0.85 + uhash_f(idx ^ 0x2545f491u) * 0.3;
    let beat_pulse = 1.0 + sin(u.beat_phase * PI * 2.0) * 0.08;
    let speed = clamp(base_spd * centroid_mod * flux_mod * per_bird * beat_pulse, base_spd * 0.5, base_spd * 2.5);
    // A hold's carry rides on top: the next frame's heading turns with it.
    var vel = new_dir * speed + carry;

    // Integrate, then push out of hands, furniture and the floor. The
    // reflected velocity is next frame's heading: a bird that meets a table
    // slides along it and turns away.
    pos += vel * dt;
    _ = xr_collide(&pos, &vel);

    // Leaving the volume: respawn (Flux XR World does the same). Still alive,
    // so the alive count and the density stay steady.
    let edge = max(abs(pos.x), max(abs(pos.y), abs(pos.z))) / half;
    if edge > 1.0 {
        p = emit_particle(idx, half, roost);
        write_particle(idx, p);
        mark_alive(idx);
        return;
    }

    // --- Size: from the initial radius, neighbor density and rms, in meters.
    // No depth-based sizing: the eye's projection gives real depth.
    let init_size = p.flags.z;
    let density_mod = 1.0 + f32(knn_count) * 0.03;
    var size = init_size * density_mod * (1.0 + u.rms * drive * 0.2);

    // --- Alpha: opaque birds with fade in/out, toward the volume's bounds and
    // near the wearer's eyes. Evaluated from the base every frame, never
    // compounded: the bounds and near fades must recover when a bird moves back.
    // Spawn fade: based on real wall-clock age (not staggered initial_age)
    let real_age = u.time - p.flags.w;
    let spawn_fade = smoothstep(0.0, 0.5, real_age);
    let fade_out = 1.0 - smoothstep(0.85, 1.0, life_frac);
    let edge_fade = 1.0 - smoothstep(1.0 - XR_EDGE_FADE, 1.0, edge);
    var alpha = 0.9 * spawn_fade * fade_out * eval_opacity_curve(life_frac) * edge_fade;

    // Near the head a sprite a centimeter across fills the view: fade it out
    // and shrink it to nothing (zero radius rasterizes no fragments).
    let head = aux[XR_AUX_HEAD].home;
    if head.w > 0.0 {
        let near = smoothstep(head.w, head.w * 1.5, distance(pos, head.xyz));
        alpha *= near;
        size *= near;
    }

    // --- Color: dark silhouettes + rim lighting at the flock's edges ---
    // Re-derived from the gradient each frame (never the previous output).
    // The 2D sim's y-depth tint is gone: depth is real here.
    var col: vec3f;
    let color_t = uhash_f(idx ^ 0x68e31da4u);  // stable per bird
    if u.gradient_count > 0u {
        col = eval_color_gradient(color_t).rgb;
    } else {
        col = vec3f(0.04 + uhash_f(idx ^ 0x1b56c4e9u) * 0.03);
    }
    let rim_param = param(2u);  // color_mode: 0=silhouette, 1=full rim
    let rim_intensity = rim_param * (u.rms * drive * 0.6 + u.onset * 0.3);
    // Dominant chroma drives rim hue — musical key → flock edge color
    // Centroid still provides warm/cool bias as secondary influence
    let hue = u.dominant_chroma + u.centroid * 0.15;
    let rim_r = abs(fract(hue) * 6.0 - 3.0) - 1.0;
    let rim_g = 2.0 - abs(fract(hue) * 6.0 - 2.0);
    let rim_b = 2.0 - abs(fract(hue) * 6.0 - 4.0);
    let rim_color = clamp(vec3f(rim_r, rim_g, rim_b), vec3f(0.3), vec3f(1.0));
    col += rim_color * edge_factor * rim_intensity * 0.12;
    col = clamp(col, vec3f(0.0), vec3f(0.35));

    p.pos_life = vec4f(pos, 1.0 + f32(holder));
    p.vel_size = vec4f(vel, size);
    p.color = vec4f(col, alpha);
    p.flags = vec4f(new_age, max_life, init_size, p.flags.w);  // preserve birth time

    write_particle(idx, p);
    mark_alive(idx);
}
