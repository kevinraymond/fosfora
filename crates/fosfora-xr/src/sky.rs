//! One sky around the room (board #5436): the ported Aurora on every wall
//! and the ceiling as one picture, and its reflection on the floor. The
//! plain-number side (the room's point, the mapping, the seam, the zenith,
//! the reflection, the rim between neighbors, the rows), so it builds and
//! tests on the desktop; [`SKY_WGSL`] is the shader side, appended to
//! Aurora's module (`surface_port::compose`).
//!
//! **The mapping** ([`angles`], [`frame`]). On a wall or the ceiling the
//! effect's frame no longer follows the face's own (u, v). The fragment's
//! world position is taken as a direction from one fixed point in the room
//! ([`Room::point`]): its azimuth about the point runs along the frame's x,
//! its elevation up the frame's y. Every wall and the ceiling then show one
//! continuous picture: the curtains run around the room and rise onto the
//! ceiling, where the azimuth converges above the point.
//!
//! **The frame's shape.** Elevation maps to y linearly: the frame's bottom
//! row is the horizon ([`HORIZON_RAD`]) and its top [`HEIGHT_RAD`] above it
//! (63°, what a 2.5 m wall spans seen from 2 m, so the curtains keep the
//! proportions they have on one wall today). The frame is a whole turn
//! wide at the same pixels per radian ([`RESOLUTION`], [`PX_PER_RAD`]), so
//! at the horizon a degree across is a degree up. With the `.pfx`'s band
//! spread the lowest band sits 2° under the horizon and the highest 65°
//! over it: with the walls 2 m off and a 2.4 m ceiling, three bands on the
//! walls, one along the ceiling's edge and three as rings on the ceiling.
//!
//! **The seam.** The effect is not periodic in x, so the turn has one
//! seam. It is behind the seated wearer: azimuth 0 is the direction the
//! wearer faced at launch or at the last recenter ([`facing`]), fixed after
//! that, and the frame's ends meet opposite it. Within [`SEAM_RAD`] of the
//! seam on either side the effect is evaluated a second time at the other
//! end's continuation and the two cross-fade, half and half on the seam
//! ([`seam_weight`]).
//!
//! **The zenith** ([`zenith_fade`]). Overhead the azimuth converges, so a
//! frame's row shrinks to a point and the noise would pinch and alias. The
//! light fades out between [`ZENITH_RAD`]'s two elevations, above the
//! highest band's reach: the rings close in on a dark center.
//!
//! **The reflection** ([`reflect`], [`exit`]). On the floor Aurora draws
//! the sky's reflection: the view ray from the eye (solved from the eye's
//! `view_proj` in the vertex stage, as the tunnel does) is reflected about
//! the floor, followed to where it leaves the room's box (its walls in
//! plan and its ceiling, [`Room`]), and the sky is shown in the direction
//! of that point from the sky's point: the floor shows what the walls
//! show, and moves as a reflection does with the head. It is
//! [`REFLECT_GAIN`] of the sky, fades out toward the wearer's feet
//! ([`REFLECT_COS`]), and the floor's uniform widens the curtains' glow
//! (`surface_port::Port::reflection`), which softens them along the view
//! direction. One evaluation per fragment: across the seam the reflection
//! fades out instead of cross-fading.
//!
//! **The rim and the joins** ([`join`]). The surfaces pass fades a face
//! out over its last 5 cm. On the sky's faces the fade is dropped on every
//! edge another of the sky's faces continues (two walls in a corner, a
//! wall under the ceiling), so no dark line runs down a corner. Where two
//! faces meet across a corner, each runs [`OVERLAP_M`] past the other's
//! drawn plane, so both quads cover every pixel near the join, and each
//! fragment there keeps itself only if its own plane is the one the
//! pixel's view ray meets first ([`keeps`]). Both faces decide that from
//! the same numbers: the ray through the pixel's center (solved from the
//! eye camera and the target's size, not from the fragment's
//! interpolated position) and the same two planes, as the same floats in
//! both faces' rows, taken in the same order. So the two answers are
//! exact complements: no pixel is left to neither (a dark crack) or drawn
//! by both (a bright one, the blend adds). Trimming the quads to meet on
//! one line left both (board #5438): the trim moved each edge to where the
//! neighbor's plane crosses its middle, and in a scanned room the planes
//! are never quite level or plumb, so the two trimmed edges crossed there
//! and parted toward their ends. An edge with nothing beyond it keeps its
//! extent and the fade.
//!
//! **The layers** (board #5438, [`Layers`], [`far`]). The sky is drawn in
//! up to three layers. The nearest is the sky above, on the surfaces. Each
//! farther one is the sky on a shell: the room's box pushed out by
//! [`SHELL_M`] (times `debug.fosfora.skydepth`) in plan and above. The view
//! ray from the eye through the fragment is followed on past the surface
//! to where it leaves the shell ([`box_exit`]), and that point is mapped
//! by its direction from the sky's point, as the near layer maps the
//! fragment. A far layer is then fixed on its shell in the room, so it
//! moves against the surfaces as the head moves, by its depth, for each
//! eye. A box keeps the shell continuous across corners and onto the
//! ceiling (it is convex), the same depth behind every wall and the
//! ceiling, and cheap: the slab exit the reflection already runs. The far
//! layers are other curtains: taken from further around the frame
//! ([`LAYER_TURN_RAD`]), their bands lower ([`LAYER_DROP_RAD`]: between
//! the near layer's), at less light ([`LAYER_LIGHT`]) and cooler
//! ([`LAYER_TINT`]). Each is composited under the nearer ones ([`under`]).
//! A far layer fades by its own elevation at the zenith, fades out across
//! the seam instead of cross-fading, fades out under its lowest band's
//! reach ([`BANDS_FLOOR_RAD`]), and is not evaluated where none of its
//! light is left. The reflection stays one layer, the near one.

use std::f32::consts::{PI, TAU};

use glam::{Quat, Vec2, Vec3};

use crate::surface_fx::LIFT_M;
use crate::surface_port::UNIFORM_ROWS;
use crate::surfaces::{Face, KIND_CEILING, KIND_FLOOR, KIND_WALL, SurfaceBehavior};

/// The frame's height: radians of elevation from its bottom row to its top.
pub const HEIGHT_RAD: f32 = 1.1;
/// The elevation of the frame's bottom row (rad): the horizon.
pub const HORIZON_RAD: f32 = 0.0;
/// The frame's pixels per radian, along both axes.
pub const PX_PER_RAD: f32 = 1024.0;
/// The sky's virtual resolution: a whole turn wide, [`HEIGHT_RAD`] high.
pub const RESOLUTION: [f32; 2] = [TAU * PX_PER_RAD, HEIGHT_RAD * PX_PER_RAD];
/// How far either side of the seam the two ends cross-fade (rad of
/// azimuth).
pub const SEAM_RAD: f32 = 0.2;
/// The elevations (rad) where the zenith fade starts and where the light
/// is gone: 74° and 84°.
pub const ZENITH_RAD: [f32; 2] = [1.29, 1.47];
/// The reflection's share of the sky's light.
pub const REFLECT_GAIN: f32 = 0.4;
/// The cosine of the view ray's angle to the floor's normal where the
/// reflection is full (60°, toward the walls) and where it is gone (25°,
/// toward the feet).
pub const REFLECT_COS: [f32; 2] = [0.5, 0.906];
/// A seated wearer's eye over the floor (m): the sky's point's height.
pub const SEATED_EYE_M: f32 = 1.2;
/// The least room between the point and the ceiling (m).
pub const HEADROOM_M: f32 = 0.3;
/// How close another face must come to the middle of an edge (m) for the
/// edge to count as continued ([`join`]).
pub const NEIGHBOR_M: f32 = 0.1;
/// The stand-in room without a scanned one: its half extents in plan and
/// its ceiling over the floor (m).
pub const STAGE_HALF_M: f32 = 2.5;
pub const STAGE_CEILING_M: f32 = 2.5;

/// The knobs of the sky's layers: how many, and a multiplier on the far
/// shells' depths.
pub const KNOB_LAYERS: &str = "debug.fosfora.skylayers";
pub const KNOB_DEPTH: &str = "debug.fosfora.skydepth";
/// The most layers, and how many without the knob.
pub const LAYERS_MAX: u32 = 3;
pub const LAYERS_DEFAULT: u32 = 2;
/// How far the far layers' shells are behind the walls and the ceiling
/// (m), the middle layer's and the farthest's, at a depth multiplier of 1.
pub const SHELL_M: [f32; 2] = [1.5, 4.0];
/// The largest depth multiplier.
pub const DEPTH_MAX: f32 = 4.0;
/// Each layer's share of the sky's light, the nearest first.
pub const LAYER_LIGHT: [f32; 3] = [1.0, 0.6, 0.35];
/// Each layer's tint on the effect's colors (after the coverage, as the
/// lane's tint): the farther, the cooler.
pub const LAYER_TINT: [[f32; 3]; 3] = [[1.0, 1.0, 1.0], [0.8, 0.9, 1.0], [0.6, 0.78, 1.0]];
/// How far around the frame each layer's curtains are taken from (rad of
/// azimuth): far enough that no band's wave lines up with the near
/// layer's or the other's (every band's two waves at least 0.4 rad of
/// phase off), and the ripple's noise is another stretch. The seam stays
/// behind: a turn on the frame moves no seam.
pub const LAYER_TURN_RAD: [f32; 3] = [0.0, 2.1, 3.2];
/// How much lower each layer's bands sit (rad of elevation): half and a
/// quarter of the bands' spacing ([`BAND_SPACING_RAD`]), so the middle
/// layer's bands run between the near layer's and the farthest's between
/// those.
pub const LAYER_DROP_RAD: [f32; 3] = [0.0, 0.098_214_29, 0.049_107_14];
/// The bands' spacing in elevation at the `.pfx`'s band spread (0.5 * 1.5
/// + 0.5 = 1.25 of the frame's height over the seven).
pub const BAND_SPACING_RAD: f32 = HEIGHT_RAD * 1.25 / 7.0;
/// The elevations on a far layer's frame (its own, plus its drop) where it
/// is gone and where it is whole: under the lowest band's reach (its
/// center, the waves' and the ripple's 0.12 and three of its widest glow's
/// widths, -0.336 rad) a layer adds only the shimmer, so it fades out
/// there and is not evaluated below.
pub const BANDS_FLOOR_RAD: [f32; 2] = [-0.44, -0.34];

/// Where the sky sits in the port block: `mode.y` (the row of
/// `surface_port::rows`' overlay flag) carries the [`Map`]; the four rows
/// after that row are the sky's own: the rim per edge, the point (and the
/// ceiling), the room in plan, the room's axis and the facing; then the
/// joins ([`Edges`]): the face's drawn plane, per edge the drawn plane of
/// the face joined across it, and per edge from how far toward it the
/// join decides.
pub const MODE_ROW: usize = 7;
pub const EDGES_ROW: usize = 8;
pub const POINT_ROW: usize = 9;
pub const ROOM_ROW: usize = 10;
pub const AXES_ROW: usize = 11;
pub const PLANE_ROW: usize = 12;
pub const CUT_ROW: usize = 13;
pub const CUT_FROM_ROW: usize = 17;
/// How far a joined edge runs past the neighbor's drawn plane (m), at
/// both its ends ([`join`]): the fragment decides the rest ([`keeps`]).
pub const OVERLAP_M: f32 = 0.05;
/// `cut_from` of an edge with no join: never reached.
pub const NO_CUT: f32 = 1e9;

/// How a ported face maps to its effect's frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Map {
    /// The face's own (u, v), as every port did before the sky.
    Face,
    /// The sky: the direction from the sky's point.
    Sky,
    /// The sky's reflection, on a floor.
    Reflection,
}

impl Map {
    /// The mapping of a face of `kind` running `behavior`, the sky `on` or
    /// off (`debug.fosfora.sky`): Aurora on a wall or the ceiling is the
    /// sky, on a floor its reflection; anything else, and everything with
    /// the sky off, maps per face.
    pub fn of(on: bool, behavior: SurfaceBehavior, kind: u32) -> Self {
        match kind {
            _ if !on || behavior != SurfaceBehavior::Aurora => Self::Face,
            KIND_WALL | KIND_CEILING => Self::Sky,
            KIND_FLOOR => Self::Reflection,
            _ => Self::Face,
        }
    }

    /// The value of `mode.y`.
    pub fn code(self) -> f32 {
        match self {
            Self::Face => 0.0,
            Self::Sky => 1.0,
            Self::Reflection => 2.0,
        }
    }
}

/// The room the sky is fit to: a rectangle in plan around the scanned
/// walls, and the floor's and the ceiling's heights.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Room {
    /// The rectangle's center (x, z).
    pub center: Vec2,
    /// Its first axis (x, z, unit), along the longest wall; the second is
    /// this turned a quarter (`(-axis.y, axis.x)`).
    pub axis: Vec2,
    /// Its half extents along the axis and across it (m).
    pub half: Vec2,
    pub floor_y: f32,
    pub ceiling_y: f32,
    /// How many walls it was fit to; 0 for the stage's stand-in.
    pub walls: usize,
}

/// A wall box's footprint in plan: the ends of its horizontal in-plane
/// axis (x, z). The box's thinnest axis is its normal; of the other two
/// the one closer to level runs along the floor.
pub fn wall_segment(center: Vec3, rot: Quat, half: Vec3) -> (Vec2, Vec2) {
    let axes = [rot * Vec3::X, rot * Vec3::Y, rot * Vec3::Z];
    let h = half.to_array();
    let thin = (0..3).fold(2, |m, i| if h[i] < h[m] { i } else { m });
    let (i, j) = ((thin + 1) % 3, (thin + 2) % 3);
    let along = if axes[i].y.abs() <= axes[j].y.abs() {
        i
    } else {
        j
    };
    let e = Vec2::new(axes[along].x, axes[along].z) * h[along];
    let mid = Vec2::new(center.x, center.z);
    (mid - e, mid + e)
}

/// The lowest and the highest point of a box (its world bounds in y).
fn y_bounds(center: Vec3, rot: Quat, half: Vec3) -> (f32, f32) {
    let reach: f32 = [Vec3::X, Vec3::Y, Vec3::Z]
        .iter()
        .zip(half.to_array())
        .map(|(a, h)| (rot * *a).y.abs() * h)
        .sum();
    (center.y - reach, center.y + reach)
}

impl Room {
    /// The room of a scan's boxes (kind, center, rotation, half extents),
    /// hidden walls included (they close the room's outline): the plan
    /// rectangle around every wall's footprint, turned to the longest
    /// wall; the floor at the scanned floor's top, else the walls' foot;
    /// the ceiling at the scanned ceiling's underside, else the walls'
    /// top. `None` without a wall.
    pub fn fit(boxes: impl IntoIterator<Item = (u32, Vec3, Quat, Vec3)>) -> Option<Self> {
        let mut ends = Vec::new();
        let mut longest = (0.0f32, Vec2::X);
        let (mut foot, mut top) = (f32::MAX, f32::MIN);
        let (mut floor, mut ceiling) = (None::<f32>, None::<f32>);
        for (kind, center, rot, half) in boxes {
            let (lo, hi) = y_bounds(center, rot, half);
            match kind {
                KIND_WALL => {
                    let (a, b) = wall_segment(center, rot, half);
                    let len = a.distance(b);
                    if len > longest.0 {
                        longest = (len, (b - a) / len);
                    }
                    ends.extend([a, b]);
                    foot = foot.min(lo);
                    top = top.max(hi);
                }
                KIND_FLOOR => floor = Some(floor.map_or(hi, |f| f.min(hi))),
                KIND_CEILING => ceiling = Some(ceiling.map_or(lo, |c| c.min(lo))),
                _ => {}
            }
        }
        if ends.is_empty() {
            return None;
        }
        let axis = longest.1;
        let across = Vec2::new(-axis.y, axis.x);
        let span = |dir: Vec2| {
            ends.iter()
                .map(|e| e.dot(dir))
                .fold((f32::MAX, f32::MIN), |(lo, hi), s| (lo.min(s), hi.max(s)))
        };
        let (s, t) = (span(axis), span(across));
        Some(Self {
            center: axis * 0.5 * (s.0 + s.1) + across * 0.5 * (t.0 + t.1),
            axis,
            half: Vec2::new(0.5 * (s.1 - s.0), 0.5 * (t.1 - t.0)),
            floor_y: floor.unwrap_or(foot),
            ceiling_y: ceiling.unwrap_or(top),
            walls: ends.len() / 2,
        })
    }

    /// The stand-in without a scanned room: a square [`STAGE_HALF_M`]
    /// around `center` (x, z), the stage's, its ceiling
    /// [`STAGE_CEILING_M`] over `floor_y`.
    pub fn stage(center: Vec2, floor_y: f32) -> Self {
        Self {
            center,
            axis: Vec2::X,
            half: Vec2::splat(STAGE_HALF_M),
            floor_y,
            ceiling_y: floor_y + STAGE_CEILING_M,
            walls: 0,
        }
    }

    /// The sky's point: the rectangle's center at a seated eye height
    /// ([`SEATED_EYE_M`]), kept [`HEADROOM_M`] under the ceiling.
    pub fn point(&self) -> Vec3 {
        let y = (self.floor_y + SEATED_EYE_M)
            .min(self.ceiling_y - HEADROOM_M)
            .max(self.floor_y);
        Vec3::new(self.center.x, y, self.center.y)
    }
}

/// The level direction a head turned by `head_rot` faces (x, z, unit): its
/// view leveled; looking straight down or up, the way the top of the head
/// points (down) or away from it (up).
pub fn facing(head_rot: Quat) -> Vec2 {
    let view = head_rot * Vec3::NEG_Z;
    let level = if Vec2::new(view.x, view.z).length() >= 0.2 {
        view
    } else {
        head_rot * Vec3::Y * -view.y.signum()
    };
    Vec2::new(level.x, level.z).normalize_or(Vec2::NEG_Y)
}

/// The sky's layers (board #5438): how many are drawn
/// (`debug.fosfora.skylayers`, 1 to [`LAYERS_MAX`]) and the multiplier on
/// the far shells' depths (`debug.fosfora.skydepth`, 0 to [`DEPTH_MAX`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layers {
    pub count: u32,
    pub depth: f32,
}

impl Default for Layers {
    fn default() -> Self {
        Self {
            count: LAYERS_DEFAULT,
            depth: 1.0,
        }
    }
}

impl Layers {
    /// The layers the knobs ask for: `skylayers` 1, 2 or 3, else the
    /// default; `skydepth` a finite number, clamped, else 1.
    pub fn from_knobs(prop: impl Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        Self {
            count: match prop(KNOB_LAYERS).as_deref().map(str::trim) {
                Some("1") => 1,
                Some("2") => 2,
                Some("3") => 3,
                _ => d.count,
            },
            depth: prop(KNOB_DEPTH)
                .and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
                .map_or(d.depth, |v| v.clamp(0.0, DEPTH_MAX)),
        }
    }

    /// How far the far layers' shells are behind the walls and the
    /// ceiling (m).
    pub fn shells(&self) -> [f32; 2] {
        SHELL_M.map(|m| m * self.depth)
    }

    /// The state in a few words, for the log.
    pub fn describe(&self) -> String {
        let [mid, far] = self.shells();
        let depth = format!("{KNOB_DEPTH} {:.2}", self.depth);
        let what = match self.count {
            1 => format!("the sky on the surfaces alone ({depth} unused)"),
            2 => format!(
                "the sky on the surfaces and a layer {mid:.2} m behind them at {} of the light ({depth})",
                LAYER_LIGHT[1]
            ),
            _ => format!(
                "the sky on the surfaces and layers {mid:.2} and {far:.2} m behind them at {} and {} of the light ({depth})",
                LAYER_LIGHT[1], LAYER_LIGHT[2]
            ),
        };
        format!("{} ({KNOB_LAYERS}), {what}", self.count)
    }

    /// The log's line at launch.
    pub fn line(&self) -> String {
        format!("sky layers: {}", self.describe())
    }

    /// The log's line when a knob changes them.
    pub fn change_line(&self, before: &Self) -> String {
        format!(
            "sky layers: {} (was: {})",
            self.describe(),
            before.describe()
        )
    }
}

/// The sky for this frame: the room, its point, the facing the seam is
/// behind, and its layers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sky {
    pub room: Room,
    pub point: Vec3,
    pub facing: Vec2,
    pub layers: Layers,
}

impl Sky {
    /// The sky of `room` with its seam behind `facing`, in the default
    /// layers.
    pub fn new(room: Room, facing: Vec2) -> Self {
        Self {
            room,
            point: room.point(),
            facing,
            layers: Layers::default(),
        }
    }

    /// The direction the seam is in (x, z): behind the facing.
    pub fn seam(&self) -> Vec2 {
        -self.facing
    }
}

/// The azimuth of `d` (a direction from the sky's point) about the point,
/// from `facing` and positive to the right, in -π..π, and its elevation.
/// The WGSL's `xr_sky_angles`; the `1e-6` keeps `atan2(0, 0)` out
/// straight overhead.
pub fn angles(d: Vec3, facing: Vec2) -> Vec2 {
    let plan = Vec2::new(d.x, d.z);
    let right = Vec2::new(-facing.y, facing.x);
    Vec2::new(
        plan.dot(right).atan2(plan.dot(facing) + 1e-6),
        d.y.atan2(plan.length()),
    )
}

/// The frame coordinate (px) of azimuth `a.x` and elevation `a.y` at
/// `resolution`: the facing at the middle column, the seam at both ends,
/// the horizon at the bottom row. The WGSL's `xr_sky_frame`.
pub fn frame(a: Vec2, resolution: [f32; 2]) -> Vec2 {
    Vec2::new(a.x / TAU + 0.5, 1.0 - (a.y - HORIZON_RAD) / HEIGHT_RAD) * Vec2::from(resolution)
}

/// WGSL's `smoothstep`.
fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The other end's weight at azimuth `az`: 0 farther than [`SEAM_RAD`]
/// from the seam, a half on it.
pub fn seam_weight(az: f32) -> f32 {
    0.5 * smoothstep(PI - SEAM_RAD, PI, az.abs())
}

/// The light left at elevation `el`: 1 under [`ZENITH_RAD`]'s first, 0
/// over its second.
pub fn zenith_fade(el: f32) -> f32 {
    1.0 - smoothstep(ZENITH_RAD[0], ZENITH_RAD[1], el)
}

/// The ray from `eye` to the floor point `p` reflected about the floor
/// (level), and the reflection's strength there: full where the view ray
/// meets the floor at a slant (toward the walls), gone where it looks
/// straight down (the feet).
pub fn reflect(eye: Vec3, p: Vec3) -> (Vec3, f32) {
    let r = (p - eye).normalize();
    let fade = 1.0 - smoothstep(REFLECT_COS[0], REFLECT_COS[1], r.y.abs());
    (Vec3::new(r.x, -r.y, r.z), fade)
}

/// The distance along one of the room's axes from `l` to the wall ahead
/// of the direction `m`; none (far) along no axis. The WGSL's
/// `xr_exit_along`.
fn exit_along(l: f32, m: f32, half: f32) -> f32 {
    if m.abs() < 1e-6 {
        return 1e6;
    }
    (if m > 0.0 { half } else { -half } - l) / m
}

/// Where the ray from `p` along `m` leaves `room`: through a wall of its
/// rectangle or its ceiling, whichever comes first; `p` itself if it is
/// outside already. The WGSL's `xr_room_exit`.
pub fn exit(room: &Room, p: Vec3, m: Vec3) -> Vec3 {
    box_exit(room, p, m, 0.0)
}

/// Where the ray from `p` along `m` leaves `room`'s box grown by `grow`
/// (m) in plan and above: a far layer's shell (board #5438). No floor: a
/// ray going down leaves through a wall, under the floor, where the sky
/// has no bands. `p` itself if it is outside already. The WGSL's
/// `xr_box_exit`.
pub fn box_exit(room: &Room, p: Vec3, m: Vec3, grow: f32) -> Vec3 {
    let across = Vec2::new(-room.axis.y, room.axis.x);
    let l = Vec2::new(p.x, p.z) - room.center;
    let mp = Vec2::new(m.x, m.z);
    let ts = exit_along(l.dot(room.axis), mp.dot(room.axis), room.half.x + grow);
    let tt = exit_along(l.dot(across), mp.dot(across), room.half.y + grow);
    let ty = (room.ceiling_y + grow - p.y) / m.y.max(1e-6);
    p + m * ts.min(tt).min(ty).max(0.0)
}

/// Where the view ray from `eye` through the surface point `p` meets the
/// shell `grow` (m) behind `room`'s walls and ceiling.
pub fn shell_hit(room: &Room, eye: Vec3, p: Vec3, grow: f32) -> Vec3 {
    box_exit(room, p, (p - eye).normalize(), grow)
}

/// The rim fade at face point `uv` (m) on a face of half extents `half`,
/// with `fade` per edge (-u, +u, -v, +v; 1 fades, 0 runs to the edge): the
/// surfaces pass's 5 cm fade on the edges that keep it. The WGSL's
/// `xr_rim`.
pub fn rim(uv: Vec2, half: [f32; 2], fade: [f32; 4]) -> f32 {
    let f = Vec2::new(
        if uv.x > 0.0 { fade[1] } else { fade[0] },
        if uv.y > 0.0 { fade[3] } else { fade[2] },
    );
    let e = crate::surface_fx::STREAM_EDGE_M;
    let s = Vec2::new(
        smoothstep(0.0, e, half[0] - uv.x.abs()),
        smoothstep(0.0, e, half[1] - uv.y.abs()),
    );
    let r = Vec2::ONE + (s - Vec2::ONE) * f;
    r.x * r.y
}

/// What `fs_sky` does at one fragment for its near layer (the only one
/// per face and on the floor): the frame coordinates it calls the effect
/// at and how it weighs them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// The frame coordinate (px).
    pub at: Vec2,
    /// Near the seam on the sky, the other end's coordinate and its weight
    /// in the mix.
    pub other: Option<(Vec2, f32)>,
    /// What the light is scaled by: the peak alpha, the strength, the rim
    /// and, on the sky, the zenith fade; on the floor also the
    /// reflection's share, its fade toward the feet and across the seam.
    pub k: f32,
}

/// The room's box as the port block carries it (its floor is not).
fn room_of(rows: &[[f32; 4]; UNIFORM_ROWS]) -> Room {
    Room {
        center: Vec2::new(rows[ROOM_ROW][0], rows[ROOM_ROW][1]),
        axis: Vec2::new(rows[AXES_ROW][0], rows[AXES_ROW][1]),
        half: Vec2::new(rows[ROOM_ROW][2], rows[ROOM_ROW][3]),
        floor_y: 0.0,
        ceiling_y: rows[POINT_ROW][3],
        walls: 0,
    }
}

/// The light every layer starts from at face point `uv`: the peak alpha,
/// the strength and the rim.
fn base_k(rows: &[[f32; 4]; UNIFORM_ROWS], uv: Vec2) -> f32 {
    let half = [rows[4][0], rows[4][1]];
    rows[6][3] * rows[5][0].clamp(0.0, 1.0) * rim(uv, half, rows[EDGES_ROW])
}

/// The fragment of a face with port block `rows` and the effect's
/// `resolution` at face point `uv` (m), world point `world` (the lifted
/// quad's), seen from `eye`: what `fs_sky` calls the effect with for the
/// near layer. `None` where it does not call it for that layer (no light
/// left, or cut away at a joined edge).
pub fn sample(
    rows: &[[f32; 4]; UNIFORM_ROWS],
    resolution: [f32; 2],
    uv: Vec2,
    world: Vec3,
    eye: Vec3,
) -> Option<Sample> {
    let half = [rows[4][0], rows[4][1]];
    let mut k = base_k(rows, uv);
    let map = rows[MODE_ROW][1];
    if map < 0.5 {
        let n = uv / Vec2::from(half) * 0.5 + 0.5;
        return Some(Sample {
            at: Vec2::new(n.x, 1.0 - n.y) * Vec2::from(resolution),
            other: None,
            k,
        });
    }
    if !keeps(rows, uv, world, eye, world - eye) {
        return None;
    }
    let point = Vec3::from_slice(&rows[POINT_ROW][..3]);
    let facing = Vec2::new(rows[AXES_ROW][2], rows[AXES_ROW][3]);
    let mut d = world - point;
    let reflection = map > 1.5;
    if reflection {
        let (m, fade) = reflect(eye, world);
        d = exit(&room_of(rows), world, m) - point;
        k *= REFLECT_GAIN * fade;
    }
    let a = angles(d, facing);
    let w = seam_weight(a.x);
    k *= zenith_fade(a.y);
    if reflection {
        k *= 1.0 - 2.0 * w;
    }
    if k <= 0.0 {
        return None;
    }
    let other = (!reflection && w > 0.0).then(|| {
        let turned = Vec2::new(a.x - a.x.signum() * TAU, a.y);
        (frame(turned, resolution), w)
    });
    Some(Sample {
        at: frame(a, resolution),
        other,
        k,
    })
}

/// One of the sky's far layers at a fragment ([`far`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Far {
    /// The layer: 1 the middle, 2 the farthest.
    pub layer: usize,
    /// Where the view ray meets the layer's shell.
    pub hit: Vec3,
    /// The frame coordinate (px): the hit's direction from the sky's
    /// point, turned and lowered on the frame by the layer's
    /// [`LAYER_TURN_RAD`] and [`LAYER_DROP_RAD`].
    pub at: Vec2,
    /// What its light is scaled by: the near layer's peak alpha, strength
    /// and rim, its share of the light ([`LAYER_LIGHT`]), and its own
    /// fades: the zenith's, across the seam and under its bands. At 0
    /// `fs_sky` does not call the effect for it.
    pub k: f32,
}

/// The far layers of a sky fragment (mapped [`Map::Sky`]) with port block
/// `rows`, as [`sample`] takes it: one for each layer past the near one
/// that `rows` asks for (none per face, on the floor, with one layer or
/// where the fragment is cut away), nearest first. The WGSL's
/// `xr_sky_layer`.
pub fn far(
    rows: &[[f32; 4]; UNIFORM_ROWS],
    resolution: [f32; 2],
    uv: Vec2,
    world: Vec3,
    eye: Vec3,
) -> Vec<Far> {
    let mode = rows[MODE_ROW];
    if !(0.5..1.5).contains(&mode[1]) || !keeps(rows, uv, world, eye, world - eye) {
        return Vec::new();
    }
    let count = if mode[2] > 2.5 {
        3
    } else if mode[2] > 1.5 {
        2
    } else {
        1
    };
    let (room, k0) = (room_of(rows), base_k(rows, uv));
    let point = Vec3::from_slice(&rows[POINT_ROW][..3]);
    let facing = Vec2::new(rows[AXES_ROW][2], rows[AXES_ROW][3]);
    (1..count)
        .map(|layer| {
            let hit = shell_hit(&room, eye, world, SHELL_M[layer - 1] * mode[3]);
            let a = angles(hit - point, facing);
            let drop = LAYER_DROP_RAD[layer];
            let k = k0
                * LAYER_LIGHT[layer]
                * zenith_fade(a.y)
                * (1.0 - 2.0 * seam_weight(a.x))
                * smoothstep(BANDS_FLOOR_RAD[0], BANDS_FLOOR_RAD[1], a.y + drop);
            Far {
                layer,
                hit,
                at: frame(
                    Vec2::new(a.x + LAYER_TURN_RAD[layer], a.y + drop),
                    resolution,
                ),
                k,
            }
        })
        .collect()
}

/// A far `layer`'s light for the effect's output `c` at a scale of `k`:
/// `surface_port::light`, its color then tinted by [`LAYER_TINT`].
pub fn layer_light(c: [f32; 4], overlay: bool, k: f32, tint: [f32; 3], layer: usize) -> [f32; 4] {
    let l = crate::surface_port::light(c, overlay, k, tint);
    let t = LAYER_TINT[layer];
    [l[0] * t[0], l[1] * t[1], l[2] * t[2], l[3]]
}

/// The premultiplied `back` under `front`: what a farther layer adds to
/// what is drawn so far, as it would drawn first with the surfaces pass's
/// blending. The alpha stays within 1.
pub fn under(front: [f32; 4], back: [f32; 4]) -> [f32; 4] {
    let t = 1.0 - front[3];
    [0, 1, 2, 3].map(|i| front[i] + t * back[i])
}

/// `rows` (`surface_port::rows`) for a face mapped by `map` under `sky`,
/// its `edges` as [`join`] gives them (or the rim per edge, -u, +u, -v,
/// +v, true fading). A face mapped per face keeps the rows as they are
/// (every edge fading). The sky's faces carry its layers (`mode.z` the
/// count, `mode.w` the depth multiplier) and their cuts; the reflection is
/// one layer, never cut, and leaves them 0.
pub fn rows(
    mut rows: [[f32; 4]; UNIFORM_ROWS],
    map: Map,
    sky: &Sky,
    edges: impl Into<Edges>,
) -> [[f32; 4]; UNIFORM_ROWS] {
    if map == Map::Face {
        return rows;
    }
    let edges = edges.into();
    let (p, room) = (sky.point, sky.room);
    rows[MODE_ROW][1] = map.code();
    if map == Map::Sky {
        let count = sky.layers.count.clamp(1, LAYERS_MAX);
        rows[MODE_ROW][2] = count as f32;
        rows[MODE_ROW][3] = sky.layers.depth;
        rows[PLANE_ROW] = edges.plane;
        for (e, cut) in edges.cut.into_iter().enumerate() {
            rows[CUT_ROW + e] = cut.unwrap_or([0.0; 4]);
        }
        rows[CUT_FROM_ROW] = edges.cut_from;
    } else {
        rows[CUT_FROM_ROW] = [NO_CUT; 4];
    }
    rows[EDGES_ROW] = edges.fade.map(|f| f32::from(u8::from(f)));
    rows[POINT_ROW] = [p.x, p.y, p.z, room.ceiling_y];
    rows[ROOM_ROW] = [room.center.x, room.center.y, room.half.x, room.half.y];
    rows[AXES_ROW] = [room.axis.x, room.axis.y, sky.facing.x, sky.facing.y];
    rows
}

/// How a sky face's edges end, per edge (-u, +u, -v, +v) ([`join`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edges {
    /// Where the rim fades: no sky face continues the edge.
    pub fade: [bool; 4],
    /// The face's drawn plane, `[n.x, n.y, n.z, k]` with `n . p = k` on
    /// it: the same floats as its neighbors carry for it.
    pub plane: [f32; 4],
    /// For an edge joined to a face across a corner, that face's drawn
    /// plane, as its own `plane`.
    pub cut: [Option<[f32; 4]>; 4],
    /// From how far toward each joined edge (m, the face's u or v from its
    /// center) the join decides, so a neighbor's plane cuts nothing far
    /// from the edge it joins; [`NO_CUT`] on the others.
    pub cut_from: [f32; 4],
}

impl Edges {
    /// Every edge fading, no join: a face nothing continues.
    pub const OPEN: Self = Self {
        fade: [true; 4],
        plane: [0.0; 4],
        cut: [None; 4],
        cut_from: [NO_CUT; 4],
    };
}

impl From<[bool; 4]> for Edges {
    /// The rim per edge, no join.
    fn from(fade: [bool; 4]) -> Self {
        Self { fade, ..Self::OPEN }
    }
}

/// Whether plane `p` comes before plane `q`: the order both faces at a
/// join take their two planes in. The WGSL's `xr_before`.
fn before(p: [f32; 4], q: [f32; 4]) -> bool {
    for (a, b) in p.iter().zip(&q) {
        match a.partial_cmp(b) {
            Some(std::cmp::Ordering::Less) => return true,
            Some(std::cmp::Ordering::Greater) => return false,
            _ => {}
        }
    }
    false
}

/// Along the ray from `eye` along `d`, how far (in lengths of `d`) to the
/// plane `p` (`n . x = k`).
fn plane_t(p: [f32; 4], eye: Vec3, d: Vec3) -> f32 {
    let n = Vec3::new(p[0], p[1], p[2]);
    (p[3] - n.dot(eye)) / n.dot(d)
}

/// Whether the face with port block `rows` keeps its fragment at face
/// point `uv` (m), seen from `eye` along the view ray `d` (either way
/// along it; `world` is the fragment's point, which orients it). At each
/// joined edge ([`Edges`]), from its `cut_from` on, the face keeps the
/// fragment only if the ray meets its own drawn plane before the
/// neighbor's, ahead of the eye: of the two faces' quads, which both cover
/// the pixel there, exactly one keeps it. Both compute the same two
/// distances from the same inputs (the planes in [`before`]'s order), so
/// a tie goes the same way for both. The WGSL's `xr_keeps`, which takes
/// `d` through the pixel's center.
pub fn keeps(rows: &[[f32; 4]; UNIFORM_ROWS], uv: Vec2, world: Vec3, eye: Vec3, d: Vec3) -> bool {
    let d = d * d.dot(world - eye).signum();
    let own = rows[PLANE_ROW];
    let toward = [-uv.x, uv.x, -uv.y, uv.y];
    (0..4).all(|e| {
        let q = rows[CUT_ROW + e];
        if toward[e] < rows[CUT_FROM_ROW][e] || q[..3].iter().all(|&x| x == 0.0) {
            return true;
        }
        let own_first = before(own, q);
        let (p1, p2) = if own_first { (own, q) } else { (q, own) };
        let (t1, t2) = (plane_t(p1, eye, d), plane_t(p2, eye, d));
        let first_wins = t1 > 0.0 && !(t2 > 0.0 && t2 < t1);
        own_first == first_wins
    })
}

/// `faces` (as drawn, `surface_port::port_face`) joined as one sky. An edge
/// another of them continues (its middle within [`NEIGHBOR_M`] of another
/// face: two walls in a corner, a wall's top under the ceiling) runs to
/// that neighbor without the rim fade. Across a corner it runs on
/// [`OVERLAP_M`] past the neighbor's drawn plane at both its ends, however
/// the two planes lean, and carries that plane ([`Edges::cut`]); the
/// fragment decides which of the two keeps each pixel ([`keeps`]). A
/// neighbor beside it in the same plane is joined without that. Every
/// other edge keeps the face's extent and the fade. Each face as drawn,
/// with its edges.
pub fn join(faces: &[Face]) -> Vec<(Face, Edges)> {
    // Each face's drawn plane, once: what the face carries for itself and
    // its neighbors for it are the same floats.
    let planes: Vec<[f32; 4]> = faces
        .iter()
        .map(|f| {
            let n = f.normal;
            [n.x, n.y, n.z, n.dot(f.center + n * LIFT_M)]
        })
        .collect();
    faces
        .iter()
        .enumerate()
        .map(|(i, f)| {
            // The quad's reach from its center along each axis, minus and
            // plus.
            let mut reach = [[f.half[0]; 2], [f.half[1]; 2]];
            let mut edges = Edges {
                plane: planes[i],
                ..Edges::OPEN
            };
            // From where along each joined edge's axis the join decides,
            // in the face's own frame before it moves.
            let mut from = [None::<f32>; 4];
            let lifted = f.center + f.normal * LIFT_M;
            for e in 0..4 {
                let (axis, side) = (e / 2, e % 2);
                let sign = if side == 1 { 1.0 } else { -1.0 };
                let dir = f.axes[axis] * sign;
                let mid = f.center + dir * f.half[axis];
                let neighbor = faces
                    .iter()
                    .enumerate()
                    .filter(|&(j, _)| j != i)
                    .map(|(j, g)| (j, g.nearest(mid).distance(mid)))
                    .filter(|&(_, d)| d <= NEIGHBOR_M)
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                let Some((j, _)) = neighbor else {
                    continue;
                };
                edges.fade[e] = false;
                // Only a neighbor across the edge (a corner) is joined
                // through the fragment, not one beside it in the same
                // plane.
                let g = &faces[j];
                let across = g.normal.dot(dir);
                let k = g.normal.dot(g.center + g.normal * LIFT_M - lifted);
                let t = k / across;
                if across.abs() <= 0.5 || t <= 0.0 || t >= f.half[axis] + NEIGHBOR_M {
                    continue;
                }
                // Where the neighbor's plane crosses this face, as a line
                // a u + b v + c = 0 in its frame, and how far toward the
                // edge it runs at either end of the edge (and a little
                // past them, for the other axis' own reach): the line is
                // straight, so its ends bound it.
                let line = [g.normal.dot(f.axes[0]), g.normal.dot(f.axes[1]), -k];
                let (along, other) = (line[axis], line[1 - axis]);
                let ends = f.half[1 - axis] + NEIGHBOR_M + OVERLAP_M;
                let at = |y: f32| -sign * (line[2] + other * y) / along;
                let (lo, hi) = (at(-ends).min(at(ends)), at(-ends).max(at(ends)));
                reach[axis][side] = hi + OVERLAP_M;
                from[e] = Some(lo - OVERLAP_M);
                edges.cut[e] = Some(planes[j]);
            }
            let mut out = *f;
            let mut shift = [0.0; 2];
            for (axis, [minus, plus]) in reach.into_iter().enumerate() {
                shift[axis] = 0.5 * (plus - minus);
                out.center += f.axes[axis] * shift[axis];
                out.half[axis] = 0.5 * (plus + minus);
            }
            // From where, in the drawn face's frame.
            for (e, from) in from.into_iter().enumerate() {
                if let Some(from) = from {
                    let sign = if e % 2 == 1 { 1.0 } else { -1.0 };
                    edges.cut_from[e] = from - sign * shift[e / 2];
                }
            }
            (out, edges)
        })
        .collect()
}

/// How much of the sky's turn a wall's footprint `(a, b)` covers seen
/// from `point` (degrees of azimuth).
pub fn span_deg(point: Vec3, (a, b): (Vec2, Vec2)) -> f32 {
    let p = Vec2::new(point.x, point.z);
    (a - p).angle_to(b - p).abs().to_degrees()
}

/// The log's line for the sky `on` or off: the point and what it came
/// from, the seam's direction and `why` it is there, and what the hidden
/// walls leave undrawn (`hidden_deg` of the turn).
pub fn line(on: bool, sky: &Sky, why: &str, hidden_deg: f32) -> String {
    // Adding 0 turns a -0 into 0, so the log never reads "-0.00".
    let (p, r, s) = (sky.point + 0.0, sky.room, sky.seam() + 0.0);
    let from = if r.walls > 0 {
        format!(
            "the center of the scanned room's {} walls in plan ({:.2} x {:.2} m, floor {:.2}, ceiling {:.2})",
            r.walls,
            2.0 * r.half.x,
            2.0 * r.half.y,
            r.floor_y,
            r.ceiling_y
        )
    } else {
        format!(
            "no scanned walls: the stage floor's center, the room a {:.1} m square",
            2.0 * STAGE_HALF_M
        )
    };
    format!(
        "sky: {} (debug.fosfora.sky) · point ({:.2}, {:.2}, {:.2}), {from}, {SEATED_EYE_M:.2} m over the floor · seam toward ({:.2}, {:.2}), behind the facing at {why} · hidden walls leave {hidden_deg:.0}° of the turn undrawn",
        if on { "on" } else { "off, Aurora per face" },
        p.x,
        p.y,
        p.z,
        s.x,
        s.y,
    )
}

/// What [`line`] says, to the centimeter and the degree it prints: the
/// app logs the line again when this changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Logged {
    on: bool,
    point_cm: [i32; 3],
    facing: [i32; 2],
    hidden_deg: i32,
    why: &'static str,
}

impl Logged {
    pub fn of(on: bool, sky: &Sky, why: &'static str, hidden_deg: f32) -> Self {
        let cm = |v: f32| (v * 100.0).round() as i32;
        Self {
            on,
            point_cm: sky.point.to_array().map(cm),
            facing: sky.facing.to_array().map(cm),
            hidden_deg: hidden_deg.round() as i32,
            why,
        }
    }
}

/// Appended after `surface_port::WRAPPER_WGSL` for a port that draws the
/// sky (`surface_port::Port::sky`): a vertex stage that also hands on the
/// world point and the eye, and `fs_sky`, which maps by `xr_port.mode.y`
/// (0 per face as `fs_main` does, 1 the sky, 2 its reflection) and on the
/// sky draws `xr_port.mode.z` layers. The Rust copies are [`angles`],
/// [`frame`], [`seam_weight`], [`zenith_fade`], [`reflect`],
/// [`box_exit`], [`rim`], [`keeps`], [`sample`] (the near layer), [`far`]
/// (the far ones), [`layer_light`] and [`under`].
pub const SKY_WGSL: &str = r"
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
// The layers (board #5438), the nearest first: the far shells' depths
// behind the walls and the ceiling (m) at a depth multiplier of 1; each
// layer's share of the light, its turn around the frame (rad of azimuth)
// and how much lower its bands sit (rad of elevation); the elevations on
// a far layer's frame where it is gone and whole under its lowest band;
// the far two's tints.
const XR_SKY_SHELL: vec2<f32> = vec2<f32>(1.5, 4.0);
const XR_SKY_LIGHT: vec3<f32> = vec3<f32>(1.0, 0.6, 0.35);
const XR_SKY_TURN: vec3<f32> = vec3<f32>(0.0, 2.1, 3.2);
const XR_SKY_DROP: vec3<f32> = vec3<f32>(0.0, 0.09821429, 0.04910714);
const XR_SKY_FLOOR: vec2<f32> = vec2<f32>(-0.44, -0.34);
const XR_SKY_TINT_1: vec3<f32> = vec3<f32>(0.8, 0.9, 1.0);
const XR_SKY_TINT_2: vec3<f32> = vec3<f32>(0.6, 0.78, 1.0);

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

// Whether plane p comes before plane q: the order both faces at a join
// take their two planes in.
fn xr_before(p: vec4<f32>, q: vec4<f32>) -> bool {
    if p.x != q.x {
        return p.x < q.x;
    }
    if p.y != q.y {
        return p.y < q.y;
    }
    if p.z != q.z {
        return p.z < q.z;
    }
    return p.w < q.w;
}

// The view ray through the pixel's center (its direction, either way
// along it): solved from the camera at the pixel's place on the target,
// the same in every draw that covers the pixel.
fn xr_pixel_ray(frag: vec2<f32>) -> vec3<f32> {
    let ndc = vec2<f32>(frag.x / xr_eye.extent.x * 2.0 - 1.0, 1.0 - frag.y / xr_eye.extent.y * 2.0);
    let m = xr_eye.view_proj;
    let a = vec3<f32>(m[0].x, m[1].x, m[2].x);
    let b = vec3<f32>(m[0].y, m[1].y, m[2].y);
    let c = vec3<f32>(m[0].w, m[1].w, m[2].w);
    return cross(a - ndc.x * c, b - ndc.y * c);
}

// Whether this face keeps its fragment at face point uv, the view ray
// from the eye along d (pointing past the fragment): at each joined edge,
// from its cut_from on, only if the ray meets this face's drawn plane
// before the neighbor's, ahead of the eye. Both faces at a join compute
// the same two distances from the same inputs, so exactly one keeps it.
fn xr_keeps(uv: vec2<f32>, eye: vec3<f32>, d: vec3<f32>) -> bool {
    let own = xr_port.plane;
    var toward = array<f32, 4>(-uv.x, uv.x, -uv.y, uv.y);
    for (var e = 0u; e < 4u; e++) {
        let q = xr_port.cut[e];
        if toward[e] < xr_port.cut_from[e] || all(q.xyz == vec3<f32>(0.0)) {
            continue;
        }
        let own_first = xr_before(own, q);
        let p1 = select(q, own, own_first);
        let p2 = select(own, q, own_first);
        let t1 = (p1.w - dot(p1.xyz, eye)) / dot(p1.xyz, d);
        let t2 = (p2.w - dot(p2.xyz, eye)) / dot(p2.xyz, d);
        let first_wins = t1 > 0.0 && !(t2 > 0.0 && t2 < t1);
        if own_first != first_wins {
            return false;
        }
    }
    return true;
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

// Where the ray from p along m leaves the room's box grown by g (m) in
// plan and above: a wall of its rectangle or its ceiling, whichever
// first. No floor: a ray going down leaves through a wall.
fn xr_box_exit(p: vec3<f32>, m: vec3<f32>, g: f32) -> vec3<f32> {
    let axis = xr_port.axes.xy;
    let across = vec2<f32>(-axis.y, axis.x);
    let l = p.xz - xr_port.room.xy;
    let ts = xr_exit_along(dot(l, axis), dot(m.xz, axis), xr_port.room.z + g);
    let tt = xr_exit_along(dot(l, across), dot(m.xz, across), xr_port.room.w + g);
    let ty = (xr_port.sky.w + g - p.y) / max(m.y, 1e-6);
    return p + m * max(min(min(ts, tt), ty), 0.0);
}

// Where the ray from p along m leaves the room itself.
fn xr_room_exit(p: vec3<f32>, m: vec3<f32>) -> vec3<f32> {
    return xr_box_exit(p, m, 0.0);
}

// A far layer at the world point seen from the eye: the view ray on past
// the surface to the layer's shell (the room's box grown by depth), the
// sky there by that point's direction from the sky's point, turned and
// lowered on the frame. Its light is k0 (the near layer's before its
// fades) times its share, faded by its own zenith, across the seam and
// under its bands, and tinted; where none is left, nothing, and the
// effect is not called.
fn xr_sky_layer(
    world: vec3<f32>,
    eye: vec3<f32>,
    k0: f32,
    depth: f32,
    light: f32,
    turn: f32,
    drop: f32,
    tint: vec3<f32>,
) -> vec4<f32> {
    let hit = xr_box_exit(world, normalize(world - eye), depth);
    let a = xr_sky_angles(hit - xr_port.sky.xyz);
    let w = 0.5 * smoothstep(XR_PI - XR_SKY_SEAM, XR_PI, abs(a.x));
    let k = k0 * light * (1.0 - smoothstep(XR_SKY_ZENITH.x, XR_SKY_ZENITH.y, a.y))
        * (1.0 - 2.0 * w) * smoothstep(XR_SKY_FLOOR.x, XR_SKY_FLOOR.y, a.y + drop);
    if k <= 0.0 {
        return vec4<f32>(0.0);
    }
    let c = effect_main(xr_sky_frame(vec2<f32>(a.x + turn, a.y + drop)));
    return xr_light(c, k) * vec4<f32>(tint, 1.0);
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
    // Where a joined neighbor's plane is met first, it draws: nothing
    // (sky.rs, join).
    let ray = xr_pixel_ray(in.pos.xy);
    if !xr_keeps(in.uv, in.eye, ray * sign(dot(ray, in.world - in.eye))) {
        return vec4<f32>(0.0);
    }
    // The far layers start from the light before the near layer's fades.
    let k0 = k;
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
    // The near layer, on the surface; nothing where no light is left.
    var out = vec4<f32>(0.0);
    if k > 0.0 {
        var c = effect_main(xr_sky_frame(a));
        if !reflection && w > 0.0 {
            // Near the seam the other end's continuation, cross-faded.
            let other = effect_main(xr_sky_frame(vec2<f32>(a.x - sign(a.x) * XR_TAU, a.y)));
            c = mix(c, other, w);
        }
        out = xr_light(c, k);
    }
    // The far layers on the sky (the reflection is one layer), each under
    // what is drawn so far.
    let layers = select(1.0, xr_port.mode.z, !reflection);
    if layers > 1.5 {
        let far = xr_sky_layer(
            in.world, in.eye, k0, XR_SKY_SHELL.x * xr_port.mode.w,
            XR_SKY_LIGHT.y, XR_SKY_TURN.y, XR_SKY_DROP.y, XR_SKY_TINT_1,
        );
        out += (1.0 - out.a) * far;
    }
    if layers > 2.5 {
        let far = xr_sky_layer(
            in.world, in.eye, k0, XR_SKY_SHELL.y * xr_port.mode.w,
            XR_SKY_LIGHT.z, XR_SKY_TURN.z, XR_SKY_DROP.z, XR_SKY_TINT_2,
        );
        out += (1.0 - out.a) * far;
    }
    return out;
}
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface_port::{FIRST_ID, PEAK_ALPHA, port_face};
    use crate::surfaces::{
        KIND_FRAME, KIND_OTHER, KIND_TABLE, PLANE_HALF_THICKNESS_M, acting_face,
    };

    /// A box room as a scan reports it: `w` by `d` m in plan (`w` along
    /// the room's axis), turned `yaw` about up, centered on `center` (x,
    /// z), the floor at y = 0 and the ceiling at `h`; every plane 2 cm
    /// thick, its local +Z into the room. Walls first (the axis' far end,
    /// near end, then across), then the floor and the ceiling.
    fn box_room(center: Vec2, yaw: f32, w: f32, d: f32, h: f32) -> Vec<(u32, Vec3, Quat, Vec3)> {
        let r = Quat::from_rotation_y(yaw);
        let c = Vec3::new(center.x, 0.0, center.y);
        let t = PLANE_HALF_THICKNESS_M;
        let wall = |out: Vec3, turn: f32, len: f32| {
            (
                KIND_WALL,
                c + r * out + Vec3::Y * 0.5 * h,
                r * Quat::from_rotation_y(turn),
                Vec3::new(0.5 * len, 0.5 * h, t),
            )
        };
        let level = |y: f32, down: bool| {
            let tilt = if down {
                std::f32::consts::FRAC_PI_2
            } else {
                -std::f32::consts::FRAC_PI_2
            };
            (
                if down { KIND_CEILING } else { KIND_FLOOR },
                c + Vec3::Y * y,
                r * Quat::from_rotation_x(tilt),
                Vec3::new(0.5 * w, 0.5 * d, t),
            )
        };
        let half_pi = std::f32::consts::FRAC_PI_2;
        vec![
            wall(Vec3::X * 0.5 * w, -half_pi, d),
            wall(Vec3::NEG_X * 0.5 * w, half_pi, d),
            wall(Vec3::Z * 0.5 * d, PI, w),
            wall(Vec3::NEG_Z * 0.5 * d, 0.0, w),
            level(0.0, false),
            level(h, true),
        ]
    }

    /// The faces a room's walls and ceiling are drawn on, seen from `head`.
    fn sky_faces(boxes: &[(u32, Vec3, Quat, Vec3)], head: Vec3) -> Vec<Face> {
        boxes
            .iter()
            .filter(|b| matches!(b.0, KIND_WALL | KIND_CEILING))
            .map(|&(_, c, r, h)| port_face(acting_face(c, r, h, head)))
            .collect()
    }

    /// The point on `face`'s lifted quad at face point `uv` (m).
    fn on(face: &Face, uv: Vec2) -> Vec3 {
        face.center + face.normal * LIFT_M + face.axes[0] * uv.x + face.axes[1] * uv.y
    }

    /// `face`'s face point (m) of a world point on its lifted quad.
    fn uv_of(face: &Face, p: Vec3) -> Vec2 {
        let d = p - face.center;
        Vec2::new(d.dot(face.axes[0]), d.dot(face.axes[1]))
    }

    #[test]
    fn the_point_is_the_walls_center_in_plan_at_a_seated_eye_height() {
        // A 5 by 4 m room turned 30°, its center at (1, -2), 2.5 m high.
        let center = Vec2::new(1.0, -2.0);
        let boxes = box_room(center, 0.52, 5.0, 4.0, 2.5);
        let room = Room::fit(boxes.iter().copied()).expect("walls");
        assert!(room.center.distance(center) < 1e-4, "{room:?}");
        assert_eq!(room.walls, 4);
        // Turned to the longest wall: 2.5 m along it, 2 m across.
        assert!(
            (room.axis.dot(Vec2::new(0.52f32.cos(), -0.52f32.sin()))).abs() > 0.9999,
            "{room:?}"
        );
        assert_close!(room.half.to_array(), [2.5, 2.0]);
        // The scanned floor's top and the ceiling's underside.
        let t = PLANE_HALF_THICKNESS_M;
        assert_close!([room.floor_y, room.ceiling_y], [t, 2.5 - t]);
        let p = room.point();
        assert_close!(p.to_array(), [center.x, t + SEATED_EYE_M, center.y]);
        // Without the floor and the ceiling: the walls' foot and top.
        let walls = Room::fit(boxes.iter().copied().filter(|b| b.0 == KIND_WALL)).unwrap();
        assert_close!([walls.floor_y, walls.ceiling_y], [0.0, 2.5]);
        assert!(walls.center.distance(center) < 1e-4);
        // A low ceiling keeps the point under it.
        let low = Room::fit(box_room(center, 0.52, 5.0, 4.0, 1.4)).unwrap();
        assert_close!(low.point().y, 1.4 - t - HEADROOM_M);
        // Other boxes do not move it: a table in a corner, a frame.
        let mut more = boxes.clone();
        more.push((
            KIND_TABLE,
            Vec3::new(3.0, 0.4, 0.0),
            Quat::IDENTITY,
            Vec3::splat(0.4),
        ));
        more.push((
            KIND_FRAME,
            Vec3::new(-1.0, 1.0, -4.0),
            Quat::IDENTITY,
            Vec3::ONE,
        ));
        more.push((
            KIND_OTHER,
            Vec3::new(9.0, 0.5, 9.0),
            Quat::IDENTITY,
            Vec3::ONE,
        ));
        assert_eq!(Room::fit(more), Some(room));
        // No wall, no room: the stage stands in.
        assert_eq!(
            Room::fit(boxes.iter().copied().filter(|b| b.0 != KIND_WALL)),
            None
        );
        let stage = Room::stage(Vec2::new(0.5, 0.25), 0.0);
        assert_close!(stage.point().to_array(), [0.5, SEATED_EYE_M, 0.25]);
        assert_close!(stage.half.to_array(), [STAGE_HALF_M; 2]);
        assert_eq!(stage.walls, 0);
    }

    #[test]
    fn an_l_shaped_room_takes_its_bounding_rectangle() {
        // Six walls along x = 0..4, z = 0..3, cut at x > 2, z > 1.5.
        let segs = [
            ((0.0, 0.0), (4.0, 0.0)),
            ((4.0, 0.0), (4.0, 1.5)),
            ((4.0, 1.5), (2.0, 1.5)),
            ((2.0, 1.5), (2.0, 3.0)),
            ((2.0, 3.0), (0.0, 3.0)),
            ((0.0, 3.0), (0.0, 0.0)),
        ];
        let boxes = segs.map(|((ax, az), (bx, bz))| {
            let (a, b) = (Vec3::new(ax, 1.25, az), Vec3::new(bx, 1.25, bz));
            let along = (b - a).normalize();
            (
                KIND_WALL,
                0.5 * (a + b),
                Quat::from_rotation_arc(Vec3::X, along),
                Vec3::new(0.5 * a.distance(b), 1.25, PLANE_HALF_THICKNESS_M),
            )
        });
        let room = Room::fit(boxes).unwrap();
        assert!(room.center.distance(Vec2::new(2.0, 1.5)) < 1e-4, "{room:?}");
        assert_close!(room.half.to_array(), [2.0, 1.5]);
        // Each wall's footprint is its center line.
        let (a, b) = wall_segment(boxes[1].1, boxes[1].2, boxes[1].3);
        assert!(a.distance(Vec2::new(4.0, 0.0)) < 1e-5 || b.distance(Vec2::new(4.0, 0.0)) < 1e-5);
        // A 4 m wall 2 m from the point spans a quarter of the turn.
        let wall = (Vec2::new(-2.0, -2.0), Vec2::new(2.0, -2.0));
        assert!((span_deg(Vec3::new(0.0, 1.2, 0.0), wall) - 90.0).abs() < 1e-3);
    }

    #[test]
    fn the_facing_is_the_heads_view_leveled() {
        let yaw = |a: f32| Quat::from_rotation_y(a);
        assert!(facing(Quat::IDENTITY).distance(Vec2::new(0.0, -1.0)) < 1e-6);
        // Turned right (clockwise from above) by 90°: facing +X.
        assert!(facing(yaw(-FRAC_PI_2)).distance(Vec2::X) < 1e-6);
        // Pitched down 40° and rolled: still the level view.
        let r = yaw(0.3) * Quat::from_rotation_x(-0.7) * Quat::from_rotation_z(0.2);
        let view = r * Vec3::NEG_Z;
        let want = Vec2::new(view.x, view.z).normalize();
        assert!(facing(r).distance(want) < 1e-5);
        // Straight down: the way the top of the head points.
        let down = yaw(0.3) * Quat::from_rotation_x(-FRAC_PI_2);
        let ahead = yaw(0.3) * Vec3::NEG_Z;
        assert!(facing(down).distance(Vec2::new(ahead.x, ahead.z)) < 1e-4);
        let up = yaw(0.3) * Quat::from_rotation_x(FRAC_PI_2);
        assert!(facing(up).distance(Vec2::new(ahead.x, ahead.z)) < 1e-4);
    }

    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn the_frame_runs_a_turn_around_from_the_seam_behind() {
        let res = RESOLUTION;
        // A degree across is a degree up.
        assert_close!(res[0] / res[1], TAU / HEIGHT_RAD);
        let facing = Vec2::new(0.0, -1.0);
        let at = |d: Vec3| frame(angles(d, facing), res);
        // Ahead at the horizon: the middle column, the bottom row.
        assert!(at(Vec3::NEG_Z).distance(Vec2::new(0.5 * res[0], res[1])) < 1e-2);
        // A quarter right: three quarters across; a quarter left: one.
        assert!((at(Vec3::X).x - 0.75 * res[0]).abs() < 1e-2);
        assert!((at(Vec3::NEG_X).x - 0.25 * res[0]).abs() < 1e-2);
        // Behind, a hair either side of the seam: the two ends.
        assert!(at(Vec3::new(1e-4, 0.0, 1.0)).x > res[0] - 1.0);
        assert!(at(Vec3::new(-1e-4, 0.0, 1.0)).x < 1.0);
        // Up the frame with elevation: its top row HEIGHT_RAD up.
        let up = Vec3::new(0.0, HEIGHT_RAD.tan(), -1.0);
        assert!(at(up).y.abs() < 1e-2);
        // Another facing turns the seam with it: facing +X, the seam is -X.
        let s = Sky::new(Room::stage(Vec2::ZERO, 0.0), Vec2::X);
        assert_close!(s.seam().to_array(), [-1.0, 0.0]);
        assert!((frame(angles(Vec3::X, s.facing), res).x - 0.5 * res[0]).abs() < 1e-2);
        // Facing +X the right is +Z: a hair to it behind is the far end.
        assert!(frame(angles(Vec3::new(-1.0, 0.0, 1e-4), s.facing), res).x > res[0] - 1.0);
        assert!(frame(angles(Vec3::new(-1.0, 0.0, -1e-4), s.facing), res).x < 1.0);
    }

    #[test]
    fn across_a_corner_and_onto_the_ceiling_the_sky_moves_less_than_a_pixel() {
        // A 4.6 by 3.8 m room turned 20°, the wearer off its center.
        let boxes = box_room(Vec2::new(0.4, -0.3), 0.35, 4.6, 3.8, 2.45);
        let room = Room::fit(boxes.iter().copied()).unwrap();
        let head = Vec3::new(0.9, 1.15, 0.2);
        let sky = Sky::new(room, facing(Quat::from_rotation_y(0.35)));
        let faces = sky_faces(&boxes, head);
        assert_eq!(faces.len(), 5);
        let at = |p: Vec3| frame(angles(p - sky.point, sky.facing), RESOLUTION);
        let pairs = across_edges(&faces, sky.point);
        for &((i, qa), (j, qb)) in &pairs {
            let (a, b) = (&faces[i], &faces[j]);
            let gap = at(qa).distance(at(qb));
            assert!(gap < 1.0, "{gap} px across {a:?} / {b:?}");
            // Per face, two walls' points land at the frame's two ends.
            if a.normal.y.abs() < 0.5 && b.normal.y.abs() < 0.5 {
                let per_face = |f: &Face, p: Vec3| {
                    let res = crate::surface_port::resolution(f.half);
                    crate::surface_port::frame_coord(uv_of(f, p), f.half, res) / Vec2::from(res)
                };
                let jump = (per_face(a, qa).x - per_face(b, qb).x).abs();
                assert!(jump > 0.9, "{jump} of the frame");
            }
        }
        // Four corners and four wall-to-ceiling edges, sampled.
        assert!(pairs.len() >= 8 * 3, "{}", pairs.len());
    }

    /// Every pair of `faces` that meet: points along the line where their
    /// lifted quads cross (around `middle`), and for each a point 0.5 mm
    /// into either face from it (0.7 mm apart across a right angle), both
    /// on their quads: (the face's index, the point) for each side.
    fn across_edges(faces: &[Face], middle: Vec3) -> Vec<((usize, Vec3), (usize, Vec3))> {
        let mut pairs = Vec::new();
        for (i, a) in faces.iter().enumerate() {
            for (j, b) in faces.iter().enumerate().skip(i + 1) {
                let along = a.normal.cross(b.normal);
                if along.length() < 0.5 {
                    continue;
                }
                let along = along.normalize();
                // A point on both lifted planes, near the room's middle.
                let (pa, pb) = (a.center + a.normal * LIFT_M, b.center + b.normal * LIFT_M);
                let m = Mat3::from_cols(a.normal, b.normal, along).transpose();
                let x =
                    m.inverse() * Vec3::new(a.normal.dot(pa), b.normal.dot(pb), along.dot(middle));
                for s in [-0.6, -0.2, 0.0, 0.3, 0.7] {
                    let q = x + along * s;
                    // Into each face: away from the other's plane, along it.
                    let into_a = (b.normal - a.normal * a.normal.dot(b.normal)).normalize();
                    let into_b = (a.normal - b.normal * b.normal.dot(a.normal)).normalize();
                    let (qa, qb) = (q + into_a * 5e-4, q + into_b * 5e-4);
                    // Both on their quads, clear of the far ends.
                    let inside = |f: &Face, p: Vec3| {
                        let uv = uv_of(f, p);
                        uv.x.abs() < f.half[0] + 0.03 && uv.y.abs() < f.half[1] + 0.03
                    };
                    if inside(a, qa) && inside(b, qb) {
                        pairs.push(((i, qa), (j, qb)));
                    }
                }
            }
        }
        pairs
    }

    use glam::Mat3;

    #[test]
    fn the_seam_cross_fades_the_two_ends_and_the_reflection_fades_across_it() {
        assert_close!(seam_weight(PI), 0.5);
        assert_close!(seam_weight(-PI), 0.5);
        assert_close!(seam_weight(PI - SEAM_RAD), 0.0);
        assert_close!(seam_weight(0.0), 0.0);
        // The back wall of a 4 m room, straight behind the point: a probe
        // that is the frame's x, mixed as fs_sky mixes it, runs on across
        // the seam, two points 1 mm apart.
        let boxes = box_room(Vec2::ZERO, 0.0, 4.0, 4.0, 2.5);
        let room = Room::fit(boxes.iter().copied()).unwrap();
        let sky = Sky::new(room, Vec2::new(0.0, -1.0));
        let faces = sky_faces(&boxes, sky.point);
        let back = *faces
            .iter()
            .find(|f| f.normal.z < -0.9)
            .expect("the back wall");
        let r = rows(
            crate::surface_port::rows(&back, 1.0, false, FIRST_ID, [1.0; 3]),
            Map::Sky,
            &sky,
            [true; 4],
        );
        let probe = |s: Sample| match s.other {
            Some((o, w)) => s.at.x * (1.0 - w) + o.x * w,
            None => s.at.x,
        };
        let eye = sky.point;
        for y in [-0.4, 0.0, 0.6] {
            let p = |x: f32| {
                on(
                    &back,
                    Vec2::new(x, y) + uv_of(&back, Vec3::new(0.0, 1.2, 2.0)) * Vec2::Y,
                )
            };
            let (a, b) = (
                sample(&r, RESOLUTION, uv_of(&back, p(-5e-4)), p(-5e-4), eye).unwrap(),
                sample(&r, RESOLUTION, uv_of(&back, p(5e-4)), p(5e-4), eye).unwrap(),
            );
            // Both sides mix the two ends: one at each end of the frame.
            let ((oa, wa), (ob, wb)) = (a.other.unwrap(), b.other.unwrap());
            assert!((wa - 0.5).abs() < 1e-3 && (wb - 0.5).abs() < 1e-3);
            assert!((a.at.x - ob.x).abs() < 1.0 && (b.at.x - oa.x).abs() < 1.0);
            assert!((probe(a) - probe(b)).abs() < 1.0, "{a:?} {b:?}");
            // And it is the turn's middle: half of each end.
            assert!((probe(a) - 0.5 * RESOLUTION[0]).abs() < 2.0);
        }
        // Ahead, nothing to mix.
        let front = *faces.iter().find(|f| f.normal.z > 0.9).unwrap();
        let rf = rows(
            crate::surface_port::rows(&front, 1.0, false, FIRST_ID, [1.0; 3]),
            Map::Sky,
            &sky,
            [true; 4],
        );
        let s = sample(&rf, RESOLUTION, Vec2::ZERO, on(&front, Vec2::ZERO), eye).unwrap();
        assert_eq!(s.other, None);
        // The reflection under the seam: nothing on it, one call either
        // side.
        let floor = port_face(acting_face(boxes[4].1, boxes[4].2, boxes[4].3, eye));
        let rr = rows(
            crate::surface_port::rows(&floor, 1.0, false, FIRST_ID, [1.0; 3]),
            Map::Reflection,
            &sky,
            [true; 4],
        );
        let behind = |x: f32| on(&floor, uv_of(&floor, Vec3::new(x, 0.0, 1.6)));
        let eye = Vec3::new(0.0, 1.2, 0.0);
        assert_eq!(
            sample(
                &rr,
                RESOLUTION,
                uv_of(&floor, behind(0.0)),
                behind(0.0),
                eye
            ),
            None
        );
        let near = sample(
            &rr,
            RESOLUTION,
            uv_of(&floor, behind(0.2)),
            behind(0.2),
            eye,
        )
        .unwrap();
        assert_eq!(near.other, None);
    }

    #[test]
    fn the_bands_rise_from_the_horizon_and_stop_short_of_the_zenith() {
        // aurora.wgsl at the .pfx's band spread (0.5 * 1.5 + 0.5): band i
        // centered at (i - 3) / 7 * 1.25 of the frame's height below its
        // middle; the waves and the ripple move it 0.12 at most.
        let spread = 1.25;
        let el = |i: f32| HORIZON_RAD + HEIGHT_RAD * (0.5 - (i - 3.0) / 7.0 * spread);
        let reach = 0.12 * HEIGHT_RAD;
        // The lowest within 3° of the horizon, the highest over 60°, and
        // its crests under the zenith fade's start.
        assert!(el(6.0).to_degrees().abs() < 3.0, "{}", el(6.0).to_degrees());
        assert!(el(0.0).to_degrees() > 60.0);
        assert!(el(0.0) + reach < ZENITH_RAD[0]);
        assert_close!(zenith_fade(ZENITH_RAD[0]), 1.0);
        assert_close!(zenith_fade(ZENITH_RAD[1]), 0.0);
        assert_close!(zenith_fade(FRAC_PI_2), 0.0);
        assert!((zenith_fade(0.5 * (ZENITH_RAD[0] + ZENITH_RAD[1])) - 0.5).abs() < 1e-5);
        // Where light is left the finest detail stays wide on the
        // display: the shimmer (aurora.wgsl, 10 per frame height across)
        // at the fade's end, at 20 display pixels per degree, a Quest 3's
        // center is about 25.
        let period_rad = HEIGHT_RAD / 10.0 * ZENITH_RAD[1].cos();
        let px = period_rad.to_degrees() * 20.0;
        assert!(px > 4.0, "{px} display pixels per period");
        // Straight overhead the angles are finite and the light is gone.
        let a = angles(Vec3::Y, Vec2::new(0.0, -1.0));
        assert!(a.is_finite());
        assert_close!(zenith_fade(a.y), 0.0);
    }

    #[test]
    fn the_reflection_is_the_sky_the_mirrored_eye_sees() {
        let boxes = box_room(Vec2::new(0.3, -0.2), 0.4, 4.4, 3.6, 2.5);
        let room = Room::fit(boxes.iter().copied()).unwrap();
        let floor_y = room.floor_y;
        // The point where the line from the eye mirrored in the floor
        // through p leaves the room, found by marching.
        let marched = |eye: Vec3, p: Vec3| {
            let mirrored = Vec3::new(eye.x, 2.0 * floor_y - eye.y, eye.z);
            let dir = (p - mirrored).normalize();
            let across = Vec2::new(-room.axis.y, room.axis.x);
            let inside = |q: Vec3| {
                let l = Vec2::new(q.x, q.z) - room.center;
                l.dot(room.axis).abs() <= room.half.x
                    && l.dot(across).abs() <= room.half.y
                    && q.y <= room.ceiling_y
            };
            let mut t = 0.0;
            while inside(p + dir * (t + 1e-4)) {
                t += 1e-4;
            }
            p + dir * t
        };
        for eye in [
            Vec3::new(0.3, 1.2 + floor_y, -0.2),
            Vec3::new(0.0, 1.05 + floor_y, 0.4),
            Vec3::new(-0.8, 1.6 + floor_y, -0.9),
        ] {
            for p in [
                Vec3::new(0.3, floor_y, -1.4),
                Vec3::new(1.6, floor_y, 0.2),
                Vec3::new(-0.7, floor_y, 1.1),
                Vec3::new(0.5, floor_y, -0.1),
            ] {
                let (m, _) = reflect(eye, p);
                // The mirror's law: the reflected ray leaves the floor at
                // the angle the view ray met it.
                let r = (p - eye).normalize();
                assert_close!([m.x, m.y, m.z], [r.x, -r.y, r.z]);
                let hit = exit(&room, p, m);
                assert!(hit.distance(marched(eye, p)) < 2e-3, "{eye} {p}: {hit}");
            }
        }
        // It moves with the head: the same floor point, two eyes.
        let p = Vec3::new(0.3, floor_y, -1.4);
        let a = exit(&room, p, reflect(Vec3::new(0.3, 1.2, -0.2), p).0);
        let b = exit(&room, p, reflect(Vec3::new(0.0, 1.2, -0.2), p).0);
        assert!(a.distance(b) > 0.1, "{a} {b}");
        // Straight down it is gone, at a slant it is whole.
        let p = Vec3::new(0.0, 0.0, -0.1);
        assert_close!(reflect(Vec3::new(0.0, 1.2, 0.0), p).1, 0.0);
        let far = Vec3::new(0.0, 0.0, -2.2);
        assert_close!(reflect(Vec3::new(0.0, 1.2, 0.0), far).1, 1.0);
        // A floor point outside the room reflects nothing past itself.
        let out = Vec3::new(9.0, 0.0, 9.0);
        assert_close!(
            exit(&room, out, Vec3::new(0.3, 0.5, 0.3).normalize()).to_array(),
            out.to_array()
        );
    }

    #[test]
    fn a_rim_fades_only_where_no_face_of_the_sky_continues_it() {
        let boxes = box_room(Vec2::ZERO, 0.25, 5.0, 4.0, 2.5);
        let head = Vec3::new(0.2, 1.2, 0.1);
        let faces = sky_faces(&boxes, head);
        let joined = join(&faces);
        for (f, (_, edges)) in faces.iter().zip(&joined) {
            if f.normal.y.abs() > 0.9 {
                // The ceiling: a wall along each edge.
                assert_eq!(edges.fade, [false; 4], "{f:?}");
            } else {
                // A wall: its neighbors left and right, the ceiling over
                // it, nothing under it (the floor is not the sky).
                assert_eq!(edges.fade, [false, false, true, false], "{f:?}");
            }
        }
        // A hidden wall is not drawn: its two neighbors and the ceiling
        // keep the rim on that side.
        let gone = faces.iter().position(|f| f.normal.y.abs() < 0.9).unwrap();
        let mut open = faces.clone();
        let hidden = open.remove(gone);
        let flags: Vec<[bool; 4]> = join(&open).into_iter().map(|(_, e)| e.fade).collect();
        let kept = flags.iter().flatten().filter(|f| **f).count();
        // Each wall's foot, the hidden wall's two neighbors on its side,
        // and the ceiling's edge over it.
        assert_eq!(kept, 3 + 2 + 1, "{flags:?}");
        let ceiling = open.iter().position(|f| f.normal.y.abs() > 0.9).unwrap();
        let open_edge = flags[ceiling].iter().position(|f| *f).unwrap();
        let f = &open[ceiling];
        let mid = [
            f.center - f.axes[0] * f.half[0],
            f.center + f.axes[0] * f.half[0],
            f.center - f.axes[1] * f.half[1],
            f.center + f.axes[1] * f.half[1],
        ][open_edge];
        assert!(hidden.nearest(mid).distance(mid) < NEIGHBOR_M);
        // The rim with every edge fading is the surfaces pass's; with none,
        // no fade at all; per side.
        let half = [1.5, 1.2];
        for uv in [Vec2::new(1.48, 0.3), Vec2::new(-1.49, -1.19), Vec2::ZERO] {
            assert_close!(
                rim(uv, half, [1.0; 4]),
                crate::surface_port::edge_fade(uv, half)
            );
            assert_close!(rim(uv, half, [0.0; 4]), 1.0);
        }
        let right_edge = Vec2::new(1.5, 0.0);
        assert_close!(rim(right_edge, half, [1.0, 0.0, 1.0, 1.0]), 1.0);
        assert_close!(rim(right_edge, half, [0.0, 1.0, 0.0, 0.0]), 0.0);
    }

    /// `boxes` (a [`box_room`]) as a scan reports them: the ceiling tilted
    /// 0.4°, the far wall along the axis leaning 0.3° off plumb and the
    /// first wall turned 0.5° off square.
    fn scanned(mut boxes: Vec<(u32, Vec3, Quat, Vec3)>) -> Vec<(u32, Vec3, Quat, Vec3)> {
        let tilt = |b: &mut (u32, Vec3, Quat, Vec3), axis: Vec3, deg: f32| {
            b.2 = Quat::from_axis_angle(axis, deg.to_radians()) * b.2;
        };
        tilt(&mut boxes[5], Vec3::Z, 0.4);
        tilt(&mut boxes[3], Vec3::X, 0.3);
        tilt(&mut boxes[0], Vec3::Y, 0.5);
        boxes
    }

    /// Where the view ray from `eye` through `target` meets `face`'s drawn
    /// quad (lifted): its face point and its world point; `None` off the
    /// quad or behind.
    fn hit(face: &Face, eye: Vec3, target: Vec3) -> Option<(Vec2, Vec3)> {
        let d = target - eye;
        let p = face.center + face.normal * LIFT_M;
        let t = face.normal.dot(p - eye) / face.normal.dot(d);
        let uv = uv_of(face, eye + d * t);
        (t > 0.0 && uv.x.abs() <= face.half[0] && uv.y.abs() <= face.half[1])
            .then_some((uv, eye + d * t))
    }

    #[test]
    fn joined_faces_run_past_each_other_and_carry_the_neighbors_plane() {
        for boxes in [
            box_room(Vec2::new(0.4, -0.3), 0.35, 4.6, 3.8, 2.45),
            scanned(box_room(Vec2::new(0.4, -0.3), 0.35, 4.6, 3.8, 2.45)),
        ] {
            let head = Vec3::new(0.6, 1.2, 0.0);
            let faces = sky_faces(&boxes, head);
            let joined = join(&faces);
            let mut cuts = 0;
            for (i, (f, edges)) in joined.iter().enumerate() {
                let before = faces[i];
                // Still in its plane, with its axes, and it carries that
                // plane.
                assert!(f.normal.abs_diff_eq(before.normal, 1e-6));
                assert!(f.axes[0].abs_diff_eq(before.axes[0], 1e-6));
                assert!((f.center - before.center).dot(before.normal).abs() < 1e-5);
                let lifted = f.center + f.normal * LIFT_M;
                let [x, y, z, k] = edges.plane;
                assert!(Vec3::new(x, y, z).abs_diff_eq(f.normal, 1e-6));
                assert!((f.normal.dot(lifted) - k).abs() < 1e-5);
                let c = f.corners(LIFT_M);
                let ends = [[c[0], c[3]], [c[1], c[2]], [c[0], c[1]], [c[3], c[2]]];
                for e in 0..4 {
                    let (axis, sign) = (e / 2, if e % 2 == 1 { 1.0 } else { -1.0 });
                    let Some(q) = edges.cut[e] else {
                        // No join through the fragment: the edge stays
                        // where it was.
                        assert_close!(edges.cut_from[e], NO_CUT);
                        for q in ends[e] {
                            let along = (q - before.center).dot(before.axes[axis]);
                            assert!((along - sign * before.half[axis]).abs() < 1e-5, "{i} {e}");
                        }
                        continue;
                    };
                    cuts += 1;
                    assert!(!edges.fade[e]);
                    // The neighbor's plane, as the very floats it carries
                    // for itself.
                    let neighbor = joined.iter().enumerate().find(|(j, (_, g))| {
                        *j != i && g.plane.map(f32::to_bits) == q.map(f32::to_bits)
                    });
                    let Some((_, (g, _))) = neighbor else {
                        panic!("face {i} edge {e}: no face carries its neighbor's plane");
                    };
                    // Both ends of the edge run at least OVERLAP_M past the
                    // neighbor's plane (along the face, more off it), on its
                    // far side from this face; not much more.
                    let p = g.center + g.normal * LIFT_M;
                    let inside = (lifted - p).dot(g.normal).signum();
                    for q in ends[e] {
                        let past = -(q - p).dot(g.normal) * inside;
                        assert!(past >= OVERLAP_M * 0.5 - 1e-4, "{i} {e}: {past}");
                        let along = (q - before.center).dot(before.axes[axis]) * sign;
                        assert!(along < before.half[axis] + NEIGHBOR_M + 2.0 * OVERLAP_M);
                    }
                    // Where the neighbor's plane crosses this face, at
                    // either end and the middle, the join decides.
                    let other = 1 - axis;
                    for s in [-1.0, 0.0, 1.0] {
                        let base = lifted + f.axes[other] * s * f.half[other];
                        let x = g.normal.dot(p - base) / g.normal.dot(f.axes[axis]);
                        assert!(sign * x > edges.cut_from[e] + 0.5 * OVERLAP_M, "{i} {e}");
                        assert!(sign * x < f.half[axis] - 0.5 * OVERLAP_M, "{i} {e}");
                    }
                }
            }
            // Four corners and four wall-to-ceiling edges, both sides each.
            assert_eq!(cuts, 16);
        }
    }

    #[test]
    fn along_every_join_exactly_one_face_keeps_each_view_ray() {
        for boxes in [
            box_room(Vec2::new(0.4, -0.3), 0.35, 4.6, 3.8, 2.45),
            scanned(box_room(Vec2::new(0.4, -0.3), 0.35, 4.6, 3.8, 2.45)),
            scanned(box_room(Vec2::new(-1.0, 2.0), -0.8, 6.17, 5.49, 2.67)),
        ] {
            let room = Room::fit(boxes.iter().copied()).unwrap();
            let sky = Sky::new(room, Vec2::new(0.0, -1.0));
            // Seated at the point, seated off it, standing, lying back.
            let p = sky.point;
            for eye in [
                p,
                p + Vec3::new(0.6, -0.05, -0.4),
                p + Vec3::new(-0.3, 0.45, 0.5),
                p + Vec3::new(0.2, -0.75, 0.9),
            ] {
                let faces = sky_faces(&boxes, eye);
                let joined = join(&faces);
                let rows: Vec<_> = joined
                    .iter()
                    .map(|(f, e)| sky_rows_edges(f, &sky, *e))
                    .collect();
                let drawn = |i: usize, target: Vec3| {
                    hit(&joined[i].0, eye, target)
                        .is_some_and(|(uv, w)| keeps(&rows[i], uv, w, eye, target - eye))
                };
                let mut rays = 0;
                for &((i, qa), (j, qb)) in &across_edges(&faces, p) {
                    // Along the line, rays through points near it on either
                    // face: from a tenth of a millimeter to 4 cm off it.
                    let q = 0.5 * (qa + qb);
                    let (into_a, into_b) = ((qa - q).normalize(), (qb - q).normalize());
                    for off in [1e-4, 1e-3, 1e-2, 4e-2] {
                        for target in [q + into_a * off, q + into_b * off] {
                            let others = (0..joined.len())
                                .filter(|&k| k != i && k != j && drawn(k, target))
                                .count();
                            assert_eq!(others, 0, "a third face at {target}");
                            let (a, b) = (drawn(i, target), drawn(j, target));
                            assert!(a != b, "eye {eye}, {target} off {off}: {a} {b}");
                            rays += 1;
                        }
                    }
                }
                assert!(rays >= 8 * 3 * 8, "{rays}");
            }
        }
    }

    #[test]
    fn the_mapping_is_the_sky_on_walls_and_ceilings_and_its_reflection_on_floors() {
        use SurfaceBehavior as B;
        for (kind, want) in [
            (KIND_WALL, Map::Sky),
            (KIND_CEILING, Map::Sky),
            (KIND_FLOOR, Map::Reflection),
            (KIND_TABLE, Map::Face),
            (KIND_FRAME, Map::Face),
            (KIND_OTHER, Map::Face),
            (crate::surfaces::KIND_NONE, Map::Face),
        ] {
            assert_eq!(Map::of(true, B::Aurora, kind), want, "{kind}");
            // The knob off: per face everywhere.
            assert_eq!(Map::of(false, B::Aurora, kind), Map::Face, "{kind}");
            // The other seven and the built-in behaviors: per face.
            for b in [B::Prism, B::Tessera, B::Rings, B::Streamlines] {
                assert_eq!(Map::of(true, b, kind), Map::Face);
            }
        }
        assert_close!(
            [Map::Face, Map::Sky, Map::Reflection].map(Map::code),
            [0.0, 1.0, 2.0]
        );
    }

    #[test]
    fn the_rows_carry_the_sky_and_the_sample_follows_them() {
        let boxes = box_room(Vec2::new(0.5, 0.0), 0.0, 4.0, 3.0, 2.4);
        let room = Room::fit(boxes.iter().copied()).unwrap();
        let sky = Sky::new(room, Vec2::new(0.6, -0.8));
        let wall = sky_faces(&boxes, sky.point)[0];
        let base = crate::surface_port::rows(&wall, 0.8, false, FIRST_ID, [1.0; 3]);
        // Per face: the rows as they were.
        assert_eq!(rows(base, Map::Face, &sky, [false; 4]), base);
        let r = rows(base, Map::Sky, &sky, [true, false, true, false]);
        assert_eq!(r[..MODE_ROW], base[..MODE_ROW]);
        // The sky's layers (board #5438): the count and the depth
        // multiplier, the default two at 1.
        assert_close!(r[MODE_ROW], [0.0, 1.0, 2.0, 1.0]);
        assert_close!(r[EDGES_ROW], [1.0, 0.0, 1.0, 0.0]);
        let p = sky.point;
        assert_close!(r[POINT_ROW], [p.x, p.y, p.z, room.ceiling_y]);
        assert_close!(r[ROOM_ROW], [0.5, 0.0, room.half.x, room.half.y]);
        assert_close!(r[AXES_ROW], [room.axis.x, room.axis.y, 0.6, -0.8]);
        // The sample: on the sky, the direction's frame and the zenith
        // fade; per face, the face's frame.
        let uv = Vec2::new(0.3, 0.4);
        let world = on(&wall, uv);
        let k = PEAK_ALPHA * 0.8;
        let s = sample(&r, RESOLUTION, uv, world, Vec3::ZERO).unwrap();
        let a = angles(world - sky.point, sky.facing);
        assert_close!(s.at.to_array(), frame(a, RESOLUTION).to_array());
        assert_close!(s.k, k * zenith_fade(a.y));
        let half_res = crate::surface_port::resolution(wall.half);
        let f = sample(&base, half_res, uv, world, Vec3::ZERO).unwrap();
        assert_close!(
            f.at.to_array(),
            crate::surface_port::frame_coord(uv, wall.half, half_res).to_array()
        );
        assert_close!(f.k, k);
        // On the floor: the reflection's share and its fade.
        let floor = port_face(acting_face(boxes[4].1, boxes[4].2, boxes[4].3, sky.point));
        let rf = rows(
            crate::surface_port::rows(&floor, 1.0, false, FIRST_ID, [1.0; 3]),
            Map::Reflection,
            &sky,
            [true; 4],
        );
        let eye = Vec3::new(0.4, 1.25, 0.2);
        let uv = uv_of(&floor, Vec3::new(1.4, 0.0, -1.0));
        let world = on(&floor, uv);
        let s = sample(&rf, RESOLUTION, uv, world, eye).unwrap();
        let (m, fade) = reflect(eye, world);
        let a = angles(exit(&room, world, m) - sky.point, sky.facing);
        assert_close!(s.at.to_array(), frame(a, RESOLUTION).to_array());
        assert_close!(
            s.k,
            PEAK_ALPHA * REFLECT_GAIN * fade * zenith_fade(a.y) * (1.0 - 2.0 * seam_weight(a.x))
        );
        assert!(s.k < REFLECT_GAIN * PEAK_ALPHA + 1e-6);
    }

    /// A constant of `module` as an f32 or a vector of them.
    fn constant(module: &naga::Module, name: &str) -> Vec<f32> {
        let (_, c) = module
            .constants
            .iter()
            .find(|(_, c)| c.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("const {name}"));
        let lit = |e: &naga::Expression| match *e {
            naga::Expression::Literal(naga::Literal::F32(v)) => v,
            ref e => panic!("{name}: {e:?}"),
        };
        match module.global_expressions[c.init] {
            naga::Expression::Compose { ref components, .. } => components
                .iter()
                .map(|h| lit(&module.global_expressions[*h]))
                .collect(),
            ref e => vec![lit(e)],
        }
    }

    #[test]
    fn the_sky_wgsl_validates_and_its_constants_are_the_rusts() {
        let source = format!(
            "struct U {{ resolution: vec2<f32> }}\n@group(0) @binding(0) var<uniform> u: U;\n\
             fn effect_main(fc: vec4<f32>) -> vec4<f32> {{ return fc; }}\n{}{SKY_WGSL}",
            crate::surface_port::WRAPPER_WGSL
        );
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("the sky validates");
        assert_close!(constant(&module, "XR_SKY_HEIGHT"), [HEIGHT_RAD]);
        assert_close!(constant(&module, "XR_SKY_HORIZON"), [HORIZON_RAD]);
        assert_close!(constant(&module, "XR_SKY_SEAM"), [SEAM_RAD]);
        assert_close!(constant(&module, "XR_SKY_ZENITH"), ZENITH_RAD);
        assert_close!(constant(&module, "XR_REFLECT_GAIN"), [REFLECT_GAIN]);
        assert_close!(constant(&module, "XR_REFLECT_COS"), REFLECT_COS);
        assert_close!(constant(&module, "XR_PI"), [PI]);
        assert_close!(constant(&module, "XR_TAU"), [TAU]);
        // The layers' (board #5438).
        assert_close!(constant(&module, "XR_SKY_SHELL"), SHELL_M);
        assert_close!(constant(&module, "XR_SKY_LIGHT"), LAYER_LIGHT);
        assert_close!(constant(&module, "XR_SKY_TURN"), LAYER_TURN_RAD);
        assert_close!(constant(&module, "XR_SKY_DROP"), LAYER_DROP_RAD);
        assert_close!(constant(&module, "XR_SKY_FLOOR"), BANDS_FLOOR_RAD);
        assert_close!(constant(&module, "XR_SKY_TINT_1"), LAYER_TINT[1]);
        assert_close!(constant(&module, "XR_SKY_TINT_2"), LAYER_TINT[2]);
        assert_close!(LAYER_TINT[0], [1.0; 3]);
        // The rows' places: the struct's members in order, one row each.
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("XrPort"))
            .expect("XrPort");
        let naga::TypeInner::Struct { ref members, span } = ty.inner else {
            panic!("XrPort");
        };
        assert_eq!(span as usize, 16 * UNIFORM_ROWS);
        let at = |name: &str| {
            members
                .iter()
                .find(|m| m.name.as_deref() == Some(name))
                .map(|m| m.offset as usize / 16)
                .unwrap_or_else(|| panic!("member {name}"))
        };
        assert_eq!(
            [
                at("mode"),
                at("edges"),
                at("sky"),
                at("room"),
                at("axes"),
                at("plane"),
                at("cut"),
                at("cut_from")
            ],
            [
                MODE_ROW,
                EDGES_ROW,
                POINT_ROW,
                ROOM_ROW,
                AXES_ROW,
                PLANE_ROW,
                CUT_ROW,
                CUT_FROM_ROW
            ]
        );
        assert_eq!(
            [CUT_ROW + 4, CUT_FROM_ROW + 1],
            [CUT_FROM_ROW, UNIFORM_ROWS]
        );
        assert_eq!(
            [at("face"), at("params"), at("color")],
            [4, 5, crate::immerse::PEAK_ROW]
        );
        // The eye the vertex stage solves is the tunnel's.
        assert!(SKY_WGSL.contains("fn xr_eye_position()"));
    }

    #[test]
    fn the_log_says_where_the_point_and_the_seam_are() {
        let room = Room::fit(box_room(Vec2::new(1.0, -2.0), 0.0, 4.0, 3.0, 2.5)).unwrap();
        let sky = Sky::new(room, Vec2::new(0.6, -0.8));
        let line = line(true, &sky, "launch", 0.0);
        assert!(
            line.starts_with("sky: on (debug.fosfora.sky) · point (1.00, 1.22, -2.00)"),
            "{line}"
        );
        assert!(
            line.contains("the scanned room's 4 walls in plan (4.00 x 3.00 m"),
            "{line}"
        );
        assert!(
            line.contains("seam toward (-0.60, 0.80), behind the facing at launch"),
            "{line}"
        );
        assert!(
            line.ends_with("hidden walls leave 0° of the turn undrawn"),
            "{line}"
        );
        let stage = Sky::new(Room::stage(Vec2::ZERO, 0.0), Vec2::X);
        let off = super::line(false, &stage, "the last recenter", 90.0);
        assert!(off.starts_with("sky: off, Aurora per face"), "{off}");
        assert!(
            off.contains("no scanned walls: the stage floor's center"),
            "{off}"
        );
        assert!(off.contains("seam toward (-1.00, 0.00)"), "{off}");
        assert!(off.contains("leave 90°"), "{off}");
    }

    /// `sky` with `count` layers at a depth multiplier of `depth`.
    fn layered(sky: Sky, count: u32, depth: f32) -> Sky {
        Sky {
            layers: Layers { count, depth },
            ..sky
        }
    }

    /// The rows of `face` on the sky `sky`, every edge fading or none.
    fn sky_rows(face: &Face, sky: &Sky, fade: bool) -> [[f32; 4]; UNIFORM_ROWS] {
        sky_rows_edges(face, sky, [fade; 4].into())
    }

    /// The rows of `face` on the sky `sky` with its `edges`.
    fn sky_rows_edges(face: &Face, sky: &Sky, edges: Edges) -> [[f32; 4]; UNIFORM_ROWS] {
        let base = crate::surface_port::rows(face, 1.0, false, FIRST_ID, [1.0; 3]);
        rows(base, Map::Sky, sky, edges)
    }

    /// Where the ray from `eye` through `p` leaves `room`'s box grown by
    /// `grow`, found by marching in 0.1 mm steps.
    fn marched_shell(room: &Room, eye: Vec3, p: Vec3, grow: f32) -> Vec3 {
        let dir = (p - eye).normalize();
        let across = Vec2::new(-room.axis.y, room.axis.x);
        let inside = |q: Vec3| {
            let l = Vec2::new(q.x, q.z) - room.center;
            l.dot(room.axis).abs() <= room.half.x + grow
                && l.dot(across).abs() <= room.half.y + grow
                && q.y <= room.ceiling_y + grow
        };
        let mut n = 0u32;
        while inside(p + dir * ((n + 1) as f32 * 1e-4)) {
            n += 1;
        }
        p + dir * (n as f32 * 1e-4)
    }

    #[test]
    fn the_view_ray_meets_each_shell_behind_the_surface_it_passes() {
        // A 4.4 by 3.6 m room turned 23°, 2.5 m high.
        let boxes = box_room(Vec2::new(0.3, -0.2), 0.4, 4.4, 3.6, 2.5);
        let room = Room::fit(boxes.iter().copied()).unwrap();
        let point = room.point();
        let faces = sky_faces(&boxes, point);
        let ax = Vec3::new(room.axis.x, 0.0, room.axis.y);
        let ac = Vec3::new(-room.axis.y, 0.0, room.axis.x);
        // How far a point is past the room's sides along its axis and
        // across it, and over its ceiling.
        let past = |q: Vec3| {
            let l = q - Vec3::new(room.center.x, 0.0, room.center.y);
            [
                l.dot(ax).abs() - room.half.x,
                l.dot(ac).abs() - room.half.y,
                q.y - room.ceiling_y,
            ]
        };
        // The face whose outside is toward `out`, and its point nearest
        // `q` on the lifted quad.
        let face = |out: Vec3| *faces.iter().find(|f| f.normal.dot(out) < -0.9).unwrap();
        let onto = |f: &Face, q: Vec3| on(f, uv_of(f, q));
        let middle = Vec3::new(room.center.x, 0.0, room.center.y);
        for grow in SHELL_M {
            let check = |eye: Vec3, p: Vec3| {
                let hit = shell_hit(&room, eye, p, grow);
                assert!(hit.distance(marched_shell(&room, eye, p, grow)) < 1e-3);
                // On the line from the eye through the surface point,
                // past it.
                let (to_p, to_hit) = (p - eye, hit - eye);
                assert!(to_p.normalize().dot(to_hit.normalize()) > 1.0 - 1e-6);
                assert!(to_hit.length() > to_p.length() + grow - 1e-3);
                (hit, past(hit))
            };
            // Square on: the wall ahead along the axis at eye height, from
            // the point. The hit is `grow` past the wall, straight behind
            // the surface point.
            let wall = face(ax);
            let p = onto(&wall, point + ax * room.half.x);
            let (hit, [a, b, c]) = check(point, p);
            assert!((a - grow).abs() < 1e-4 && b < 0.0 && c < 0.0, "{hit}");
            assert!((hit - p).normalize().dot(ax) > 1.0 - 1e-6);
            // At a slant: a wall across the axis, 45° off its normal, from
            // an eye off the point.
            let eye = point + Vec3::new(0.4, -0.1, 0.3);
            let wall = face(ac);
            let to_wall = room.half.y - (eye - middle).dot(ac);
            let p = onto(&wall, eye + (ac + ax) * to_wall + Vec3::Y * 0.3);
            let slant = (p - eye).normalize().dot(ac);
            assert!(slant > 0.55 && slant < 0.75, "{slant}");
            let (hit, [a, b, c]) = check(eye, p);
            assert!((b - grow).abs() < 1e-4 && a < grow && c < grow, "{hit}");
            // Near a corner: 10 cm from the end of the wall along the
            // axis, seen across the room; past the wall the ray leaves
            // through the shell's side beyond the next wall.
            let eye = point - ac;
            let wall = face(ax);
            let p = onto(
                &wall,
                middle + ax * room.half.x + ac * (room.half.y - 0.1) + Vec3::Y * 1.4,
            );
            let (hit, [a, b, _]) = check(eye, p);
            assert!((b - grow).abs() < 1e-4 && a < grow - 0.1, "{hit}");
            // On the ceiling: straight up, through the shell's top...
            let ceiling = face(Vec3::Y);
            let p = onto(&ceiling, point + ax * 0.3 + Vec3::Y);
            let (hit, [a, b, c]) = check(point, p);
            assert!((c - grow).abs() < 1e-4 && a < 0.0 && b < 0.0, "{hit}");
            // ...and at a slant near its edge, through the shell's side
            // over the wall.
            let eye = point - ax * 1.5;
            let p = onto(&ceiling, middle + ax * (room.half.x - 0.1) + Vec3::Y * 2.5);
            let (hit, [a, _, c]) = check(eye, p);
            assert!((a - grow).abs() < 1e-4 && c > 0.0 && c < grow, "{hit}");
        }
    }

    #[test]
    fn a_far_layer_moves_against_the_wall_by_its_depth_as_the_head_moves() {
        for dist in [2.0f32, 3.0] {
            // A 5 m wide room, the wall ahead `dist` from the point.
            let boxes = box_room(Vec2::ZERO, 0.0, 5.0, 2.0 * dist, 2.5);
            let room = Room::fit(boxes.iter().copied()).unwrap();
            for depth in [1.0, 2.0] {
                let sky = layered(Sky::new(room, Vec2::new(0.0, -1.0)), 3, depth);
                let wall = *sky_faces(&boxes, sky.point)
                    .iter()
                    .find(|f| f.normal.z > 0.9)
                    .unwrap();
                let r = sky_rows(&wall, &sky, true);
                // Seated at the point, the wall straight ahead at eye
                // height.
                let eye = sky.point;
                let p = on(&wall, uv_of(&wall, Vec3::new(eye.x, eye.y, -dist)));
                let before = far(&r, RESOLUTION, uv_of(&wall, p), p, eye);
                assert_eq!(before.len(), 2);
                // The head 10 cm to the right. Where on the wall does each
                // layer now show what it showed at p?
                let moved = eye + Vec3::X * 0.1;
                for f in &before {
                    let to = f.hit - moved;
                    let q = moved + to * (wall.normal.dot(p - moved) / wall.normal.dot(to));
                    let after = far(&r, RESOLUTION, uv_of(&wall, q), q, moved)[f.layer - 1];
                    // The same curtain: the same frame coordinate.
                    assert!(after.at.distance(f.at) < 0.05, "{after:?} {f:?}");
                    // Along the wall with the head, by the move times the
                    // share of the layer's distance that lies past the
                    // wall.
                    let (to_wall, to_shell) =
                        (wall.normal.dot(eye - p), wall.normal.dot(eye - f.hit));
                    let want = 0.1 * (1.0 - to_wall / to_shell);
                    let shift = q - p;
                    assert!(
                        (shift.x - want).abs() < 1e-4 && shift.y.abs() < 1e-4,
                        "{shift} {want}"
                    );
                    // The shell is its depth behind the wall's center line
                    // (the room's side); the quad is 4 cm in front of that
                    // (the slab's half and the lift).
                    assert!(
                        (to_shell - dist - SHELL_M[f.layer - 1] * depth).abs() < 1e-4,
                        "{to_shell}"
                    );
                    // Several centimeters at 2 to 3 m.
                    assert!(shift.x > 0.03, "{shift}");
                    eprintln!(
                        "wall {to_wall:.2} m, skydepth {depth}: layer {} on its shell {:.2} m behind it moves {:.1} cm for a 10 cm head move",
                        f.layer,
                        to_shell - to_wall,
                        100.0 * shift.x
                    );
                }
                // The near layer stays on the wall: what it shows at p does
                // not depend on the eye.
                let uv = uv_of(&wall, p);
                assert_eq!(
                    sample(&r, RESOLUTION, uv, p, eye),
                    sample(&r, RESOLUTION, uv, p, moved)
                );
            }
        }
    }

    #[test]
    fn a_far_layer_runs_on_across_a_corner_and_onto_the_ceiling() {
        let boxes = box_room(Vec2::new(0.4, -0.3), 0.35, 4.6, 3.8, 2.45);
        let room = Room::fit(boxes.iter().copied()).unwrap();
        // A seated eye off the point, so the rays through a corner do not
        // run along the shells' own corners.
        let eye = Vec3::new(0.9, 1.15, 0.2);
        let faces = sky_faces(&boxes, eye);
        let pairs = across_edges(&faces, room.point());
        assert!(pairs.len() >= 8 * 3, "{}", pairs.len());
        for depth in [1.0, 2.5] {
            let sky = layered(
                Sky::new(room, facing(Quat::from_rotation_y(0.35))),
                3,
                depth,
            );
            let far_at = |i: usize, q: Vec3| {
                let f = &faces[i];
                far(&sky_rows(f, &sky, false), RESOLUTION, uv_of(f, q), q, eye)
            };
            for &((i, qa), (j, qb)) in &pairs {
                let (a, b) = (far_at(i, qa), far_at(j, qb));
                assert_eq!(a.len(), 2);
                for (x, y) in a.iter().zip(&b) {
                    // Rays 0.7 mm apart at the surface meet the shell
                    // under a millimeter apart per meter of ray, and the
                    // light runs on.
                    let gap = x.hit.distance(y.hit) / x.hit.distance(eye);
                    assert!(gap < 1e-3, "{gap}: {x:?} {y:?}");
                    assert!((x.k - y.k).abs() < 1e-3, "{x:?} {y:?}");
                    // So does the picture, away from the seam (where the
                    // far layers fade out).
                    let az = angles(x.hit - sky.point, sky.facing).x;
                    if az.abs() < PI - SEAM_RAD {
                        assert!(x.at.distance(y.at) < 1.0, "{x:?} {y:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn one_layer_is_the_sky_as_it_was() {
        let boxes = box_room(Vec2::new(0.5, 0.0), 0.0, 4.0, 3.0, 2.4);
        let room = Room::fit(boxes.iter().copied()).unwrap();
        let sky = Sky::new(room, Vec2::new(0.6, -0.8));
        let eye = Vec3::new(0.7, 1.1, 0.3);
        for face in sky_faces(&boxes, eye) {
            let base = crate::surface_port::rows(&face, 0.8, false, FIRST_ID, [1.0; 3]);
            let [one, two, three] =
                [1, 2, 3].map(|n| rows(base, Map::Sky, &layered(sky, n, 1.0), [true; 4]));
            // The rows differ in the count alone.
            assert_close!(one[MODE_ROW], [0.0, 1.0, 1.0, 1.0]);
            for (r, n) in [(two, 2.0), (three, 3.0)] {
                assert_close!(r[MODE_ROW][2], n);
                let mut same = r;
                same[MODE_ROW][2] = 1.0;
                assert_eq!(same, one);
            }
            for uv in [
                Vec2::ZERO,
                Vec2::new(0.3, 0.4),
                Vec2::new(-0.8, -0.5),
                Vec2::new(1.1, 0.9),
            ] {
                let world = on(&face, uv);
                // One layer: no far one. The near layer is the same
                // whatever the count.
                assert!(far(&one, RESOLUTION, uv, world, eye).is_empty());
                let near = sample(&one, RESOLUTION, uv, world, eye);
                for (r, n) in [(two, 1), (three, 2)] {
                    assert_eq!(sample(&r, RESOLUTION, uv, world, eye), near);
                    assert_eq!(far(&r, RESOLUTION, uv, world, eye).len(), n);
                }
            }
        }
        // The reflection and a face mapped per face are one layer whatever
        // the count.
        let floor = port_face(acting_face(boxes[4].1, boxes[4].2, boxes[4].3, eye));
        let base = crate::surface_port::rows(&floor, 1.0, false, FIRST_ID, [1.0; 3]);
        let uv = uv_of(&floor, Vec3::new(1.4, 0.0, -1.0));
        for n in 1..=3 {
            let s = layered(sky, n, 1.0);
            let rf = rows(base, Map::Reflection, &s, [true; 4]);
            assert_close!(rf[MODE_ROW], [0.0, 2.0, 0.0, 0.0]);
            assert!(far(&rf, RESOLUTION, uv, on(&floor, uv), eye).is_empty());
            assert_eq!(rows(base, Map::Face, &s, [true; 4]), base);
            assert!(far(&base, RESOLUTION, uv, on(&floor, uv), eye).is_empty());
        }
        // Out of range, the count is held to one to three.
        let wall = sky_faces(&boxes, eye)[0];
        assert_close!(
            sky_rows(&wall, &layered(sky, 0, 1.0), true)[MODE_ROW][2],
            1.0
        );
        assert_close!(
            sky_rows(&wall, &layered(sky, 7, 1.0), true)[MODE_ROW][2],
            3.0
        );
        // With nothing under it the light is the near layer's, bit for bit.
        let light = crate::surface_port::light([0.3, 0.7, 0.2, 1.0], false, 0.45, [1.0; 3]);
        assert_eq!(
            under(light, [0.0; 4]).map(f32::to_bits),
            light.map(f32::to_bits)
        );
    }

    #[test]
    fn the_farther_a_layer_the_less_light_and_the_cooler() {
        let boxes = box_room(Vec2::ZERO, 0.0, 4.0, 4.0, 2.5);
        let room = Room::fit(boxes.iter().copied()).unwrap();
        let sky = layered(Sky::new(room, Vec2::new(0.0, -1.0)), 3, 1.0);
        // Seated at the point, each layer's hit lies in the fragment's
        // direction, so its fades are the near layer's: ahead, 30° up.
        let eye = sky.point;
        let wall = *sky_faces(&boxes, eye)
            .iter()
            .find(|f| f.normal.z > 0.9)
            .unwrap();
        let r = sky_rows(&wall, &sky, true);
        let world = on(
            &wall,
            uv_of(&wall, eye + Vec3::new(0.3, 2.0 * 0.52f32.tan(), -2.0)),
        );
        let uv = uv_of(&wall, world);
        let near = sample(&r, RESOLUTION, uv, world, eye).unwrap();
        let a = angles(world - sky.point, sky.facing);
        for f in far(&r, RESOLUTION, uv, world, eye) {
            assert_close!(f.k / near.k, LAYER_LIGHT[f.layer]);
            // Its picture is the near layer's place turned around the
            // frame and lowered.
            let turned = Vec2::new(a.x + LAYER_TURN_RAD[f.layer], a.y + LAYER_DROP_RAD[f.layer]);
            assert!(f.at.distance(frame(turned, RESOLUTION)) < 0.01, "{f:?}");
        }
        assert_close!(LAYER_LIGHT[0], 1.0);
        // The tint: the near layer's light is the port's, bit for bit; the
        // farther, the cooler; the coverage is not tinted.
        let c = [0.7, 0.6, 0.5, 1.0];
        let lit = |l: usize| layer_light(c, false, 0.4, [1.0; 3], l);
        assert_eq!(
            lit(0).map(f32::to_bits),
            crate::surface_port::light(c, false, 0.4, [1.0; 3]).map(f32::to_bits)
        );
        let warmth = |l: usize| (lit(l)[0] / lit(l)[2], lit(l)[1] / lit(l)[2]);
        assert!(warmth(0).0 > warmth(1).0 && warmth(1).0 > warmth(2).0);
        assert!(warmth(0).1 > warmth(1).1 && warmth(1).1 > warmth(2).1);
        for l in 1..3 {
            assert_close!(lit(l)[3], lit(0)[3]);
        }
        // Under: a far layer fills in what the nearer ones do not cover,
        // and the alpha stays within 1.
        let full = [0.5, 0.4, 0.3, 1.0];
        assert_close!(under(full, [0.2; 4]), full);
        assert_close!(under([0.0; 4], [0.1, 0.2, 0.3, 0.4]), [0.1, 0.2, 0.3, 0.4]);
        for i in 0..=10 {
            for j in 0..=10 {
                let (x, y) = (i as f32 / 10.0, j as f32 / 10.0);
                let a = under([0.0, 0.0, 0.0, x], [0.0, 0.0, 0.0, y])[3];
                assert_close!(a, 1.0 - (1.0 - x) * (1.0 - y));
                assert!(a <= 1.0);
            }
        }
    }

    #[test]
    fn a_far_layer_is_not_evaluated_where_it_adds_no_light() {
        // A 3 m square room, seated at the point: each layer's hit lies in
        // the fragment's direction.
        let dist = 1.5;
        let boxes = box_room(Vec2::ZERO, 0.0, 2.0 * dist, 2.0 * dist, 2.5);
        let room = Room::fit(boxes.iter().copied()).unwrap();
        let sky = layered(Sky::new(room, Vec2::new(0.0, -1.0)), 3, 1.0);
        let eye = sky.point;
        let faces = sky_faces(&boxes, eye);
        let face = |n: Vec3| *faces.iter().find(|f| f.normal.dot(n) > 0.9).unwrap();
        let at = |f: &Face, q: Vec3| {
            let r = sky_rows(f, &sky, true);
            let p = on(f, uv_of(f, q));
            let uv = uv_of(f, p);
            (
                sample(&r, RESOLUTION, uv, p, eye),
                far(&r, RESOLUTION, uv, p, eye),
            )
        };
        // Straight overhead: each layer's own zenith fade, none left.
        let (near, layers) = at(&face(Vec3::NEG_Y), eye + Vec3::Y * 2.0);
        assert_eq!(near, None);
        assert!(layers.iter().all(|f| f.k == 0.0), "{layers:?}");
        // Straight behind, on the seam: the near layer cross-fades its two
        // ends, the far ones fade out; 12° to the side they are whole.
        let back = face(Vec3::NEG_Z);
        let (near, layers) = at(&back, eye + Vec3::new(0.0, 0.5, dist));
        assert!(near.unwrap().other.is_some());
        assert!(layers.iter().all(|f| f.k < 1e-5), "{layers:?}");
        let side = Vec3::new(dist * (SEAM_RAD + 0.01).tan(), 0.5, dist);
        let (near, layers) = at(&back, eye + side);
        let k = near.unwrap().k;
        for f in &layers {
            assert_close!(f.k, k * LAYER_LIGHT[f.layer]);
        }
        // Low on a wall, under every layer's bands (the lowest layer's
        // drop under the fade's end): none left; at the fade's start, all
        // of it.
        let ahead = face(Vec3::Z);
        let low = dist * (BANDS_FLOOR_RAD[0] - LAYER_DROP_RAD[1] - 0.02).tan();
        assert!(eye.y + low > 0.1);
        let (_, layers) = at(&ahead, eye + Vec3::new(0.0, low, -dist));
        assert!(layers.iter().all(|f| f.k == 0.0), "{layers:?}");
        let high = dist * BANDS_FLOOR_RAD[1].tan();
        let (near, layers) = at(&ahead, eye + Vec3::new(0.0, high, -dist));
        let k = near.unwrap().k;
        for f in &layers {
            assert_close!(f.k, k * LAYER_LIGHT[f.layer]);
        }
        // Where the floor fade starts aurora.wgsl's lowest band has left
        // nothing: its center at the .pfx's spread, the waves' and the
        // ripple's 0.12 and three widths of its widest glow (0.02 of the
        // frame's height, 2.5 times at a full band), where exp(-9) of a
        // band's light (at most 4) is left.
        let spread = 1.25;
        let width = (0.3 * 0.04 + 0.008) * 2.5;
        let reach = HORIZON_RAD + HEIGHT_RAD * (0.5 - (3.0 / 7.0 * spread + 0.12 + 3.0 * width));
        assert!(BANDS_FLOOR_RAD[1] <= reach, "{reach}");
        assert!(4.0 * (-9.0f32).exp() < 1e-3);
        // The middle layer's bands run halfway between the near layer's,
        // the farthest's a quarter of the way.
        assert_close!(BAND_SPACING_RAD, HEIGHT_RAD * spread / 7.0);
        assert_close!(
            LAYER_DROP_RAD,
            [0.0, 0.5 * BAND_SPACING_RAD, 0.25 * BAND_SPACING_RAD]
        );
        // Turned around the frame, no band's two waves line up with
        // another layer's: aurora.wgsl's waves run at 2 + 0.5 i and 2.3
        // times that per frame height across, and a frame height is
        // HEIGHT_RAD of azimuth.
        let t = LAYER_TURN_RAD;
        for turn in [t[1] - t[0], t[2] - t[0], t[2] - t[1]] {
            for i in 0..7 {
                for m in [1.0, 2.3] {
                    let phase = ((2.0 + 0.5 * i as f32) * m * turn / HEIGHT_RAD).rem_euclid(TAU);
                    assert!(phase.min(TAU - phase) > 0.39, "{turn} {i} {m}: {phase}");
                }
            }
        }
    }

    #[test]
    fn the_layers_knobs_and_their_log_lines() {
        let knobs = |layers: Option<&str>, depth: Option<&str>| {
            Layers::from_knobs(|name| match name {
                KNOB_LAYERS => layers.map(String::from),
                KNOB_DEPTH => depth.map(String::from),
                _ => None,
            })
        };
        let at = |count, depth| Layers { count, depth };
        assert_eq!(knobs(None, None), at(LAYERS_DEFAULT, 1.0));
        assert_eq!(Layers::default(), at(2, 1.0));
        assert_eq!(knobs(Some("1"), None), at(1, 1.0));
        assert_eq!(knobs(Some(" 3 "), None), at(3, 1.0));
        for wrong in ["0", "4", "two", ""] {
            assert_eq!(knobs(Some(wrong), None), at(2, 1.0), "{wrong}");
        }
        assert_eq!(knobs(None, Some("2.5")), at(2, 2.5));
        assert_eq!(knobs(None, Some("9")), at(2, DEPTH_MAX));
        assert_eq!(knobs(None, Some("-1")), at(2, 0.0));
        for wrong in ["nan", "inf", "deep"] {
            assert_eq!(knobs(None, Some(wrong)), at(2, 1.0), "{wrong}");
        }
        assert_close!(at(3, 2.0).shells(), [3.0, 8.0]);
        assert_eq!(
            Layers::default().line(),
            "sky layers: 2 (debug.fosfora.skylayers), the sky on the surfaces and a layer 1.50 m \
             behind them at 0.6 of the light (debug.fosfora.skydepth 1.00)"
        );
        let three = at(3, 1.0).line();
        assert!(
            three.contains("layers 1.50 and 4.00 m behind them at 0.6 and 0.35 of the light"),
            "{three}"
        );
        let change = at(1, 1.0).change_line(&at(3, 1.0));
        assert!(
            change.starts_with(
                "sky layers: 1 (debug.fosfora.skylayers), the sky on the surfaces alone"
            ),
            "{change}"
        );
        assert!(
            change.contains("(was: 3 (debug.fosfora.skylayers)"),
            "{change}"
        );
    }
}
