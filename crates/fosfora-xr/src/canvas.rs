//! The wall spectrum (board #3327): the mel spectrum as bars of light
//! climbing the wall the wearer faces. Plain numbers in, so it builds and
//! tests on the desktop as well; `gfx.rs` draws it with [`CANVAS_WGSL`]
//! from the rows [`Canvas::uniform`] packs, on the ripple's pattern.
//!
//! **Which wall.** Walls arrive as `WALL_FACE` anchors, 4 cm boxes whose
//! thinnest axis is the plane normal; the face toward the head is the
//! room-facing one ([`WallFace::of`]). [`WallPick`] scores each wall by the
//! cosine between the view direction and the direction to its center and
//! keeps a pick: a wall behind the head (cosine <= 0) is never picked, and
//! another wall takes over only after it has scored [`SWITCH_MARGIN`] above
//! the current one for [`SWITCH_HOLD_S`], so a glance at the next wall
//! changes nothing.
//!
//! **The bars.** Up to [`MAX_BARS`] across the wall's width (the knob's
//! count, or the mel length if smaller), each the mean of its share of the
//! mel bands, low bands on the wearer's left. The bands are dB values in
//! 0..1 (80 dB range), so the normalization is in the log domain: the top
//! of the wall, for each bar, is that bar's running maximum (instant rise,
//! decay with [`TOP_TAU_S`]), and the wall shows the [`SPAN`] below it. A
//! quiet track still fills the wall. Music falls off toward the high bands
//! (the first device run, one top for every bar: mean height 0.1-0.25, the
//! right half of the wall dark), so each bar is normalized on its own, but
//! never to a top more than [`TILT_RANGE`] below the loudest bar's, nor
//! below [`MIN_TOP`]: a band far under the rest, or near silence, does not
//! stretch its noise over the wall, and silence (0) shows nothing. A bar's
//! height rises at once and falls at [`RELEASE_PER_S`] wall heights per
//! second, by time, so 72 and 90 Hz behave alike.
//!
//! **The look.** The floor ripple's family: premultiplied warm white, peak
//! alpha 0.25 (times `canvasgain`), light on the real wall rather than paint
//! over it. Each bar is brightest at its top, with a bright cap line, over a
//! faint glow along the bottom that breathes with the level.

use glam::Vec3;

/// Bars the uniform holds.
pub const MAX_BARS: usize = 64;
/// Bars by default (`debug.fosfora.canvasbars`).
pub const DEFAULT_BARS: usize = 24;
/// Fall speed of a bar (wall heights per second).
pub const RELEASE_PER_S: f32 = 2.5;
/// The band range the wall shows below the running maximum (dB/80: 28 dB).
pub const SPAN: f32 = 0.35;
/// A bar's top never drops below this (dB/80), so near silence does not
/// stretch the noise floor over the wall...
pub const MIN_TOP: f32 = 0.5;
/// ...nor more than this below the loudest bar's (dB/80: 16 dB), the
/// spectral tilt the per-bar normalization evens out.
pub const TILT_RANGE: f32 = 0.2;
/// Decay time constant of the bars' running maxima (s).
pub const TOP_TAU_S: f32 = 8.0;
/// A challenger wall must beat the current one's score by this much...
pub const SWITCH_MARGIN: f32 = 0.15;
/// ...for this long (s) to take over.
pub const SWITCH_HOLD_S: f32 = 1.0;
/// Alpha at light 1, as the ripple's.
pub const PEAK_ALPHA: f32 = crate::ripple::PEAK_ALPHA;
/// The ripple's warm white.
pub const COLOR: [f32; 3] = crate::ripple::COLOR;
/// How far off the wall's face the quad sits (m), plus the ripple's depth
/// bias: the wall's occluder writes depth at its face.
pub const LIFT_M: f32 = crate::ripple::LIFT_M;
/// Fraction of a bar's slot the bar fills (the rest is the gap).
pub const BAR_FILL: f32 = 0.7;
/// The bottom glow's intensity at full level, and its floor.
pub const BASE_GLOW: f32 = 0.35;
const BASE_GLOW_FLOOR: f32 = 0.3;
/// Rows of [`Canvas::uniform`], `struct Canvas` in [`CANVAS_WGSL`].
pub const UNIFORM_ROWS: usize = 8 + MAX_BARS / 4;

/// A wall's room-facing face: center, outward normal (toward the head),
/// the in-plane axis closest to up (signed up) and the one to the wearer's
/// right as they face it, with their half extents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WallFace {
    pub center: Vec3,
    pub normal: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub half_right: f32,
    pub half_up: f32,
}

impl WallFace {
    /// The face of the box `center` / `rot` / `half` toward `head`.
    pub fn of(center: Vec3, rot: glam::Quat, half: Vec3, head: Vec3) -> Self {
        let f = crate::surfaces::TopFace::facing(center, rot, half, head);
        let (up_i, right_i) = if f.axes[1].y.abs() > f.axes[0].y.abs() {
            (1, 0)
        } else {
            (0, 1)
        };
        let up = f.axes[up_i] * if f.axes[up_i].y >= 0.0 { 1.0 } else { -1.0 };
        // As seen from the head (looking along -normal): right = up x normal.
        let right = up.cross(f.normal).normalize_or(f.axes[right_i]);
        Self {
            center: f.center,
            normal: f.normal,
            right,
            up,
            half_right: f.half[right_i],
            half_up: f.half[up_i],
        }
    }

    /// The quad's corners, `LIFT_M` off the face toward the room, in the
    /// order the shader's u, v run: bottom left, bottom right, top right,
    /// top left (as the wearer sees it).
    pub fn corners(&self) -> [Vec3; 4] {
        let c = self.center + self.normal * LIFT_M;
        let (r, u) = (self.right * self.half_right, self.up * self.half_up);
        [c - r - u, c + r - u, c + r + u, c - r + u]
    }

    /// How squarely the view looks at the wall: the cosine between
    /// `forward` and the direction from `head` to the face's center.
    pub fn score(&self, head: Vec3, forward: Vec3) -> f32 {
        forward.dot((self.center - head).normalize_or_zero())
    }
}

/// The wall the canvas is on, kept with hysteresis (see the module docs).
#[derive(Debug, Clone, Copy, Default)]
pub struct WallPick {
    current: Option<usize>,
    /// A wall beating the current one by the margin, and for how long.
    challenger: Option<(usize, f32)>,
}

impl WallPick {
    /// This frame's pick among `walls` (their room-facing faces, by index)
    /// for a head at `head` looking along `forward`, `dt` seconds after the
    /// last call. `None` while no wall is in front of the head and none is
    /// held.
    pub fn update(
        &mut self,
        head: Vec3,
        forward: Vec3,
        walls: &[WallFace],
        dt: f32,
    ) -> Option<usize> {
        if self.current.is_some_and(|c| c >= walls.len()) {
            *self = Self::default();
        }
        // A wall counts only with its face toward the head (always, for a
        // face built by `WallFace::of` from this head) and in front of it.
        let best = walls
            .iter()
            .enumerate()
            .filter(|(_, w)| w.normal.dot(head - w.center) > 0.0)
            .map(|(i, w)| (i, w.score(head, forward)))
            .filter(|&(_, s)| s > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        let Some(current) = self.current else {
            self.current = best.map(|(i, _)| i);
            self.challenger = None;
            return self.current;
        };
        let held = walls[current].score(head, forward);
        match best {
            Some((i, s)) if i != current && s > held + SWITCH_MARGIN => {
                let since = match self.challenger {
                    Some((c, t)) if c == i => t + dt,
                    _ => dt,
                };
                if since >= SWITCH_HOLD_S - 1e-6 {
                    self.current = Some(i);
                    self.challenger = None;
                } else {
                    self.challenger = Some((i, since));
                }
            }
            _ => self.challenger = None,
        }
        self.current
    }

    pub fn current(&self) -> Option<usize> {
        self.current
    }
}

/// The mean of each bar's share of `bands` into `out` (one per bar).
pub fn bands_to_bars(bands: &[f32], out: &mut [f32]) {
    let (m, n) = (bands.len(), out.len());
    if m == 0 || n == 0 {
        out.fill(0.0);
        return;
    }
    for (i, o) in out.iter_mut().enumerate() {
        let lo = i * m / n;
        let hi = ((i + 1) * m / n).max(lo + 1).min(m);
        *o = bands[lo..hi].iter().sum::<f32>() / (hi - lo) as f32;
    }
}

#[derive(Debug, Clone)]
pub struct Canvas {
    /// Bars asked for (`canvasbars`); fewer when the mel is shorter.
    pub bars: usize,
    /// Brightness multiplier (`canvasgain`).
    pub gain: f32,
    heights: [f32; MAX_BARS],
    /// Each bar's running maximum (dB/80).
    tops: [f32; MAX_BARS],
    shown: usize,
    glow: f32,
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new(DEFAULT_BARS, 1.0)
    }
}

impl Canvas {
    pub fn new(bars: usize, gain: f32) -> Self {
        Self {
            bars: bars.clamp(1, MAX_BARS),
            gain,
            heights: [0.0; MAX_BARS],
            tops: [0.0; MAX_BARS],
            shown: 0,
            glow: 0.0,
        }
    }

    /// Advance by `dt` seconds on this frame's mel bands (dB in 0..1, any
    /// length).
    pub fn update(&mut self, dt: f32, mel: &[f32]) {
        let n = self.bars.min(mel.len());
        let mut raw = [0.0f32; MAX_BARS];
        bands_to_bars(mel, &mut raw[..n]);
        let decay = (-dt.max(0.0) / TOP_TAU_S).exp();
        for (top, r) in self.tops.iter_mut().zip(raw).take(n) {
            *top = (*top * decay).max(r);
        }
        let loudest = self.top();
        let fall = RELEASE_PER_S * dt.max(0.0);
        for ((h, top), r) in self.heights.iter_mut().zip(self.tops).zip(raw).take(n) {
            let floor = top.max(loudest - TILT_RANGE).max(MIN_TOP) - SPAN;
            let target = if r > 0.0 {
                ((r - floor) / SPAN).clamp(0.0, 1.0)
            } else {
                0.0
            };
            *h = target.max(*h - fall);
        }
        self.heights[n..].fill(0.0);
        self.tops[n..].fill(0.0);
        self.shown = n;
        let mean = if n > 0 {
            self.heights[..n].iter().sum::<f32>() / n as f32
        } else {
            0.0
        };
        self.glow = if n > 0 {
            BASE_GLOW * (BASE_GLOW_FLOOR + (1.0 - BASE_GLOW_FLOOR) * mean)
        } else {
            0.0
        };
    }

    /// This frame's bar heights, 0..1 of the wall, left to right.
    pub fn heights(&self) -> &[f32] {
        &self.heights[..self.shown]
    }

    /// The loudest bar's top (dB/80), at least [`MIN_TOP`].
    pub fn top(&self) -> f32 {
        // Bars past this frame's count hold 0.
        self.tops.iter().copied().fold(MIN_TOP, f32::max)
    }

    /// The uniform rows, `struct Canvas` in [`CANVAS_WGSL`]: the four
    /// corners ([`WallFace::corners`] order), the shape (bar count, peak
    /// alpha x gain, bottom glow, bar fill), the color, then the heights
    /// four to a row.
    pub fn uniform(&self, corners: [Vec3; 4]) -> [[f32; 4]; UNIFORM_ROWS] {
        let mut rows = [[0.0f32; 4]; UNIFORM_ROWS];
        for (row, c) in rows.iter_mut().zip(corners) {
            *row = [c.x, c.y, c.z, 1.0];
        }
        rows[4] = [
            self.shown as f32,
            PEAK_ALPHA * self.gain,
            self.glow,
            BAR_FILL,
        ];
        rows[5] = [COLOR[0], COLOR[1], COLOR[2], 0.0];
        for (i, h) in self.heights().iter().enumerate() {
            rows[8 + i / 4][i % 4] = *h;
        }
        rows
    }
}

/// The canvas's shader: group 0 is the eye pass's shared camera
/// (`view_proj`), group 1 the rows of [`Canvas::uniform`]. The vertex
/// stage places the quad and hands the fragment the face's (u, v), u
/// across the wall to the wearer's right, v up it; the fragment finds the
/// bar from u, compares v with its height and writes premultiplied warm
/// white, at most the peak alpha.
pub const CANVAS_WGSL: &str = r"
struct Eye { view_proj: mat4x4<f32> }
struct Canvas {
    // bottom left, bottom right, top right, top left
    corners: array<vec4<f32>, 4>,
    // x bar count, y peak alpha (gain applied), z bottom glow, w bar fill
    shape: vec4<f32>,
    color: vec4<f32>,
    _pad0: vec4<f32>,
    _pad1: vec4<f32>,
    // bar heights 0..1, four per row
    levels: array<vec4<f32>, 16>,
}
@group(0) @binding(0) var<uniform> eye: Eye;
@group(1) @binding(0) var<uniform> canvas: Canvas;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var order = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    var uvs = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let k = order[i];
    var out: VsOut;
    out.pos = eye.view_proj * vec4<f32>(canvas.corners[k].xyz, 1.0);
    out.uv = uvs[k];
    return out;
}

// Softness of a bar's sides (fraction of its slot) and of its top and cap
// (fraction of the wall's height), and the bottom glow's height.
const SIDE_SOFT: f32 = 0.12;
const TOP_SOFT: f32 = 0.015;
const CAP_WIDTH: f32 = 0.012;
const GLOW_HEIGHT: f32 = 0.03;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let n = max(canvas.shape.x, 1.0);
    let x = clamp(in.uv.x, 0.0, 0.99999) * n;
    let i = min(u32(x), u32(n) - 1u);
    let f = fract(x);
    let gap = (1.0 - canvas.shape.w) * 0.5;
    let across = smoothstep(gap, gap + SIDE_SOFT, f)
        * (1.0 - smoothstep(1.0 - gap - SIDE_SOFT, 1.0 - gap, f));
    let h = canvas.levels[i / 4u][i % 4u];
    let v = in.uv.y;
    // Below the top, brighter toward it: light climbing the wall.
    let below = 1.0 - smoothstep(h - TOP_SOFT, h, v);
    let grad = mix(0.2, 1.0, clamp(v / max(h, 1e-3), 0.0, 1.0));
    let c = (v - h) / CAP_WIDTH;
    let cap = exp(-c * c) * smoothstep(0.0, 0.02, h);
    let light = across * (0.7 * below * grad + 0.6 * cap)
        + canvas.shape.z * exp(-v / GLOW_HEIGHT);
    let a = canvas.shape.y * clamp(light, 0.0, 1.0);
    return vec4<f32>(canvas.color.rgb * a, a);
}
";

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;

    const DT: f32 = 1.0 / 72.0;
    const HEAD: Vec3 = Vec3::new(0.0, 1.2, 0.0);

    /// A wall plane (4 cm thick, local +Z its normal) centered at `center`
    /// with its normal along `normal` (horizontal), 4 m wide, 2.5 m tall.
    fn wall(center: Vec3, normal: Vec3) -> WallFace {
        let rot = Quat::from_rotation_arc(Vec3::Z, normal.normalize());
        WallFace::of(center, rot, Vec3::new(2.0, 1.25, 0.02), HEAD)
    }

    /// Four walls of a 4 m room around the head: ahead (-Z), behind, left
    /// and right, in that order.
    fn room() -> [WallFace; 4] {
        [
            wall(Vec3::new(0.0, 1.25, -2.0), Vec3::Z),
            wall(Vec3::new(0.0, 1.25, 2.0), Vec3::NEG_Z),
            wall(Vec3::new(-2.0, 1.25, 0.0), Vec3::X),
            wall(Vec3::new(2.0, 1.25, 0.0), Vec3::NEG_X),
        ]
    }

    fn look(yaw_deg: f32) -> Vec3 {
        Quat::from_rotation_y(yaw_deg.to_radians()) * Vec3::NEG_Z
    }

    fn run(pick: &mut WallPick, walls: &[WallFace], forward: Vec3, s: f32) -> Option<usize> {
        let mut out = pick.current();
        for _ in 0..(s / DT).round() as usize {
            out = pick.update(HEAD, forward, walls, DT);
        }
        out
    }

    #[test]
    fn the_face_is_the_room_side_with_the_wearers_right_and_up() {
        let w = room()[0];
        assert!(w.normal.abs_diff_eq(Vec3::Z, 1e-5), "{:?}", w.normal);
        assert!((w.center.z + 1.98).abs() < 1e-5);
        assert!(w.up.abs_diff_eq(Vec3::Y, 1e-5), "{:?}", w.up);
        // Facing -Z, the wearer's right is +X.
        assert!(w.right.abs_diff_eq(Vec3::X, 1e-5), "{:?}", w.right);
        assert_eq!((w.half_right, w.half_up), (2.0, 1.25));
        let c = w.corners();
        // Lifted off the face into the room; bottom left first.
        assert!(c.iter().all(|p| (p.z - (-1.98 + LIFT_M)).abs() < 1e-5));
        assert!(c[0].abs_diff_eq(Vec3::new(-2.0, 0.0, -1.96), 1e-5), "{c:?}");
        assert!(c[2].abs_diff_eq(Vec3::new(2.0, 2.5, -1.96), 1e-5), "{c:?}");
        // A wall whose runtime pose puts local Y along the width still gets
        // up = world up.
        let rot = Quat::from_rotation_arc(Vec3::Z, Vec3::X)
            * Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let side = WallFace::of(
            Vec3::new(-2.0, 1.25, 0.0),
            rot,
            Vec3::new(1.25, 2.0, 0.02),
            HEAD,
        );
        assert!(side.up.abs_diff_eq(Vec3::Y, 1e-4), "{:?}", side.up);
        assert!((side.half_up - 1.25).abs() < 1e-5 && (side.half_right - 2.0).abs() < 1e-5);
        assert!(
            side.right.abs_diff_eq(Vec3::NEG_Z, 1e-4),
            "{:?}",
            side.right
        );
    }

    #[test]
    fn the_pick_prefers_the_wall_in_front_and_ignores_walls_behind() {
        let walls = room();
        assert_eq!(
            WallPick::default().update(HEAD, look(0.0), &walls, DT),
            Some(0)
        );
        assert_eq!(
            WallPick::default().update(HEAD, look(90.0), &walls, DT),
            Some(2)
        );
        assert_eq!(
            WallPick::default().update(HEAD, look(-90.0), &walls, DT),
            Some(3)
        );
        // Only the wall behind the head: nothing to draw on.
        let behind = [walls[1]];
        assert_eq!(
            WallPick::default().update(HEAD, look(0.0), &behind, DT),
            None
        );
        assert_eq!(WallPick::default().update(HEAD, look(0.0), &[], DT), None);
    }

    #[test]
    fn a_glance_keeps_the_wall_and_a_steady_look_switches_after_a_second() {
        let walls = room();
        let mut pick = WallPick::default();
        assert_eq!(run(&mut pick, &walls, look(0.0), 0.5), Some(0));
        // A 0.6 s glance at the left wall: held.
        assert_eq!(run(&mut pick, &walls, look(90.0), 0.6), Some(0));
        assert_eq!(run(&mut pick, &walls, look(0.0), 0.2), Some(0));
        // Looking left for 0.9 s: still held; at 1 s it switches.
        assert_eq!(run(&mut pick, &walls, look(90.0), 0.9), Some(0));
        assert_eq!(run(&mut pick, &walls, look(90.0), 0.15), Some(2));
        // Between two walls, within the margin: no switch however long.
        let mut pick = WallPick::default();
        assert_eq!(run(&mut pick, &walls, look(40.0), 0.1), Some(0));
        assert_eq!(run(&mut pick, &walls, look(50.0), 3.0), Some(0));
        // The same second at 90 Hz switches too: by time, not frames.
        let mut pick = WallPick::default();
        pick.update(HEAD, look(0.0), &walls, 1.0 / 90.0);
        let mut t = 0.0;
        let mut at = None;
        while t < 2.0 {
            t += 1.0 / 90.0;
            if pick.update(HEAD, look(90.0), &walls, 1.0 / 90.0) == Some(2) {
                at = Some(t);
                break;
            }
        }
        let at = at.expect("switched");
        assert!((at - SWITCH_HOLD_S).abs() < 0.02, "{at}");
        // A room that lost walls drops a pick past its end.
        let mut pick = WallPick::default();
        assert_eq!(run(&mut pick, &walls, look(-90.0), 0.1), Some(3));
        assert_eq!(pick.update(HEAD, look(0.0), &walls[..2], DT), Some(0));
    }

    #[test]
    fn bands_split_evenly_into_bars() {
        let bands: Vec<f32> = (0..64).map(|i| i as f32).collect();
        let mut bars = [0.0; 24];
        bands_to_bars(&bands, &mut bars);
        // Bar 0 is bands 0..2 (mean 0.5), the last 61..64 (mean 62).
        assert_eq!(bars[0], 0.5);
        assert_eq!(bars[23], 62.0);
        assert!(bars.windows(2).all(|w| w[1] > w[0]));
        // Fewer bands than bars is the canvas's job to avoid; still no panic.
        let mut many = [0.0; 4];
        bands_to_bars(&[1.0, 3.0], &mut many);
        assert_eq!(many, [1.0, 1.0, 3.0, 3.0]);
        bands_to_bars(&[], &mut many);
        assert_eq!(many, [0.0; 4]);
    }

    #[test]
    fn a_bar_rises_at_once_and_falls_by_time_not_frame_count() {
        let fall = |dt: f32| {
            let mut c = Canvas::new(4, 1.0);
            // The band's own maximum sets its top: 0.8 - SPAN is the floor.
            c.update(dt, &[0.8, 0.8, 0.8, 0.8]);
            assert!(c.heights().iter().all(|&h| h > 0.999), "{:?}", c.heights());
            // 1/6 s: 12 frames at 72 Hz, 15 at 90.
            for _ in 0..(1.0 / (6.0 * dt)).round() as usize {
                c.update(dt, &[0.8, 0.25, 0.8, 0.8]);
            }
            c.heights()[1]
        };
        let (a, b) = (fall(1.0 / 72.0), fall(1.0 / 90.0));
        assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        // 1/6 s at 2.5 heights/s toward the band's level (0.25 is under
        // the span below its 0.8 top).
        assert!((a - (1.0 - RELEASE_PER_S / 6.0)).abs() < 1e-3, "{a}");
        // A band the tilt range below the loudest, new: its top is held
        // at 0.6, so 0.25 is the bottom of the span.
        let mut c = Canvas::new(4, 1.0);
        c.update(DT, &[0.8, 0.25, 0.8, 0.8]);
        assert!(c.heights()[1] < 1e-4, "{:?}", c.heights());
        // Back up in one frame.
        c.update(DT, &[0.8, 0.8, 0.8, 0.8]);
        assert!(c.heights()[1] > 0.999, "{:?}", c.heights());
    }

    #[test]
    fn the_gain_follows_the_running_maximum_so_a_quiet_track_fills_the_wall() {
        let mut c = Canvas::new(2, 1.0);
        // A loud passage, then a quiet one 20 dB down.
        for _ in 0..72 {
            c.update(DT, &[0.9, 0.7]);
        }
        assert!(c.heights()[0] > 0.999, "{:?}", c.heights());
        for _ in 0..(10 * 72) {
            c.update(DT, &[0.65, 0.5]);
        }
        // After a few time constants each bar's top has come down to the
        // quiet track's level, which fills the wall again.
        assert!((c.top() - 0.65).abs() < 0.02, "{}", c.top());
        assert!(c.heights().iter().all(|&h| h > 0.95), "{:?}", c.heights());
        // A band 32 dB under the loudest stays low: its top is held within
        // the tilt range of the loudest (0.9 - 0.2), a span 0.35 deep.
        let mut c = Canvas::new(2, 1.0);
        for _ in 0..(10 * 72) {
            c.update(DT, &[0.9, 0.5]);
        }
        let expect = (0.5 - (0.9 - TILT_RANGE - SPAN)) / SPAN;
        assert!((c.heights()[1] - expect).abs() < 1e-3, "{:?}", c.heights());
        // Near silence does not stretch the noise floor: the top stays at
        // MIN_TOP and the floor below the span stays dark.
        let mut c = Canvas::new(2, 1.0);
        for _ in 0..(20 * 72) {
            c.update(DT, &[0.1, 0.0]);
        }
        assert_eq!(c.top(), MIN_TOP);
        assert_eq!(c.heights(), &[0.0, 0.0]);
    }

    #[test]
    fn the_bar_count_is_the_knobs_or_the_mel_length() {
        let mut c = Canvas::new(24, 1.0);
        c.update(DT, &[0.6; 64]);
        assert_eq!(c.heights().len(), 24);
        c.update(DT, &[0.6; 13]);
        assert_eq!(c.heights().len(), 13);
        assert_eq!(Canvas::new(500, 1.0).bars, MAX_BARS);
        assert_eq!(Canvas::new(0, 1.0).bars, 1);
    }

    #[test]
    fn the_uniform_carries_the_corners_and_the_bars() {
        let mut c = Canvas::new(6, 0.5);
        let mel: Vec<f32> = (0..6).map(|i| 0.4 + 0.08 * i as f32).collect();
        c.update(DT, &mel);
        let corners = room()[0].corners();
        let rows = c.uniform(corners);
        for (row, p) in rows.iter().zip(corners) {
            assert_eq!(row[..3], p.to_array());
        }
        assert_eq!(rows[4][0], 6.0);
        assert_eq!(rows[4][1], PEAK_ALPHA * 0.5);
        assert!(rows[4][2] > 0.0);
        assert_eq!(rows[5][..3], COLOR);
        let packed: Vec<f32> = rows[8..].iter().flatten().copied().collect();
        assert_eq!(packed[..6], *c.heights());
        assert!(packed[6..].iter().all(|&h| h == 0.0));
        // The loudest bar reaches the top, the others below it in order.
        assert!(packed[5] > 0.999, "{packed:?}");
        assert!(packed[..6].windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn the_shader_validates_and_matches_the_rows() {
        let module = naga::front::wgsl::parse_str(CANVAS_WGSL).expect("canvas WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("canvas WGSL validates");
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("Canvas"))
            .expect("struct Canvas");
        let naga::TypeInner::Struct { span, .. } = ty.inner else {
            panic!("Canvas is not a struct");
        };
        assert_eq!(span as usize, UNIFORM_ROWS * 16);
        assert_eq!(MAX_BARS, 16 * 4, "levels: array<vec4<f32>, 16>");
    }
}
