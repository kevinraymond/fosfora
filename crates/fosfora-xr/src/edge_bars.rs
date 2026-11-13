//! Edge bars (board #5420): the room's outline drawn as the spectrum. Bars
//! rise and fall along the edges of each plane and box the app trusts as
//! an occluder, so the room's geometry is drawn in sound. Plain numbers in,
//! so it builds and tests on the desktop; `app.rs` assembles the shapes and
//! the outline once a frame and `gfx.rs` draws it with [`EDGE_BARS_WGSL`]
//! from the rows [`rows`] packs: one buffer written once a frame, one
//! instanced draw per eye for the strips and one for the tunnels.
//!
//! **Which edges** ([`outline`]). The shapes are the boxes the frame pushes
//! into the obstacle block, taken where `app.rs` assembles them, since the
//! occluder uniform carries neither a hidden wall's flag nor a live shape's
//! fade: the scanned room's boxes, the live planes and closed boxes in the
//! show, then the stage floor. The stage floor and hidden walls occlude but
//! are not outlined ([`Shape::look`] `None`); the hands are not boxes. None
//! while `debug.fosfora.occluders` is 0. A plane (a thin box,
//! `surfaces::is_plane`) has one face, the one toward the head
//! (`surfaces::acting_face`: a wall's room side); a volume has the faces
//! turned toward the head ([`faces`], one to three). Each face gives four
//! strips, one per edge, or in the in mode one tunnel.
//!
//! **Each shape in its own mode** ([`Look::mode`], board #5431). A theme
//! names a [`Mode`] per kind of surface (`themes.rs`): off, on the face,
//! standing or in. The Edges control and `debug.fosfora.edgebars` either
//! follow the theme or force one mode on every kind ([`Choice`]); the
//! control's choice holds until the knob's value changes ([`Override`]).
//! Mixed modes in one frame stay one buffer write and at most two draws
//! per eye, as faces on the face and standing are both strips.
//!
//! **Clear of the other boxes** ([`clearance`]). A scene plane is a slab
//! 4 cm thick centered on the real surface, so where a wall meets the floor
//! or another wall, the neighbor's slab covers the first 2 cm of the face
//! along that edge and would hide what is drawn there. Each edge moves in
//! along its face until the point midway along it, lifted as the strips
//! are, is out of every other box grown by [`CLEAR_MARGIN_M`], by at most
//! [`MAX_CLEAR_M`]: 2.5 cm at a wall's junctions, nothing on a free edge.
//! The tunnel's opening is the face moved clear the same way.
//!
//! **The growth** ([`Growth`], chosen in [`Mode::growth`] and applied in one
//! place). On the face (`edgebars 1`): the bars lie on the face and grow
//! inward from its edge ([`Growth::Inward`]), so whatever stands in front
//! of the surface hides them as it hides the surface. Each strip is a
//! trapezoid mitered at 45 degrees at the face's corners, so neighboring
//! strips meet without overlapping. On a volume, an edge shared by two
//! visible faces gets bars on both, which reads as the edge itself
//! glowing. Standing (`edgebars 2`): the bars stand off the edge along the
//! face's normal ([`Growth::Standing`]), a fence of light on a table's rim,
//! with no miter.
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
//! **In** (`edgebars 3`, board #5431): an LED infinity mirror. Each face
//! becomes a window into a tunnel ([`Tunnel`]): a box behind the face with
//! the face's outline as its opening, its four inner walls running straight
//! back along the inward normal, [`tunnel_depth`] deep ([`DEPTH_FRACTION`]
//! of the face's smaller side, [`MIN_DEPTH_M`] to [`MAX_DEPTH_M`]). Nothing
//! is drawn behind the face: the face's own occluder writes depth there,
//! and whatever stands in front of the face must still hide the tunnel. The
//! face's quad is drawn instead, lifted and depth-tested as the strips are,
//! and its fragment finds where the view ray through it meets the inner
//! walls ([`tunnel_hit`]). The eye comes from each eye's own camera
//! ([`eye_position`]), so the tunnel is drawn at its true place for each
//! eye and each head position: lean and the far rungs shift against the
//! rim, which holds with one eye, by motion alone. The ray crossed the
//! opening inside it, so it leaves the box through the wall ahead of it
//! along each axis, or through the end: the nearer of those two walls is
//! the one it meets, which is the nearest of the four planes with the two
//! behind the ray left out. That gives a wall, a point along its edge and a
//! depth. Where the ray reaches the end without meeting a wall nothing is
//! drawn (alpha 0), so the center stays dark and the passthrough or the
//! surface's own effect shows there.
//!
//! **Rungs** ([`rung_light`]). On each inner wall the spectrum runs along
//! the edge as on the face: the same bands, ends, merging and corners. A
//! bar is a stack of lit rungs at even depths, like the mirror's receding
//! frames: [`RUNGS`] at full height, each [`RUNG_FILL`] of the spacing wide
//! and lit where its bar reached its depth (a bar's height times the
//! tunnel's depth). The rungs line up across the four walls, so a loud
//! moment reads as whole frames lit deep into the wall and a quiet one as
//! the rim alone. The rim rung runs along the whole edge while the music
//! plays, brighter where its bar is loud. Each rung is [`RUNG_DIM`] of the
//! one in front, so the far end fades into dark. A rung thinner than a
//! pixel dims by the share of the pixel it covers ([`rung_cover`]) instead
//! of flickering, and a face seen nearly edge-on fades out over
//! [`GRAZE_LO`] to [`GRAZE_HI`] of the cosine to its normal ([`graze`]).
//!
//! **Sound travels in** ([`History`]). Rung k shows the bars from k steps
//! of [`HISTORY_STEP_S`] ago, so a hit starts at the rim and travels into
//! the wall a rung a step. The history keeps each bar's loudest per step,
//! stepped by time, and its rows' running sums go in the same uniform as
//! this frame's, one block per rung.
//!
//! **Occlusion.** In the eye pass after every occluder (the depth map, the
//! boxes, the hand mesh) and after the lit faces, before the sprites, on
//! the surfaces pass's terms: depth-tested, no depth write, the same depth
//! bias, the strips and the tunnel's quad lifted [`LIFT_M`] off the face.
//! The face's own box never hides them: a face is outlined only while it
//! is turned toward the head, and the lift alone keeps a strip hundreds of
//! depth steps in front of its face at 4 m and at grazing angles (the
//! tests).
//!
//! **Cost.** One quad per strip or tunnel, instanced from one uniform
//! written once a frame (only its used rows), at most two draws per eye, no
//! loop in either fragment. At most [`MAX_STRIPS`] strips and tunnels
//! together (32 boxes, three faces of four edges each): [`UNIFORM_BYTES`]
//! bytes, of which a frame writes 2,736 (the head and ten rungs of sums)
//! plus 80 per strip or tunnel. A strip of length L and height h covers
//! (L - h) h, so a face's strips cover 27 % of a 4 x 2.5 m wall, up to
//! 46 % of a square face under 1.67 m, and all of a face under 4.4 cm
//! across; the walls, floor and ceiling of a 4 x 4 x 2.5 m room 24 %.
//! Strips write no depth and never overlap on one face, so a pixel is
//! shaded about once. A tunnel covers its whole face: seated in a room
//! whose walls and ceiling go in, that is most of each eye's pixels, each
//! shaded once (what stands in front fails the depth test). Its fragment
//! does one intersection (two divisions and a minimum), one rung lookup
//! (a floor), the bar's mean from two running sums of that rung's row, two
//! smoothsteps across the slot, a power for the dimming and a square root
//! for the grazing fade: no texture, no loop.
//!
//! **Knobs**, read at launch and polled once a second
//! ([`EdgeBarsOptions::from_knobs`]): `debug.fosfora.edgebars` (unset each
//! kind in its theme's mode; 0 off, 1 on the face, 2 standing, 3 in, each
//! forced on every kind), `edgebarsgain` (0 to [`GAIN_MAX`], default 1),
//! `edgebarsheight` (the fraction, 0 to [`HEIGHT_FRACTION_MAX`], default
//! [`HEIGHT_FRACTION`]) and `edgebarsdepth` (the tunnel's fraction, 0 to
//! [`DEPTH_FRACTION_MAX`], default [`DEPTH_FRACTION`]). Off, the frame is
//! what it was: no strips, no buffer write, no draw.

use glam::{Mat4, Quat, Vec3};

use crate::canvas::MAX_BARS;
use crate::immerse::{QUIET_HOLD_S, QUIET_LOUDNESS};
use crate::surfaces::{Face, SURFACE_LANE_ROWS, acting_face, is_plane};

/// The knobs' names.
pub const KNOB: &str = "debug.fosfora.edgebars";
pub const KNOB_GAIN: &str = "debug.fosfora.edgebarsgain";
pub const KNOB_HEIGHT: &str = "debug.fosfora.edgebarsheight";
pub const KNOB_DEPTH: &str = "debug.fosfora.edgebarsdepth";

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
/// The in mode's tunnel: its depth against the face's smaller side
/// (`debug.fosfora.edgebarsdepth`), the knob's range, and the clamps (m).
pub const DEPTH_FRACTION: f32 = 0.6;
pub const DEPTH_FRACTION_MAX: f32 = 2.0;
pub const MIN_DEPTH_M: f32 = 0.15;
pub const MAX_DEPTH_M: f32 = 1.5;
/// The rungs a full bar lights, evenly spaced from the rim to the tunnel's
/// end, and the history's rows: rung k shows the bars from k steps ago.
pub const RUNGS: usize = 10;
/// A rung's width against the spacing between rungs.
pub const RUNG_FILL: f32 = 0.3;
/// Each rung's light against the one in front of it.
pub const RUNG_DIM: f32 = 0.8;
/// The light of a rung along its whole edge while music plays, before the
/// dimming: every rung is a faint frame, as the copies of a mirror's lights
/// are, so the tunnel reads at any level and the bars light it further.
pub const FRAME_LIGHT: f32 = 0.25;
/// The history's step (s), by time, so 72 and 90 Hz behave alike.
pub const HISTORY_STEP_S: f32 = 0.06;
/// The cosines of the view ray's angle to the face's normal over which a
/// tunnel fades in: none of it at a grazing angle, all of it from about
/// 78 degrees off the normal.
pub const GRAZE_LO: f32 = 0.05;
pub const GRAZE_HI: f32 = 0.2;
/// The records a frame holds, strips and tunnels together (a tunnel takes a
/// strip's rows): three faces of four edges for each of the obstacle
/// block's boxes.
pub const MAX_STRIPS: usize = SURFACE_LANE_ROWS * 3 * 4;
/// Rows of one record, `struct Strip` in [`EDGE_BARS_WGSL`]: the four
/// corners, then the color.
pub const STRIP_ROWS: usize = 5;
/// Rows of the bars' running sums (`MAX_BARS + 1` of them, four to a row).
pub const SUM_ROWS: usize = MAX_BARS / 4 + 1;
/// Rows before the records: the head row, then the sums of each rung's
/// bars, rung 0 (this frame's) first.
pub const HEAD_ROWS: usize = 1 + RUNGS * SUM_ROWS;
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
const _: () = assert!(
    RUNG_FILL > 0.0 && RUNG_FILL < 1.0,
    "rungs with dark between them"
);

/// How one shape's edges draw: what a theme names per kind of surface
/// (`themes.rs`) or the Edges control and `debug.fosfora.edgebars` force on
/// every kind ([`Choice`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// No edge bars.
    Off,
    /// Bars on each face, growing inward from its edges.
    #[default]
    Face,
    /// Bars standing off each edge along the face's normal.
    Standing,
    /// Each face a window into a tunnel behind it, the bars on its inner
    /// walls growing into depth.
    In,
}

impl Mode {
    /// Every mode, in the knob's order (0 to 3).
    pub const ALL: [Self; 4] = [Self::Off, Self::Face, Self::Standing, Self::In];

    /// A word for the log and the Edges control.
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Face => "face",
            Self::Standing => "stand",
            Self::In => "in",
        }
    }

    /// How a strip's bars grow; `None` for no strips (off, or the in
    /// mode's tunnel).
    pub fn growth(self) -> Option<Growth> {
        match self {
            Self::Off | Self::In => None,
            Self::Face => Some(Growth::Inward),
            Self::Standing => Some(Growth::Standing),
        }
    }
}

/// What the Edges control and `debug.fosfora.edgebars` ask: the theme's
/// mode for each kind of surface, or one mode forced on every kind. Not
/// saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Choice {
    /// Unset: each shape in its kind's mode under the room's theme.
    #[default]
    Theme,
    /// The knob's 0 to 3: this mode on every kind.
    Force(Mode),
}

impl Choice {
    /// The Edges control's cycle: theme, off, face, stand, in, theme.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Theme => Self::Force(Mode::Off),
            Self::Force(Mode::Off) => Self::Force(Mode::Face),
            Self::Force(Mode::Face) => Self::Force(Mode::Standing),
            Self::Force(Mode::Standing) => Self::Force(Mode::In),
            Self::Force(Mode::In) => Self::Theme,
        }
    }

    /// The Edges control's words, short enough for half a cell.
    pub fn label(self) -> &'static str {
        match self {
            Self::Theme => "Edges theme",
            Self::Force(Mode::Off) => "Edges off",
            Self::Force(Mode::Face) => "Edges face",
            Self::Force(Mode::Standing) => "Edges stand",
            Self::Force(Mode::In) => "Edges in",
        }
    }

    /// The mode a shape draws in when its kind's mode under the theme is
    /// `theme`.
    pub fn mode(self, theme: Mode) -> Mode {
        match self {
            Self::Theme => theme,
            Self::Force(m) => m,
        }
    }

    /// Whether it forces the bars off everywhere: then nothing is drawn,
    /// written or stepped.
    pub fn off(self) -> bool {
        self == Self::Force(Mode::Off)
    }
}

/// The Edges control over the knob: the control's choice holds until the
/// knob's own value changes, and then the knob's holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Override {
    /// The knob's choice when last polled.
    knob: Choice,
    /// The control's, until the knob changes.
    menu: Option<Choice>,
}

impl Override {
    /// At launch, on the knob's choice.
    pub fn new(knob: Choice) -> Self {
        Self { knob, menu: None }
    }

    /// The control pressed to `choice`.
    pub fn press(&mut self, choice: Choice) {
        self.menu = Some(choice);
    }

    /// The knob polled at `knob`: a change clears the control's choice.
    /// Returns the choice in force.
    pub fn poll(&mut self, knob: Choice) -> Choice {
        if knob != self.knob {
            self.knob = knob;
            self.menu = None;
        }
        self.choice()
    }

    /// The choice in force.
    pub fn choice(&self) -> Choice {
        self.menu.unwrap_or(self.knob)
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
    pub choice: Choice,
    /// `debug.fosfora.edgebarsgain`: multiplies the peak alpha.
    pub gain: f32,
    /// `debug.fosfora.edgebarsheight`: [`HEIGHT_FRACTION`].
    pub height: f32,
    /// `debug.fosfora.edgebarsdepth`: [`DEPTH_FRACTION`].
    pub depth: f32,
}

impl Default for EdgeBarsOptions {
    fn default() -> Self {
        Self {
            choice: Choice::Theme,
            gain: 1.0,
            height: HEIGHT_FRACTION,
            depth: DEPTH_FRACTION,
        }
    }
}

impl EdgeBarsOptions {
    /// The options the four knobs ask, each read through `prop` (a system
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
            choice: match prop(KNOB).as_deref().map(str::trim) {
                Some("0") => Choice::Force(Mode::Off),
                Some("1") => Choice::Force(Mode::Face),
                Some("2") => Choice::Force(Mode::Standing),
                Some("3") => Choice::Force(Mode::In),
                _ => d.choice,
            },
            gain: number(KNOB_GAIN).map_or(d.gain, |v| v.clamp(0.0, GAIN_MAX)),
            height: number(KNOB_HEIGHT).map_or(d.height, |v| v.clamp(0.0, HEIGHT_FRACTION_MAX)),
            depth: number(KNOB_DEPTH).map_or(d.depth, |v| v.clamp(0.0, DEPTH_FRACTION_MAX)),
        }
    }

    /// The state in a few words, for the log.
    pub fn describe(&self) -> String {
        format!(
            "{} · gain {} · tallest bar {} of the face's smaller side, {} to {} m · tunnel {} of it, {} to {} m",
            match self.choice {
                Choice::Theme => "on, each kind of surface in its theme's mode",
                Choice::Force(Mode::Off) => "off",
                Choice::Force(Mode::Face) => "on, bars on each face growing in from its edges",
                Choice::Force(Mode::Standing) =>
                    "on, bars standing off each edge along the face's normal",
                Choice::Force(Mode::In) => "on, each face a tunnel with the bars growing into it",
            },
            self.gain,
            self.height,
            MIN_HEIGHT_M,
            MAX_HEIGHT_M,
            self.depth,
            MIN_DEPTH_M,
            MAX_DEPTH_M,
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
    if !o.choice.off() && !boxes {
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

/// The in mode's tunnel depth behind a face of in-plane half extents
/// `half` for the fraction `fraction` of its smaller side: clamped to
/// [`MIN_DEPTH_M`]..[`MAX_DEPTH_M`], so a 40 cm box and a 4 m wall both
/// read as a tunnel.
pub fn tunnel_depth(half: [f32; 2], fraction: f32) -> f32 {
    let small = half[0].min(half[1]).max(0.0);
    let fraction = if fraction.is_finite() {
        fraction.clamp(0.0, DEPTH_FRACTION_MAX)
    } else {
        DEPTH_FRACTION
    };
    (fraction * 2.0 * small).clamp(MIN_DEPTH_M, MAX_DEPTH_M)
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

/// An outlined shape's color (linear RGB), its fade (0..1: 1 for a
/// scanned box, a live shape's `live_wire::Shown::level`) and the mode its
/// edges draw in (its kind's under the theme, or the one forced).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub color: [f32; 3],
    pub fade: f32,
    pub mode: Mode,
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

/// One face's tunnel in the in mode: a box behind the face with the face's
/// outline (moved clear of its neighbors) as its opening, its four inner
/// walls running straight back along the face's inward normal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tunnel {
    /// The opening's center, on the face (not lifted).
    pub center: Vec3,
    /// The face's two in-plane axes (unit) and their half extents.
    pub axes: [Vec3; 2],
    pub half: [f32; 2],
    /// The face's outward normal: the tunnel runs along its opposite.
    pub normal: Vec3,
    /// How deep the tunnel is (m, [`tunnel_depth`]).
    pub depth: f32,
    /// The bars on the edges along the first axis and along the second
    /// ([`bar_count`]).
    pub bars: [u32; 2],
    /// Whether the opening's corner at `-axes[0] - axes[1]` is a low end
    /// ([`is_low`]); the other corners alternate from it.
    pub low: bool,
    pub color: [f32; 3],
    /// The peak alpha, the fade in.
    pub alpha: f32,
}

/// What a frame draws: the strips of the faces on the face or standing,
/// then the tunnels of the faces in. Reused each frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outline {
    pub strips: Vec<Strip>,
    pub tunnels: Vec<Tunnel>,
}

impl Outline {
    pub fn clear(&mut self) {
        self.strips.clear();
        self.tunnels.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.strips.is_empty() && self.tunnels.is_empty()
    }
}

/// What [`outline`] drew: shapes with at least one strip or tunnel, the
/// strips and the tunnels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Drawn {
    pub planes: usize,
    pub volumes: usize,
    pub strips: usize,
    pub tunnels: usize,
}

/// The line logged when what is outlined changes; `silent` while nothing
/// draws for the silence.
pub fn drawn_line(drawn: Drawn, silent: bool) -> String {
    if silent {
        "edge bars: silent, nothing drawn".to_owned()
    } else if drawn.strips + drawn.tunnels == 0 {
        "edge bars: no shape to outline".to_owned()
    } else if drawn.tunnels == 0 {
        format!(
            "edge bars: {} planes and {} volumes outlined, {} strips",
            drawn.planes, drawn.volumes, drawn.strips
        )
    } else {
        format!(
            "edge bars: {} planes and {} volumes outlined, {} strips and {} tunnels",
            drawn.planes, drawn.volumes, drawn.strips, drawn.tunnels
        )
    }
}

/// The modes the shapes of each kind draw in this frame, in the kinds'
/// order of `kinds` (pairs of a kind and its mode), for the log: "table in
/// · floor face · wall in".
pub fn kinds_line(kinds: &[(u32, Mode)]) -> String {
    kinds
        .iter()
        .map(|&(kind, mode)| format!("{} {}", crate::surfaces::kind_name(kind), mode.name()))
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
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

/// The strips and tunnels of every outlined shape of `shapes` seen from
/// `head`, into `out` (at most [`MAX_STRIPS`] together), each face of
/// [`faces`] in its shape's mode ([`Look::mode`]): four strips on the face
/// or standing, the bars growing to at most [`max_height`] for `fraction`;
/// one tunnel in, [`tunnel_depth`] for `depth` deep; nothing off. Each
/// edge is moved clear of the other shapes ([`clearance`]) and shows
/// [`bar_count`] of the canvas's `bars`, at `peak` alpha times the shape's
/// fade.
pub fn outline(
    shapes: &[Shape],
    head: Vec3,
    fraction: f32,
    depth: f32,
    bars: usize,
    peak: f32,
    out: &mut Outline,
) -> Drawn {
    out.clear();
    let mut drawn = Drawn::default();
    let mut lit = Vec::with_capacity(3);
    for (k, s) in shapes.iter().enumerate() {
        let Some(look) = s.look.filter(|l| l.mode != Mode::Off) else {
            continue;
        };
        let alpha = peak * look.fade.clamp(0.0, 1.0);
        if alpha <= 0.0 {
            continue;
        }
        let before = out.strips.len() + out.tunnels.len();
        let edges = Edges {
            rot: s.rot,
            skip: k,
            shapes,
            mode: look.mode,
            fraction,
            depth,
            bars,
            color: look.color,
            alpha,
        };
        faces(s.center, s.rot, s.half, head, &mut lit);
        for face in &lit {
            edges.push(face, out);
        }
        if out.strips.len() + out.tunnels.len() > before {
            if is_plane(s.half) {
                drawn.planes += 1;
            } else {
                drawn.volumes += 1;
            }
        }
    }
    out.strips.truncate(MAX_STRIPS);
    out.tunnels.truncate(MAX_STRIPS - out.strips.len());
    drawn.strips = out.strips.len();
    drawn.tunnels = out.tunnels.len();
    drawn
}

/// What one face's strips or tunnel share.
struct Edges<'a> {
    /// The box's rotation, for its corners' ends ([`is_low`]).
    rot: Quat,
    /// The box's own index in `shapes`, which never clears its edges.
    skip: usize,
    shapes: &'a [Shape],
    mode: Mode,
    fraction: f32,
    depth: f32,
    bars: usize,
    color: [f32; 3],
    alpha: f32,
}

impl Edges<'_> {
    /// `face`'s four strips into `out`, edge `e` from corner `e` to corner
    /// `e + 1` of [`Face::corners`]' order, each run from its low end; in
    /// the in mode its tunnel instead.
    fn push(&self, face: &Face, out: &mut Outline) {
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
        let Some(growth) = self.mode.growth() else {
            if self.mode == Mode::In {
                out.tunnels.push(Tunnel {
                    center: center - lift,
                    axes: [a, b],
                    half: [hu, hv],
                    normal: face.normal,
                    depth: tunnel_depth([hu, hv], self.depth),
                    bars: [
                        bar_count(2.0 * hu, self.bars),
                        bar_count(2.0 * hv, self.bars),
                    ],
                    low: is_low(self.rot, face.normal - a - b),
                    color: self.color,
                    alpha: self.alpha,
                });
            }
            return;
        };
        let height = max_height([hu, hv], self.fraction);
        let reach = height * REACH;
        let near = SIGNS.map(|[su, sv]| center + a * (su * hu) + b * (sv * hv));
        // The growth, here alone: the far side of every strip.
        let (far, inset) = match growth {
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
            out.strips.push(Strip {
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

/// A bar height as the rows take it: 0..1, 0 for a value that is not a
/// number.
fn clean(h: f32) -> f32 {
    if h.is_finite() {
        h.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The canvas's bars over the last few steps, for the in mode's rungs:
/// rung k (from 1) shows each bar's loudest over the k-th step of
/// [`HISTORY_STEP_S`] back, so a hit starts at the rim and travels into
/// the wall a rung a step. Stepped by time, so 72 and 90 Hz behave alike;
/// the loudest of each step, so a hit between two steps is not lost.
#[derive(Debug, Clone, PartialEq)]
pub struct History {
    /// Rung k's bars at row k - 1, newest first.
    past: [[f32; MAX_BARS]; RUNGS - 1],
    /// The loudest each bar has been in the step under way.
    peak: [f32; MAX_BARS],
    /// Seconds into the step under way.
    since: f32,
    /// The bar count the rows hold.
    bars: usize,
}

impl Default for History {
    fn default() -> Self {
        Self {
            past: [[0.0; MAX_BARS]; RUNGS - 1],
            peak: [0.0; MAX_BARS],
            since: 0.0,
            bars: 0,
        }
    }
}

impl History {
    /// Once a frame, `dt` seconds after the last, on this frame's bar
    /// `heights` (0..1). A change of the bar count starts it over.
    pub fn step(&mut self, dt: f32, heights: &[f32]) {
        let n = heights.len().min(MAX_BARS);
        if n != self.bars {
            *self = Self {
                bars: n,
                ..Self::default()
            };
        }
        let now = |i: usize| clean(heights[i]);
        for (i, p) in self.peak[..n].iter_mut().enumerate() {
            *p = p.max(now(i));
        }
        self.since += if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let steps = (self.since / HISTORY_STEP_S).floor();
        if steps < 1.0 {
            return;
        }
        self.since -= steps * HISTORY_STEP_S;
        // After a stall longer than the rows hold, every row has the last
        // step's bars.
        for _ in 0..(steps as usize).min(RUNGS - 1) {
            self.past.rotate_right(1);
            self.past[0] = self.peak;
            // The next step starts from the bars as they are now.
            for (i, p) in self.peak[..n].iter_mut().enumerate() {
                *p = now(i);
            }
        }
    }

    /// Rung `k`'s bars (`k` from 1 to [`RUNGS`] - 1): each bar's loudest
    /// over the k-th step back.
    pub fn rung(&self, k: usize) -> &[f32] {
        &self.past[k.clamp(1, RUNGS - 1) - 1][..self.bars]
    }
}

/// The running sums of the first `n` of `heights`, four to a row, into
/// `out`: the k-th the first k summed, held past the n-th.
fn push_sums(heights: &[f32], n: usize, out: &mut Vec<[f32; 4]>) {
    let mut sums = [[0.0f32; 4]; SUM_ROWS];
    let mut total = 0.0f32;
    for k in 0..SUM_ROWS * 4 {
        sums[k / 4][k % 4] = total;
        if let Some(&h) = heights.get(k).filter(|_| k < n) {
            total += clean(h);
        }
    }
    out.extend_from_slice(&sums);
}

/// The uniform rows for the canvas's bar `heights` (0..1, at most
/// [`MAX_BARS`]), their `history` and the frame's `outline` (at most
/// [`MAX_STRIPS`] strips and tunnels together), into `out`: the head row
/// (x the bar count, y the strip count, where the tunnels start), each
/// rung's running sums (rung 0 of `heights`, rung k of the history's
/// [`History::rung`]; the k-th sum the first k heights summed, four to a
/// row), then per strip its four corners, w the length, the tallest bar's
/// height, the bar count and the inset, and its color with the peak alpha;
/// then per tunnel its opening's center (w the depth), its first axis
/// times its half extent (w the bars along it), the second's, its normal
/// (w 1 when the corner at minus both axes is a low end, else -1), and its
/// color with the peak alpha. Only these rows are written: the draws read
/// no record past them.
pub fn rows(heights: &[f32], history: &History, outline: &Outline, out: &mut Vec<[f32; 4]>) {
    out.clear();
    let n = heights.len().min(MAX_BARS);
    let strips = &outline.strips[..outline.strips.len().min(MAX_STRIPS)];
    let tunnels = &outline.tunnels[..outline.tunnels.len().min(MAX_STRIPS - strips.len())];
    out.push([n as f32, strips.len() as f32, 0.0, 0.0]);
    push_sums(heights, n, out);
    for k in 1..RUNGS {
        push_sums(history.rung(k), n, out);
    }
    for s in strips {
        let w = [s.length, s.height, s.bars as f32, s.inset];
        for (c, w) in s.corners.iter().zip(w) {
            out.push([c.x, c.y, c.z, w]);
        }
        out.push([s.color[0], s.color[1], s.color[2], s.alpha]);
    }
    for t in tunnels {
        let [u, v] = [t.axes[0] * t.half[0], t.axes[1] * t.half[1]];
        out.push([t.center.x, t.center.y, t.center.z, t.depth]);
        out.push([u.x, u.y, u.z, t.bars[0] as f32]);
        out.push([v.x, v.y, v.z, t.bars[1] as f32]);
        let low = if t.low { 1.0 } else { -1.0 };
        out.push([t.normal.x, t.normal.y, t.normal.z, low]);
        out.push([t.color[0], t.color[1], t.color[2], t.alpha]);
    }
}

/// Rung `k`'s running sums in `rows` ([`rows`]).
fn rung_sums(rows: &[[f32; 4]], k: usize) -> &[[f32; 4]] {
    let start = 1 + k * SUM_ROWS;
    &rows[start..start + SUM_ROWS]
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
        let h = merged(rung_sums(rows, 0), i, m, n);
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

/// The eye's position from its camera's `view_proj` (the projection times
/// the view): the one point it maps to clip x = y = w = 0, where every ray
/// of a perspective projection starts. The WGSL's `eye_position`: the eye
/// pass's camera uniform carries only the matrix.
pub fn eye_position(view_proj: Mat4) -> Vec3 {
    let [a, b, c] = [0, 1, 3].map(|r| view_proj.row(r).truncate());
    let t = Vec3::new(view_proj.row(0).w, view_proj.row(1).w, view_proj.row(3).w);
    let bc = b.cross(c);
    -(t.x * bc + t.y * c.cross(a) + t.z * a.cross(b)) / a.dot(bc)
}

/// Where a view ray meets a tunnel ([`tunnel_hit`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TunnelHit {
    /// The wall, by the opening's edge it runs back from, numbered as a
    /// face's edges are ([`Face::corners`]' order): 0 at minus the second
    /// axis, 1 at plus the first, 2 at plus the second, 3 at minus the
    /// first.
    pub edge: usize,
    /// The point along that edge from its low end (m), and the edge's
    /// length.
    pub along: f32,
    pub length: f32,
    /// How deep behind the face (m), at least 0.
    pub depth: f32,
    /// Whether the ray reaches the tunnel's end before any wall: nothing
    /// is drawn there.
    pub end: bool,
    /// The cosine of the ray's angle to the face's normal: 0 grazing, 1
    /// square on.
    pub facing: f32,
}

/// The view ray from `eye` through `point`, both in the tunnel's frame (m
/// along the face's first axis, its second, and off the face toward the
/// eye), against a tunnel of opening half extents `half` and depth
/// `depth` whose corner at minus both axes is a low end when `low` is
/// positive. One analytic intersection: the ray crosses the opening inside
/// it, so it leaves the box through the wall ahead of it along each axis or
/// through the end. The nearer of those two walls is the one it meets, and
/// the end when that is deeper than the tunnel. Only two of the four wall
/// planes are tested, as the other two are behind the ray. The WGSL's
/// `fs_tunnel`.
pub fn tunnel_hit(half: [f32; 2], depth: f32, low: f32, eye: Vec3, point: Vec3) -> TunnelHit {
    let d = point - eye;
    let down = (-d.z).max(1e-6);
    let sx = if d.x >= 0.0 { 1.0 } else { -1.0 };
    let sy = if d.y >= 0.0 { 1.0 } else { -1.0 };
    let tx = (half[0] - sx * eye.x) / d.x.abs().max(1e-6);
    let ty = (half[1] - sy * eye.y) / d.y.abs().max(1e-6);
    let on_x = tx < ty;
    let t = tx.min(ty);
    let z = (t * down - eye.z).max(0.0);
    let (x, h, side) = if on_x {
        (eye.y + t * d.y, half[1], sx)
    } else {
        (eye.x + t * d.x, half[0], sy)
    };
    TunnelHit {
        edge: match (on_x, side > 0.0) {
            (false, false) => 0,
            (true, true) => 1,
            (false, true) => 2,
            (true, false) => 3,
        },
        along: (h - low * side * x).clamp(0.0, 2.0 * h),
        length: 2.0 * h,
        depth: z,
        end: z >= depth,
        facing: down / d.length(),
    }
}

/// The rung nearest a point `z` m deep in a tunnel `depth` deep, and how
/// far the point is from the rung's middle (m). Rung k spans k to
/// k + [`RUNG_FILL`] spacings deep, the spacing `depth /` [`RUNGS`].
pub fn rung_at(z: f32, depth: f32) -> (usize, f32) {
    let spacing = depth.max(1e-4) / RUNGS as f32;
    let w = RUNG_FILL * spacing;
    let k = ((z - 0.5 * w) / spacing + 0.5)
        .floor()
        .clamp(0.0, (RUNGS - 1) as f32);
    (k as usize, z - (k * spacing + 0.5 * w))
}

/// How much of a pixel `px` m deep (its footprint along the wall's depth)
/// a rung `w` m wide covers, the pixel's middle `dz` from the rung's: the
/// box filter, so a rung thinner than a pixel dims instead of flickering.
pub fn rung_cover(dz: f32, w: f32, px: f32) -> f32 {
    let px = px.max(1e-6);
    (((dz + 0.5 * px).min(0.5 * w) - (dz - 0.5 * px).max(-0.5 * w)) / px).clamp(0.0, 1.0)
}

/// Rung `k`'s light `f` across its bar's slot (0..1), the bar's height
/// k steps ago `h` (0..1): the rim (rung 0) along the whole edge, brighter
/// where its bar is loud; a deeper rung a faint frame along the whole edge
/// ([`FRAME_LIGHT`]), brighter in its bar's slot where the bar reached it
/// (half lit at a height of k / [`RUNGS`]); each dimmer by [`RUNG_DIM`] a
/// rung.
pub fn rung_light(k: usize, f: f32, h: f32) -> f32 {
    let gap = 0.5 * (1.0 - BAR_FILL);
    let across = smoothstep(gap, gap + SIDE_SOFT, f)
        * (1.0 - smoothstep(1.0 - gap - SIDE_SOFT, 1.0 - gap, f));
    let lit = (h * RUNGS as f32 - k as f32 + 0.5).clamp(0.0, 1.0);
    let level = LINE_LIGHT + (1.0 - LINE_LIGHT) * h;
    let rung = if k == 0 {
        LINE_LIGHT + (1.0 - LINE_LIGHT) * h * across
    } else {
        (across * lit * level).max(FRAME_LIGHT)
    };
    RUNG_DIM.powf(k as f32) * rung
}

/// How much of a tunnel shows for a view ray at `facing` (the cosine of
/// its angle to the face's normal): none grazing, so a face seen edge-on
/// fades instead of flickering, all of it from [`GRAZE_HI`].
pub fn graze(facing: f32) -> f32 {
    smoothstep(GRAZE_LO, GRAZE_HI, facing)
}

/// The light the tunnel shader gives tunnel `k` (the k-th after the
/// strips) of `rows` for an eye at `eye` at the point `point` of its quad
/// (world, m; the quad lifted [`LIFT_M`] off the face), `px` a pixel's
/// worth of the depth (m): 0..1, before the alpha. The vertex stage's
/// frame and the fragment's work, in Rust.
pub fn tunnel_light_at(rows: &[[f32; 4]], k: usize, eye: Vec3, point: Vec3, px: f32) -> f32 {
    let base = HEAD_ROWS + ((rows[0][1] + 0.5) as usize + k) * STRIP_ROWS;
    let r = |j: usize| Vec3::new(rows[base + j][0], rows[base + j][1], rows[base + j][2]);
    let (center, u, v, normal) = (r(0), r(1), r(2), r(3));
    let half = [u.length(), v.length()];
    let frame = |p: Vec3| {
        let q = p - center;
        Vec3::new(q.dot(u) / half[0], q.dot(v) / half[1], q.dot(normal))
    };
    let depth = rows[base][3].max(1e-4);
    let hit = tunnel_hit(half, depth, rows[base + 3][3], frame(eye), frame(point));
    let bars = if hit.edge % 2 == 1 {
        rows[base + 2][3]
    } else {
        rows[base + 1][3]
    };
    let m = ((bars + 0.5) as u32).max(1);
    let (rung, dz) = rung_at(hit.depth, depth);
    let cover = rung_cover(dz, RUNG_FILL * depth / RUNGS as f32, px);
    let x = (hit.along / hit.length).clamp(0.0, 0.99999) * m as f32;
    let i = (x as u32).min(m - 1);
    let n = (rows[0][0] + 0.5) as u32;
    let bar = if n > 0 {
        merged(rung_sums(rows, rung), i, m, n)
    } else {
        0.0
    };
    if hit.end {
        return 0.0;
    }
    (cover * rung_light(rung, x.fract(), bar) * graze(hit.facing)).clamp(0.0, 1.0)
}

/// The alpha the tunnel shader writes ([`tunnel_light_at`] times the
/// tunnel's peak alpha, at most 1).
pub fn tunnel_alpha_at(rows: &[[f32; 4]], k: usize, eye: Vec3, point: Vec3, px: f32) -> f32 {
    let base = HEAD_ROWS + ((rows[0][1] + 0.5) as usize + k) * STRIP_ROWS;
    (rows[base + 4][3] * tunnel_light_at(rows, k, eye, point, px)).clamp(0.0, 1.0)
}

/// The edge bars' shader: group 0 is the eye pass's shared camera
/// (`view_proj`), group 1 the rows of [`rows`]. Two pairs of stages on the
/// one uniform, each drawn instanced once per eye. `vs_main` / `fs_main`,
/// one instance per strip: the vertex stage places the strip from its
/// corners and hands the fragment its point in meters along the edge and
/// off it, and the strip's shape and color unchanged; the fragment writes
/// the bars and the line, premultiplied. `vs_tunnel` / `fs_tunnel`, one
/// instance per tunnel: the vertex stage places the face's quad lifted
/// [`LIFT_M`] off it and hands the fragment the point and the eye in the
/// face's frame; the fragment finds where the view ray meets the tunnel's
/// inner walls ([`tunnel_hit`]) and writes the rungs there ([`rung_light`]),
/// premultiplied, nothing past the tunnel's end.
pub const EDGE_BARS_WGSL: &str = r"
struct Eye { view_proj: mat4x4<f32> }
struct Strip {
    // A strip, lifted off the face: the edge's low end, its high end, then
    // the far side's corners over the high end and over the low end. w, in
    // order: the edge's length (m), the tallest bar's height (m), the bar
    // count, how far in from the edge's ends the far side's ends are (m).
    // A tunnel: the opening's center on the face (w the depth, m), the
    // face's first axis times its half extent (w the bars on the edges
    // along it), the second's (w the bars along it), the outward normal (w
    // 1 when the corner at minus both axes is a low end, else -1).
    corners: array<vec4<f32>, 4>,
    // rgb the color, w the peak alpha
    color: vec4<f32>,
}
struct EdgeBars {
    // x the canvas's bar count, y the strip count (the first tunnel's
    // index)
    head: vec4<f32>,
    // each rung's bars' running sums, four to a row: the k-th is the first
    // k bars' heights summed. Rung 0 is this frame's bars, rung k the bars
    // k steps ago.
    sums: array<array<vec4<f32>, 17>, 10>,
    // the strips, then the tunnels
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
// The tunnel: its quad's lift off the face (m); the rungs a full bar
// lights; a rung's width against the spacing; each rung's light against
// the one in front; the cosines of the view ray to the normal over which
// it fades in.
const LIFT_M: f32 = 0.01;
const RUNGS: u32 = 10u;
const RUNG_FILL: f32 = 0.3;
const RUNG_DIM: f32 = 0.8;
const FRAME_LIGHT: f32 = 0.25;
const GRAZE_LO: f32 = 0.05;
const GRAZE_HI: f32 = 0.2;

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

// Bar i of the m on an edge, as it was k steps ago: the mean of its share
// of the canvas's n bars, from rung k's running sums (edge_bars.rs has the
// same in Rust).
fn merged(k: u32, i: u32, m: u32, n: u32) -> f32 {
    let lo = min(i * n / m, n - 1u);
    let hi = clamp((i + 1u) * n / m, lo + 1u, n);
    let total = edge.sums[k][hi / 4u][hi % 4u] - edge.sums[k][lo / 4u][lo % 4u];
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
        let h = merged(0u, i, m, n);
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

struct TunnelOut {
    @builtin(position) pos: vec4<f32>,
    // the point on the quad in the face's frame (m): along its first axis,
    // along its second, off the face toward the eye
    @location(0) p: vec3<f32>,
    // the eye in the same frame
    @location(1) @interpolate(flat) eye: vec3<f32>,
    // the opening's half extents, the depth (m), the low end's sign
    @location(2) @interpolate(flat) shape: vec4<f32>,
    // the bars on the edges along the first axis and along the second
    @location(3) @interpolate(flat) bars: vec2<f32>,
    @location(4) @interpolate(flat) color: vec4<f32>,
}

// The eye's position: the one point the camera maps to clip x = y = w = 0
// (rows 0, 1 and 3 of view_proj), solved by Cramer's rule (edge_bars.rs
// has the same in Rust).
fn eye_position() -> vec3<f32> {
    let m = eye.view_proj;
    let a = vec3<f32>(m[0].x, m[1].x, m[2].x);
    let b = vec3<f32>(m[0].y, m[1].y, m[2].y);
    let c = vec3<f32>(m[0].w, m[1].w, m[2].w);
    let t = vec3<f32>(m[3].x, m[3].y, m[3].w);
    let bc = cross(b, c);
    return -(t.x * bc + t.y * cross(c, a) + t.z * cross(a, b)) / dot(a, bc);
}

@vertex
fn vs_tunnel(@builtin(vertex_index) i: u32, @builtin(instance_index) k: u32) -> TunnelOut {
    var signs = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let t = edge.strips[u32(edge.head.y + 0.5) + k];
    let center = t.corners[0].xyz;
    let u = t.corners[1].xyz;
    let v = t.corners[2].xyz;
    let n = t.corners[3].xyz;
    let half = vec2<f32>(length(u), length(v));
    let s = signs[i];
    var out: TunnelOut;
    out.pos = eye.view_proj * vec4<f32>(center + n * LIFT_M + u * s.x + v * s.y, 1.0);
    out.p = vec3<f32>(s * half, LIFT_M);
    let e = eye_position() - center;
    out.eye = vec3<f32>(dot(e, u) / half.x, dot(e, v) / half.y, dot(e, n));
    out.shape = vec4<f32>(half, t.corners[0].w, t.corners[3].w);
    out.bars = vec2<f32>(t.corners[1].w, t.corners[2].w);
    out.color = t.color;
    return out;
}

// How much of a pixel px m deep a rung w m wide covers, the pixel's middle
// dz from the rung's: the box filter, so a rung thinner than a pixel dims
// instead of flickering.
fn rung_cover(dz: f32, w: f32, px: f32) -> f32 {
    return clamp((min(dz + 0.5 * px, 0.5 * w) - max(dz - 0.5 * px, -0.5 * w)) / px, 0.0, 1.0);
}

// Rung k's light f across its bar's slot, the bar's height k steps ago h:
// the rim (rung 0) along the whole edge, brighter where its bar is loud; a
// deeper rung a faint frame along the whole edge (FRAME_LIGHT), brighter in
// its bar's slot where the bar reached it; each dimmer by RUNG_DIM a rung.
fn rung_light(k: u32, f: f32, h: f32) -> f32 {
    let gap = 0.5 * (1.0 - BAR_FILL);
    let across = smoothstep(gap, gap + SIDE_SOFT, f)
        * (1.0 - smoothstep(1.0 - gap - SIDE_SOFT, 1.0 - gap, f));
    let lit = clamp(h * f32(RUNGS) - f32(k) + 0.5, 0.0, 1.0);
    let level = LINE_LIGHT + (1.0 - LINE_LIGHT) * h;
    let rung = select(
        max(across * lit * level, FRAME_LIGHT),
        LINE_LIGHT + (1.0 - LINE_LIGHT) * h * across,
        k == 0u,
    );
    return pow(RUNG_DIM, f32(k)) * rung;
}

@fragment
fn fs_tunnel(in: TunnelOut) -> @location(0) vec4<f32> {
    let half = in.shape.xy;
    let depth = max(in.shape.z, 1e-4);
    // The view ray from the eye through the point, and how fast it goes
    // into the face.
    let d = in.p - in.eye;
    let down = max(-d.z, 1e-6);
    // It crossed the opening inside it, so it leaves the box through the
    // wall ahead of it along each axis, or through the end: the nearer of
    // the two walls is the one it meets. One intersection, no loop.
    let sx = select(-1.0, 1.0, d.x >= 0.0);
    let sy = select(-1.0, 1.0, d.y >= 0.0);
    let tx = (half.x - sx * in.eye.x) / max(abs(d.x), 1e-6);
    let ty = (half.y - sy * in.eye.y) / max(abs(d.y), 1e-6);
    let on_x = tx < ty;
    let t = min(tx, ty);
    // How deep behind the face it meets the wall (m), and a pixel's worth
    // of that, taken before any branch, where the derivatives are defined.
    let z = max(t * down - in.eye.z, 0.0);
    let px = max(fwidth(z), 1e-6);
    // The wall's edge: the point along it from its low end, its half
    // length, its bar count.
    let x = select(in.eye.x + t * d.x, in.eye.y + t * d.y, on_x);
    let h = select(half.x, half.y, on_x);
    let side = select(sy, sx, on_x);
    let along = clamp(h - in.shape.w * side * x, 0.0, 2.0 * h);
    let m = max(u32(select(in.bars.x, in.bars.y, on_x) + 0.5), 1u);
    // The nearest rung, and how much of the pixel it covers.
    let spacing = depth / f32(RUNGS);
    let w = RUNG_FILL * spacing;
    let k = u32(clamp(floor((z - 0.5 * w) / spacing + 0.5), 0.0, f32(RUNGS - 1u)));
    let cover = rung_cover(z - (f32(k) * spacing + 0.5 * w), w, px);
    // The bar under the point, as it was k steps ago.
    let xb = clamp(along / (2.0 * h), 0.0, 0.99999) * f32(m);
    let i = min(u32(xb), m - 1u);
    let n = u32(edge.head.x + 0.5);
    var bar = 0.0;
    if n > 0u {
        bar = merged(k, i, m, n);
    }
    // Nothing past the end: the dark center, where the passthrough or the
    // surface's own effect shows. Grazing, it fades.
    let graze = smoothstep(GRAZE_LO, GRAZE_HI, down / length(d));
    let light = select(cover * rung_light(k, fract(xb), bar) * graze, 0.0, z >= depth);
    let a = clamp(in.color.w * light, 0.0, 1.0);
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
        mode: Mode::Face,
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

    /// `shapes` with every outlined one in `mode`.
    fn in_mode(shapes: &[Shape], mode: Mode) -> Vec<Shape> {
        shapes
            .iter()
            .map(|s| Shape {
                look: s.look.map(|l| Look { mode, ..l }),
                ..*s
            })
            .collect()
    }

    /// The outline of `shapes` from `head`, every outlined one in `mode`.
    fn outlined(shapes: &[Shape], head: Vec3, mode: Mode) -> (Outline, Drawn) {
        let mut out = Outline::default();
        let drawn = outline(
            &in_mode(shapes, mode),
            head,
            HEIGHT_FRACTION,
            DEPTH_FRACTION,
            24,
            0.25,
            &mut out,
        );
        (out, drawn)
    }

    /// The strips of `shapes` from `head`, every outlined one in `mode`.
    fn run(shapes: &[Shape], head: Vec3, mode: Mode) -> (Vec<Strip>, Drawn) {
        let (out, drawn) = outlined(shapes, head, mode);
        (out.strips, drawn)
    }

    fn near(a: Vec3, b: Vec3) -> bool {
        a.abs_diff_eq(b, 1e-5)
    }

    #[test]
    fn the_control_steps_theme_off_face_stand_in_and_forces_every_kind() {
        let mut c = Choice::Theme;
        let mut seen = Vec::new();
        for _ in 0..5 {
            seen.push(c.label());
            c = c.next();
        }
        assert_eq!(c, Choice::Theme);
        assert_eq!(
            seen,
            [
                "Edges theme",
                "Edges off",
                "Edges face",
                "Edges stand",
                "Edges in"
            ]
        );
        // Half a cell holds about a dozen characters.
        assert!(seen.iter().all(|l| l.len() <= 12));
        // The theme's mode per kind, or the one forced on every kind.
        for theme in Mode::ALL {
            assert_eq!(Choice::Theme.mode(theme), theme);
            for forced in Mode::ALL {
                assert_eq!(Choice::Force(forced).mode(theme), forced);
            }
        }
        assert!(Choice::Force(Mode::Off).off());
        assert!(!Choice::Theme.off() && !Choice::Force(Mode::In).off());
        assert_eq!(Mode::ALL.map(Mode::name), ["off", "face", "stand", "in"]);
        assert_eq!(Mode::In.growth(), None);
        assert_eq!(Mode::Off.growth(), None);
    }

    #[test]
    fn the_controls_choice_holds_until_the_knobs_value_changes() {
        // Unset at launch: the theme's.
        let mut o = Override::new(Choice::Theme);
        assert_eq!(o.poll(Choice::Theme), Choice::Theme);
        // The control pressed to in: it holds over the knob's unchanged
        // value, poll after poll.
        o.press(Choice::Force(Mode::In));
        for _ in 0..3 {
            assert_eq!(o.poll(Choice::Theme), Choice::Force(Mode::In));
        }
        // The knob set to 2: its value changed, so it holds, and the
        // control's choice is dropped.
        assert_eq!(
            o.poll(Choice::Force(Mode::Standing)),
            Choice::Force(Mode::Standing)
        );
        assert_eq!(o.choice(), Choice::Force(Mode::Standing));
        // A press again holds until the next change; back to unset, the
        // theme's again.
        o.press(Choice::Theme);
        assert_eq!(o.poll(Choice::Force(Mode::Standing)), Choice::Theme);
        o.press(Choice::Force(Mode::Off));
        assert_eq!(o.poll(Choice::Theme), Choice::Theme);
        // A knob set at launch holds until a press.
        let mut o = Override::new(Choice::Force(Mode::Face));
        assert_eq!(o.poll(Choice::Force(Mode::Face)), Choice::Force(Mode::Face));
        o.press(Choice::Theme);
        assert_eq!(o.poll(Choice::Force(Mode::Face)), Choice::Theme);
    }

    #[test]
    fn a_plane_gives_its_room_side_four_edges() {
        let (s, drawn) = run(&[wall()], HEAD, Mode::Face);
        assert_eq!(
            drawn,
            Drawn {
                planes: 1,
                volumes: 0,
                strips: 4,
                tunnels: 0
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
        let (s, _) = run(&[wall()], Vec3::new(0.0, 1.2, -4.0), Mode::Face);
        assert_eq!(s.len(), 4);
        assert!(s.iter().all(|s| s.corners.iter().all(|c| c.z < -2.0)));
        // The far side lies on the face, a strip's height in from the edge.
        let (s, _) = run(&[wall()], HEAD, Mode::Face);
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
        let (s, drawn) = run(&[t], side, Mode::Face);
        assert_eq!(
            drawn,
            Drawn {
                planes: 0,
                volumes: 1,
                strips: 12,
                tunnels: 0
            }
        );
        assert_eq!(s.len(), 12);
        // Straight above it: the top alone.
        faces(t.center, t.rot, t.half, Vec3::new(0.0, 2.0, -1.0), &mut lit);
        assert_eq!(lit.len(), 1);
        assert!(near(lit[0].normal, Vec3::Y));
        // From inside it: nothing.
        let (s, drawn) = run(&[t], t.center, Mode::Face);
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
                    mode: Mode::Face,
                }),
                ..wall()
            },
        ];
        assert!(run(&quiet, HEAD, Mode::Face).0.is_empty());
        // A live shape half faded in: half the alpha.
        let half = Shape {
            look: Some(Look {
                color: COLOR,
                fade: 0.5,
                mode: Mode::Face,
            }),
            ..wall()
        };
        assert!(
            run(&[half], HEAD, Mode::Face)
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
            let (s, _) = run(&[shape], head, Mode::Face);
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
        let (s, drawn) = run(&[wall(), floor(), stage], HEAD, Mode::Face);
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
        let (s, _) = run(&[table()], Vec3::new(0.0, 2.0, -1.0), Mode::Standing);
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
        let (inward, _) = run(&[table()], Vec3::new(0.0, 2.0, -1.0), Mode::Face);
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
        rows(
            &heights,
            &History::default(),
            &Outline::default(),
            &mut rows_out,
        );
        let sums = rung_sums(&rows_out, 0);
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
        let outline = Outline {
            strips: vec![strip],
            tunnels: Vec::new(),
        };
        rows(heights, &History::default(), &outline, &mut out);
        out
    }

    #[test]
    fn the_rows_carry_the_sums_and_the_strips() {
        let heights = [1.0, 0.5, 0.0, 0.25, 2.0, f32::NAN];
        let r = one_strip(&heights, 4, 0.25);
        assert_eq!(r.len(), HEAD_ROWS + STRIP_ROWS);
        // Six bars, one strip.
        assert_close!(r[0], [6.0, 1.0, 0.0, 0.0]);
        // Running sums, the out-of-range heights clamped, held past the
        // last bar.
        assert_close!(r[1], [0.0, 1.0, 1.5, 1.5]);
        assert_close!(r[2], [1.75, 2.75, 2.75, 2.75]);
        assert_close!(r[SUM_ROWS], [2.75, 2.75, 2.75, 2.75]);
        // An empty history: the deeper rungs' sums all 0.
        assert!(
            r[1 + SUM_ROWS..HEAD_ROWS]
                .iter()
                .flatten()
                .all(|&v| v == 0.0)
        );
        let base = HEAD_ROWS;
        assert_close!(r[base], [0.0, 0.0, 0.0, 0.4]);
        assert_close!(r[base + 1], [0.4, 0.0, 0.0, 0.1]);
        assert_close!(r[base + 2], [0.29, 0.11, 0.0, 4.0]);
        assert_close!(r[base + 3], [0.11, 0.11, 0.0, 0.11]);
        assert_close!(r[base + 4], [COLOR[0], COLOR[1], COLOR[2], 0.25]);
        // No strips: the head and the sums alone; past the capacity, cut.
        let mut out = Vec::new();
        rows(&[], &History::default(), &Outline::default(), &mut out);
        assert_eq!(out.len(), HEAD_ROWS);
        assert_close!(out[0][0], 0.0);
        let strips = vec![
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
        let many = Outline {
            strips,
            tunnels: vec![tunnel(0.5); 3],
        };
        rows(&[0.5], &History::default(), &many, &mut out);
        assert_eq!(out.len(), UNIFORM_ROWS);
        assert_close!(out[0][1], MAX_STRIPS as f32);
        // Strips and tunnels share the records, the tunnels after the
        // strips.
        let mixed = Outline {
            strips: many.strips[..2].to_vec(),
            tunnels: vec![tunnel(0.5); 3],
        };
        rows(&[0.5], &History::default(), &mixed, &mut out);
        assert_eq!(out.len(), HEAD_ROWS + 5 * STRIP_ROWS);
        assert_close!(out[0], [1.0, 2.0, 0.0, 0.0]);
        let t = &out[HEAD_ROWS + 2 * STRIP_ROWS..];
        assert_close!(t[0], [0.0, 1.25, -1.98, 1.5]);
        assert_close!(t[1], [2.0, 0.0, 0.0, 24.0]);
        assert_close!(t[2], [0.0, 1.25, 0.0, 24.0]);
        assert_close!(t[3], [0.0, 0.0, 1.0, 1.0]);
        assert_close!(t[4], [COLOR[0], COLOR[1], COLOR[2], 0.5]);
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
    fn the_knobs_follow_the_theme_unset_force_0_to_3_and_log_their_state() {
        let none = |_: &str| None::<String>;
        let d = EdgeBarsOptions::from_knobs(none);
        assert_eq!(d, EdgeBarsOptions::default());
        assert_eq!(d.choice, Choice::Theme);
        assert_close!(
            [d.gain, d.height, d.depth],
            [1.0, HEIGHT_FRACTION, DEPTH_FRACTION]
        );
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
            (KNOB_DEPTH, "0.9"),
        ]));
        assert_eq!(o.choice, Choice::Force(Mode::Face));
        assert_close!([o.gain, o.height, o.depth], [2.0, 0.2, 0.9]);
        let o = EdgeBarsOptions::from_knobs(set(&[
            (KNOB, " 2 "),
            (KNOB_GAIN, "99"),
            (KNOB_HEIGHT, "-3"),
            (KNOB_DEPTH, "7"),
        ]));
        assert_eq!(o.choice, Choice::Force(Mode::Standing));
        assert_close!(
            [o.gain, o.height, o.depth],
            [GAIN_MAX, 0.0, DEPTH_FRACTION_MAX]
        );
        let o = EdgeBarsOptions::from_knobs(set(&[(KNOB, "3")]));
        assert_eq!(o.choice, Choice::Force(Mode::In));
        let o = EdgeBarsOptions::from_knobs(set(&[
            (KNOB, "yes"),
            (KNOB_GAIN, "loud"),
            (KNOB_HEIGHT, "NaN"),
            (KNOB_DEPTH, "deep"),
        ]));
        assert_eq!(o, EdgeBarsOptions::default());
        // 0 turns them off.
        let off = EdgeBarsOptions::from_knobs(set(&[(KNOB, "0")]));
        assert_eq!(off.choice, Choice::Force(Mode::Off));
        // The log lines.
        let on = d;
        assert!(off.line(true).starts_with("edge bars: off · gain 1"));
        assert!(on.line(true).contains("in its theme's mode"));
        assert!(on.line(true).contains("tunnel 0.6 of it, 0.15 to 1.5 m"));
        assert!(
            EdgeBarsOptions::from_knobs(set(&[(KNOB, "1")]))
                .line(true)
                .contains("growing in from its edges")
        );
        assert!(on.line(false).contains("debug.fosfora.occluders 0"));
        assert!(!off.line(false).contains("occluders"));
        assert!(
            on.change_line(&off, true)
                .ends_with(&format!("(was: {})", off.describe()))
        );
        assert_eq!(
            kinds_line(&[(1, Mode::In), (2, Mode::Face), (3, Mode::Standing)]),
            "table in \u{b7} floor face \u{b7} wall stand"
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
                    strips: 40,
                    tunnels: 0
                },
                false
            ),
            "edge bars: 6 planes and 2 volumes outlined, 40 strips"
        );
        assert_eq!(
            drawn_line(
                Drawn {
                    planes: 6,
                    volumes: 2,
                    strips: 12,
                    tunnels: 5
                },
                false
            ),
            "edge bars: 6 planes and 2 volumes outlined, 12 strips and 5 tunnels"
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
        assert_eq!(UNIFORM_BYTES, 33_456);
        assert_eq!(SUM_ROWS, 17, "sums: array<array<vec4<f32>, 17>, 10>");
        assert_eq!(RUNGS, 10, "sums: array<array<vec4<f32>, 17>, 10>");
        assert_eq!(MAX_STRIPS, 384, "strips: array<Strip, 384>");
        for (name, value) in [
            ("REACH", REACH),
            ("BAR_FILL", BAR_FILL),
            ("SIDE_SOFT", SIDE_SOFT),
            ("TOP_SOFT", TOP_SOFT),
            ("CAP_WIDTH", CAP_WIDTH),
            ("LINE_M", LINE_M),
            ("LINE_LIGHT", LINE_LIGHT),
            ("LIFT_M", LIFT_M),
            ("RUNG_FILL", RUNG_FILL),
            ("RUNG_DIM", RUNG_DIM),
            ("FRAME_LIGHT", FRAME_LIGHT),
            ("GRAZE_LO", GRAZE_LO),
            ("GRAZE_HI", GRAZE_HI),
        ] {
            assert_close!(wgsl_const(&module, name), value, "{name}");
        }
        let (_, rungs) = module
            .constants
            .iter()
            .find(|(_, c)| c.name.as_deref() == Some("RUNGS"))
            .expect("const RUNGS");
        assert!(matches!(
            module.global_expressions[rungs.init],
            naga::Expression::Literal(naga::Literal::U32(n)) if n as usize == RUNGS
        ));
        // Both pairs of stages, for the two pipelines on the one uniform.
        let entries: Vec<&str> = module
            .entry_points
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(entries, ["vs_main", "fs_main", "vs_tunnel", "fs_tunnel"]);
    }

    /// A wall's tunnel as `outline` gives one: the 4 x 2.5 m wall's room
    /// side at z = -1.98 facing +Z, its first axis X and its second Y, 1.5 m
    /// deep, 24 bars on every edge, the corner at -X -Y a low end.
    fn tunnel(alpha: f32) -> Tunnel {
        Tunnel {
            center: Vec3::new(0.0, 1.25, -1.98),
            axes: [Vec3::X, Vec3::Y],
            half: [2.0, 1.25],
            normal: Vec3::Z,
            depth: 1.5,
            bars: [24, 24],
            low: true,
            color: COLOR,
            alpha,
        }
    }

    /// The point of `tunnel(..)`'s quad (lifted LIFT_M off the face) on the
    /// line from `eye` to `w`, a point behind the face.
    fn on_quad(eye: Vec3, w: Vec3) -> Vec3 {
        let z = -1.98 + LIFT_M;
        eye + (w - eye) * ((z - eye.z) / (w.z - eye.z))
    }

    /// A point of `tunnel(..)`'s frame from the world.
    fn local(p: Vec3) -> Vec3 {
        p - Vec3::new(0.0, 1.25, -1.98)
    }

    #[test]
    fn the_eye_is_where_the_camera_puts_it() {
        let fov = crate::math::Fov {
            left: -0.8,
            right: 0.7,
            up: 0.75,
            down: -0.8,
        };
        let proj = crate::math::projection(fov, 0.05, 100.0);
        for (eye, yaw, pitch) in [
            (Vec3::new(0.0, 1.2, 0.0), 0.0, 0.0),
            (Vec3::new(0.032, 1.25, -0.4), 0.7, -0.3),
            (Vec3::new(-2.5, 1.7, 3.1), -2.4, 0.5),
        ] {
            let rot = Quat::from_euler(glam::EulerRot::YXZ, yaw, pitch, 0.1);
            let view = Mat4::from_rotation_translation(rot, eye).inverse();
            let got = eye_position(proj * view);
            assert!(got.distance(eye) < 1e-4, "{got} vs {eye}");
        }
    }

    #[test]
    fn a_ray_meets_the_wall_ahead_of_it_or_reaches_the_end() {
        let (half, depth) = ([2.0, 1.25], 1.5);
        let p = |u: f32, v: f32| Vec3::new(u, v, LIFT_M);
        // Square on, 2 m out: the center ray reaches the end.
        let eye = Vec3::new(0.0, 0.0, 2.0);
        let hit = tunnel_hit(half, depth, 1.0, eye, p(0.0, 0.0));
        assert!(hit.end, "{hit:?}");
        assert_close!(hit.facing, 1.0);
        // Near the rim on the right: the right wall (edge 1), as deep as
        // the ray goes past the opening before it reaches x = 2.
        let hit = tunnel_hit(half, depth, 1.0, eye, p(1.9, 0.0));
        assert_eq!(hit.edge, 1);
        assert!(!hit.end);
        let expect = -(2.0 + (2.0 / 1.9) * (LIFT_M - 2.0));
        assert!(
            (hit.depth - expect).abs() < 1e-4,
            "{} vs {expect}",
            hit.depth
        );
        // From each side the ray meets the far wall, the one across the
        // opening from the eye.
        for (eye, edge) in [
            (Vec3::new(4.0, 0.0, 1.0), 3),
            (Vec3::new(-4.0, 0.0, 1.0), 1),
            (Vec3::new(0.0, 4.0, 1.0), 0),
            (Vec3::new(0.0, -4.0, 1.0), 2),
        ] {
            for q in [p(0.0, 0.0), p(1.0, 0.5), p(-1.0, -0.5)] {
                let hit = tunnel_hit(half, depth, 1.0, eye, q);
                assert_eq!(hit.edge, edge, "{eye} {q}");
                assert!(!hit.end && hit.depth > 0.0 && hit.depth < depth, "{hit:?}");
            }
        }
        // Along each edge from its low end: the corner at minus both axes
        // is low, the others alternate. Edge 0 runs from -X, edge 1 from
        // +Y, edge 2 from +X, edge 3 from -Y.
        let eye = Vec3::new(0.0, 0.0, 2.0);
        let near_rim = |u: f32, v: f32| tunnel_hit(half, depth, 1.0, eye, p(u, v));
        // (Hits a little past the point on the opening: the parallax.)
        assert!((near_rim(-1.5, -1.24).along - 0.5).abs() < 0.02);
        assert_eq!(near_rim(-1.5, -1.24).edge, 0);
        assert!((near_rim(1.99, 1.0).along - 0.25).abs() < 0.01);
        assert!((near_rim(1.5, 1.24).along - 0.5).abs() < 0.02);
        assert!((near_rim(-1.99, -1.0).along - 0.25).abs() < 0.01);
        // The other low end flips each.
        assert!((tunnel_hit(half, depth, -1.0, eye, p(-1.5, -1.24)).along - 3.5).abs() < 0.02);
        // Grazing, 40 m off to the side and 0.5 m out: a shallow hit on the
        // far wall, and nothing drawn.
        let hit = tunnel_hit(half, depth, 1.0, Vec3::new(40.0, 0.0, 0.5), p(0.0, 0.0));
        assert_eq!(hit.edge, 3);
        assert!(hit.depth < 0.05 && hit.facing < GRAZE_LO, "{hit:?}");
        assert_close!(graze(hit.facing), 0.0);
        // It fades in as the eye rises, never jumping.
        let fades: Vec<f32> = (1..60)
            .map(|k| {
                let eye = Vec3::new(10.0, 0.0, k as f32 * 0.05);
                graze(tunnel_hit(half, depth, 1.0, eye, p(0.0, 0.0)).facing)
            })
            .collect();
        assert!(
            fades.windows(2).all(|w| w[1] >= w[0] && w[1] - w[0] < 0.1),
            "{fades:?}"
        );
        assert_close!(*fades.last().unwrap(), 1.0);
    }

    #[test]
    fn a_far_rung_holds_its_place_as_the_head_moves() {
        // A point on the right inner wall at rung 7's middle, seen from two
        // head positions: the same wall, the same place along it and the
        // same depth from both, while the point of the face it shows
        // through moves; the rim does not. Real parallax, one eye at a
        // time.
        let t = tunnel(0.5);
        let z7 = 7.0 * t.depth / RUNGS as f32 + 0.5 * RUNG_FILL * t.depth / RUNGS as f32;
        let w = Vec3::new(2.0, 1.9, -1.98 - z7);
        let eyes = [Vec3::new(0.0, 1.2, 0.0), Vec3::new(0.4, 1.35, 0.25)];
        let hits = eyes.map(|e| {
            let q = on_quad(e, w);
            tunnel_hit(t.half, t.depth, 1.0, local(e), local(q))
        });
        for h in &hits {
            assert_eq!(h.edge, 1);
            assert!((h.depth - z7).abs() < 1e-4, "{h:?}");
            assert!((h.along - (1.25 - 0.65)).abs() < 1e-4, "{h:?}");
        }
        let shift = on_quad(eyes[0], w).distance(on_quad(eyes[1], w));
        assert!(shift > 0.1, "{shift}");
        // Lit the same from both: rung 7 of full bars.
        let mut hist = History::default();
        let full = [1.0f32; 24];
        for _ in 0..RUNGS {
            hist.step(HISTORY_STEP_S, &full);
        }
        let mut r = Vec::new();
        let outline = Outline {
            strips: Vec::new(),
            tunnels: vec![t],
        };
        rows(&full, &hist, &outline, &mut r);
        let lit = eyes.map(|e| tunnel_light_at(&r, 0, e, on_quad(e, w), 1e-6));
        assert!((lit[0] - lit[1]).abs() < 1e-4, "{lit:?}");
    }

    #[test]
    fn rungs_are_evenly_spaced_and_dim_with_depth() {
        let depth = 1.5;
        let spacing = depth / RUNGS as f32;
        let w = RUNG_FILL * spacing;
        for k in 0..RUNGS {
            let middle = k as f32 * spacing + 0.5 * w;
            let (at, dz) = rung_at(middle, depth);
            assert_eq!(at, k);
            assert!(dz.abs() < 1e-5);
            // Dark between rungs.
            let (_, dz) = rung_at(middle + 0.5 * spacing - 1e-3, depth);
            assert_close!(rung_cover(dz, w, 1e-6), 0.0);
            // A full bar lights every rung, each RUNG_DIM of the one in
            // front, in the middle of its slot.
            assert!((rung_light(k, 0.5, 1.0) - RUNG_DIM.powf(k as f32)).abs() < 1e-5);
        }
        assert!(rung_light(RUNGS - 1, 0.5, 1.0) < 0.15);
        // A rung covers a pixel inside it; one thinner than a pixel dims
        // by the share it covers.
        assert_close!(rung_cover(0.0, w, 1e-6), 1.0);
        assert_close!(rung_cover(0.0, w, 4.0 * w), 0.25);
        assert_close!(rung_cover(0.5 * w, w, 0.5 * w), 0.5);
        // A bar's height is how deep its light reaches: 0.45 of the depth
        // lights rungs 0 to 4 (0.4 of it), not 5 (0.5); half at 0.5.
        // Past its bar a rung is the faint frame alone.
        let frame = |k: usize| FRAME_LIGHT * RUNG_DIM.powf(k as f32);
        let lit: Vec<bool> = (0..RUNGS)
            .map(|k| rung_light(k, 0.5, 0.45) > frame(k) + 1e-6)
            .collect();
        assert_eq!(lit.iter().filter(|&&l| l).count(), 5, "{lit:?}");
        assert!(lit[..5].iter().all(|&l| l));
        assert!((rung_light(5, 0.5, 0.5) - 0.5 * 0.8 * RUNG_DIM.powi(5)).abs() < 1e-5);
        // Silence: the rim alone, along the whole edge, gaps included.
        assert_close!(rung_light(0, 0.5, 0.0), LINE_LIGHT);
        assert_close!(rung_light(0, 0.0, 0.0), LINE_LIGHT);
        assert!((1..RUNGS).all(|k| (rung_light(k, 0.5, 0.0) - frame(k)).abs() < 1e-6));
        // The rim brightens with its bar, in the bar's slot; a deeper rung
        // is only in the slot.
        assert_close!(rung_light(0, 0.5, 1.0), 1.0);
        assert_close!(rung_light(0, 0.0, 1.0), LINE_LIGHT);
        assert_close!(rung_light(3, 0.0, 1.0), frame(3));
    }

    #[test]
    fn the_history_steps_by_time_and_carries_a_hit_into_the_wall() {
        let bars = |hit: bool| -> Vec<f32> {
            (0..24)
                .map(|i| if hit && i == 3 { 1.0 } else { 0.0 })
                .collect()
        };
        // One frame of a hit on bar 3, then quiet, for a third of a second,
        // at 72 and at 90 Hz: five steps of 60 ms, the hit on rung 5 in both.
        let run = |hz: f32| {
            let mut h = History::default();
            let frames = (hz / 3.0).round() as usize;
            for f in 0..frames {
                h.step(1.0 / hz, &bars(f == 0));
            }
            h
        };
        for h in [run(72.0), run(90.0)] {
            for k in 1..RUNGS {
                let want: Vec<f32> = bars(k == 5);
                assert_close!(h.rung(k), want, "rung {k}");
            }
        }
        // Step by step: rung k shows the bars k steps ago.
        let mut h = History::default();
        h.step(0.0, &bars(true));
        for k in 1..RUNGS {
            h.step(HISTORY_STEP_S, &bars(false));
            assert_close!(h.rung(k)[3], 1.0, "step {k}");
            assert!((1..k).all(|j| h.rung(j)[3] == 0.0));
        }
        // A hit shorter than a step is kept: the step's loudest.
        let mut h = History::default();
        h.step(0.01, &bars(false));
        h.step(0.01, &bars(true));
        h.step(0.045, &bars(false));
        assert_close!(h.rung(1)[3], 1.0);
        // A stall steps at most every row once, and they hold the bars as
        // they are; bad numbers do not move it; a new bar count starts over.
        h.step(30.0, &bars(false));
        assert!((1..RUNGS).all(|k| h.rung(k).iter().all(|&v| v == 0.0)));
        h.step(f32::NAN, &[f32::NAN; 24]);
        assert!((1..RUNGS).all(|k| h.rung(k).iter().all(|&v| v == 0.0)));
        h.step(HISTORY_STEP_S, &[1.0; 8]);
        assert_eq!(h.rung(1).len(), 8);
        assert_close!(h.rung(1), [1.0; 8]);
        assert_close!(h.rung(2), [0.0; 8]);
    }

    #[test]
    fn the_tunnel_depth_scales_with_the_face_and_is_clamped() {
        // A 40 cm box face: 0.6 of 40 cm.
        assert_close!(tunnel_depth([0.3, 0.2], DEPTH_FRACTION), 0.24);
        // A 4 x 2.5 m wall: 1.5 m, at the clamp; a 4 x 3 m one clamped.
        assert_close!(tunnel_depth([2.0, 1.25], DEPTH_FRACTION), MAX_DEPTH_M);
        assert_close!(tunnel_depth([2.0, 1.5], DEPTH_FRACTION), MAX_DEPTH_M);
        // A 10 cm frame: 6 cm, raised to 15.
        assert_close!(tunnel_depth([0.05, 0.4], DEPTH_FRACTION), MIN_DEPTH_M);
        // The knob: 1.2 doubles the box's; past the range clamped; not a
        // number, the default.
        assert_close!(tunnel_depth([0.3, 0.2], 1.2), 0.48);
        assert_close!(tunnel_depth([0.3, 0.2], 9.0), 0.8);
        assert_close!(tunnel_depth([0.3, 0.2], -1.0), MIN_DEPTH_M);
        assert_close!(tunnel_depth([0.3, 0.2], f32::NAN), 0.24);
    }

    #[test]
    fn the_fragment_lights_rungs_into_depth_and_leaves_the_center_dark() {
        let t = tunnel(0.5);
        let spacing = t.depth / RUNGS as f32;
        let middle = |k: usize| k as f32 * spacing + 0.5 * RUNG_FILL * spacing;
        // Bar 5 of the right wall's 24 (each 2.5 / 24 m), in the middle of
        // its slot: along the edge from its low end (the top, +Y) 5.5 bars.
        let y = 1.25 + 1.25 - 5.5 * 2.5 / 24.0;
        let at = |k: usize| Vec3::new(2.0, y, -1.98 - middle(k));
        let eye = Vec3::new(0.0, 1.25, 0.0);
        let light = |r: &[[f32; 4]], w: Vec3| tunnel_light_at(r, 0, eye, on_quad(eye, w), 1e-6);
        let outline = Outline {
            strips: Vec::new(),
            tunnels: vec![t],
        };
        let mut r = Vec::new();
        // Every bar full now and every step back: every rung lit, each
        // dimmer than the one in front.
        let full = [1.0f32; 24];
        let mut hist = History::default();
        for _ in 0..RUNGS {
            hist.step(HISTORY_STEP_S, &full);
        }
        rows(&full, &hist, &outline, &mut r);
        for k in 0..RUNGS {
            let got = light(&r, at(k));
            assert!(
                (got - RUNG_DIM.powf(k as f32)).abs() < 1e-3,
                "rung {k}: {got}"
            );
            // Dark between rungs.
            let between = at(k) - Vec3::Z * 0.5 * spacing;
            assert!(light(&r, between) < 1e-4, "after rung {k}");
        }
        // The alpha: the peak times the light.
        let a = tunnel_alpha_at(&r, 0, eye, on_quad(eye, at(2)), 1e-6);
        assert!((a - 0.5 * RUNG_DIM.powi(2)).abs() < 1e-3, "{a}");
        // The center reaches the end: nothing, so the surface shows.
        let center = Vec3::new(0.0, 1.25, -1.97);
        assert_close!(light(&r, center), 0.0);
        // Quiet: the rim alone, at the line's light.
        let quiet = [0.0f32; 24];
        rows(&quiet, &History::default(), &outline, &mut r);
        assert!((light(&r, at(0)) - LINE_LIGHT).abs() < 1e-3);
        // Past the rim, quiet rungs are the faint frames alone.
        let frame = |k: usize| FRAME_LIGHT * RUNG_DIM.powf(k as f32);
        assert!((1..RUNGS).all(|k| (light(&r, at(k)) - frame(k)).abs() < 1e-3));
        // No bars at all: the rim alone too.
        rows(&[], &History::default(), &outline, &mut r);
        assert!((light(&r, at(0)) - LINE_LIGHT).abs() < 1e-3);
        // A hit three steps ago, quiet since: rung 3 alone lit past its frame.
        let mut hist = History::default();
        hist.step(0.0, &full);
        for _ in 0..3 {
            hist.step(HISTORY_STEP_S, &quiet);
        }
        rows(&quiet, &hist, &outline, &mut r);
        let lit: Vec<bool> = (1..RUNGS)
            .map(|k| light(&r, at(k)) > frame(k) + 1e-3)
            .collect();
        assert_eq!(
            lit,
            [false, false, true, false, false, false, false, false, false]
        );
        // From a grazing eye, nothing.
        rows(&full, &hist, &outline, &mut r);
        let low = Vec3::new(30.0, 1.25, -1.98 + 0.5);
        assert!(tunnel_light_at(&r, 0, low, on_quad(low, at(1)), 1e-6) < 1e-6);
    }

    #[test]
    fn each_shape_draws_in_its_own_mode() {
        let with = |s: Shape, mode: Mode| Shape {
            look: s.look.map(|l| Look { mode, ..l }),
            ..s
        };
        let side_wall = Shape {
            center: Vec3::new(-2.0, 1.25, 0.0),
            rot: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            ..wall()
        };
        let shapes = [
            with(wall(), Mode::In),
            with(floor(), Mode::Face),
            with(table(), Mode::Standing),
            with(side_wall, Mode::Off),
        ];
        let mut out = Outline::default();
        let drawn = outline(
            &shapes,
            HEAD,
            HEIGHT_FRACTION,
            DEPTH_FRACTION,
            24,
            0.25,
            &mut out,
        );
        // The wall a tunnel, the floor and the table's two faces strips,
        // the side wall nothing.
        assert_eq!(
            drawn,
            Drawn {
                planes: 2,
                volumes: 1,
                strips: 12,
                tunnels: 1
            }
        );
        assert!(
            out.strips[..4].iter().all(|s| s.inset > 0.0),
            "the floor on its face"
        );
        assert!(
            out.strips[4..].iter().all(|s| s.inset == 0.0),
            "the table standing"
        );
        let t = out.tunnels[0];
        assert!(near(t.normal, Vec3::Z));
        // 0.6 of its smaller side, 2.5 m less the 2.5 cm clear of the floor.
        assert!((t.depth - 0.6 * 2.475).abs() < 1e-5, "{}", t.depth);
        // Its opening is the face the strips would outline: moved clear of
        // the floor's slab at the bottom and the side wall's on the left,
        // 2.5 cm each.
        let (strips, _) = run(&[wall(), floor(), side_wall], HEAD, Mode::Face);
        let corners: Vec<Vec3> = strips[..4].iter().flat_map(|s| s.corners).collect();
        let reach = (t.axes[0] * t.half[0]).abs() + (t.axes[1] * t.half[1]).abs();
        let (lo, hi) = (t.center - reach, t.center + reach);
        let min = corners.iter().fold(Vec3::MAX, |m, c| m.min(*c));
        let max = corners.iter().fold(Vec3::MIN, |m, c| m.max(*c));
        assert!(
            (lo.x - min.x).abs() < 1e-5 && (lo.y - min.y).abs() < 1e-5,
            "{lo} {min}"
        );
        assert!(
            (hi.x - max.x).abs() < 1e-5 && (hi.y - max.y).abs() < 1e-5,
            "{hi} {max}"
        );
        assert!(
            (min.x - (-2.0 + 0.02 + CLEAR_MARGIN_M)).abs() < 1e-5,
            "{min}"
        );
        assert!((min.y - (0.02 + CLEAR_MARGIN_M)).abs() < 1e-5, "{min}");
        assert_eq!(t.bars, [24, 24]);
        // A volume in: a tunnel per face turned to the head.
        let (out, drawn) = outlined(&[table()], HEAD, Mode::In);
        assert_eq!(out.tunnels.len(), 2);
        assert!(out.strips.is_empty());
        assert_eq!(drawn.volumes, 1);
        // Every kind off: nothing.
        let (out, drawn) = outlined(&[wall(), table()], HEAD, Mode::Off);
        assert!(out.is_empty());
        assert_eq!(drawn, Drawn::default());
    }

    #[test]
    fn the_themes_mode_per_kind_reaches_the_outline() {
        use crate::surfaces::{KIND_FLOOR, KIND_TABLE, KIND_WALL};
        use crate::themes::{Theme, edge_mode};
        // The app's rule: each shape in the choice's mode for its kind's
        // mode under the room's saved theme.
        let room = |choice: Choice, theme: Option<Theme>| {
            [
                (wall(), KIND_WALL),
                (floor(), KIND_FLOOR),
                (table(), KIND_TABLE),
            ]
            .map(|(s, kind)| Shape {
                look: s.look.map(|l| Look {
                    mode: choice.mode(edge_mode(theme, kind)),
                    ..l
                }),
                ..s
            })
        };
        let draw = |shapes: &[Shape]| {
            let mut out = Outline::default();
            outline(
                shapes,
                HEAD,
                HEIGHT_FRACTION,
                DEPTH_FRACTION,
                24,
                0.25,
                &mut out,
            );
            out
        };
        // Mirror: the wall and the table's two faces in, the floor's rings
        // with the bars on its face.
        let out = draw(&room(Choice::Theme, Some(Theme::Mirror)));
        assert_eq!(out.tunnels.len(), 3);
        assert_eq!(out.strips.len(), 4);
        assert!(out.strips.iter().all(|s| s.inset > 0.0));
        assert!(near(out.tunnels[0].normal, Vec3::Z), "the wall first");
        // Drift, or no theme saved: every kind on the face, as worn.
        for theme in [Some(Theme::Drift), None] {
            let out = draw(&room(Choice::Theme, theme));
            assert!(out.tunnels.is_empty());
            assert_eq!(out.strips.len(), 16);
        }
        // Forced, every kind in that mode whatever the theme.
        let out = draw(&room(Choice::Force(Mode::Standing), Some(Theme::Mirror)));
        assert!(out.tunnels.is_empty());
        assert!(out.strips.len() == 16 && out.strips.iter().all(|s| s.inset == 0.0));
        let out = draw(&room(Choice::Force(Mode::In), Some(Theme::Drift)));
        assert_eq!((out.strips.len(), out.tunnels.len()), (0, 4));
        assert!(draw(&room(Choice::Force(Mode::Off), Some(Theme::Mirror))).is_empty());
        // One buffer for the mix: the strips, then the tunnels.
        let out = draw(&room(Choice::Theme, Some(Theme::Mirror)));
        let mut r = Vec::new();
        rows(&[0.5; 24], &History::default(), &out, &mut r);
        assert_eq!(r.len(), HEAD_ROWS + 7 * STRIP_ROWS);
        assert_close!(r[0][1], 4.0);
    }

    #[test]
    fn a_tunnel_runs_its_bars_from_the_same_low_ends_as_the_strips() {
        // Each strip's low end, in the tunnel's frame, is where the
        // tunnel's along-the-edge position is 0: the same band meets
        // itself at every corner in both modes.
        let tilted = Shape {
            rot: Quat::from_euler(glam::EulerRot::YXZ, 0.4, 0.3, -0.2),
            ..table()
        };
        let head = Vec3::new(1.5, 1.2, 0.0);
        for shape in [table(), tilted, wall(), floor()] {
            let (strips, _) = outlined(&[shape], head, Mode::Face);
            let (tunnels, _) = outlined(&[shape], head, Mode::In);
            assert_eq!(strips.strips.len(), 4 * tunnels.tunnels.len());
            for (t, face) in tunnels.tunnels.iter().zip(strips.strips.chunks(4)) {
                let chi = if t.low { 1.0 } else { -1.0 };
                for s in face {
                    let q = s.corners[0] - LIFT_M * t.normal - t.center;
                    let (u, v) = (q.dot(t.axes[0]), q.dot(t.axes[1]));
                    let mid = (s.corners[0] + s.corners[1]) * 0.5 - t.center;
                    let (mu, mv) = (mid.dot(t.axes[0]), mid.dot(t.axes[1]));
                    // The edge the strip runs on, and the tunnel's position
                    // along it at the strip's low end.
                    let along = if (mu.abs() - t.half[0]).abs() < 1e-4 {
                        t.half[1] - chi * mu.signum() * v
                    } else {
                        assert!((mv.abs() - t.half[1]).abs() < 1e-4);
                        t.half[0] - chi * mv.signum() * u
                    };
                    assert!(along.abs() < 1e-4, "{along}");
                }
            }
        }
    }
}
