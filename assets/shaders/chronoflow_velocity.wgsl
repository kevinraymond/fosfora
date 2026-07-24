// Chronoflow (#1482) — self-advecting screen-space velocity field.
//   feedback() = own previous frame: uv-space velocity in .xy
//   input0     = @particles.velocity: (vx, vy, coverage), velocity in NDC/s,
//                resolved by the compute rasterizer this same frame
// Where particles cover a pixel their velocity is stamped into the field;
// everywhere else the field advects itself and decays. A streak therefore
// keeps flowing along the path the particle took even after it has moved on —
// this is what makes trails follow curved motion instead of smearing radially.
// Runs at scale 0.5; shared verbatim by every Chronoflow effect (no params).

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(prev_frame));
    let uv = frag_coord.xy / dims;
    let dt = clamp(u.delta_time, 1e-4, 0.05);

    // Semi-Lagrangian self-advection with per-second decay, so stale motion
    // fades once the trail image it carried has decayed too.
    let v_here = feedback(uv).xy;
    let back = uv - v_here * dt;
    let field_keep = pow(0.90, dt * 60.0);
    var v = feedback(back).xy * field_keep;

    // Stamp fresh particle velocity where particles claim the pixel.
    let p = input0(uv);
    let pv = chrono_uv_vel(p.xy);
    let cov = clamp(p.z * 3.0, 0.0, 1.0);
    v = mix(v, pv, cov);

    return vec4f(v, 0.0, 1.0);
}
