// S5 test sim: curl-noise flow in a cube, every slot always alive.

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
    p.vel = mix(p.vel, flow * sim.speed, 0.1);
    p.pos += p.vel * sim.dt;
    p.life -= sim.dt;
    let d = abs(p.pos - sim.cube_center);
    if p.life <= 0.0 || max(d.x, max(d.y, d.z)) > sim.cube_half {
        p = spawn(i, sim.time + 7.0);
    }
    particles[i] = p;
}
