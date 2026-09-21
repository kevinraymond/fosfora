/*! trama
{
  "name": "Edge",
  "id": "edge",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "strength",  "default": 1.5, "min": 0.0, "max": 4.0 },
    { "type": "Float", "name": "thickness", "default": 1.0, "min": 0.5, "max": 8.0 },
    { "type": "Float", "name": "fill",      "default": 0.0, "min": 0.0, "max": 1.0 }
  ]
}
*/
// Edge — a Sobel outline of the picture: bright where brightness changes,
// black where it does not. `thickness` is the tap spacing in pixels, so
// widening it finds coarser structure rather than blurring the line. `fill`
// mixes the original back underneath, which keeps the subject and adds the
// outline on top instead of replacing it.
//
// The outline is drawn as opaque light, so an edge found on a transparent
// part of the input still draws. Put a Key after this to cut the black back
// out again.

const EDGE_LUMA = vec3f(0.2126, 0.7152, 0.0722);

fn edge_luma(uv: vec2f) -> f32 {
    return dot(input0(clamp(uv, vec2f(0.0), vec2f(1.0))).rgb, EDGE_LUMA);
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let d = param(1u) / res;

    let tl = edge_luma(uv + vec2f(-d.x, -d.y));
    let tc = edge_luma(uv + vec2f(0.0, -d.y));
    let tr = edge_luma(uv + vec2f(d.x, -d.y));
    let ml = edge_luma(uv + vec2f(-d.x, 0.0));
    let mr = edge_luma(uv + vec2f(d.x, 0.0));
    let bl = edge_luma(uv + vec2f(-d.x, d.y));
    let bc = edge_luma(uv + vec2f(0.0, d.y));
    let br = edge_luma(uv + vec2f(d.x, d.y));

    let gx = (tr + 2.0 * mr + br) - (tl + 2.0 * ml + bl);
    let gy = (bl + 2.0 * bc + br) - (tl + 2.0 * tc + tr);
    let e = clamp(length(vec2f(gx, gy)) * param(0u), 0.0, 1.0);

    let c = input0(uv);
    let rgb = vec3f(e) + c.rgb * param(2u);
    let a = max(e, c.a * param(2u));
    return vec4f(rgb, a);
}
