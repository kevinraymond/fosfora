// Photosensitivity flash limiter (#108): measure this frame, decide its gain.
//
// Appended to post_composite.wgsl, whose post_color() it samples, and run as
// one 16x16 workgroup before the composite pass. The composite multiplies its
// final colour by the gain written here, so the display, the output window and
// every capture sink (NDI, Spout, Syphon, v4l2, recording) get the same frame.
//
// What counts as a flash follows WCAG 2.x / ITU-R BT.1702 (the Harding test):
// a pair of opposing changes in relative luminance of 10% or more where the
// darker state is below 0.8, or an opposing change of the saturated-red
// measure. More than three flashes in any one second fails.
//
// The frame is split into 4x4 tiles (a flash only has to cover part of the
// view), each with a luminance and a red track. A track counts its rises; once
// a track has spent the second's budget, the next rise is capped just under the
// threshold by dimming the whole frame. Falls are never touched (dimming can't
// undo one), and a flash needs a rise, so capping rises bounds flashes. When the
// oldest rise ages out of the window, the next one is allowed through whole.

const GRID: u32 = 64u;        // samples per side (16 threads x 4)
const TILES: u32 = 4u;        // tiles per side
const TRACKS: u32 = 32u;      // 16 tiles x {luminance, red}
const LUMA_STEP: f32 = 0.1;   // relative-luminance swing that makes half a flash
const DARK_MAX: f32 = 0.8;    // ...when the darker state is below this
const RED_STEP: f32 = 0.0625; // BT.1702: (R-G-B)*320 changing by more than 20
const HEADROOM: f32 = 0.9;    // cap a blocked rise at 90% of a step
const WINDOW_S: f32 = 1.0;
const UNSET: f32 = 1e9;       // extreme not yet seen (after a reset)
const LONG_AGO: f32 = -1e9;   // an empty rise slot

struct Track {
    // x: phase (0 = falling, tracking the minimum; 1 = rising, tracking the
    // maximum), y: that extreme, z: next rise slot (0..3), w: unused.
    st: vec4f,
    // Times of the last four rises.
    rises: vec4f,
}

struct FlashState {
    gain: f32,
    last_t: f32,
    budget: f32,
    _pad: f32,
    tracks: array<Track, 32>,
}

@group(1) @binding(0) var<storage, read_write> flash_state: FlashState;

var<workgroup> part_luma: array<f32, 256>;
var<workgroup> part_red: array<f32, 256>;

fn relative_luminance(c: vec3f) -> f32 {
    return dot(c, vec3f(0.2126, 0.7152, 0.0722));
}

// BT.1702's saturated-red measure on gamma-encoded values: R-G-B where red is
// at least 80% of R+G+B, else 0. Linear in the encoded values, so dimming the
// linear colour by g scales it by pow(g, 1/2.2).
fn red_measure(c: vec3f) -> f32 {
    let e = pow(max(c, vec3f(0.0)), vec3f(1.0 / 2.2));
    let sum = e.r + e.g + e.b;
    if sum <= 0.0 || e.r < 0.8 * sum {
        return 0.0;
    }
    return max(e.r - e.g - e.b, 0.0);
}

fn rises_in_window(r: vec4f, now: f32) -> f32 {
    let recent = (vec4f(now) - r) < vec4f(WINDOW_S);
    return dot(select(vec4f(0.0), vec4f(1.0), recent), vec4f(1.0));
}

@compute @workgroup_size(16, 16)
fn cs_flash(@builtin(local_invocation_id) lid: vec3u) {
    let idx = lid.y * 16u + lid.x;
    var luma = 0.0;
    var red = 0.0;
    for (var sy = 0u; sy < 4u; sy++) {
        for (var sx = 0u; sx < 4u; sx++) {
            let p = vec2f(f32(lid.x * 4u + sx), f32(lid.y * 4u + sy));
            let c = post_color((p + 0.5) / f32(GRID)).rgb;
            luma += relative_luminance(c);
            red += red_measure(c);
        }
    }
    part_luma[idx] = luma / 16.0;
    part_red[idx] = red / 16.0;
    workgroupBarrier();
    if idx != 0u {
        return;
    }

    // Tile means: tile (tx, ty) is the 4x4 block of threads it covers.
    var metric: array<f32, 32>;
    for (var t = 0u; t < 16u; t++) {
        let tx = t % TILES;
        let ty = t / TILES;
        var l = 0.0;
        var r = 0.0;
        for (var j = 0u; j < 16u; j++) {
            let i = (ty * 4u + j / 4u) * 16u + tx * 4u + j % 4u;
            l += part_luma[i];
            r += part_red[i];
        }
        metric[t] = l / 16.0;
        metric[t + 16u] = r / 16.0;
    }

    let now = post.time;
    let budget = post.flash_budget;
    // A new budget (the setting changed, or the limiter was just switched on)
    // starts every track afresh.
    if budget != flash_state.budget {
        for (var k = 0u; k < TRACKS; k++) {
            flash_state.tracks[k].st = vec4f(0.0, UNSET, 0.0, 0.0);
            flash_state.tracks[k].rises = vec4f(LONG_AGO);
        }
    } else if now < flash_state.last_t {
        // The clock went backwards: the shader clock wraps hourly
        // (SHADER_TIME_PERIOD_S), or an offline render restarted. Carry the
        // rises over as if no time passed. Forgetting them instead would hand
        // a running strobe a fresh budget at every wrap.
        let back = vec4f(flash_state.last_t - now);
        for (var k = 0u; k < TRACKS; k++) {
            flash_state.tracks[k].rises -= back;
        }
    }

    // The gain: the strongest dimming any blocked track needs.
    var gain = 1.0;
    for (var k = 0u; k < TRACKS; k++) {
        let tr = flash_state.tracks[k];
        let is_red = k >= 16u;
        let raw = metric[k];
        let lo = tr.st.y;
        let blocked = tr.st.x < 0.5 && rises_in_window(tr.rises, now) >= budget;
        let counts = is_red || lo < DARK_MAX;
        if blocked && counts {
            let cap = lo + select(LUMA_STEP, RED_STEP, is_red) * HEADROOM;
            if raw > cap {
                var g = cap / raw;
                if is_red {
                    g = pow(g, 2.2);
                }
                gain = min(gain, g);
            }
        }
    }

    // Advance every track on what will actually be shown.
    let red_gain = pow(gain, 1.0 / 2.2);
    for (var k = 0u; k < TRACKS; k++) {
        var tr = flash_state.tracks[k];
        let is_red = k >= 16u;
        let step = select(LUMA_STEP, RED_STEP, is_red);
        let shown = metric[k] * select(gain, red_gain, is_red);
        if tr.st.x < 0.5 {
            tr.st.y = min(tr.st.y, shown);
            if shown - tr.st.y >= step && (is_red || tr.st.y < DARK_MAX) {
                let slot = u32(tr.st.z) % 4u;
                tr.rises[slot] = now;
                tr.st = vec4f(1.0, shown, f32((slot + 1u) % 4u), 0.0);
            }
        } else {
            tr.st.y = max(tr.st.y, shown);
            if tr.st.y - shown >= step {
                tr.st.x = 0.0;
                tr.st.y = shown;
            }
        }
        flash_state.tracks[k] = tr;
    }

    flash_state.gain = gain;
    flash_state.last_t = now;
    flash_state.budget = budget;
}
