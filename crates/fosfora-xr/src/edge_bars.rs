//! Edge bars (board #5420): the room's outline drawn as the spectrum. Bars
//! rise and fall along the edges of each plane and box the app trusts as
//! an occluder, so the room's geometry is drawn in sound. Plain numbers in,
//! so it builds and tests on the desktop; `app.rs` assembles the shapes and
//! the strips once a frame and `gfx.rs` draws them with [`EDGE_BARS_WGSL`]
//! from the rows [`rows`] packs: one buffer written once a frame, one
//! instanced draw per eye.
//!
//! **Which edges** ([`strips`]). The shapes are the boxes the frame pushes
//! into the obstacle block, taken where `app.rs` assembles them, since the
//! occluder uniform carries neither a hidden wall's flag nor a live shape's
//! fade: the scanned room's boxes, the live planes and closed boxes in the
//! show, then the stage floor. The stage floor and hidden walls occlude but
//! are not outlined ([`Shape::look`] `None`); the hands are not boxes. None
//! while `debug.fosfora.occluders` is 0. A plane (a thin box,
//! `surfaces::is_plane`) has one face, the one toward the head
//! (`surfaces::acting_face`: a wall's room side); a volume has the faces
//! turned toward the head ([`faces`], one to three). Each face gives four
//! strips, one per edge.
//!
//! **Clear of the other boxes** ([`clearance`]). A scene plane is a slab
//! 4 cm thick centered on the real surface, so where a wall meets the floor
//! or another wall, the neighbor's slab covers the first 2 cm of the face
//! along that edge and would hide what is drawn there. Each edge moves in
//! along its face until the point midway along it, lifted as the strips
//! are, is out of every other box grown by [`CLEAR_MARGIN_M`], by at most
//! [`MAX_CLEAR_M`]: 2.5 cm at a wall's junctions, nothing on a free edge.
//!
//! **The growth** ([`Growth`], chosen in [`Mode::growth`] and applied in one
//! place). `edgebars 1`: the bars lie on the face and grow inward from its
//! edge ([`Growth::Inward`]), so whatever stands in front of the surface
//! hides them as it hides the surface. Each strip is a trapezoid mitered at
//! 45 degrees at the face's corners, so neighboring strips meet without
//! overlapping. On a volume, an edge shared by two visible faces gets bars
//! on both, which reads as the edge itself glowing. `edgebars 2`: the bars
//! stand off the edge along the face's normal ([`Growth::Standing`]), a
//! fence of light on a table's rim, with no miter.
//!
//! **Bars along the edge.** Each edge shows the wall spectrum's bars
//! (`canvas::Canvas::heights`, normalized per bar) from its low end to its
//! high end. A box corner is a low end when an even number of its signs
//! along the box's axes is positive ([`is_low`]). Neighboring corners differ
//! in one sign, so the direction alternates around every face, the same
//! band meets itself at every corner, and an edge shared by two faces runs
//! the same way on both. An edge of length L shows `min(n, L / 3 cm)` of
//! the canvas's n bars ([`bar_count`]), each the mean of its share of them
//! as the canvas splits the mel bands ([`merged`]), so a long wall edge
//! shows them all and no bar is thinner than [`MIN_BAR_M`]. The tallest bar
//! is [`HEIGHT_FRACTION`] of the face's smaller side, 2 to 20 cm
//! ([`max_height`]), so a 40 cm box and a 4 m wall read alike.
//!
//! **The look.** The wall spectrum's (`surface_fx.rs`, `spectrum_light`):
//! brightest at the tip, a cap line, soft sides, premultiplied. The top's
//! softness and the cap are fractions of the tallest bar ([`TOP_SOFT`],
//! [`CAP_WIDTH`]), at least a pixel. A line [`LINE_M`] wide runs along the
//! whole edge at [`LINE_LIGHT`], so the outline stays at zero height while
//! the music plays; in silence ([`Presence`]) nothing draws. The color is
//! the surface's own (its lane's, `surfaces::surface_color`; a live shape
//! its kind's palette color). The peak alpha rises from [`ALPHA_LOW`] at
//! immersion level 0 to [`ALPHA_HIGH`] at 1 ([`peak_alpha`]), times
//! `debug.fosfora.edgebarsgain` and a live shape's fade.
//!
//! **Occlusion.** In the eye pass after every occluder (the depth map, the
//! boxes, the hand mesh) and after the lit faces, before the sprites, on
//! the surfaces pass's terms: depth-tested, no depth write, the same depth
//! bias, the strips lifted [`LIFT_M`] off the face. The face's own box
//! never hides its strips: a face is outlined only while it is turned
//! toward the head, and the lift alone keeps a strip hundreds of depth
//! steps in front of its face at 4 m and at grazing angles (the tests).
//!
//! **Cost.** One quad per strip, instanced from one uniform written once a
//! frame (only its used rows), one draw per eye, no loop in the fragment.
//! At most [`MAX_STRIPS`] strips (32 boxes, three faces of four edges
//! each): [`UNIFORM_BYTES`] bytes, of which a frame writes 288 plus 80 per
//! strip. A strip of length L and height h covers (L - h) h, so a face's
//! strips cover 27 % of a 4 x 2.5 m wall, up to 46 % of a square face under
//! 1.67 m, and all of a face under 4.4 cm across; the walls, floor and
//! ceiling of a 4 x 4 x 2.5 m room 24 %. Strips write no depth and never
//! overlap on one face, so a pixel is shaded about once.
//!
//! **Knobs**, read at launch and polled once a second
//! ([`EdgeBarsOptions::from_knobs`]): `debug.fosfora.edgebars 0|1|2` (off,
//! the default until it is worn; inward; standing), `edgebarsgain` (0 to
//! [`GAIN_MAX`], default 1) and `edgebarsheight` (the fraction, 0 to
//! [`HEIGHT_FRACTION_MAX`], default [`HEIGHT_FRACTION`]). Off, the frame is
//! what it was: no strips, no buffer write, no draw.

use glam::{Quat, Vec3};

use crate::canvas::MAX_BARS;
use crate::immerse::{QUIET_HOLD_S, QUIET_LOUDNESS};
use crate::surfaces::{Face, SURFACE_LANE_ROWS, acting_face, is_plane};

/// The knobs' names.
pub const KNOB: &str = "debug.fosfora.edgebars";
pub const KNOB_GAIN: &str = "debug.fosfora.edgebarsgain";
pub const KNOB_HEIGHT: &str = "debug.fosfora.edgebarsheight";

/// How far off the face the strips sit (m), plus the pipeline's depth bias:
/// the highlight's, as the face's occluder writes depth at the face.
pub const LIFT_M: f32 = crate::highlight::LIFT_M;
/// The tallest bar against the face's smaller side
/// (`debug.fosfora.edgebarsheight`), and the knob's range.
pub const HEIGHT_FRACTION: f32 = 0.12;
pub const HEIGHT_FRACTION_MAX: f32 = 0.5;
/// The tallest bar's height is clamped to this range (m).
pub const MIN_HEIGHT_M: f32 = 0.02;
pub const MAX_HEIGHT_M: f32 = 0.20;
/// No bar is thinner than this along its edge (m): a shorter edge merges
/// neighboring bars.
pub const MIN_BAR_M: f32 = 0.03;
/// The peak alpha at immersion level 0 and at level 1.
pub const ALPHA_LOW: f32 = 0.25;
pub const ALPHA_HIGH: f32 = 0.7;
/// The gain knob's range.
pub const GAIN_MAX: f32 = 4.0;
/// The line along the whole edge: its width (m) and its light.
pub const LINE_M: f32 = 0.004;
pub const LINE_LIGHT: f32 = 0.6;
/// A bar's share of its slot and the softness of its sides (a fraction of
/// the slot): the wall spectrum's.
pub const BAR_FILL: f32 = crate::canvas::BAR_FILL;
pub const SIDE_SOFT: f32 = crate::canvas::SIDE_SOFT;
/// The softness of a bar's top and the cap line's width, as fractions of
/// the tallest bar (at least a pixel in the shader).
pub const TOP_SOFT: f32 = 0.06;
pub const CAP_WIDTH: f32 = 0.05;
/// A strip's height against the tallest bar's: room for the cap's glow
/// above a full bar.
pub const REACH: f32 = 1.1;
/// Other boxes are grown by this when an edge is moved clear of them (m).
pub const CLEAR_MARGIN_M: f32 = 0.005;
/// An edge moves in by at most this (m).
pub const MAX_CLEAR_M: f32 = 0.06;
/// A face narrower than twice this (m) once its edges are clear draws
/// nothing.
pub const MIN_FACE_HALF_M: f32 = 0.01;
/// How long the outline takes to come when the music does, and to go in
/// silence (s).
pub const PRESENCE_S: f32 = 0.3;
/// The strips a frame holds: three faces of four edges for each of the
/// obstacle block's boxes.
pub const MAX_STRIPS: usize = SURFACE_LANE_ROWS * 3 * 4;
/// Rows of one strip, `struct Strip` in [`EDGE_BARS_WGSL`]: the four
/// corners, then the color.
pub const STRIP_ROWS: usize = 5;
/// Rows of the bars' running sums (`MAX_BARS + 1` of them, four to a row).
pub const SUM_ROWS: usize = MAX_BARS / 4 + 1;
/// Rows before the strips: the head row, then the sums.
pub const HEAD_ROWS: usize = 1 + SUM_ROWS;
/// Rows of the uniform, `struct EdgeBars` in [`EDGE_BARS_WGSL`], and its
/// bytes.
pub const UNIFORM_ROWS: usize = HEAD_ROWS + MAX_STRIPS * STRIP_ROWS;
pub const UNIFORM_BYTES: usize = 16 * UNIFORM_ROWS;
const _: () = assert!(
    UNIFORM_BYTES <= 64 << 10,
    "within WebGPU's default uniform binding size, the device's limit"
);
const _: () = assert!(
    REACH >= 1.0 + 2.0 * CAP_WIDTH - 1e-6,
    "a strip holds the cap's glow over a full bar"
);

/// `debug.fosfora.edgebars`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// 0 or unset: no edge bars.
    #[default]
    Off,
    /// 1: bars on each face, growing inward from its edges.
    Inward,
    /// 2: bars standing off each edge along the face's normal.
    Standing,
}

impl Mode {
    /// How the bars grow; `None` when off.
    pub fn growth(self) -> Option<Growth> {
        match self {
            Self::Off => None,
            Self::Inward => Some(Growth::Inward),
            Self::Standing => Some(Growth::Standing),
        }
    }
}

/// Which way a bar grows from its edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Growth {
    /// On the face, toward its center; the strips mitered at the corners.
    Inward,
    /// Off the face, along its normal.
    Standing,
}

/// What the edge bars' knobs ask.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeBarsOptions {
    pub mode: Mode,
    /// `debug.fosfora.edgebarsgain`: multiplies the peak alpha.
    pub gain: f32,
    /// `debug.fosfora.edgebarsheight`: [`HEIGHT_FRACTION`].
    pub height: f32,
}

impl Default for EdgeBarsOptions {
    fn default() -> Self {
        Self {
            mode: Mode::Off,
            gain: 1.0,
            height: HEIGHT_FRACTION,
        }
    }
}

impl EdgeBarsOptions {
    /// The options the three knobs ask, each read through `prop` (a system
    /// property by name, `None` unset). An unreadable value takes the
    /// default; one out of range the nearest end of it.
    pub fn from_knobs(prop: impl Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        let number = |name: &str| {
            prop(name)
                .and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
        };
        Self {
            mode: match prop(KNOB).as_deref().map(str::trim) {
                Some("1") => Mode::Inward,
                Some("2") => Mode::Standing,
                _ => d.mode,
            },
            gain: number(KNOB_GAIN).map_or(d.gain, |v| v.clamp(0.0, GAIN_MAX)),
            height: number(KNOB_HEIGHT).map_or(d.height, |v| v.clamp(0.0, HEIGHT_FRACTION_MAX)),
        }
    }

    /// The state in a few words, for the log.
    pub fn describe(&self) -> String {
        format!(
            "{} · gain {} · tallest bar {} of the face's smaller side, {} to {} m",
            match self.mode {
                Mode::Off => "off",
                Mode::Inward => "on, bars on each face growing in from its edges",
                Mode::Standing => "on, bars standing off each edge along the face's normal",
            },
            self.gain,
            self.height,
            MIN_HEIGHT_M,
            MAX_HEIGHT_M,
        )
    }

    /// The launch line. `boxes`: whether the room's boxes are drawn as
    /// occluders in this run (`debug.fosfora.occluders`).
    pub fn line(&self, boxes: bool) -> String {
        format!("edge bars: {}{}", self.describe(), no_boxes(self, boxes))
    }

    /// The line for a change from `before`.
    pub fn change_line(&self, before: &Self, boxes: bool) -> String {
        format!(
            "edge bars: {}{} (was: {})",
            self.describe(),
            no_boxes(self, boxes),
            before.describe()
        )
    }
}

fn no_boxes(o: &EdgeBarsOptions, boxes: bool) -> &'static str {
    if o.mode != Mode::Off && !boxes {
        " · the room's boxes are not occluders (debug.fosfora.occluders 0), so nothing is outlined"
    } else {
        ""
    }
}

/// Whether music plays, as a level that fades: 1 while it is heard, 0 in
/// silence, moving over [`PRESENCE_S`]. Silence is immersion's quiet
/// (`immerse.rs`): no real audio driving the analysis, or `loudness_m`
/// under [`QUIET_LOUDNESS`] for [`QUIET_HOLD_S`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Presence {
    quiet_s: f32,
    level: f32,
}

/// Silent: nothing draws until the music is heard.
impl Default for Presence {
    fn default() -> Self {
        Self {
            quiet_s: QUIET_HOLD_S,
            level: 0.0,
        }
    }
}

impl Presence {
    /// Once a frame, `dt` seconds after the last: `playing` whether real
    /// audio drives the analysis, `loudness_m` this frame's. Returns the
    /// level.
    pub fn step(&mut self, dt: f32, playing: bool, loudness_m: f32) -> f32 {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let loud = if loudness_m.is_finite() {
            loudness_m
        } else {
            0.0
        };
        self.quiet_s = if !playing {
            QUIET_HOLD_S
        } else if loud < QUIET_LOUDNESS {
            (self.quiet_s + dt).min(QUIET_HOLD_S)
        } else {
            0.0
        };
        let heard = self.quiet_s < QUIET_HOLD_S;
        let step = dt / PRESENCE_S;
        self.level = if heard {
            (self.level + step).min(1.0)
        } else {
            (self.level - step).max(0.0)
        };
        self.level
    }

    pub fn level(&self) -> f32 {
        self.level
    }
}

/// The peak alpha at immersion level `level` (0..1) and gain `gain`: from
/// [`ALPHA_LOW`] at level 0 to [`ALPHA_HIGH`] at 1, so the outline carries
/// more of the look as the room dims.
pub fn peak_alpha(level: f32, gain: f32) -> f32 {
    let level = if level.is_finite() {
        level.clamp(0.0, 1.0)
    } else {
        0.0
    };
    (ALPHA_LOW + (ALPHA_HIGH - ALPHA_LOW) * level) * gain.clamp(0.0, GAIN_MAX)
}

/// The tallest bar on a face of in-plane half extents `half` for the
/// fraction `fraction` of its smaller side: clamped to [`MIN_HEIGHT_M`]..
/// [`MAX_HEIGHT_M`], and never more than the strip a face that small can
/// hold (its smaller half extent over [`REACH`]), so the miters never
/// cross.
pub fn max_height(half: [f32; 2], fraction: f32) -> f32 {
    let small = half[0].min(half[1]).max(0.0);
    (fraction.clamp(0.0, HEIGHT_FRACTION_MAX) * 2.0 * small)
        .clamp(MIN_HEIGHT_M, MAX_HEIGHT_M)
        .min(small / REACH)
}

/// The bars an edge of `length` (m) shows of the canvas's `bars`: all of
/// them while each is at least [`MIN_BAR_M`] wide, fewer on a shorter
/// edge, at least one; none without bars.
pub fn bar_count(length: f32, bars: usize) -> u32 {
    let bars = bars.min(MAX_BARS);
    if bars == 0 {
        return 0;
    }
    let fit = (length.max(0.0) / MIN_BAR_M + 1e-4).floor() as usize;
    fit.clamp(1, bars) as u32
}

/// The height of bar `i` of the `m` on an edge, from the running sums of
/// the canvas's `n` bars (the rows' sums, [`rows`]): the mean of its share
/// of them, split as `canvas::bands_to_bars` splits the mel bands. The
/// WGSL's `merged`.
pub fn merged(sums: &[[f32; 4]], i: u32, m: u32, n: u32) -> f32 {
    if n == 0 || m == 0 {
        return 0.0;
    }
    let lo = (i * n / m).min(n - 1);
    let hi = ((i + 1) * n / m).clamp(lo + 1, n);
    let at = |k: u32| sums[(k / 4) as usize][(k % 4) as usize];
    (at(hi) - at(lo)) / (hi - lo) as f32
}

/// A box the frame trusts as an occluder: its center, rotation (box to
/// world) and half extents, and how its edges look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    pub center: Vec3,
    pub rot: Quat,
    pub half: Vec3,
    /// `None` for a box that occludes but is not outlined: the stage
    /// floor, a hidden wall. Its edges are still moved clear of it.
    pub look: Option<Look>,
}

/// An outlined shape's color (linear RGB) and its fade (0..1: 1 for a
/// scanned box, a live shape's `live_wire::Shown::level`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub color: [f32; 3],
    pub fade: f32,
}

/// One edge's strip, lifted [`LIFT_M`] off its face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strip {
    /// The edge's low end, its high end, then the far side's corners over
    /// the high end and over the low end.
    pub corners: [Vec3; 4],
    /// The edge's length (m).
    pub length: f32,
    /// The tallest bar's height (m); the strip is [`REACH`] times this.
    pub height: f32,
    /// The bars it shows ([`bar_count`]).
    pub bars: u32,
    /// How far in from the edge's ends the far side's ends are (m): the
    /// strip's height for a mitered strip, 0 for a standing one.
    pub inset: f32,
    pub color: [f32; 3],
    /// The peak alpha, the fade in.
    pub alpha: f32,
}

/// What [`strips`] outlined: shapes with at least one strip, and strips.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Drawn {
    pub planes: usize,
    pub volumes: usize,
    pub strips: usize,
}

/// The line logged when what is outlined changes; `silent` while nothing
/// draws for the silence.
pub fn drawn_line(drawn: Drawn, silent: bool) -> String {
    if silent {
        "edge bars: silent, nothing drawn".to_owned()
    } else if drawn.strips == 0 {
        "edge bars: no shape to outline".to_owned()
    } else {
        format!(
            "edge bars: {} planes and {} volumes outlined, {} strips",
            drawn.planes, drawn.volumes, drawn.strips
        )
    }
}

/// The corners of a face in [`Face::corners`]' order, as signs along its
/// two axes.
const SIGNS: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];

/// The faces of a box (`center`, `rot` box to world, `half`) whose edges are
/// outlined for a head at `head`, into `out`: a plane's face toward the
/// head (`surfaces::acting_face`), a volume's faces turned toward it, one
/// to three, none from inside it.
pub fn faces(center: Vec3, rot: Quat, half: Vec3, head: Vec3, out: &mut Vec<Face>) {
    out.clear();
    if is_plane(half) {
        out.push(acting_face(center, rot, half, head));
        return;
    }
    for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
        let n = rot * axis;
        let toward = if n.dot(head - center) >= 0.0 { n } else { -n };
        let face = Face::across(center, rot, half, toward);
        if face.normal.dot(head - face.center) > 0.0 {
            out.push(face);
        }
    }
}

/// Whether the corner of a box (`rot` box to world) in the direction `dir`
/// from its center is a low end: an even number of `dir`'s signs along the
/// box's axes positive. Only the signs count.
pub fn is_low(rot: Quat, dir: Vec3) -> bool {
    let l = rot.inverse() * dir;
    (u32::from(l.x > 0.0) + u32::from(l.y > 0.0) + u32::from(l.z > 0.0)) % 2 == 0
}

/// How far `p` has to move along `inward` (unit) to be out of every box of
/// `shapes` but the one at `skip`, each grown by [`CLEAR_MARGIN_M`]: 0 when
/// it is in none, at most [`MAX_CLEAR_M`].
pub fn clearance(p: Vec3, inward: Vec3, shapes: &[Shape], skip: usize) -> f32 {
    let mut t = 0.0f32;
    // Leaving one box can land the point in another: a few rounds.
    for _ in 0..4 {
        let q = p + inward * t;
        let out = shapes
            .iter()
            .enumerate()
            .filter(|&(j, _)| j != skip)
            .filter_map(|(_, s)| exit_along(q, inward, s))
            .fold(0.0f32, f32::max);
        if out <= 0.0 {
            break;
        }
        t += out;
        if t >= MAX_CLEAR_M {
            break;
        }
    }
    t.min(MAX_CLEAR_M)
}

/// How far along `dir` the point `q` leaves `s` grown by
/// [`CLEAR_MARGIN_M`], when it is inside it.
fn exit_along(q: Vec3, dir: Vec3, s: &Shape) -> Option<f32> {
    let half = s.half + Vec3::splat(CLEAR_MARGIN_M);
    let d = q - s.center;
    if d.length_squared() > half.length_squared() {
        return None;
    }
    let inv = s.rot.inverse();
    let (l, v) = ((inv * d).to_array(), (inv * dir).to_array());
    let h = half.to_array();
    if (0..3).any(|k| l[k].abs() >= h[k]) {
        return None;
    }
    (0..3)
        .filter_map(|k| {
            if v[k] > 1e-6 {
                Some((h[k] - l[k]) / v[k])
            } else if v[k] < -1e-6 {
                Some((-h[k] - l[k]) / v[k])
            } else {
                None
            }
        })
        .reduce(f32::min)
}

/// The strips of every outlined shape of `shapes` seen from `head`, into
/// `out` (at most [`MAX_STRIPS`]): four per face of [`faces`], each edge
/// moved clear of the other shapes ([`clearance`]), the bars growing as
/// `growth` says to at most [`max_height`] for `fraction`, [`bar_count`] of
/// the canvas's `bars` along each, at `peak` alpha times the shape's fade.
pub fn strips(
    shapes: &[Shape],
    head: Vec3,
    growth: Growth,
    fraction: f32,
    bars: usize,
    peak: f32,
    out: &mut Vec<Strip>,
) -> Drawn {
    out.clear();
    let mut drawn = Drawn::default();
    let mut lit = Vec::with_capacity(3);
    for (k, s) in shapes.iter().enumerate() {
        let Some(look) = s.look else {
            continue;
        };
        let alpha = peak * look.fade.clamp(0.0, 1.0);
        if alpha <= 0.0 {
            continue;
        }
        let before = out.len();
        let edges = Edges {
            rot: s.rot,
            skip: k,
            shapes,
            growth,
            fraction,
            bars,
            color: look.color,
            alpha,
        };
        faces(s.center, s.rot, s.half, head, &mut lit);
        for face in &lit {
            edges.push(face, out);
        }
        if out.len() > before {
            if is_plane(s.half) {
                drawn.planes += 1;
            } else {
                drawn.volumes += 1;
            }
        }
    }
    out.truncate(MAX_STRIPS);
    drawn.strips = out.len();
    drawn
}

/// What one face's strips share.
struct Edges<'a> {
    /// The box's rotation, for its corners' ends ([`is_low`]).
    rot: Quat,
    /// The box's own index in `shapes`, which never clears its edges.
    skip: usize,
    shapes: &'a [Shape],
    growth: Growth,
    fraction: f32,
    bars: usize,
    color: [f32; 3],
    alpha: f32,
}

impl Edges<'_> {
    /// `face`'s four strips into `out`, edge `e` from corner `e` to corner
    /// `e + 1` of [`Face::corners`]' order, each run from its low end.
    fn push(&self, face: &Face, out: &mut Vec<Strip>) {
        let [a, b] = face.axes;
        let [hu, hv] = face.half;
        let lift = face.normal * LIFT_M;
        // Each edge's outward direction in the face and the half extent
        // across to it: -b, +a, +b, -a.
        let outward = [-b, a, b, -a];
        let across = [hv, hu, hv, hu];
        let clear: [f32; 4] = std::array::from_fn(|e| {
            let mid = face.center + lift + outward[e] * across[e];
            clearance(mid, -outward[e], self.shapes, self.skip)
        });
        let hu = hu - 0.5 * (clear[3] + clear[1]);
        let hv = hv - 0.5 * (clear[0] + clear[2]);
        if hu < MIN_FACE_HALF_M || hv < MIN_FACE_HALF_M {
            return;
        }
        let center = face.center
            + lift
            + a * (0.5 * (clear[3] - clear[1]))
            + b * (0.5 * (clear[0] - clear[2]));
        let height = max_height([hu, hv], self.fraction);
        let reach = height * REACH;
        let near = SIGNS.map(|[su, sv]| center + a * (su * hu) + b * (sv * hv));
        // The growth, here alone: the far side of every strip.
        let (far, inset) = match self.growth {
            Growth::Inward => (
                SIGNS.map(|[su, sv]| center + a * (su * (hu - reach)) + b * (sv * (hv - reach))),
                reach,
            ),
            Growth::Standing => (near.map(|c| c + face.normal * reach), 0.0),
        };
        for e in 0..4 {
            let (i, j) = (e, (e + 1) % 4);
            let [su, sv] = SIGNS[i];
            let (lo, hi) = if is_low(self.rot, face.normal + a * su + b * sv) {
                (i, j)
            } else {
                (j, i)
            };
            let length = 2.0 * if e % 2 == 0 { hu } else { hv };
            out.push(Strip {
                corners: [near[lo], near[hi], far[hi], far[lo]],
                length,
                height,
                bars: bar_count(length, self.bars),
                inset,
                color: self.color,
                alpha: self.alpha,
            });
        }
    }
}

/// The uniform rows for the canvas's bar `heights` (0..1, at most
/// [`MAX_BARS`]) and `strips` (at most [`MAX_STRIPS`]), into `out`: the
/// head row (x the bar count), the bars' running sums (the k-th the first
/// k heights summed, four to a row), then per strip its four corners, w
/// the length, the tallest bar's height, the bar count and the inset, and
/// its color with the peak alpha. Only these rows are written: the draw
/// reads no strip past them.
pub fn rows(heights: &[f32], strips: &[Strip], out: &mut Vec<[f32; 4]>) {
    out.clear();
    let n = heights.len().min(MAX_BARS);
    out.push([n as f32, 0.0, 0.0, 0.0]);
    let mut sums = [[0.0f32; 4]; SUM_ROWS];
    let mut total = 0.0f32;
    for k in 0..SUM_ROWS * 4 {
        sums[k / 4][k % 4] = total;
        if let Some(&h) = heights.get(k).filter(|_| k < n) {
            total += if h.is_finite() {
                h.clamp(0.0, 1.0)
            } else {
                0.0
            };
        }
    }
    out.extend_from_slice(&sums);
    for s in strips.iter().take(MAX_STRIPS) {
        let w = [s.length, s.height, s.bars as f32, s.inset];
        for (c, w) in s.corners.iter().zip(w) {
            out.push([c.x, c.y, c.z, w]);
        }
        out.push([s.color[0], s.color[1], s.color[2], s.alpha]);
    }
}

/// WGSL's `smoothstep`.
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The light the fragment shader gives strip `k` of `rows` at `(s, d)`: m
/// along its edge from the low end, m off the edge toward the far side;
/// 0..1, before the alpha, without the antialiasing (a pixel taken as 0
/// wide): the bars' look, the line along the edge under them.
pub fn light_at(rows: &[[f32; 4]], k: usize, s: f32, d: f32) -> f32 {
    let base = HEAD_ROWS + k * STRIP_ROWS;
    let len = rows[base][3].max(1e-4);
    let height = rows[base + 1][3].max(1e-4);
    let m = ((rows[base + 2][3] + 0.5) as u32).max(1);
    let n = (rows[0][0] + 0.5) as u32;
    let mut light = 0.0;
    if n > 0 {
        let x = (s / len).clamp(0.0, 0.99999) * m as f32;
        let i = (x as u32).min(m - 1);
        let f = x.fract();
        let h = merged(&rows[1..HEAD_ROWS], i, m, n);
        let t = d / height;
        let gap = 0.5 * (1.0 - BAR_FILL);
        let side = smoothstep(gap, gap + SIDE_SOFT, f)
            * (1.0 - smoothstep(1.0 - gap - SIDE_SOFT, 1.0 - gap, f));
        let below = 1.0 - smoothstep(h - TOP_SOFT, h, t);
        let grad = 0.2 + 0.8 * (t / h.max(1e-3)).clamp(0.0, 1.0);
        let c = (t - h) / CAP_WIDTH;
        let cap = (-c * c).exp() * smoothstep(0.0, 0.02, h);
        light = side * (0.7 * below * grad + 0.6 * cap);
    }
    let line = if d < LINE_M { LINE_LIGHT } else { 0.0 };
    light.max(line).clamp(0.0, 1.0)
}

/// The alpha the fragment shader writes at `(s, d)` on strip `k` of `rows`
/// ([`light_at`] times the strip's peak alpha, at most 1).
pub fn alpha_at(rows: &[[f32; 4]], k: usize, s: f32, d: f32) -> f32 {
    let peak = rows[HEAD_ROWS + k * STRIP_ROWS + 4][3];
    (peak * light_at(rows, k, s, d)).clamp(0.0, 1.0)
}

/// The edge bars' shader: group 0 is the eye pass's shared camera
/// (`view_proj`), group 1 the rows of [`rows`]. One instance per strip:
/// the vertex stage places the strip from its corners and hands the
/// fragment its point in meters along the edge and off it, and the strip's
/// shape and color unchanged; the fragment writes the bars and the line,
/// premultiplied.
pub const EDGE_BARS_WGSL: &str = r"
struct Eye { view_proj: mat4x4<f32> }
struct Strip {
    // Lifted off the face: the edge's low end, its high end, then the far
    // side's corners over the high end and over the low end. w, in order:
    // the edge's length (m), the tallest bar's height (m), the bar count,
    // how far in from the edge's ends the far side's ends are (m).
    corners: array<vec4<f32>, 4>,
    // rgb the color, w the peak alpha
    color: vec4<f32>,
}
struct EdgeBars {
    // x the canvas's bar count
    head: vec4<f32>,
    // the bars' running sums, four to a row: the k-th is the first k bars'
    // heights summed
    sums: array<vec4<f32>, 17>,
    strips: array<Strip, 384>,
}
@group(0) @binding(0) var<uniform> eye: Eye;
@group(1) @binding(0) var<uniform> edge: EdgeBars;

// A strip's height against the tallest bar's; a bar's share of its slot
// and the softness of its sides (fractions of the slot); the softness of
// its top and the cap's width (fractions of the tallest bar); the line
// along the edge: its width (m) and its light.
const REACH: f32 = 1.1;
const BAR_FILL: f32 = 0.7;
const SIDE_SOFT: f32 = 0.12;
const TOP_SOFT: f32 = 0.06;
const CAP_WIDTH: f32 = 0.05;
const LINE_M: f32 = 0.004;
const LINE_LIGHT: f32 = 0.6;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    // m along the edge from its low end, m off it toward the far side
    @location(0) sd: vec2<f32>,
    // the edge's length (m), the tallest bar's height (m), the bar count
    @location(1) @interpolate(flat) shape: vec3<f32>,
    @location(2) @interpolate(flat) color: vec4<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32, @builtin(instance_index) k: u32) -> VsOut {
    var order = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    let c = order[i];
    let len = edge.strips[k].corners[0].w;
    let height = edge.strips[k].corners[1].w;
    let inset = edge.strips[k].corners[3].w;
    let far = height * REACH;
    var sd = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 0.0), vec2<f32>(len, 0.0),
        vec2<f32>(len - inset, far), vec2<f32>(inset, far),
    );
    var out: VsOut;
    out.pos = eye.view_proj * vec4<f32>(edge.strips[k].corners[c].xyz, 1.0);
    out.sd = sd[c];
    out.shape = vec3<f32>(len, height, edge.strips[k].corners[2].w);
    out.color = edge.strips[k].color;
    return out;
}

// Bar i of the m on an edge: the mean of its share of the canvas's n bars,
// from their running sums (edge_bars.rs has the same in Rust).
fn merged(i: u32, m: u32, n: u32) -> f32 {
    let lo = min(i * n / m, n - 1u);
    let hi = clamp((i + 1u) * n / m, lo + 1u, n);
    let total = edge.sums[hi / 4u][hi % 4u] - edge.sums[lo / 4u][lo % 4u];
    return total / f32(hi - lo);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // A pixel's worth of the distance off the edge (m), taken before any
    // branch, where the derivatives are defined.
    let px = max(fwidth(in.sd.y), 1e-5);
    let len = max(in.shape.x, 1e-4);
    let height = max(in.shape.y, 1e-4);
    let m = max(u32(in.shape.z + 0.5), 1u);
    let n = u32(edge.head.x + 0.5);
    var light = 0.0;
    if n > 0u {
        // The bar under the point, its height and the point's height on
        // it, both in tallest bars.
        let x = clamp(in.sd.x / len, 0.0, 0.99999) * f32(m);
        let i = min(u32(x), m - 1u);
        let f = fract(x);
        let h = merged(i, m, n);
        let t = in.sd.y / height;
        let gap = 0.5 * (1.0 - BAR_FILL);
        let side = smoothstep(gap, gap + SIDE_SOFT, f)
            * (1.0 - smoothstep(1.0 - gap - SIDE_SOFT, 1.0 - gap, f));
        // Below the top, brighter toward it, with a cap line: the wall
        // spectrum's bar.
        let soft = max(TOP_SOFT, px / height);
        let below = 1.0 - smoothstep(h - soft, h, t);
        let grad = mix(0.2, 1.0, clamp(t / max(h, 1e-3), 0.0, 1.0));
        let c = (t - h) / max(CAP_WIDTH, px / height);
        let cap = exp(-c * c) * smoothstep(0.0, 0.02, h);
        light = side * (0.7 * below * grad + 0.6 * cap);
    }
    // The line along the whole edge, at least a pixel wide.
    let w = max(LINE_M, px);
    let line = LINE_LIGHT * (1.0 - smoothstep(w - 0.5 * px, w + 0.5 * px, in.sd.y));
    let a = clamp(in.color.w * clamp(max(light, line), 0.0, 1.0), 0.0, 1.0);
    return vec4<f32>(in.color.rgb * a, a);
}
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::bands_to_bars;

    /// The head of a seated wearer.
    const HEAD: Vec3 = Vec3::new(0.0, 1.2, 0.0);
    const COLOR: [f32; 3] = [0.3, 1.0, 0.5];

    const LOOK: Option<Look> = Some(Look {
        color: COLOR,
        fade: 1.0,
    });

    /// A wall plane as `room.rs` gives one: 4 m wide, 2.5 m tall, a slab
    /// 4 cm thick centered on the wall at z = -2, facing +Z (local Z its
    /// normal).
    fn wall() -> Shape {
        Shape {
            center: Vec3::new(0.0, 1.25, -2.0),
            rot: Quat::IDENTITY,
            half: Vec3::new(2.0, 1.25, 0.02),
            look: LOOK,
        }
    }

    /// A floor plane at y = 0, 4 x 4 m, as a scene floor (local +Z up).
    fn floor() -> Shape {
        Shape {
            center: Vec3::new(0.0, 0.0, 0.0),
            rot: Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            half: Vec3::new(2.0, 2.0, 0.02),
            look: LOOK,
        }
    }

    /// A table: 1.2 x 0.4 x 0.6 m, its top at 0.75 m, 1 m ahead.
    fn table() -> Shape {
        Shape {
            center: Vec3::new(0.0, 0.55, -1.0),
            rot: Quat::IDENTITY,
            half: Vec3::new(0.6, 0.2, 0.3),
            look: LOOK,
        }
    }

    fn run(shapes: &[Shape], head: Vec3, growth: Growth) -> (Vec<Strip>, Drawn) {
        let mut out = Vec::new();
        let drawn = strips(shapes, head, growth, HEIGHT_FRACTION, 24, 0.25, &mut out);
        (out, drawn)
    }

    fn near(a: Vec3, b: Vec3) -> bool {
        a.abs_diff_eq(b, 1e-5)
    }

    #[test]
    fn a_plane_gives_its_room_side_four_edges() {
        let (s, drawn) = run(&[wall()], HEAD, Growth::Inward);
        assert_eq!(
            drawn,
            Drawn {
                planes: 1,
                volumes: 0,
                strips: 4
            }
        );
        // On the room side of the slab (z = -1.98), lifted toward the head.
        for strip in &s {
            for c in strip.corners {
                assert!((c.z - (-1.98 + LIFT_M)).abs() < 1e-5, "{c}");
            }
        }
        // Nothing to clear: the whole face, 4 m and 2.5 m edges, every bar
        // of 24 on each (16.7 and 10.4 cm), the tallest bar clamped to
        // 20 cm (12 % of 2.5 m is 30 cm).
        let mut lengths: Vec<f32> = s.iter().map(|s| s.length).collect();
        lengths.sort_by(f32::total_cmp);
        assert_close!(lengths, [2.5, 2.5, 4.0, 4.0]);
        for strip in &s {
            assert_eq!(strip.bars, 24);
            assert_close!(strip.height, MAX_HEIGHT_M);
            assert_close!(strip.inset, MAX_HEIGHT_M * REACH);
            assert_close!(strip.color, COLOR);
            assert_close!(strip.alpha, 0.25);
        }
        // Seen from behind the wall, its other side.
        let (s, _) = run(&[wall()], Vec3::new(0.0, 1.2, -4.0), Growth::Inward);
        assert_eq!(s.len(), 4);
        assert!(s.iter().all(|s| s.corners.iter().all(|c| c.z < -2.0)));
        // The far side lies on the face, a strip's height in from the edge.
        let (s, _) = run(&[wall()], HEAD, Growth::Inward);
        for strip in &s {
            let [lo, hi, far_hi, far_lo] = strip.corners;
            let along = (hi - lo).normalize();
            let reach = strip.height * REACH;
            assert!(((far_lo - lo).dot(along) - reach).abs() < 1e-5);
            assert!(((far_hi - hi).dot(along) + reach).abs() < 1e-5);
            assert!(((far_lo - lo).length() - reach * std::f32::consts::SQRT_2).abs() < 1e-5);
            assert!(((far_hi - hi).length() - reach * std::f32::consts::SQRT_2).abs() < 1e-5);
            assert!(((far_hi - far_lo).length() - (strip.length - 2.0 * reach)).abs() < 1e-5);
        }
    }

    #[test]
    fn a_volume_shows_the_faces_turned_toward_the_head() {
        let mut lit = Vec::new();
        let t = table();
        // Seated in front of it and above its top: the top and the front.
        faces(t.center, t.rot, t.half, HEAD, &mut lit);
        let normals: Vec<Vec3> = lit.iter().map(|f| f.normal).collect();
        assert_eq!(normals.len(), 2, "{normals:?}");
        assert!(normals.iter().any(|n| near(*n, Vec3::Y)));
        assert!(normals.iter().any(|n| near(*n, Vec3::Z)));
        // Off to its right as well: three faces, twelve strips.
        let side = Vec3::new(1.5, 1.2, 0.0);
        faces(t.center, t.rot, t.half, side, &mut lit);
        assert_eq!(lit.len(), 3);
        let (s, drawn) = run(&[t], side, Growth::Inward);
        assert_eq!(
            drawn,
            Drawn {
                planes: 0,
                volumes: 1,
                strips: 12
            }
        );
        assert_eq!(s.len(), 12);
        // Straight above it: the top alone.
        faces(t.center, t.rot, t.half, Vec3::new(0.0, 2.0, -1.0), &mut lit);
        assert_eq!(lit.len(), 1);
        assert!(near(lit[0].normal, Vec3::Y));
        // From inside it: nothing.
        let (s, drawn) = run(&[t], t.center, Growth::Inward);
        assert!(s.is_empty());
        assert_eq!(drawn, Drawn::default());
        // A box that occludes but is not outlined, and a live shape faded
        // out, give no strips.
        let quiet = [
            Shape { look: None, ..t },
            Shape {
                look: Some(Look {
                    color: COLOR,
                    fade: 0.0,
                }),
                ..wall()
            },
        ];
        assert!(run(&quiet, HEAD, Growth::Inward).0.is_empty());
        // A live shape half faded in: half the alpha.
        let half = Shape {
            look: Some(Look {
                color: COLOR,
                fade: 0.5,
            }),
            ..wall()
        };
        assert!(
            run(&[half], HEAD, Growth::Inward)
                .0
                .iter()
                .all(|s| (s.alpha - 0.125).abs() < 1e-6)
        );
    }

    #[test]
    fn the_direction_alternates_and_the_same_band_meets_itself_at_every_corner() {
        // A table seen from the side, three faces, and a tilted box.
        let tilted = Shape {
            rot: Quat::from_euler(glam::EulerRot::YXZ, 0.4, 0.3, -0.2),
            ..table()
        };
        for shape in [table(), tilted, wall(), floor()] {
            let head = Vec3::new(1.5, 1.2, 0.0);
            let (s, _) = run(&[shape], head, Growth::Inward);
            assert!(!s.is_empty());
            for face in s.chunks(4) {
                // Around the face, edge e from corner e to e + 1: the edges
                // run with the walk (their high end the corner they share
                // with the next) and against it in turn.
                let corners = [
                    face[0].corners[0],
                    face[0].corners[1],
                    face[2].corners[0],
                    face[2].corners[1],
                ];
                let with_walk: Vec<bool> = (0..4)
                    .map(|e| {
                        let next = &face[(e + 1) % 4];
                        let hi = face[e].corners[1];
                        near(hi, next.corners[0]) || near(hi, next.corners[1])
                    })
                    .collect();
                assert_ne!(with_walk[0], with_walk[1], "{with_walk:?}");
                assert_eq!(with_walk[0], with_walk[2]);
                assert_eq!(with_walk[1], with_walk[3]);
                // Each corner of the face is the low end of both its edges
                // or the high end of both.
                for c in corners {
                    let ends: Vec<usize> = face
                        .iter()
                        .filter_map(|s| s.corners[..2].iter().position(|&p| near(p, c)))
                        .collect();
                    assert_eq!(ends.len(), 2, "{c}");
                    assert_eq!(ends[0], ends[1], "{c}");
                }
            }
            // An edge two faces share runs the same way on both: their
            // low ends are the same box corner, a lift apart. Two faces
            // share one edge, three share three; each is seen from both.
            {
                let mut shared = 0;
                for a in &s {
                    for b in &s {
                        let gap = |p: Vec3, q: Vec3| (p - q).length();
                        let lifts = std::f32::consts::SQRT_2 * LIFT_M + 1e-4;
                        if !std::ptr::eq(a, b)
                            && gap(a.corners[0], b.corners[0]) < lifts
                            && gap(a.corners[1], b.corners[1]) < lifts
                        {
                            shared += 1;
                        }
                        // Never the other way round.
                        assert!(
                            !(gap(a.corners[0], b.corners[1]) < lifts
                                && gap(a.corners[1], b.corners[0]) < lifts)
                        );
                    }
                }
                let expect = match s.len() / 4 {
                    2 => 2,
                    3 => 6,
                    _ => 0,
                };
                assert_eq!(shared, expect, "{} faces", s.len() / 4);
            }
        }
    }

    #[test]
    fn edges_move_clear_of_the_boxes_they_run_into() {
        // The wall meets the floor: the floor's slab covers the wall's
        // bottom 2 cm. The stage floor, a 10 cm slab topped at 0, occludes
        // but is not outlined.
        let stage = Shape {
            center: Vec3::new(0.0, -0.05, 0.0),
            rot: Quat::IDENTITY,
            half: Vec3::new(10.0, 0.05, 10.0),
            look: None,
        };
        let (s, drawn) = run(&[wall(), floor(), stage], HEAD, Growth::Inward);
        assert_eq!(drawn.planes, 2);
        assert_eq!(s.len(), 8);
        let wall_strips = &s[..4];
        // The wall's bottom edge sits on the floor's slab grown by the
        // margin (2.5 cm), its top is free: 2.5 m less 2.5 cm tall.
        let bottom = wall_strips
            .iter()
            .flat_map(|s| s.corners)
            .map(|c| c.y)
            .fold(f32::MAX, f32::min);
        assert!((bottom - (0.02 + CLEAR_MARGIN_M)).abs() < 1e-5, "{bottom}");
        let mut lengths: Vec<f32> = wall_strips.iter().map(|s| s.length).collect();
        lengths.sort_by(f32::total_cmp);
        assert_close!(lengths, [2.475, 2.475, 4.0, 4.0]);
        // The floor's far edge runs into the wall's slab (2 cm in front of
        // z = -2), moved 2.5 cm clear: its strips stop at z = -1.975.
        let far = s[4..]
            .iter()
            .flat_map(|s| s.corners)
            .map(|c| c.z)
            .fold(f32::MAX, f32::min);
        assert!((far - (-2.0 + 0.02 + CLEAR_MARGIN_M)).abs() < 1e-5, "{far}");
        // A free point needs nothing; one deep in a box moves at most the
        // cap; an edge in two boxes leaves both.
        assert_close!(
            clearance(Vec3::new(0.0, 1.0, 0.0), Vec3::Y, &[wall()], 9),
            0.0
        );
        let deep = Shape {
            center: Vec3::ZERO,
            rot: Quat::IDENTITY,
            half: Vec3::splat(1.0),
            look: None,
        };
        assert_close!(clearance(Vec3::ZERO, Vec3::Y, &[deep], 9), MAX_CLEAR_M);
        let slab = |y: f32| Shape {
            center: Vec3::new(0.0, y, 0.0),
            rot: Quat::IDENTITY,
            half: Vec3::new(1.0, 0.01, 1.0),
            look: None,
        };
        let t = clearance(Vec3::ZERO, Vec3::Y, &[slab(0.0), slab(0.02)], 9);
        assert!((t - (0.03 + CLEAR_MARGIN_M)).abs() < 1e-5, "{t}");
        // A box never clears its own edges.
        assert_close!(clearance(Vec3::ZERO, Vec3::Y, &[deep], 0), 0.0);
    }

    #[test]
    fn standing_bars_rise_off_the_edge_along_the_normal() {
        let (s, _) = run(&[table()], Vec3::new(0.0, 2.0, -1.0), Growth::Standing);
        assert_eq!(s.len(), 4);
        for strip in &s {
            let [lo, hi, far_hi, far_lo] = strip.corners;
            let up = Vec3::Y * strip.height * REACH;
            assert!(near(far_lo, lo + up) && near(far_hi, hi + up));
            assert_close!(strip.inset, 0.0);
            // On the top's rim, lifted.
            assert!((lo.y - (0.75 + LIFT_M)).abs() < 1e-5);
        }
        // The same edges, ends and bars as the inward strips.
        let (inward, _) = run(&[table()], Vec3::new(0.0, 2.0, -1.0), Growth::Inward);
        for (a, b) in s.iter().zip(&inward) {
            assert!(near(a.corners[0], b.corners[0]) && near(a.corners[1], b.corners[1]));
            assert_eq!(a.bars, b.bars);
        }
    }

    #[test]
    fn a_short_edge_merges_bars_never_under_3_cm() {
        assert_eq!(bar_count(4.0, 24), 24);
        assert_eq!(bar_count(0.72, 24), 24);
        assert_eq!(bar_count(0.71, 24), 23);
        assert_eq!(bar_count(0.5, 24), 16);
        assert_eq!(bar_count(0.1, 24), 3);
        assert_eq!(bar_count(0.02, 24), 1);
        assert_eq!(bar_count(4.0, 0), 0);
        assert_eq!(bar_count(10.0, 500), MAX_BARS as u32);
        for cm in 3..400 {
            let len = cm as f32 / 100.0;
            let m = bar_count(len, 24);
            assert!(len / m as f32 >= MIN_BAR_M - 1e-6, "{len} {m}");
        }
        // The merge is the mean of each bar's share, as the canvas splits
        // the mel bands into its bars.
        let heights: Vec<f32> = (0..24).map(|i| (i as f32 * 0.37).fract()).collect();
        let mut rows_out = Vec::new();
        rows(&heights, &[], &mut rows_out);
        let sums = &rows_out[1..HEAD_ROWS];
        for m in 1..=24u32 {
            let mut expect = vec![0.0; m as usize];
            bands_to_bars(&heights, &mut expect);
            let got: Vec<f32> = (0..m).map(|i| merged(sums, i, m, 24)).collect();
            for (g, e) in got.iter().zip(&expect) {
                assert!((g - e).abs() < 1e-5, "m {m}: {got:?} vs {expect:?}");
            }
        }
        assert_close!(merged(sums, 0, 0, 24), 0.0);
        assert_close!(merged(sums, 0, 4, 0), 0.0);
    }

    #[test]
    fn the_tallest_bar_scales_with_the_face_and_is_clamped() {
        // A 40 cm box face: 12 % of 40 cm.
        assert_close!(max_height([0.3, 0.2], HEIGHT_FRACTION), 0.048);
        // A 4 x 2.5 m wall: 30 cm, clamped to 20.
        assert_close!(max_height([2.0, 1.25], HEIGHT_FRACTION), MAX_HEIGHT_M);
        // A 10 cm frame: 1.2 cm, raised to 2.
        assert_close!(max_height([0.05, 0.4], HEIGHT_FRACTION), MIN_HEIGHT_M);
        // A face 3 cm across holds a strip of 1.5 cm at most.
        assert_close!(max_height([0.015, 0.4], HEIGHT_FRACTION), 0.015 / REACH);
        // The knob: a fraction of 0.24 doubles the 40 cm box's; past the
        // range it is clamped, and the miters never cross.
        assert_close!(max_height([0.3, 0.2], 0.24), 0.096);
        assert_close!(max_height([0.3, 0.2], 9.0), 0.2 / REACH);
        assert_close!(max_height([0.3, 0.2], -1.0), MIN_HEIGHT_M);
        for h in [0.01, 0.05, 0.2, 0.6, 2.0] {
            for f in [0.0, 0.12, 0.3, 0.5] {
                assert!(max_height([h, 3.0], f) * REACH <= h + 1e-6);
            }
        }
    }

    #[test]
    fn the_alpha_follows_the_level_the_gain_and_the_fade() {
        assert_close!(peak_alpha(0.0, 1.0), ALPHA_LOW);
        assert_close!(peak_alpha(1.0, 1.0), ALPHA_HIGH);
        assert_close!(peak_alpha(0.5, 1.0), 0.475);
        assert_close!(peak_alpha(0.5, 2.0), 0.95);
        assert_close!(peak_alpha(3.0, 1.0), ALPHA_HIGH);
        assert_close!(peak_alpha(-1.0, 1.0), ALPHA_LOW);
        assert_close!(peak_alpha(f32::NAN, 1.0), ALPHA_LOW);
        assert_close!(peak_alpha(1.0, 0.0), 0.0);
        assert_close!(peak_alpha(0.0, 100.0), ALPHA_LOW * GAIN_MAX);
        // The fade multiplies in per shape (`strips`), and the alpha the
        // shader writes is capped at 1.
        let rows_out = one_strip(&[1.0], 1, 3.0);
        assert_close!(alpha_at(&rows_out, 0, 0.2, 0.1), 1.0);
    }

    /// The rows of one strip, 0.4 m long with `bars` bars, its tallest bar
    /// 10 cm, at peak alpha `peak`, over the canvas's `heights`.
    fn one_strip(heights: &[f32], bars: u32, peak: f32) -> Vec<[f32; 4]> {
        let strip = Strip {
            corners: [
                Vec3::ZERO,
                Vec3::new(0.4, 0.0, 0.0),
                Vec3::new(0.4 - 0.11, 0.11, 0.0),
                Vec3::new(0.11, 0.11, 0.0),
            ],
            length: 0.4,
            height: 0.1,
            bars,
            inset: 0.11,
            color: COLOR,
            alpha: peak,
        };
        let mut out = Vec::new();
        rows(heights, &[strip], &mut out);
        out
    }

    #[test]
    fn the_rows_carry_the_sums_and_the_strips() {
        let heights = [1.0, 0.5, 0.0, 0.25, 2.0, f32::NAN];
        let r = one_strip(&heights, 4, 0.25);
        assert_eq!(r.len(), HEAD_ROWS + STRIP_ROWS);
        assert_close!(r[0], [6.0, 0.0, 0.0, 0.0]);
        // Running sums, the out-of-range heights clamped, held past the
        // last bar.
        assert_close!(r[1], [0.0, 1.0, 1.5, 1.5]);
        assert_close!(r[2], [1.75, 2.75, 2.75, 2.75]);
        assert_close!(r[HEAD_ROWS - 1], [2.75, 2.75, 2.75, 2.75]);
        let base = HEAD_ROWS;
        assert_close!(r[base], [0.0, 0.0, 0.0, 0.4]);
        assert_close!(r[base + 1], [0.4, 0.0, 0.0, 0.1]);
        assert_close!(r[base + 2], [0.29, 0.11, 0.0, 4.0]);
        assert_close!(r[base + 3], [0.11, 0.11, 0.0, 0.11]);
        assert_close!(r[base + 4], [COLOR[0], COLOR[1], COLOR[2], 0.25]);
        // No strips: the head and the sums alone; past the capacity, cut.
        let mut out = Vec::new();
        rows(&[], &[], &mut out);
        assert_eq!(out.len(), HEAD_ROWS);
        assert_close!(out[0][0], 0.0);
        let many = vec![
            Strip {
                corners: [Vec3::ZERO; 4],
                length: 1.0,
                height: 0.1,
                bars: 1,
                inset: 0.0,
                color: COLOR,
                alpha: 1.0,
            };
            MAX_STRIPS + 5
        ];
        rows(&[0.5], &many, &mut out);
        assert_eq!(out.len(), UNIFORM_ROWS);
    }

    #[test]
    fn the_fragment_draws_bars_from_the_edge_with_a_cap_and_a_line() {
        // Four bars 10 cm wide over a canvas of four: full, half, none, a
        // quarter; the tallest bar 10 cm; peak alpha 0.25.
        let r = one_strip(&[1.0, 0.5, 0.0, 0.25], 4, 0.25);
        // The middle of the full bar, half way up: under its top, 0.6 of
        // the way from the base's glow to the tip's.
        assert_close!(light_at(&r, 0, 0.05, 0.05), 0.7 * 0.6);
        assert_close!(alpha_at(&r, 0, 0.05, 0.05), 0.25 * 0.7 * 0.6);
        // At its top, the cap; at the strip's far side, the cap's tail
        // under 2 %.
        assert_close!(light_at(&r, 0, 0.05, 0.1), 0.6);
        assert!(light_at(&r, 0, 0.05, 0.1 * REACH) < 0.02);
        // The half bar ends half way: the cap at 5 cm, dark above.
        assert_close!(light_at(&r, 0, 0.15, 0.05), 0.6);
        assert!(light_at(&r, 0, 0.15, 0.08) < 1e-6);
        // Brightest at the tip: rising up the bar.
        let up: Vec<f32> = (1..9)
            .map(|k| light_at(&r, 0, 0.05, k as f32 * 0.01))
            .collect();
        assert!(up.windows(2).all(|w| w[1] > w[0]), "{up:?}");
        // The empty bar shows only the line along the edge.
        assert_close!(light_at(&r, 0, 0.25, 0.002), LINE_LIGHT);
        assert_close!(light_at(&r, 0, 0.25, 0.01), 0.0);
        // The line runs through the gaps between the bars too; the bar's
        // light does not.
        assert_close!(light_at(&r, 0, 0.0995, 0.002), LINE_LIGHT);
        assert_close!(light_at(&r, 0, 0.0995, 0.05), 0.0);
        // Soft sides: a bar's edge is dimmer than its middle.
        let edge = light_at(&r, 0, 0.1 * (0.15 + 0.06), 0.05);
        assert!(edge > 0.0 && edge < light_at(&r, 0, 0.05, 0.05), "{edge}");
        // Two bars on the same canvas: the means of their halves.
        let two = one_strip(&[1.0, 0.5, 0.0, 0.25], 2, 0.25);
        // Bar 0 is 0.75 tall: its cap 7.5 cm up.
        assert_close!(light_at(&two, 0, 0.1, 0.075), 0.6);
        assert!(light_at(&two, 0, 0.3, 0.05) < 1e-6);
        // Silence on the canvas: the line alone.
        let none = one_strip(&[], 4, 0.25);
        assert_close!(light_at(&none, 0, 0.05, 0.002), LINE_LIGHT);
        assert_close!(light_at(&none, 0, 0.05, 0.05), 0.0);
    }

    #[test]
    fn silence_draws_nothing_and_the_music_brings_the_outline() {
        let dt = 1.0 / 72.0;
        let mut p = Presence::default();
        assert_close!(p.level(), 0.0);
        // Music: in over 0.3 s.
        for _ in 0..11 {
            p.step(dt, true, 0.5);
        }
        assert!(p.level() > 0.4 && p.level() < 1.0, "{}", p.level());
        for _ in 0..12 {
            p.step(dt, true, 0.5);
        }
        assert_close!(p.level(), 1.0);
        // A gap shorter than the quiet hold changes nothing.
        for _ in 0..18 {
            p.step(dt, true, 0.05);
        }
        assert_close!(p.level(), 1.0);
        // Quiet for the hold: out over 0.3 s, to nothing.
        for _ in 0..(0.7 / dt) as usize {
            p.step(dt, true, 0.05);
        }
        assert_close!(p.level(), 0.0);
        // No real audio (the synthetic groove) is silence, at once.
        let mut p = Presence::default();
        for _ in 0..72 {
            p.step(dt, true, 0.5);
        }
        p.step(dt, false, 0.5);
        assert!(p.level() < 1.0);
        for _ in 0..72 {
            p.step(dt, false, 0.5);
        }
        assert_close!(p.level(), 0.0);
        // Bad numbers do not move it.
        let mut p = Presence::default();
        p.step(f32::NAN, true, f32::NAN);
        assert_close!(p.level(), 0.0);
    }

    #[test]
    fn the_knobs_read_off_by_default_and_log_their_state() {
        let none = |_: &str| None::<String>;
        let d = EdgeBarsOptions::from_knobs(none);
        assert_eq!(d, EdgeBarsOptions::default());
        assert_eq!(d.mode, Mode::Off);
        assert_eq!(d.mode.growth(), None);
        let set = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        let o = EdgeBarsOptions::from_knobs(set(&[
            (KNOB, "1"),
            (KNOB_GAIN, "2"),
            (KNOB_HEIGHT, "0.2"),
        ]));
        assert_eq!(o.mode.growth(), Some(Growth::Inward));
        assert_close!([o.gain, o.height], [2.0, 0.2]);
        let o = EdgeBarsOptions::from_knobs(set(&[
            (KNOB, " 2 "),
            (KNOB_GAIN, "99"),
            (KNOB_HEIGHT, "-3"),
        ]));
        assert_eq!(o.mode.growth(), Some(Growth::Standing));
        assert_close!([o.gain, o.height], [GAIN_MAX, 0.0]);
        let o = EdgeBarsOptions::from_knobs(set(&[
            (KNOB, "yes"),
            (KNOB_GAIN, "loud"),
            (KNOB_HEIGHT, "NaN"),
        ]));
        assert_eq!(o, EdgeBarsOptions::default());
        // The log lines.
        let on = EdgeBarsOptions {
            mode: Mode::Inward,
            ..d
        };
        assert!(d.line(true).starts_with("edge bars: off · gain 1"));
        assert!(on.line(true).contains("growing in from its edges"));
        assert!(on.line(false).contains("debug.fosfora.occluders 0"));
        assert!(!d.line(false).contains("occluders"));
        assert!(
            on.change_line(&d, true)
                .ends_with(&format!("(was: {})", d.describe()))
        );
        assert_eq!(
            drawn_line(Drawn::default(), true),
            "edge bars: silent, nothing drawn"
        );
        assert_eq!(
            drawn_line(
                Drawn {
                    planes: 6,
                    volumes: 2,
                    strips: 40
                },
                false
            ),
            "edge bars: 6 planes and 2 volumes outlined, 40 strips"
        );
    }

    /// The depth `math::projection` gives a point `z` meters ahead on the
    /// view axis and `y` off it, with the eye pass's near and far planes
    /// (`xr.rs`: 0.05 and 100 m).
    fn depth(y: f32, z: f32) -> f32 {
        let fov = crate::math::Fov {
            left: -0.9,
            right: 0.9,
            up: 0.9,
            down: -0.9,
        };
        let c = crate::math::projection(fov, 0.05, 100.0) * glam::Vec4::new(0.0, y, -z, 1.0);
        c.z / c.w
    }

    #[test]
    fn the_lift_keeps_a_strip_in_front_of_its_own_face() {
        // A face `below` meters under the eye, seen at distance `dist`
        // along it: the ray through the strip's point (lifted LIFT_M off
        // the face) meets the face behind it at a depth hundreds of float
        // steps further at every angle, grazing (0.3 degrees) to square
        // on, out to 4 m. The occluder writes that depth; the depth bias
        // only pulls the strip nearer.
        let mut fewest = f32::MAX;
        for dist in [0.5f32, 1.0, 2.0, 4.0] {
            for below in [0.02f32, 0.05, 0.2, 0.6, 1.2] {
                let strip = depth(-(below - LIFT_M), dist);
                let face = depth(-below, dist * below / (below - LIFT_M));
                let step = f32::from_bits(face.to_bits() + 1) - face;
                fewest = fewest.min((face - strip) / step);
            }
            // Square on: the face LIFT_M behind the strip.
            let strip = depth(0.0, dist - LIFT_M);
            let face = depth(0.0, dist);
            let step = f32::from_bits(face.to_bits() + 1) - face;
            fewest = fewest.min((face - strip) / step);
        }
        assert!(fewest > 400.0, "{fewest}");
    }

    fn module() -> naga::Module {
        let module = naga::front::wgsl::parse_str(EDGE_BARS_WGSL).expect("edge bars WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("edge bars WGSL validates");
        module
    }

    fn span(module: &naga::Module, name: &str) -> usize {
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("struct {name}"));
        let naga::TypeInner::Struct { span, .. } = ty.inner else {
            panic!("{name} is not a struct");
        };
        span as usize
    }

    fn wgsl_const(module: &naga::Module, name: &str) -> f32 {
        let (_, c) = module
            .constants
            .iter()
            .find(|(_, c)| c.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("const {name}"));
        match module.global_expressions[c.init] {
            naga::Expression::Literal(naga::Literal::F32(v)) => v,
            ref e => panic!("{name}: {e:?}"),
        }
    }

    #[test]
    fn the_shader_validates_and_matches_the_rows_and_the_constants() {
        let module = module();
        assert_eq!(span(&module, "Strip"), STRIP_ROWS * 16);
        assert_eq!(span(&module, "EdgeBars"), UNIFORM_ROWS * 16);
        assert_eq!(UNIFORM_BYTES, 31_008);
        assert_eq!(SUM_ROWS, 17, "sums: array<vec4<f32>, 17>");
        assert_eq!(MAX_STRIPS, 384, "strips: array<Strip, 384>");
        for (name, value) in [
            ("REACH", REACH),
            ("BAR_FILL", BAR_FILL),
            ("SIDE_SOFT", SIDE_SOFT),
            ("TOP_SOFT", TOP_SOFT),
            ("CAP_WIDTH", CAP_WIDTH),
            ("LINE_M", LINE_M),
            ("LINE_LIGHT", LINE_LIGHT),
        ] {
            assert_close!(wgsl_const(&module, name), value, "{name}");
        }
    }
}
