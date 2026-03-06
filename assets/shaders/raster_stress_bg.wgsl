// Minimal black background for raster stress test.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    return vec4f(0.02, 0.02, 0.03, 1.0);
}
