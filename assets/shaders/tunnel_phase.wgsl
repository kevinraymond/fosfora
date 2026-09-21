// Tunnel — rib screw accumulator (#2984).
//
// The ribs twist with depth, `twist * z`, and z includes how far the camera has
// flown. So the rib angle on screen was `twist * distance_flown`, which grows
// for as long as the app runs: every change in twist was multiplied by the
// whole flight, and a binding on twist spun the ribs by a random amount each
// frame instead of re-twisting them. Measured on the GPU, a 0.01 nudge to twist
// at a 300 s clock moved the picture 24x as far as a frame of ordinary motion.
//
// The frame-correct quantity is the screw: the integral of twist * speed, which
// is exactly `twist * distance_flown` while both hold still. It is a product of
// two params, which the engine's `"rates"` cannot integrate (they integrate one
// param each), so this pass keeps it. The main pass adds it to the geometric
// twist `twist * z_geo`, which depends only on the current twist and so moves
// smoothly when twist does.
//
// Stored with phase_pack (lib/chronoflow.wgsl). Wrapped at TAU: the main pass
// reaches the screen through cos(ta), sin(ta) and fract(ta / TAU), all periodic
// in TAU.

const TAU: f32 = 6.2831853;

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    // Mirrors the main pass: param(0) = twist_amount, param(1) = speed.
    let twist = (param(0u) - 0.5) * 10.0;
    let speed = param(1u) * 0.8 + 0.08;

    // Every texel carries the same phase; sampling anywhere works.
    let screw = phase_unpack(feedback(vec2f(0.5, 0.5))) + twist * speed * 0.05 * u.delta_time;
    return phase_pack(screw, TAU);
}
