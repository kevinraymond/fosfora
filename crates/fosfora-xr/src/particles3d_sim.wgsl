// S5 test sim: curl-noise flow in a cube, every slot always alive.
// S7 adds obstacles: spheres (hand joints) and oriented boxes (scene planes
// and volumes, the floor) that particles are pushed out of and bounce off.

const MAX_SPHERES: u32 = 64u;
const MAX_BOXES: u32 = 32u;

struct Obstacles {
    sphere_count: u32,
    box_count: u32,
    // Velocity kept along the normal after a bounce (0 = stick, 1 = elastic)
    // and the clearance added around every obstacle (meters).
    restitution: f32,
    margin: f32,
    // xyz center, w radius.
    spheres: array<vec4<f32>, 64>,
    // Boxes: center xyz; rotation as a quaternion (box -> world); half extents.
    box_center: array<vec4<f32>, 32>,
    box_rot: array<vec4<f32>, 32>,
    box_half: array<vec4<f32>, 32>,
}

@group(0) @binding(2) var<uniform> obstacles: Obstacles;

fn quat_rotate(q: vec4<f32>, v: vec3<f32>) -> vec3<f32> {
    return v + 2.0 * cross(q.xyz, cross(q.xyz, v) + q.w * v);
}

fn quat_conj(q: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(-q.xyz, q.w);
}

// sign() that never returns 0, so a particle exactly on a box's mid-plane
// still picks a side.
fn side(x: f32) -> f32 {
    return select(-1.0, 1.0, x >= 0.0);
}

// Push `p` out of every obstacle it is inside and reflect the inward part of
// its velocity. One pass per frame is enough at these speeds; a particle
// deep inside a box (spawned there) exits through the nearest face.
fn collide(p: ptr<function, Particle>) {
    let margin = obstacles.margin;
    let restitution = obstacles.restitution;
    for (var k = 0u; k < min(obstacles.sphere_count, MAX_SPHERES); k++) {
        let s = obstacles.spheres[k];
        let d = (*p).pos - s.xyz;
        let r = s.w + margin;
        let dist2 = dot(d, d);
        if dist2 < r * r {
            let dist = max(sqrt(dist2), 1e-4);
            let n = d / dist;
            (*p).pos = s.xyz + n * r;
            let vn = dot((*p).vel, n);
            if vn < 0.0 {
                (*p).vel -= (1.0 + restitution) * vn * n;
            }
        }
    }
    for (var k = 0u; k < min(obstacles.box_count, MAX_BOXES); k++) {
        let c = obstacles.box_center[k].xyz;
        let q = obstacles.box_rot[k];
        let h = obstacles.box_half[k].xyz + vec3<f32>(margin);
        let l = quat_rotate(quat_conj(q), (*p).pos - c);
        let pen = h - abs(l);
        if pen.x > 0.0 && pen.y > 0.0 && pen.z > 0.0 {
            var n_local = vec3<f32>(0.0);
            var l2 = l;
            if pen.z <= pen.x && pen.z <= pen.y {
                n_local.z = side(l.z);
                l2.z = n_local.z * h.z;
            } else if pen.x <= pen.y {
                n_local.x = side(l.x);
                l2.x = n_local.x * h.x;
            } else {
                n_local.y = side(l.y);
                l2.y = n_local.y * h.y;
            }
            (*p).pos = c + quat_rotate(q, l2);
            let n = quat_rotate(q, n_local);
            let vn = dot((*p).vel, n);
            if vn < 0.0 {
                (*p).vel -= (1.0 + restitution) * vn * n;
            }
        }
    }
}

// ---- hashing and value noise --------------------------------------------

fn hash31(p: vec3<f32>) -> f32 {
    var q = fract(p * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}

fn hash13(n: f32) -> vec3<f32> {
    let p = fract(vec3<f32>(n) * vec3<f32>(0.1031, 0.1030, 0.0973));
    let q = p + dot(p, p.yxz + 33.33);
    return fract((q.xxy + q.yxx) * q.zyx);
}

// Divergence-free flow: the analytic curl of a vector potential made of two
// sine octaves per component, so particles swirl without piling up and the
// field costs a few transcendentals per particle (a ported sim samples the
// core's flow-field texture instead; this stands in for that cost).
fn curl(p: vec3<f32>) -> vec3<f32> {
    let t = sim.time * 0.3;
    // Frequency vectors (1/m) and phases for potential components x, y, z.
    let wx = vec3<f32>(1.7, 2.3, 1.1);
    let wy = vec3<f32>(2.1, 1.3, 1.9);
    let wz = vec3<f32>(1.2, 2.7, 1.5);
    let cx = cos(dot(wx, p) + t);
    let cy = cos(dot(wy, p) - t * 0.8);
    let cz = cos(dot(wz, p) + t * 0.6);
    // Second octave, finer and weaker.
    let vx = wx.zxy * 2.9;
    let vy = wy.yzx * 3.1;
    let vz = wz.xzy * 2.7;
    let dx = 0.4 * cos(dot(vx, p) - t * 1.3);
    let dy = 0.4 * cos(dot(vy, p) + t * 1.1);
    let dz = 0.4 * cos(dot(vz, p) - t * 0.9);
    // d(psi_i)/d(p_j) = w_i.j * cos(...); curl assembles the cross terms.
    let dpsix = wx * cx + vx * dx;
    let dpsiy = wy * cy + vy * dy;
    let dpsiz = wz * cz + vz * dz;
    return vec3<f32>(
        dpsiz.y - dpsiy.z,
        dpsix.z - dpsiz.x,
        dpsiy.x - dpsix.y,
    ) * 0.25;
}

fn spawn(i: u32, salt: f32) -> Particle {
    let r = hash13(f32(i) * 0.6180339 + salt);
    var p: Particle;
    p.pos = sim.cube_center + (r * 2.0 - 1.0) * sim.cube_half;
    p.life = sim.lifetime * (0.3 + 0.7 * hash31(r + salt));
    p.vel = vec3<f32>(0.0);
    p.seed = r.x;
    return p;
}

@compute @workgroup_size(256)
fn cs_step(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if i >= sim.count {
        return;
    }
    var p = particles[i];
    // First frame (or a count change): buffers start zeroed; life 0 spawns.
    if p.life <= 0.0 {
        p = spawn(i, sim.time);
    }
    let flow = curl((p.pos - sim.cube_center) * sim.flow_scale);
    // Blend toward the field rather than snapping, for smooth trails.
    // Gravity is a steady downward drift folded into the target velocity:
    // an acceleration would be damped away by the blend (10 % per frame
    // leaves g*dt/0.1, a few cm/s), and a drift is what settles particles
    // onto real surfaces at a visible rate.
    p.vel = mix(p.vel, flow * sim.speed - vec3<f32>(0.0, sim.gravity, 0.0), 0.1);
    p.pos += p.vel * sim.dt;
    collide(&p);
    p.life -= sim.dt;
    let d = abs(p.pos - sim.cube_center);
    if p.life <= 0.0 || max(d.x, max(d.y, d.z)) > sim.cube_half {
        p = spawn(i, sim.time + 7.0);
    }
    particles[i] = p;
}
