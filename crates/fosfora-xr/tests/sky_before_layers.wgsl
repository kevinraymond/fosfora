// fs_sky as it was before the sky's layers (board #5436, at 44009ce), kept
// for tests/sky_gpu.rs: with one layer the sky must draw what this drew, bit
// for bit (board #5438). Appended after surface_port::WRAPPER_WGSL, as
// sky::SKY_WGSL is. Not used by the app.
const XR_PI: f32 = 3.14159265;
const XR_TAU: f32 = 6.28318531;
// The frame's height and its bottom row's elevation (rad); the seam's
// cross-fade either side (rad of azimuth); the zenith fade's start and end
// (rad of elevation).
const XR_SKY_HEIGHT: f32 = 1.1;
const XR_SKY_HORIZON: f32 = 0.0;
const XR_SKY_SEAM: f32 = 0.2;
const XR_SKY_ZENITH: vec2<f32> = vec2<f32>(1.29, 1.47);
// The reflection's share of the sky, and the cosines of the view ray to
// the floor's normal where it is full and where it is gone.
const XR_REFLECT_GAIN: f32 = 0.4;
const XR_REFLECT_COS: vec2<f32> = vec2<f32>(0.5, 0.906);

struct XrSkyOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world: vec3<f32>,
    @location(2) @interpolate(flat) eye: vec3<f32>,
}

// The eye's position: the one point the camera maps to clip x = y = w = 0
// (edge_bars.rs, eye_position).
fn xr_eye_position() -> vec3<f32> {
    let m = xr_eye.view_proj;
    let a = vec3<f32>(m[0].x, m[1].x, m[2].x);
    let b = vec3<f32>(m[0].y, m[1].y, m[2].y);
    let c = vec3<f32>(m[0].w, m[1].w, m[2].w);
    let t = vec3<f32>(m[3].x, m[3].y, m[3].w);
    let bc = cross(b, c);
    return -(t.x * bc + t.y * cross(c, a) + t.z * cross(a, b)) / dot(a, bc);
}

@vertex
fn vs_sky(@builtin(vertex_index) i: u32) -> XrSkyOut {
    var order = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    var signs = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let k = order[i];
    let p = xr_port.corners[k].xyz;
    var out: XrSkyOut;
    out.pos = xr_eye.view_proj * vec4<f32>(p, 1.0);
    out.uv = signs[k] * xr_port.face.xy;
    out.world = p;
    out.eye = xr_eye_position();
    return out;
}

// The rim fade with a flag per edge (-u, +u, -v, +v): 1 fades over the
// last XR_EDGE_M, 0 runs to the edge.
fn xr_rim(uv: vec2<f32>) -> f32 {
    let e = xr_port.edges;
    let fade = vec2<f32>(select(e.x, e.y, uv.x > 0.0), select(e.z, e.w, uv.y > 0.0));
    let s = vec2<f32>(
        smoothstep(0.0, XR_EDGE_M, xr_port.face.x - abs(uv.x)),
        smoothstep(0.0, XR_EDGE_M, xr_port.face.y - abs(uv.y)),
    );
    let r = vec2<f32>(1.0) + (s - vec2<f32>(1.0)) * fade;
    return r.x * r.y;
}

// The azimuth of d about the sky's point from the facing, positive to the
// right, and its elevation.
fn xr_sky_angles(d: vec3<f32>) -> vec2<f32> {
    let f = xr_port.axes.zw;
    let az = atan2(dot(d.xz, vec2<f32>(-f.y, f.x)), dot(d.xz, f) + 1e-6);
    return vec2<f32>(az, atan2(d.y, length(d.xz)));
}

// The frame coordinate of azimuth a.x and elevation a.y, in pixels.
fn xr_sky_frame(a: vec2<f32>) -> vec4<f32> {
    let n = vec2<f32>(a.x / XR_TAU + 0.5, 1.0 - (a.y - XR_SKY_HORIZON) / XR_SKY_HEIGHT);
    return vec4<f32>(n * u.resolution, 0.0, 1.0);
}

// Along one of the room's axes, the distance from l to the wall ahead of
// the direction m.
fn xr_exit_along(l: f32, m: f32, half: f32) -> f32 {
    if abs(m) < 1e-6 {
        return 1e6;
    }
    return (select(-half, half, m > 0.0) - l) / m;
}

// Where the ray from p along m leaves the room: a wall of its rectangle
// in plan or its ceiling, whichever first.
fn xr_room_exit(p: vec3<f32>, m: vec3<f32>) -> vec3<f32> {
    let axis = xr_port.axes.xy;
    let across = vec2<f32>(-axis.y, axis.x);
    let l = p.xz - xr_port.room.xy;
    let ts = xr_exit_along(dot(l, axis), dot(m.xz, axis), xr_port.room.z);
    let tt = xr_exit_along(dot(l, across), dot(m.xz, across), xr_port.room.w);
    let ty = (xr_port.sky.w - p.y) / max(m.y, 1e-6);
    return p + m * max(min(min(ts, tt), ty), 0.0);
}

@fragment
fn fs_sky(in: XrSkyOut) -> @location(0) vec4<f32> {
    let map = xr_port.mode.y;
    var k = xr_port.color.w * clamp(xr_port.params.x, 0.0, 1.0) * xr_rim(in.uv);
    if map < 0.5 {
        // Per face, as fs_main.
        let n = in.uv / xr_port.face.xy * 0.5 + 0.5;
        let c = effect_main(vec4<f32>(vec2<f32>(n.x, 1.0 - n.y) * u.resolution, 0.0, 1.0));
        return xr_light(c, k);
    }
    var d = in.world - xr_port.sky.xyz;
    let reflection = map > 1.5;
    if reflection {
        // The view ray reflected about the floor, to where it leaves the
        // room: the sky seen there.
        let r = normalize(in.world - in.eye);
        let m = vec3<f32>(r.x, -r.y, r.z);
        d = xr_room_exit(in.world, m) - xr_port.sky.xyz;
        k *= XR_REFLECT_GAIN * (1.0 - smoothstep(XR_REFLECT_COS.x, XR_REFLECT_COS.y, abs(r.y)));
    }
    let a = xr_sky_angles(d);
    let w = 0.5 * smoothstep(XR_PI - XR_SKY_SEAM, XR_PI, abs(a.x));
    k *= 1.0 - smoothstep(XR_SKY_ZENITH.x, XR_SKY_ZENITH.y, a.y);
    if reflection {
        // One evaluation: the reflection fades out across the seam.
        k *= 1.0 - 2.0 * w;
    }
    if k <= 0.0 {
        return vec4<f32>(0.0);
    }
    var c = effect_main(xr_sky_frame(a));
    if !reflection && w > 0.0 {
        // Near the seam the other end's continuation, cross-faded.
        let other = effect_main(xr_sky_frame(vec2<f32>(a.x - sign(a.x) * XR_TAU, a.y)));
        c = mix(c, other, w);
    }
    return xr_light(c, k);
}
