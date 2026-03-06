// Turing — transparent background pass.
// Particles render on top via LoadOp::Load.
// Background is transparent for layer compositing.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    return vec4f(0.0, 0.0, 0.0, 0.0);
}
