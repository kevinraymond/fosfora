// Intarsia analysis pass (1/8 scale): average the backdrop into soft color
// patches. Downsampling to this pass's resolution already box-averages;
// the extra taps widen the neighborhood so patch colors are stable regions,
// not per-pixel noise. Premultiplied in, premultiplied out.

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution * 0.125;
    let uv = frag_coord.xy / res;
    let ts = 1.5 / res;
    var acc = vec4f(0.0);
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            acc += input0(uv + vec2f(f32(x), f32(y)) * ts);
        }
    }
    return acc / 9.0;
}
