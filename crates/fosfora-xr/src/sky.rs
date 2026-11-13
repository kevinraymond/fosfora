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
//! **The rim** ([`join`]). The surfaces pass fades a face out over its
//! last 5 cm. On the sky's faces the fade is dropped on every edge another
//! of the sky's faces continues (two walls in a corner, a wall under the
//! ceiling), so no dark line runs down a corner, and the quad is cut back
//! to its neighbor's, so no bright line runs there either where the two
//! would overlap; an edge with nothing beyond it keeps the fade.

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

/// Where the sky sits in the port block: `mode.y` (the row of
/// `surface_port::rows`' overlay flag) carries the [`Map`]; the four rows
/// after that row are the sky's own: the rim per edge, the point (and the
/// ceiling), the room in plan, the room's axis and the facing.
pub const MODE_ROW: usize = 7;
pub const EDGES_ROW: usize = 8;
pub const POINT_ROW: usize = 9;
pub const ROOM_ROW: usize = 10;
pub const AXES_ROW: usize = 11;

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

/// The sky for this frame: the room, its point and the facing the seam is
/// behind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sky {
    pub room: Room,
    pub point: Vec3,
    pub facing: Vec2,
}

impl Sky {
    pub fn new(room: Room, facing: Vec2) -> Self {
        Self {
            room,
            point: room.point(),
            facing,
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
    let across = Vec2::new(-room.axis.y, room.axis.x);
    let l = Vec2::new(p.x, p.z) - room.center;
    let mp = Vec2::new(m.x, m.z);
    let ts = exit_along(l.dot(room.axis), mp.dot(room.axis), room.half.x);
    let tt = exit_along(l.dot(across), mp.dot(across), room.half.y);
    let ty = (room.ceiling_y - p.y) / m.y.max(1e-6);
    p + m * ts.min(tt).min(ty).max(0.0)
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

/// What `fs_sky` does at one fragment: the frame coordinates it calls the
/// effect at and how it weighs them.
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

/// The fragment of a face with port block `rows` and the effect's
/// `resolution` at face point `uv` (m), world point `world` (the lifted
/// quad's), seen from `eye`: what `fs_sky` calls the effect with. `None`
/// where it writes nothing without calling it (no light left).
pub fn sample(
    rows: &[[f32; 4]; UNIFORM_ROWS],
    resolution: [f32; 2],
    uv: Vec2,
    world: Vec3,
    eye: Vec3,
) -> Option<Sample> {
    let half = [rows[4][0], rows[4][1]];
    let mut k = rows[6][3] * rows[5][0].clamp(0.0, 1.0) * rim(uv, half, rows[EDGES_ROW]);
    let map = rows[MODE_ROW][1];
    if map < 0.5 {
        let n = uv / Vec2::from(half) * 0.5 + 0.5;
        return Some(Sample {
            at: Vec2::new(n.x, 1.0 - n.y) * Vec2::from(resolution),
            other: None,
            k,
        });
    }
    let point = Vec3::from_slice(&rows[POINT_ROW][..3]);
    let facing = Vec2::new(rows[AXES_ROW][2], rows[AXES_ROW][3]);
    let mut d = world - point;
    let reflection = map > 1.5;
    if reflection {
        let room = Room {
            center: Vec2::new(rows[ROOM_ROW][0], rows[ROOM_ROW][1]),
            axis: Vec2::new(rows[AXES_ROW][0], rows[AXES_ROW][1]),
            half: Vec2::new(rows[ROOM_ROW][2], rows[ROOM_ROW][3]),
            floor_y: 0.0,
            ceiling_y: rows[POINT_ROW][3],
            walls: 0,
        };
        let (m, fade) = reflect(eye, world);
        d = exit(&room, world, m) - point;
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

/// `rows` (`surface_port::rows`) for a face mapped by `map` under `sky`,
/// its rim fading on the edges `fade` marks (-u, +u, -v, +v). A face
/// mapped per face keeps the rows as they are (every edge fading).
pub fn rows(
    mut rows: [[f32; 4]; UNIFORM_ROWS],
    map: Map,
    sky: &Sky,
    fade: [bool; 4],
) -> [[f32; 4]; UNIFORM_ROWS] {
    if map == Map::Face {
        return rows;
    }
    let (p, room) = (sky.point, sky.room);
    rows[MODE_ROW][1] = map.code();
    rows[EDGES_ROW] = fade.map(|f| f32::from(u8::from(f)));
    rows[POINT_ROW] = [p.x, p.y, p.z, room.ceiling_y];
    rows[ROOM_ROW] = [room.center.x, room.center.y, room.half.x, room.half.y];
    rows[AXES_ROW] = [room.axis.x, room.axis.y, sky.facing.x, sky.facing.y];
    rows
}

/// `faces` (as drawn, `surface_port::port_face`) joined as one sky. An edge
/// another of them continues (its middle within [`NEIGHBOR_M`] of another
/// face: two walls in a corner, a wall's top under the ceiling) runs to
/// that neighbor without the rim fade, cut back to the neighbor's drawn
/// plane: the quads are lifted [`LIFT_M`] off their faces and run on into
/// each other's slabs, so uncut, two would draw the same sky twice along
/// the corner, a bright line. Every other edge keeps the face's extent and
/// the fade. Each face as drawn, with its rim per edge (-u, +u, -v, +v;
/// true fades).
pub fn join(faces: &[Face]) -> Vec<(Face, [bool; 4])> {
    faces
        .iter()
        .enumerate()
        .map(|(i, f)| {
            // The quad's reach from its center along each axis, minus and
            // plus.
            let mut reach = [[f.half[0]; 2], [f.half[1]; 2]];
            let mut fade = [true; 4];
            let lifted = f.center + f.normal * LIFT_M;
            for (e, faded) in fade.iter_mut().enumerate() {
                let (axis, side) = (e / 2, e % 2);
                let dir = if side == 1 {
                    f.axes[axis]
                } else {
                    -f.axes[axis]
                };
                let mid = f.center + dir * f.half[axis];
                let neighbor = faces
                    .iter()
                    .enumerate()
                    .filter(|&(j, _)| j != i)
                    .map(|(_, g)| (g, g.nearest(mid).distance(mid)))
                    .filter(|&(_, d)| d <= NEIGHBOR_M)
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                let Some((g, _)) = neighbor else {
                    continue;
                };
                *faded = false;
                // Where the quad's axis meets the neighbor's drawn plane: a
                // neighbor across the edge (a corner), not one beside it in
                // the same plane.
                let across = g.normal.dot(dir);
                if across.abs() > 0.5 {
                    let t = g.normal.dot(g.center + g.normal * LIFT_M - lifted) / across;
                    if t > 0.0 && t < f.half[axis] + NEIGHBOR_M {
                        reach[axis][side] = t;
                    }
                }
            }
            let mut out = *f;
            for (axis, [minus, plus]) in reach.into_iter().enumerate() {
                out.center += f.axes[axis] * 0.5 * (plus - minus);
                out.half[axis] = 0.5 * (plus + minus);
            }
            (out, fade)
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
/// (0 per face as `fs_main` does, 1 the sky, 2 its reflection). The Rust
/// copies are [`angles`], [`frame`], [`seam_weight`], [`zenith_fade`],
/// [`reflect`], [`exit`], [`rim`] and [`sample`].
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
        // Every pair of faces that meet: the line where their lifted
        // quads cross, and a point 0.5 mm into each from it (0.7 mm apart
        // across a right angle).
        let mut seams = 0;
        for (i, a) in faces.iter().enumerate() {
            for b in &faces[i + 1..] {
                let along = a.normal.cross(b.normal);
                if along.length() < 0.5 {
                    continue;
                }
                let along = along.normalize();
                // A point on both lifted planes, near the room's middle.
                let (pa, pb) = (a.center + a.normal * LIFT_M, b.center + b.normal * LIFT_M);
                let m = Mat3::from_cols(a.normal, b.normal, along).transpose();
                let x = m.inverse()
                    * Vec3::new(a.normal.dot(pa), b.normal.dot(pb), along.dot(sky.point));
                for s in [-0.6, -0.2, 0.0, 0.3, 0.7] {
                    let q = x + along * s;
                    // Into each face: away from the other's plane, along it.
                    let into_a = (b.normal - a.normal * a.normal.dot(b.normal)).normalize();
                    let into_b = (a.normal - b.normal * b.normal.dot(a.normal)).normalize();
                    let (qa, qb) = (q + into_a * 5e-4, q + into_b * 5e-4);
                    let (ua, ub) = (uv_of(a, qa), uv_of(b, qb));
                    // Both on their quads, clear of the far ends.
                    let inside = |f: &Face, uv: Vec2| {
                        uv.x.abs() < f.half[0] + 0.03 && uv.y.abs() < f.half[1] + 0.03
                    };
                    if !inside(a, ua) || !inside(b, ub) {
                        continue;
                    }
                    let gap = at(qa).distance(at(qb));
                    assert!(gap < 1.0, "{gap} px across {a:?} / {b:?}");
                    // Per face, two walls' points land at the frame's two
                    // ends.
                    if a.normal.y.abs() < 0.5 && b.normal.y.abs() < 0.5 {
                        let per_face = |f: &Face, uv: Vec2| {
                            let res = crate::surface_port::resolution(f.half);
                            crate::surface_port::frame_coord(uv, f.half, res) / Vec2::from(res)
                        };
                        let jump = (per_face(a, ua).x - per_face(b, ub).x).abs();
                        assert!(jump > 0.9, "{jump} of the frame");
                    }
                    seams += 1;
                }
            }
        }
        // Four corners and four wall-to-ceiling edges, sampled.
        assert!(seams >= 8 * 3, "{seams}");
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
        for (f, (_, fade)) in faces.iter().zip(&joined) {
            if f.normal.y.abs() > 0.9 {
                // The ceiling: a wall along each edge.
                assert_eq!(*fade, [false; 4], "{f:?}");
            } else {
                // A wall: its neighbors left and right, the ceiling over
                // it, nothing under it (the floor is not the sky).
                assert_eq!(*fade, [false, false, true, false], "{f:?}");
            }
        }
        // A hidden wall is not drawn: its two neighbors and the ceiling
        // keep the rim on that side.
        let gone = faces.iter().position(|f| f.normal.y.abs() < 0.9).unwrap();
        let mut open = faces.clone();
        let hidden = open.remove(gone);
        let flags: Vec<[bool; 4]> = join(&open).into_iter().map(|(_, f)| f).collect();
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

    #[test]
    fn joined_faces_meet_on_one_line_and_draw_no_strip_twice() {
        let boxes = box_room(Vec2::new(0.4, -0.3), 0.35, 4.6, 3.8, 2.45);
        let head = Vec3::new(0.6, 1.2, 0.0);
        let faces = sky_faces(&boxes, head);
        let joined = join(&faces);
        let plane = |f: &Face| (f.normal, f.center + f.normal * LIFT_M);
        let corners = |f: &Face| f.corners(LIFT_M);
        for (i, (f, fade)) in joined.iter().enumerate() {
            let before = faces[i];
            // Still in its plane, with its axes, no larger.
            assert!(f.normal.abs_diff_eq(before.normal, 1e-6));
            assert!((f.center - before.center).dot(before.normal).abs() < 1e-5);
            assert!(f.half[0] <= before.half[0] + 1e-6 && f.half[1] <= before.half[1] + 1e-6);
            // Each cut edge lies in its neighbor's drawn plane: both of its
            // corners do, for some other face. An uncut edge stays on its
            // line.
            let c = corners(f);
            let ends = [[c[0], c[3]], [c[1], c[2]], [c[0], c[1]], [c[3], c[2]]];
            for e in 0..4 {
                if fade[e] {
                    let (axis, sign) = (e / 2, if e % 2 == 1 { 1.0 } else { -1.0 });
                    for q in ends[e] {
                        let along = (q - before.center).dot(before.axes[axis]);
                        assert!(
                            (along - sign * before.half[axis]).abs() < 1e-5,
                            "{i} edge {e}"
                        );
                    }
                    continue;
                }
                let on_a_neighbor = joined.iter().enumerate().any(|(j, (g, _))| {
                    let (n, p) = plane(g);
                    j != i && ends[e].iter().all(|q| (*q - p).dot(n).abs() < 1e-4)
                });
                assert!(on_a_neighbor, "face {i} edge {e}: {:?}", ends[e]);
            }
        }
        // A wall's top was cut down by the ceiling's slab and lift (2 cm
        // and 1 cm) and its sides in by its neighbors' (3 cm each).
        let wall = joined
            .iter()
            .find(|(f, _)| f.normal.y.abs() < 0.5)
            .unwrap()
            .0;
        let before = faces.iter().find(|f| f.normal.y.abs() < 0.5).unwrap();
        let t = PLANE_HALF_THICKNESS_M;
        assert!((before.half[0] - wall.half[0] - (t + LIFT_M)).abs() < 1e-4);
        assert!((before.half[1] - wall.half[1] - 0.5 * (t + LIFT_M)).abs() < 1e-4);
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
        assert_close!(r[MODE_ROW], [0.0, 1.0, 0.0, 0.0]);
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
            [at("mode"), at("edges"), at("sky"), at("room"), at("axes")],
            [MODE_ROW, EDGES_ROW, POINT_ROW, ROOM_ROW, AXES_ROW]
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
}
