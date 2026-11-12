//! Floor footprints of things (board #5403): where the things that are not
//! flat stand on the floor, kept as a small, stable set of rectangles in
//! world space. A chair, a pile of boxes, a person or a pet fits no plane,
//! and a box fitted around one is wrong everywhere above the floor. Where
//! it stands can be measured from any viewpoint. Plain numbers in (glam
//! only), so it builds and tests on the desktop. It takes what the plane
//! finder takes (`live_planes::Observation`), the planes it reported and
//! the floor's height, once per update.
//!
//! **Which points count.** A point is bare floor within [`ON_FLOOR_M`] of
//! the floor's height. It is stuff from [`LOW_M`] to [`HIGH_M`] above it
//! (both bands widened by the noise's vertical part), farther than
//! [`SELF_M`] (level) from the camera, within [`MAX_RANGE_M`], and not on
//! a reported plane: within [`ON_PLANE_M`] (or 3 noise deviations) of its
//! box and inside its rectangle grown by a cell. Planes win. Left out
//! first: blends of two depths at a silhouette (see [`JUMP_FRAC`]), walls
//! (an upright surface reaching [`TALL_M`], followed down its column), and
//! stuff with no neighbor near it in the grid ([`SUPPORT_CELLS`]). A stuff
//! point stands for the level segment half way to its neighbors left and
//! right on its surface; on a top (a level neighbor above or below and one
//! along the row) also for the strip of the top its grid cell sees, toward
//! and away from the camera, so a top seen in one row from a low eye still
//! gives its depth. Bare floor is spread the same way, so far floor between
//! sparse rows is seen too.
//!
//! **The floor grid.** Sparse cells of [`CELL_M`] in world x and z, made
//! by stuff. A cell keeps its occupancy score, its lowest and highest
//! stuff, its extreme samples in eight directions and a probe (a stuff
//! point it held). The score rises by the update's time step while stuff
//! lands in it, up to [`SCORE_MAX_S`]; the cell is occupied at [`ON_S`]
//! (the second update at 4 Hz), or at once beside an occupied cell near a
//! moving footprint. Stuff in a cell in an update wins over anything else
//! in it. Without stuff, a cell is free at once when a ray reached the
//! floor in it lower than its lowest stuff (the floor under a chair's seat
//! says nothing about the seat), or when the map is farther than its probe
//! on all four grid cells around it (seen through). Its score falls by the
//! time step when points on a plane land in it (a thing found later as
//! planes leaves), or when the map around its probe is farther on some
//! grid cells and on it on none (a blend that got through hangs in the
//! air). A cell not observed keeps its state. Cells farther than
//! [`FORGET_M`] are dropped.
//!
//! **Groups.** Occupied cells joined to a neighbor (8-neighbor, or one
//! cell over) when their samples come within [`JOIN_M`] and their height
//! ranges within [`STEP_M`], grown only from cells with stuff below
//! [`TOUCH_M`]: a hanging lamp or a wall shelf stands on nothing, and a
//! shelf beside a low cabinet stays out of its footprint. A rectangle is
//! the minimum-area one over yaw of the cells' extreme samples (every
//! point a sample, so a turned face does not grow the corners a box around
//! its samples would), keeping the previous yaw unless another is
//! [`YAW_KEEP`] smaller.
//!
//! **Tracking.** A group follows the footprint whose estimate, moved by
//! its velocity and grown by [`MATCH_M`], holds most of its cells. A
//! footprint is reported once it had a group of [`MIN_AREA_M2`] (its short
//! side counted as at least [`MIN_DEPTH_M`]) in [`APPEAR_FRAC`] of its
//! updates over [`APPEAR_S`]. A reported footprint not moving fits only
//! its cells seen for [`MATURE_S`], and its rectangle follows that estimate
//! inside dead bands and rate limits like the planes': an edge moves only
//! past [`EDGE_DEADBAND_M`], at most [`EDGE_STILL_M_S`] while the camera
//! is still. It has moved when its estimate is more than [`MOVE_M`] from
//! it and neither rectangle holds [`HOLD_FRAC`] of the other, or when its
//! estimate's smoothed velocity passes [`MOVE_SPEED_M_S`] and it has left
//! by half of that: noise jitters an estimate both ways and averages out,
//! a growth or a shrink keeps one rectangle inside the other. Moving, it
//! follows fast while the estimate keeps going and [`MOVE_HOLD_S`] after;
//! the cells it left are cleared by bare floor or by being seen through,
//! so no trail is left. A footprint with no group for [`VANISH_S`] is
//! dropped; out of view its cells keep their state, so it is kept. Ids
//! are never reused.
//!
//! **Budget.** At most [`FootprintsConfig::max_footprints`] are reported.
//! Over that, the nearest small neighbors are merged first (within
//! [`MERGE_GAP_M`], last update's pairs favored), then the rest are ranked
//! by area and nearness with the reported ones favored.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};
use std::hash::{BuildHasherDefault, Hasher};

use glam::{Mat3, Quat, Vec2, Vec3};

use crate::live_planes::{LivePlane, Observation, PlaneBox};
use crate::math::Fov;
use crate::surfaces::{KIND_FLOOR, KIND_OTHER};

// ---- Which points count ------------------------------------------------

/// A point is stuff from this high above the floor (m), plus 3 noise
/// deviations' vertical part (the noise runs along the ray), so a patch of
/// far floor the noise lifts is not a thing...
pub const LOW_M: f32 = 0.08;
/// ...to this high.
pub const HIGH_M: f32 = 1.8;
/// A point within this level distance of the camera is the wearer's own
/// legs, feet or hands, never stuff (m).
pub const SELF_M: f32 = 0.5;
/// Points farther than this from the camera are ignored (m).
pub const MAX_RANGE_M: f32 = 4.5;
/// A point within this of a reported plane's box, or 3 noise deviations
/// if more, and inside its rectangle grown by a cell, is the plane's (m).
pub const ON_PLANE_M: f32 = 0.06;
/// A point within this of the floor's height is bare floor (m), plus the
/// same noise allowance.
pub const ON_FLOOR_M: f32 = 0.06;
/// A point this high above the floor on an upright surface is a wall's,
/// a wardrobe's or a curtain's, and so is every point below it in the
/// grid's column on its vertical plane (see `walls`): such points are not
/// stuff, and their cells are tall and never in a footprint (m). Nothing
/// a footprint is for is this tall, and planes take walls; this keeps
/// walls out before the planes find them.
pub const TALL_M: f32 = 2.0;
/// The noise the thresholds assume: one deviation along the ray is this
/// times the squared range (m⁻¹), 3 cm at 3.5 m as measured on the
/// headset (far less nearer), never under [`NOISE_MIN_M`].
pub const NOISE_K: f32 = 0.0025;
pub const NOISE_MIN_M: f32 = 0.005;
/// A cell holding a blend of two depths at a silhouette is left out. Its
/// neighbors above and below, or left and right, differ in range by more
/// than twice [`JUMP_FRAC_ACROSS`] of its range plus 3 noise deviations,
/// or twice this fraction for a point near the floor above and below (a
/// floor seen at a grazing angle changes this much from row to row); its
/// range lies between theirs by more than 3 deviations, at least
/// [`MIXED_MIN_M`]; and its neighbors the other way do not agree with it
/// (within [`AGREE_FRAC`] of its range plus 3 deviations): for a jump
/// above and below, neither of them; for one left and right, not both.
/// The last keeps a top seen in a single row from a low eye, which lies
/// between the face below it and the room behind it just like a blend,
/// but runs along the row.
pub const JUMP_FRAC: f32 = 0.12;
pub const JUMP_FRAC_ACROSS: f32 = 0.03;
pub const MIXED_MIN_M: f32 = 0.02;
pub const AGREE_FRAC: f32 = 0.03;
/// A point's surface, from its neighbors above and below in the grid
/// within [`NEIGHBOR_M`]: flat when one rises at most this much per meter
/// across...
pub const FLAT_SLOPE: f32 = 0.27;
/// ...steep when one lies within this much per meter of rise straight
/// above or below. A point with a flat neighbor, no steep one and a
/// neighbor left or right agreeing in range (a top runs along the row) is
/// on a top. Blends are no one's neighbors.
pub const STEEP_SLOPE: f32 = 0.36;
pub const NEIGHBOR_M: f32 = 0.5;
/// A surface is upright for the wall test when its normal is within this
/// of level (degrees)...
pub const UPRIGHT_DEG: f32 = 15.0;
/// ...and a point below one is on its plane within 3 noise deviations,
/// never under this (m).
pub const WALL_TOL_M: f32 = 0.05;
/// A point on a top stands for the strip of the top its grid cell sees,
/// between where the cell's upper and lower edges meet the top's height,
/// never past the points of the grid cells above and below, at most this
/// far from the point each way (m)...
pub const SPREAD_MAX_M: f32 = 0.25;
/// ...and at a top's silhouette, where nothing past it bounds the strip,
/// at most this (m).
pub const SILHOUETTE_MAX_M: f32 = 0.10;

// ---- The floor grid ----------------------------------------------------

/// A stuff point needs a neighbor in the grid (not a blend) within this
/// many grid cells' width at its range, at least [`SUPPORT_MIN_M`].
pub const SUPPORT_CELLS: f32 = 2.0;
pub const SUPPORT_MIN_M: f32 = 0.04;

/// The floor grid's cell (m).
pub const CELL_M: f32 = 0.10;
/// A cell becomes occupied when its score (seconds of updates with stuff
/// in it) reaches this (the second update at 4 Hz), or at once beside an
/// occupied cell within [`GROW_NEAR_M`] of a footprint moving at the last
/// update: its leading edge. Anywhere else that would let a silhouette's
/// blends join a thing.
pub const ON_S: f32 = 0.3;
pub const GROW_NEAR_M: f32 = 0.4;
/// A footprint not moving fits its rectangle to the cells whose score has
/// reached this (s; three updates at 4 Hz), when it has any.
pub const MATURE_S: f32 = 0.7;
/// The score's ceiling (s): a cell on a plane's points only, losing the
/// score at the update's time step, is free after this long.
pub const SCORE_MAX_S: f32 = 1.0;
/// A cell's lowest and highest stuff heights follow what it sees; an old
/// extreme relaxes toward the new ones at this rate (m/s).
pub const RELAX_M_S: f32 = 0.5;
/// Bare floor frees a cell when the ray that reached it crossed the cell
/// (taken a cell's width before the floor) above the cell's lowest stuff
/// less this (m).
pub const BARE_MARGIN_M: f32 = 0.05;
/// A cell's probe is seen through when the map's range on the four grid
/// cells around it is farther than the probe's by this, or 3 noise
/// deviations if more (m).
pub const SEE_TOL_M: f32 = 0.08;
/// Cells farther than this (level) from the camera are dropped (m).
pub const FORGET_M: f32 = 8.0;
/// A free cell with no stuff for this long is recycled (s).
pub const RECYCLE_S: f32 = 5.0;
/// Cells room is kept for from the start.
pub const CELLS_RESERVED: usize = 4096;

// ---- Groups ------------------------------------------------------------

/// A group is a footprint only when it grew from cells with stuff lower
/// than this above the floor (m).
pub const TOUCH_M: f32 = 0.4;
/// Two neighbor cells join when their stuff height ranges come within
/// this (m)...
pub const STEP_M: f32 = 0.2;
/// ...and their stuff samples within this (m): 8-neighbors, or cells one
/// over across an empty one.
pub const JOIN_M: f32 = 0.15;
/// A footprint is reported only with a rectangle at least this big (m²),
/// its short side counted as at least [`MIN_DEPTH_M`]: a thing seen from
/// one side shows only its near surface (a person seen from the front is
/// a thin arc), and anything standing has that much behind it. A table's
/// leg is still far too small.
pub const MIN_AREA_M2: f32 = 0.04;
pub const MIN_DEPTH_M: f32 = 0.15;
/// The rectangle keeps the previous yaw unless the best is this fraction
/// smaller, so a square thing does not spin.
pub const YAW_KEEP: f32 = 0.1;

// ---- Tracking ----------------------------------------------------------

/// A group follows the footprint whose estimate, grown by this (m), holds
/// most of its cells.
pub const MATCH_M: f32 = 0.15;
/// A second group with at least this fraction of its cells on a footprint
/// that already has one is a piece of it.
pub const FRAGMENT_FRAC: f32 = 0.5;
/// A moving footprint that found no group takes one no footprint took
/// within this of where its velocity puts it, plus its motion in the
/// update (m).
pub const REACQUIRE_M: f32 = 0.3;
/// A footprint without a group of its own, with at least this fraction of
/// its last cells in another footprint's group, has merged into it.
pub const ABSORB_FRAC: f32 = 0.5;
/// A footprint is reported once it has had a group of [`MIN_AREA_M2`] in
/// [`APPEAR_FRAC`] of its updates over this long (s).
pub const APPEAR_S: f32 = 0.5;
pub const APPEAR_FRAC: f32 = 0.7;
/// A footprint not yet reported with no group for this long is dropped (s).
pub const CANDIDATE_TTL_S: f32 = 0.5;
/// A reported footprint with no group for this long is dropped (s).
pub const VANISH_S: f32 = 0.5;
/// The time step of the first update (s), and the least and most any
/// update counts for.
pub const NOMINAL_DT_S: f32 = 0.25;
pub const MIN_DT_S: f32 = 0.02;
pub const MAX_DT_S: f32 = 1.0;
/// A footprint has moved when its estimate's center is more than this
/// from the reported one (m; a cell flickering at an edge moves the center
/// half a cell) and neither rectangle has [`HOLD_FRAC`] of the other
/// inside it (a growth or a shrink keeps one inside the other, and follows
/// at the still limits below). It then follows fast while it keeps going
/// and [`MOVE_HOLD_S`] after: center within [`MOVE_DEADBAND_M`] at gain
/// [`MOVE_GAIN`] up to [`MOVE_MAX_M_S`], size up to [`RESIZE_MAX_M_S`],
/// yaw up to [`TURN_FAST_DEG_S`].
pub const MOVE_M: f32 = 0.15;
pub const MOVE_HOLD_S: f32 = 0.5;
/// A footprint whose estimate's center moves faster than this (m/s, its
/// velocity smoothed over [`SPEED_TAU_S`]) and has left by half of
/// [`MOVE_M`] has moved too, and a moving one keeps following fast while it
/// does.
pub const MOVE_SPEED_M_S: f32 = 0.2;
pub const SPEED_TAU_S: f32 = 0.5;
pub const MOVE_DEADBAND_M: f32 = 0.02;
pub const MOVE_GAIN: f32 = 8.0;
pub const MOVE_MAX_M_S: f32 = 2.0;
pub const RESIZE_MAX_M_S: f32 = 1.0;
pub const TURN_FAST_DEG_S: f32 = 90.0;
pub const HOLD_FRAC: f32 = 0.75;
/// Still, a reported edge moves only past this dead band (m), at most
/// [`EDGE_STILL_M_S`] while the camera is still and [`EDGE_MAX_M_S`] while
/// it moves (faster than [`MOVING_M_S`] or [`MOVING_DEG_S`]).
pub const EDGE_DEADBAND_M: f32 = 0.12;
pub const EDGE_STILL_M_S: f32 = 0.008;
pub const EDGE_MAX_M_S: f32 = 0.5;
/// Still, the reported yaw turns only past this dead band (degrees), at
/// most [`TURN_STILL_DEG_S`] while the camera is still and
/// [`TURN_MAX_DEG_S`] while it moves.
pub const TURN_DEADBAND_DEG: f32 = 3.0;
pub const TURN_STILL_DEG_S: f32 = 0.5;
pub const TURN_MAX_DEG_S: f32 = 10.0;
pub const MOVING_M_S: f32 = 0.1;
pub const MOVING_DEG_S: f32 = 5.0;
/// Past its dead band, a still reported value closes this fraction of the
/// gap per second (as `1 - exp(-gain * dt)`), within its rate limit.
pub const FOLLOW_GAIN: f32 = 4.0;
/// Groups for the confidence to reach 1 - 1/e.
pub const CONF_HITS: f32 = 3.0;

// ---- Budget and output -------------------------------------------------

/// Footprints reported at most by default.
pub const MAX_FOOTPRINTS: usize = 8;
/// Footprints tracked at most, reported or not.
pub const MAX_TRACKED: usize = 32;
/// Over the budget, two footprints merge only when their rectangles come
/// within this (m); last update's pairs cost [`MERGE_KEEP`] times as much.
pub const MERGE_GAP_M: f32 = 1.0;
pub const MERGE_KEEP: f32 = 0.5;
/// Still over, a reported footprint's score is raised by this fraction.
pub const KEEP_BONUS: f32 = 1.0;
/// The height of a footprint's box (m).
pub const CURB_M: f32 = 0.12;
/// The first eight bytes of every footprint's UUID: `fsfoot` and two zero
/// bytes, then the id as a big-endian u64. Not `live_planes`' `fslive`, so
/// never a plane; the zero version nibble (byte 6) and variant bits
/// (byte 8) are a layout no runtime's anchor UUID has; never all zero, so
/// never the stage floor.
pub const UUID_PREFIX: [u8; 8] = *b"fsfoot\0\0";

/// The noise the thresholds assume at range `r` (m): one deviation.
fn noise_sigma(k: f32, r: f32) -> f32 {
    (k * r * r).max(NOISE_MIN_M)
}

// ---- Config, input, output ----------------------------------------------

/// The thresholds, so they can be tuned against recordings of the
/// headset's depth. [`Default`] is the constants above.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FootprintsConfig {
    pub low_m: f32,
    pub high_m: f32,
    pub self_m: f32,
    pub max_range_m: f32,
    pub on_plane_m: f32,
    pub on_floor_m: f32,
    pub tall_m: f32,
    pub noise_k: f32,
    pub see_tol_m: f32,
    pub cell_m: f32,
    pub on_s: f32,
    pub forget_m: f32,
    pub touch_m: f32,
    pub join_m: f32,
    pub min_area_m2: f32,
    pub appear_s: f32,
    pub vanish_s: f32,
    pub move_m: f32,
    pub edge_deadband_m: f32,
    pub max_footprints: usize,
}

impl Default for FootprintsConfig {
    fn default() -> Self {
        Self {
            low_m: LOW_M,
            high_m: HIGH_M,
            self_m: SELF_M,
            max_range_m: MAX_RANGE_M,
            on_plane_m: ON_PLANE_M,
            on_floor_m: ON_FLOOR_M,
            tall_m: TALL_M,
            noise_k: NOISE_K,
            see_tol_m: SEE_TOL_M,
            cell_m: CELL_M,
            on_s: ON_S,
            forget_m: FORGET_M,
            touch_m: TOUCH_M,
            join_m: JOIN_M,
            min_area_m2: MIN_AREA_M2,
            appear_s: APPEAR_S,
            vanish_s: VANISH_S,
            move_m: MOVE_M,
            edge_deadband_m: EDGE_DEADBAND_M,
            max_footprints: MAX_FOOTPRINTS,
        }
    }
}

/// A reported plane, as the footprints take it: an oriented box. Points
/// on it are its, not a thing's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub center: Vec3,
    /// Box to world.
    pub rot: Quat,
    pub half: Vec3,
    /// `surfaces::KIND_*`; the floor's planes explain nothing (bare floor
    /// is the floor's height's).
    pub kind: u32,
}

impl Plane {
    pub fn from_live(p: &LivePlane) -> Self {
        Self {
            center: p.center,
            rot: p.rot,
            half: p.half,
            kind: p.kind,
        }
    }

    pub fn from_box(b: &PlaneBox) -> Self {
        Self {
            center: Vec3::from_array(b.center),
            rot: Quat::from_array(b.rot),
            half: Vec3::from_array(b.half),
            kind: b.kind,
        }
    }
}

/// A reported footprint: a rectangle on the floor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Footprint {
    /// Unique in a session, never reused.
    pub id: u32,
    /// The rectangle's center, at the floor's height.
    pub center: Vec3,
    /// The rectangle's turn about +Y (rad), as `Quat::from_rotation_y`:
    /// local X is `(cos, 0, -sin)`, local Z `(sin, 0, cos)`. In
    /// `[-45°, 45°)`.
    pub yaw: f32,
    /// Half extents along local X and local Z (m).
    pub half: Vec2,
    /// The highest stuff seen on it, above the floor (m).
    pub height: f32,
    /// 0 to 1: rises with repeated groups, falls while it has none.
    pub confidence: f32,
    /// Moving: its estimate left where it was reported, within the last
    /// [`MOVE_HOLD_S`].
    pub moving: bool,
    /// When it was first reported (s).
    pub since_s: f32,
}

impl Footprint {
    /// Box to world.
    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw)
    }

    /// Its four corners on the floor.
    pub fn corners(&self) -> [Vec3; 4] {
        let r = self.rect();
        r.corners().map(|c| Vec3::new(c.x, self.center.y, c.y))
    }

    fn rect(&self) -> Rect2 {
        Rect2 {
            c: Vec2::new(self.center.x, self.center.z),
            yaw: self.yaw,
            half: self.half,
        }
    }
}

/// A footprint as an obstacle box (the fields of `particles3d::ObstacleBox`
/// in `live_planes::PlaneBox`): [`CURB_M`] tall, standing on the floor, kind
/// other, an emitter like a room box, never hidden, with [`footprint_uuid`].
pub fn to_obstacle_box(f: &Footprint) -> PlaneBox {
    let h = 0.5 * CURB_M;
    PlaneBox {
        center: [f.center.x, f.center.y + h, f.center.z],
        rot: f.rotation().to_array(),
        half: [f.half.x, h, f.half.y],
        kind: KIND_OTHER,
        emit: 1.0,
        hidden: false,
        uuid: footprint_uuid(f.id),
    }
}

/// The synthetic UUID of footprint `id`: [`UUID_PREFIX`] then the id as a
/// big-endian u64.
pub fn footprint_uuid(id: u32) -> [u8; 16] {
    let mut u = [0; 16];
    u[..8].copy_from_slice(&UUID_PREFIX);
    u[8..].copy_from_slice(&u64::from(id).to_be_bytes());
    u
}

/// The footprint id a UUID carries, `None` for any other UUID.
pub fn footprint_id(uuid: &[u8; 16]) -> Option<u32> {
    if uuid[..8] != UUID_PREFIX || uuid[8..12] != [0; 4] {
        return None;
    }
    Some(u32::from_be_bytes([uuid[12], uuid[13], uuid[14], uuid[15]]))
}

/// What one update did, for a log line.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Stats {
    /// The floor height used, and whether it was the one passed (not the
    /// fallback).
    pub floor_y: f32,
    pub floor_found: bool,
    /// Points: stuff, bare floor, on a plane, the wearer's own, blends left
    /// out, on walls.
    pub stuff: usize,
    pub bare: usize,
    pub on_plane: usize,
    pub own: usize,
    pub mixed: usize,
    /// Points on walls (see [`TALL_M`]), and stuff points left out for
    /// having no neighbor near (see [`SUPPORT_CELLS`]).
    pub wall: usize,
    pub lone: usize,
    /// Cells kept, occupied, and groups grown from the floor.
    pub cells: usize,
    pub occupied: usize,
    pub groups: usize,
    pub tracked: usize,
    pub reported: usize,
    /// Reported footprints that are two or more merged for the budget.
    pub merged: usize,
}

// ---- Geometry ------------------------------------------------------------

/// A fov as tangents, for a grid of `cols` x `rows`.
#[derive(Debug, Clone, Copy)]
struct Lens {
    l: f32,
    r: f32,
    u: f32,
    d: f32,
    cols: f32,
    rows: f32,
}

impl Lens {
    fn new(fov: Fov, cols: usize, rows: usize) -> Self {
        Self {
            l: fov.left.tan(),
            r: fov.right.tan(),
            u: fov.up.tan(),
            d: fov.down.tan(),
            cols: cols as f32,
            rows: rows as f32,
        }
    }

    /// The camera-space ray (z = -1) through grid position (`x`, `y`), in
    /// cells from the left and from the top: (0.5, 0.5) is the middle of
    /// the first cell.
    fn ray(&self, x: f32, y: f32) -> Vec3 {
        Vec3::new(
            self.l + x / self.cols * (self.r - self.l),
            self.u - y / self.rows * (self.u - self.d),
            -1.0,
        )
    }

    /// The grid position of a camera-space point; `None` behind the camera.
    fn project(&self, local: Vec3) -> Option<Vec2> {
        if local.z > -1e-3 {
            return None;
        }
        let x = local.x / -local.z;
        let y = local.y / -local.z;
        Some(Vec2::new(
            (x - self.l) / (self.r - self.l) * self.cols,
            (self.u - y) / (self.u - self.d) * self.rows,
        ))
    }

    /// A cell's width near the middle of the view (radians).
    fn cell_angle(&self) -> f32 {
        ((self.r - self.l) / self.cols).min((self.u - self.d) / self.rows)
    }
}

/// A rectangle on the floor: center (world x, z), yaw as [`Footprint`],
/// half extents along its local X and Z.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect2 {
    c: Vec2,
    yaw: f32,
    half: Vec2,
}

impl Rect2 {
    /// Local X and local Z in (x, z).
    fn axes(&self) -> (Vec2, Vec2) {
        let (s, c) = self.yaw.sin_cos();
        (Vec2::new(c, -s), Vec2::new(s, c))
    }

    fn local(&self, p: Vec2) -> Vec2 {
        let (u, v) = self.axes();
        let d = p - self.c;
        Vec2::new(d.dot(u), d.dot(v))
    }

    fn contains(&self, p: Vec2, margin: f32) -> bool {
        let l = self.local(p);
        l.x.abs() <= self.half.x + margin && l.y.abs() <= self.half.y + margin
    }

    /// Level distance from `p`, 0 inside.
    fn distance(&self, p: Vec2) -> f32 {
        (self.local(p).abs() - self.half).max(Vec2::ZERO).length()
    }

    fn corners(&self) -> [Vec2; 4] {
        let (u, v) = self.axes();
        let (a, b) = (u * self.half.x, v * self.half.y);
        [
            self.c - a - b,
            self.c + a - b,
            self.c + a + b,
            self.c - a + b,
        ]
    }

    fn area(&self) -> f32 {
        4.0 * self.half.x * self.half.y
    }

    /// The area for [`MIN_AREA_M2`]: the short side at least
    /// [`MIN_DEPTH_M`].
    fn presence(&self) -> f32 {
        let (a, b) = (2.0 * self.half.x, 2.0 * self.half.y);
        a.max(b) * a.min(b).max(MIN_DEPTH_M)
    }

    /// The same rectangle with its yaw in `[-45°, 45°)`.
    fn canonical(mut self) -> Self {
        let k = ((self.yaw + FRAC_PI_4) / FRAC_PI_2).floor();
        self.yaw -= k * FRAC_PI_2;
        if (k as i64).rem_euclid(2) == 1 {
            self.half = Vec2::new(self.half.y, self.half.x);
        }
        self
    }

    /// The same rectangle described with its yaw within 45° of `yaw`.
    fn aligned(mut self, yaw: f32) -> Self {
        let k = ((yaw - self.yaw) / FRAC_PI_2).round();
        self.yaw += k * FRAC_PI_2;
        if (k as i64).rem_euclid(2) == 1 {
            self.half = Vec2::new(self.half.y, self.half.x);
        }
        self
    }

    /// The fraction of a 4x4 grid of points over this rectangle that `o`
    /// holds.
    fn inside_frac(&self, o: &Rect2) -> f32 {
        let (u, v) = self.axes();
        let mut n = 0;
        for i in 0..4 {
            for j in 0..4 {
                let a = (i as f32 + 0.5) / 4.0 * 2.0 - 1.0;
                let b = (j as f32 + 0.5) / 4.0 * 2.0 - 1.0;
                let p = self.c + u * (a * self.half.x) + v * (b * self.half.y);
                n += u32::from(o.contains(p, 0.0));
            }
        }
        n as f32 / 16.0
    }

    /// The level gap between two rectangles (0 when they overlap): the
    /// least distance from a corner of one to the other, exact for two
    /// that do not overlap.
    fn gap(&self, o: &Rect2) -> f32 {
        if self.contains(o.c, 0.0) || o.contains(self.c, 0.0) {
            return 0.0;
        }
        let a = self.corners().map(|p| o.distance(p));
        let b = o.corners().map(|p| self.distance(p));
        a.into_iter().chain(b).fold(f32::INFINITY, f32::min)
    }
}

/// The rectangle of `hull` turned by `yaw`: its extents along the axes.
fn extent(hull: &[Vec2], yaw: f32) -> Rect2 {
    let r = Rect2 {
        c: Vec2::ZERO,
        yaw,
        half: Vec2::ZERO,
    };
    let (u, v) = r.axes();
    let (mut lo, mut hi) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
    for &p in hull {
        let l = Vec2::new(p.dot(u), p.dot(v));
        lo = lo.min(l);
        hi = hi.max(l);
    }
    let mid = 0.5 * (lo + hi);
    Rect2 {
        c: u * mid.x + v * mid.y,
        yaw,
        half: 0.5 * (hi - lo),
    }
}

/// The convex hull of `pts` (sorted in place), counterclockwise, into
/// `out` (Andrew's monotone chain).
fn convex_hull(pts: &mut [Vec2], out: &mut Vec<Vec2>) {
    out.clear();
    pts.sort_unstable_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    if pts.len() <= 2 {
        out.extend_from_slice(pts);
        return;
    }
    let turn = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    for &p in pts.iter() {
        while out.len() >= 2 && turn(out[out.len() - 2], out[out.len() - 1], p) <= 0.0 {
            out.pop();
        }
        out.push(p);
    }
    let lower = out.len() + 1;
    for &p in pts.iter().rev().skip(1) {
        while out.len() >= lower && turn(out[out.len() - 2], out[out.len() - 1], p) <= 0.0 {
            out.pop();
        }
        out.push(p);
    }
    out.pop();
}

/// The minimum-area rectangle over `pts` (reordered), its yaw along one of
/// the hull's edges, or `prev` when that is within [`YAW_KEEP`] of the
/// least area. `None` with no points.
fn fit_rect(pts: &mut [Vec2], hull: &mut Vec<Vec2>, prev: Option<f32>) -> Option<Rect2> {
    if pts.is_empty() {
        return None;
    }
    convex_hull(pts, hull);
    let mut best = extent(hull, prev.unwrap_or(0.0));
    let n = hull.len();
    for k in 0..n {
        let e = hull[(k + 1) % n] - hull[k];
        if e.length_squared() < 1e-12 {
            continue;
        }
        let r = extent(hull, (-e.y).atan2(e.x));
        if r.area() < best.area() {
            best = r;
        }
    }
    if let Some(p) = prev {
        let r = extent(hull, p);
        if r.area() <= best.area() * (1.0 + YAW_KEEP) + 1e-6 {
            best = r;
        }
    }
    Some(best.canonical())
}

/// Move `err` toward zero: nothing inside `deadband`, past it the gap
/// beyond the band closes at `gain`, at most `max_speed`.
fn follow(err: f32, deadband: f32, max_speed: f32, dt: f32, gain: f32) -> f32 {
    let excess = err.abs() - deadband;
    if excess <= 0.0 {
        return 0.0;
    }
    (excess * (1.0 - (-gain * dt).exp()))
        .min(max_speed * dt)
        .copysign(err)
}

/// The near and far ends, on the level plane at `p`'s height, of the
/// strip grid cell (`col`, `row`) sees there: where the rays through the
/// middle of its upper and lower edges meet that plane, as signed
/// distances from `p` along the level direction away from the camera,
/// kept within `limits`. `None` when the camera is not above `p`.
fn strip(
    p: Vec3,
    cam: Vec3,
    rot: Quat,
    lens: &Lens,
    col: usize,
    row: usize,
    limits: (f32, f32),
) -> Option<(f32, f32)> {
    let drop = cam.y - p.y;
    if drop <= 0.05 {
        return None;
    }
    let pxz = Vec2::new(p.x, p.z);
    let away = (pxz - Vec2::new(cam.x, cam.z)).normalize_or_zero();
    if away == Vec2::ZERO {
        return None;
    }
    let x = col as f32 + 0.5;
    let mut ends = [0.0f32; 2];
    for (k, y) in [row as f32, row as f32 + 1.0].into_iter().enumerate() {
        let d = rot * lens.ray(x, y);
        let s = if d.y < -1e-4 {
            let q = cam + d * (-drop / d.y);
            (Vec2::new(q.x, q.z) - pxz).dot(away)
        } else {
            f32::INFINITY
        };
        ends[k] = s.clamp(limits.0, limits.1);
    }
    Some((ends[0].min(ends[1]), ends[0].max(ends[1])))
}

/// The eight level directions a cell keeps its extreme stuff samples in:
/// +x, -x, +z, -z and the diagonals.
const EXTREMES: [Vec2; 8] = [
    Vec2::new(1.0, 0.0),
    Vec2::new(-1.0, 0.0),
    Vec2::new(0.0, 1.0),
    Vec2::new(0.0, -1.0),
    Vec2::new(1.0, 1.0),
    Vec2::new(-1.0, -1.0),
    Vec2::new(1.0, -1.0),
    Vec2::new(-1.0, 1.0),
];

/// A cell's stuff bounds `[x0, x1, z0, z1]` from its extreme samples.
fn bounds_of(ext: &[Vec2; 8]) -> [f32; 4] {
    [ext[1].x, ext[0].x, ext[3].y, ext[2].y]
}

/// The level gap between two cells' stuff bounds `[x0, x1, z0, z1]`.
fn bounds_gap(a: [f32; 4], b: [f32; 4]) -> f32 {
    let dx = (b[0] - a[1]).max(a[0] - b[1]).max(0.0);
    let dz = (b[2] - a[3]).max(a[2] - b[3]).max(0.0);
    (dx * dx + dz * dz).sqrt()
}

// ---- Cells ---------------------------------------------------------------

/// A hasher for cell keys (a splitmix64 finalizer): the std one is slower
/// than the rest of a cell's work.
#[derive(Debug, Default, Clone, Copy)]
struct CellHasher(u64);

impl Hasher for CellHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }

    fn write_u64(&mut self, n: u64) {
        let mut z = (self.0 ^ n).wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        self.0 = z ^ (z >> 31);
    }
}

type CellIndex = HashMap<u64, u32, BuildHasherDefault<CellHasher>>;

fn cell_key(ix: i32, iz: i32) -> u64 {
    (u64::from(ix as u32) << 32) | u64::from(iz as u32)
}

/// One floor cell: its lasting state, then what this update put in it.
#[derive(Debug, Clone, Copy)]
struct Cell {
    ix: i32,
    iz: i32,
    alive: bool,
    /// Seconds of updates with stuff, lowered by points on planes and
    /// cleared by bare floor or a see-through.
    score: f32,
    occupied: bool,
    tall: bool,
    /// The lowest and highest stuff seen recently (world y).
    lo: f32,
    hi: f32,
    /// The stuff samples farthest along each of [`EXTREMES`]: every point
    /// a rectangle is fitted to is a sample, so a turned face does not
    /// grow the corners a box around its samples would.
    ext: [Vec2; 8],
    /// The stuff samples' level bounds `[x0, x1, z0, z1]`, for the join.
    bounds: [f32; 4],
    /// A stuff point it held (a measured one when it had one).
    probe: Vec3,
    has_probe: bool,
    last_stuff_s: f32,
    /// The update the fields below are of.
    stamp: u32,
    n_stuff: u32,
    n_plane: u32,
    /// The highest a ray that reached the floor here crossed it (world y).
    bare_y: f32,
    seen_through: bool,
    /// The probe has the map farther on some of the four grid cells around
    /// it and on none of them.
    in_air: bool,
    saw_tall: bool,
    grow: bool,
    u_lo: f32,
    u_hi: f32,
    u_ext: [Vec2; 8],
    u_probe: Vec3,
    u_probe_measured: bool,
    /// The group pass's mark.
    visit: u32,
}

impl Cell {
    fn new(ix: i32, iz: i32) -> Self {
        Self {
            ix,
            iz,
            alive: true,
            score: 0.0,
            occupied: false,
            tall: false,
            lo: 0.0,
            hi: 0.0,
            bounds: [0.0; 4],
            ext: [Vec2::ZERO; 8],
            probe: Vec3::ZERO,
            has_probe: false,
            last_stuff_s: 0.0,
            stamp: 0,
            n_stuff: 0,
            n_plane: 0,
            bare_y: f32::NEG_INFINITY,
            seen_through: false,
            in_air: false,
            saw_tall: false,
            grow: false,
            u_lo: f32::INFINITY,
            u_hi: f32::NEG_INFINITY,
            u_ext: [Vec2::ZERO; 8],
            u_probe: Vec3::ZERO,
            u_probe_measured: false,
            visit: 0,
        }
    }

    /// Start this update's fields.
    fn start(&mut self, stamp: u32) {
        self.stamp = stamp;
        self.n_stuff = 0;
        self.n_plane = 0;
        self.bare_y = f32::NEG_INFINITY;
        self.seen_through = false;
        self.in_air = false;
        self.saw_tall = false;
        self.grow = false;
        self.u_lo = f32::INFINITY;
        self.u_hi = f32::NEG_INFINITY;

        self.u_probe_measured = false;
    }

    fn center(&self, cell: f32) -> Vec2 {
        Vec2::new((self.ix as f32 + 0.5) * cell, (self.iz as f32 + 0.5) * cell)
    }
}

/// A reported plane ready for the point test: world to box, the box grown
/// by a cell in the plane, and its world bounds with the most any point
/// may lie off it.
#[derive(Debug, Clone, Copy)]
struct PlaneTest {
    center: Vec3,
    to_box: Mat3,
    half: Vec3,
    lo: Vec3,
    hi: Vec3,
}

/// The 8 neighbors, then the 16 cells one over.
const RING: [(i32, i32); 24] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
    (-2, -2),
    (-1, -2),
    (0, -2),
    (1, -2),
    (2, -2),
    (-2, -1),
    (2, -1),
    (-2, 0),
    (2, 0),
    (-2, 1),
    (2, 1),
    (-2, 2),
    (-1, 2),
    (0, 2),
    (1, 2),
    (2, 2),
];

// ---- Tracking ------------------------------------------------------------

/// Occupied cells grown from cells touching the floor: `members[start..start
/// + len]`.
#[derive(Debug, Clone, Copy)]
struct Group {
    start: usize,
    len: usize,
    /// The highest stuff in it (world y).
    hi: f32,
    /// The footprint it went to, if any.
    owner: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct Track {
    id: u32,
    alive: bool,
    /// The latest estimate, from its groups.
    est: Rect2,
    /// The reported rectangle.
    rep: Rect2,
    height: f32,
    appeared: bool,
    reported: bool,
    /// A group this update.
    got: bool,
    first_s: f32,
    last_hit_s: f32,
    /// Updates with a group of [`MIN_AREA_M2`], and updates, while not yet
    /// reported.
    hits: u32,
    updates: u32,
    /// Groups ever, for the confidence.
    groups: u32,
    miss_s: f32,
    fast_until: f32,
    moving: bool,
    /// The estimate's center's level velocity, smoothed over
    /// [`SPEED_TAU_S`] (m/s), and where it was.
    vel: Vec2,
    last_c: Vec2,
    confidence: f32,
    /// Cells in its last group.
    cells: usize,
    since_s: f32,
    /// The id it is reported under (its own, or the one it is merged
    /// into for the budget).
    shown_as: u32,
}

impl Track {
    fn new(id: u32, est: Rect2, now_s: f32) -> Self {
        Self {
            id,
            alive: true,
            est,
            rep: est,
            height: 0.0,
            appeared: false,
            reported: false,
            got: true,
            first_s: now_s,
            last_hit_s: now_s,
            hits: 0,
            updates: 0,
            groups: 0,
            miss_s: 0.0,
            fast_until: f32::NEG_INFINITY,
            moving: false,
            vel: Vec2::ZERO,
            last_c: est.c,
            confidence: 0.0,
            cells: 0,
            since_s: now_s,
            shown_as: id,
        }
    }

    /// Move the reported rectangle toward the estimate.
    fn follow(&mut self, now_s: f32, dt: f32, cam_moving: bool, config: &FootprintsConfig) {
        let est = self.est.aligned(self.rep.yaw);
        let drift = est.c.distance(self.rep.c);
        let speed = self.vel.length();
        // Moved: far, and neither holds the other (a growth or a shrink
        // keeps one inside the other); or its estimate goes one way steadily
        // (the smoothed velocity: noise jitters both ways and averages out)
        // and has left by half as much.
        let started = (drift > config.move_m
            && self.rep.inside_frac(&est) < HOLD_FRAC
            && est.inside_frac(&self.rep) < HOLD_FRAC)
            || (speed > MOVE_SPEED_M_S && drift > 0.5 * config.move_m);
        // Following closely, the drift stays small while the thing still
        // moves: its estimate's own velocity keeps it going.
        let going = now_s < self.fast_until && speed > MOVE_SPEED_M_S;
        if started || going {
            self.fast_until = now_s + MOVE_HOLD_S;
        }
        self.moving = now_s < self.fast_until;
        let rep = &mut self.rep;
        if now_s < self.fast_until {
            let d = est.c - rep.c;
            let len = d.length();
            if len > 0.0 {
                rep.c += d / len * follow(len, MOVE_DEADBAND_M, MOVE_MAX_M_S, dt, MOVE_GAIN);
            }
            for k in 0..2 {
                rep.half[k] += follow(
                    est.half[k] - rep.half[k],
                    MOVE_DEADBAND_M,
                    RESIZE_MAX_M_S,
                    dt,
                    MOVE_GAIN,
                );
            }
            rep.yaw += follow(
                est.yaw - rep.yaw,
                TURN_DEADBAND_DEG.to_radians(),
                TURN_FAST_DEG_S.to_radians(),
                dt,
                MOVE_GAIN,
            );
        } else {
            let (turn, edge) = if cam_moving {
                (TURN_MAX_DEG_S, EDGE_MAX_M_S)
            } else {
                (TURN_STILL_DEG_S, EDGE_STILL_M_S)
            };
            rep.yaw += follow(
                est.yaw - rep.yaw,
                TURN_DEADBAND_DEG.to_radians(),
                turn.to_radians(),
                dt,
                FOLLOW_GAIN,
            );
            let est = est.aligned(rep.yaw);
            let (u, v) = rep.axes();
            let d = est.c - rep.c;
            let (cu, cv) = (d.dot(u), d.dot(v));
            let target = [
                cu - est.half.x,
                cu + est.half.x,
                cv - est.half.y,
                cv + est.half.y,
            ];
            let mut e = [-rep.half.x, rep.half.x, -rep.half.y, rep.half.y];
            for k in 0..4 {
                e[k] += follow(
                    target[k] - e[k],
                    config.edge_deadband_m,
                    edge,
                    dt,
                    FOLLOW_GAIN,
                );
            }
            for k in [0, 2] {
                if e[k + 1] < e[k] {
                    let m = 0.5 * (e[k] + e[k + 1]);
                    e[k] = m;
                    e[k + 1] = m;
                }
            }
            rep.c += u * (0.5 * (e[0] + e[1])) + v * (0.5 * (e[2] + e[3]));
            rep.half = Vec2::new(0.5 * (e[1] - e[0]), 0.5 * (e[3] - e[2]));
        }
        *rep = rep.canonical();
    }
}

/// A reported footprint while the budget is drawn: one track, or several
/// merged.
#[derive(Debug, Clone, Copy)]
struct Entry {
    /// The union-find parent, an index into the entries.
    parent: usize,
    rect: Rect2,
    /// The members' summed area and the largest member's.
    area: f32,
    big: f32,
    id: u32,
    members: usize,
    height: f32,
    confidence: f32,
    moving: bool,
    since_s: f32,
}

// ---- The store -----------------------------------------------------------

/// The footprints: feed it an update's observation, the reported planes
/// and the floor's height; read [`Self::footprints`].
#[derive(Debug)]
pub struct Footprints {
    config: FootprintsConfig,
    next_id: u32,
    stamp: u32,
    last_s: Option<f32>,
    /// The camera's position and forward at the last update.
    last_view: Option<(Vec3, Vec3)>,
    cells: Vec<Cell>,
    index: CellIndex,
    free: Vec<u32>,
    touched: Vec<u32>,
    planes: Vec<PlaneTest>,
    range: Vec<f32>,
    blend: Vec<bool>,
    wall: Vec<bool>,
    /// The moving footprints' estimates at the last update: a cell beside
    /// an occupied one in them is occupied at once.
    fast: Vec<Rect2>,
    stack: Vec<u32>,
    members: Vec<u32>,
    groups: Vec<Group>,
    pts: Vec<Vec2>,
    hull: Vec<Vec2>,
    overlaps: Vec<(usize, usize, usize)>,
    tracks: Vec<Track>,
    entries: Vec<Entry>,
    merged: Vec<(u32, u32)>,
    merged_before: Vec<(u32, u32)>,
    ranked: Vec<(f32, usize)>,
    out: Vec<Footprint>,
    /// What happened to footprints, for the tests' failure messages.
    #[cfg(test)]
    events: Vec<String>,
}

impl Default for Footprints {
    fn default() -> Self {
        Self::new(FootprintsConfig::default())
    }
}

impl Footprints {
    pub fn new(config: FootprintsConfig) -> Self {
        let mut index = CellIndex::default();
        index.reserve(CELLS_RESERVED);
        Self {
            config,
            next_id: 1,
            stamp: 0,
            last_s: None,
            last_view: None,
            cells: Vec::with_capacity(CELLS_RESERVED),
            index,
            free: Vec::with_capacity(CELLS_RESERVED),
            touched: Vec::with_capacity(CELLS_RESERVED),
            planes: Vec::with_capacity(64),
            range: Vec::new(),
            blend: Vec::new(),
            wall: Vec::new(),
            fast: Vec::with_capacity(2 * MAX_TRACKED),
            stack: Vec::with_capacity(CELLS_RESERVED),
            members: Vec::with_capacity(CELLS_RESERVED),
            groups: Vec::with_capacity(256),
            pts: Vec::with_capacity(4 * CELLS_RESERVED),
            hull: Vec::with_capacity(4 * CELLS_RESERVED),
            overlaps: Vec::with_capacity(1024),
            tracks: Vec::with_capacity(2 * MAX_TRACKED),
            entries: Vec::with_capacity(2 * MAX_TRACKED),
            merged: Vec::with_capacity(MAX_TRACKED),
            merged_before: Vec::with_capacity(MAX_TRACKED),
            ranked: Vec::with_capacity(MAX_TRACKED + 1),
            out: Vec::with_capacity(MAX_TRACKED),
            #[cfg(test)]
            events: Vec::new(),
        }
    }

    pub fn config(&self) -> FootprintsConfig {
        self.config
    }

    /// The reported footprints, in id order.
    pub fn footprints(&self) -> &[Footprint] {
        &self.out
    }

    /// Footprints tracked, reported or not.
    pub fn tracked_count(&self) -> usize {
        self.tracks.len()
    }

    /// The remembered footprint nearest `point` (level), in view or not,
    /// and its level distance (0 inside it): the id it is reported under.
    pub fn nearest(&self, point: Vec3) -> Option<(u32, f32)> {
        let p = Vec2::new(point.x, point.z);
        self.tracks
            .iter()
            .filter(|t| t.appeared)
            .map(|t| (t.shown_as, t.rep.distance(p)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// Take one update at `now_s` (seconds, any monotonic clock): the
    /// observation, the floor's height when known (`floor_y`, the plane
    /// finder's `floor_y()`), else `fallback_y` (the caller passes the stage
    /// floor's height: 0 in the floor-level space until the finder has a
    /// floor), and the reported planes.
    pub fn update(
        &mut self,
        now_s: f32,
        obs: &Observation<'_>,
        floor_y: Option<f32>,
        fallback_y: f32,
        planes: &[Plane],
    ) -> Stats {
        let mut stats = Stats::default();
        let n = obs.cols * obs.rows;
        if n == 0 || obs.points.len() != n {
            return stats;
        }
        let dt = self.last_s.map_or(NOMINAL_DT_S, |last| {
            (now_s - last).clamp(MIN_DT_S, MAX_DT_S)
        });
        self.last_s = Some(now_s);
        self.stamp = self.stamp.wrapping_add(1);
        let floor = floor_y.unwrap_or(fallback_y);
        stats.floor_y = floor;
        stats.floor_found = floor_y.is_some();
        let cam = obs.camera.position;
        let lens = Lens::new(obs.camera.fov, obs.cols, obs.rows);

        self.prepare_planes(planes);
        self.touched.clear();
        self.observe(obs, &lens, floor, &mut stats);
        self.probe(obs, &lens);
        self.settle(now_s, dt);
        self.forget(now_s, cam);

        let forward = obs.camera.forward();
        let cam_moving = self.last_view.is_some_and(|(at, fwd)| {
            at.distance(cam) > MOVING_M_S * dt
                || fwd.angle_between(forward) > MOVING_DEG_S.to_radians() * dt
        });
        self.last_view = Some((cam, forward));

        self.group(floor);
        self.track(now_s, dt, cam_moving, floor);
        self.select(Vec2::new(cam.x, cam.z), floor);

        stats.cells = self.index.len();
        stats.occupied = self.cells.iter().filter(|c| c.alive && c.occupied).count();
        stats.groups = self.groups.len();
        stats.tracked = self.tracks.len();
        stats.reported = self.out.len();
        stats.merged = self.entries.iter().filter(|e| e.members > 1).count();
        stats
    }

    fn prepare_planes(&mut self, planes: &[Plane]) {
        let cfg = self.config;
        let off = (3.0 * noise_sigma(cfg.noise_k, cfg.max_range_m)).max(cfg.on_plane_m);
        let grow = Vec3::new(cfg.cell_m, cfg.cell_m, 0.0);
        self.planes.clear();
        for p in planes {
            if p.kind == KIND_FLOOR {
                continue;
            }
            let m = Mat3::from_quat(p.rot.normalize());
            let half = p.half + grow;
            let reach =
                m.x_axis.abs() * half.x + m.y_axis.abs() * half.y + m.z_axis.abs() * (half.z + off);
            self.planes.push(PlaneTest {
                center: p.center,
                to_box: m.transpose(),
                half,
                lo: p.center - reach,
                hi: p.center + reach,
            });
        }
    }

    /// Whether `p` (range `r`) is on a reported plane.
    fn on_plane(&self, p: Vec3, r: f32) -> bool {
        let off = (3.0 * noise_sigma(self.config.noise_k, r)).max(self.config.on_plane_m);
        self.planes.iter().any(|t| {
            if !(p.cmpge(t.lo).all() && p.cmple(t.hi).all()) {
                return false;
            }
            let l = (t.to_box * (p - t.center)).abs();
            l.x <= t.half.x && l.y <= t.half.y && l.z <= t.half.z + off
        })
    }

    /// Whether grid cell `i` (range `r`) holds a blend of two depths;
    /// `low` when its point is near the floor (a grazing floor's rows are
    /// far apart, and only there).
    fn mixed(&self, i: usize, cols: usize, rows: usize, r: f32, low: bool) -> bool {
        let (col, row) = (i % cols, i / cols);
        let near = 3.0 * noise_sigma(self.config.noise_k, r);
        let across = (col > 0 && col + 1 < cols).then(|| (i - 1, i + 1));
        let along = (row > 0 && row + 1 < rows).then(|| (i - cols, i + cols));
        let between = |pair: Option<(usize, usize)>, frac: f32| {
            pair.is_some_and(|(a, b)| {
                let (ra, rb) = (self.range[a], self.range[b]);
                let (lo, hi) = (ra.min(rb), ra.max(rb));
                let margin = near.max(MIXED_MIN_M);
                lo > 0.0 && hi - lo > 2.0 * (frac * r + near) && r > lo + margin && r < hi - margin
            })
        };
        // A top seen in one row runs along it, and its ends have only one
        // neighbor on it: one agreeing is enough. A silhouette's side has
        // no such twin, so there both must agree.
        let agree = |pair: Option<(usize, usize)>, both: bool| {
            pair.is_some_and(|(a, b)| {
                let ok = |j: usize| {
                    let rj = self.range[j];
                    rj > 0.0 && (rj - r).abs() <= AGREE_FRAC * r + near
                };
                if both { ok(a) && ok(b) } else { ok(a) || ok(b) }
            })
        };
        let frac = if low { JUMP_FRAC } else { JUMP_FRAC_ACROSS };
        (between(along, frac) && !agree(across, false))
            || (between(across, JUMP_FRAC_ACROSS) && !agree(along, true))
    }

    /// The unit normal at grid cell `i` from its neighbors two cells away
    /// left and right and above and below (one, or one-sided, at a gap).
    fn normal(obs: &Observation<'_>, i: usize) -> Option<Vec3> {
        let (cols, rows) = (obs.cols, obs.rows);
        let (col, row) = (i % cols, i / cols);
        let at = |j: usize| obs.points[j].filter(|p| p.is_finite());
        let p = at(i)?;
        let near = |q: &Vec3| q.distance_squared(p) <= 4.0 * NEIGHBOR_M * NEIGHBOR_M;
        // The difference across `i` along one axis: `back(k)`/`ahead(k)`
        // are the cells `k` away, if inside the grid.
        let diff = |back: &dyn Fn(usize) -> Option<usize>,
                    ahead: &dyn Fn(usize) -> Option<usize>| {
            for k in [2, 1] {
                let a = back(k).and_then(at).filter(near);
                let b = ahead(k).and_then(at).filter(near);
                match (a, b) {
                    (Some(a), Some(b)) => return Some(b - a),
                    (Some(a), None) if k == 1 => return Some(p - a),
                    (None, Some(b)) if k == 1 => return Some(b - p),
                    _ => {}
                }
            }
            None
        };
        let u = diff(&|k| col.checked_sub(k).map(|c| i - (col - c)), &|k| {
            (col + k < cols).then_some(i + k)
        })?;
        let v = diff(&|k| row.checked_sub(k).map(|_| i - k * cols), &|k| {
            (row + k < rows).then_some(i + k * cols)
        })?;
        let n = u.cross(v);
        (n.length_squared() > 1e-12).then(|| n.normalize())
    }

    /// Mark the points of walls. A seed is upright (normal within
    /// [`UPRIGHT_DEG`] of level) and at least [`TALL_M`] above the floor;
    /// down the grid's column from it, every point on its vertical plane
    /// (within 3 noise deviations, at least [`WALL_TOL_M`], and
    /// [`NEIGHBOR_M`] a row) is the wall's too, the plane turning to each
    /// upright one. One point off it, or a hole, is bridged.
    fn walls(&mut self, obs: &Observation<'_>, floor: f32) {
        let (cols, rows) = (obs.cols, obs.rows);
        let tall = floor + self.config.tall_m;
        let k = self.config.noise_k;
        let level_sin = UPRIGHT_DEG.to_radians().sin();
        self.wall.clear();
        self.wall.resize(cols * rows, false);
        for col in 0..cols {
            // The last wall point up the column, its plane's level normal
            // and its row.
            let mut chain: Option<(Vec3, Vec2, usize)> = None;
            for row in 0..rows {
                let i = row * cols + col;
                chain = chain.filter(|c| row - c.2 <= 2);
                let Some(p) = obs.points[i].filter(|p| p.is_finite()) else {
                    continue;
                };
                let upright = Self::normal(obs, i)
                    .filter(|n| n.y.abs() <= level_sin)
                    .map(|n| Vec2::new(n.x, n.z).normalize());
                let joined = chain.is_some_and(|(q, n, r)| {
                    let d = p - q;
                    let reach = NEIGHBOR_M * (row - r) as f32;
                    let tol = (3.0 * noise_sigma(k, self.range[i].max(0.0))).max(WALL_TOL_M);
                    d.length_squared() <= reach * reach && (d.x * n.x + d.z * n.y).abs() <= tol
                });
                if joined {
                    self.wall[i] = true;
                    let n = chain.map_or(Vec2::X, |c| c.1);
                    chain = Some((p, upright.unwrap_or(n), row));
                } else if let Some(n) = upright
                    && p.y >= tall
                {
                    self.wall[i] = true;
                    chain = Some((p, n, row));
                }
            }
        }
    }

    /// Whether stuff point `i` (at `p`) has a neighbor in the grid (not a
    /// blend) within [`SUPPORT_CELLS`] grid cells' width (`width` at its
    /// range): a blend that got through the test for blends lies alone in
    /// the air behind a silhouette; a surface's points have neighbors.
    fn supported(&self, obs: &Observation<'_>, i: usize, p: Vec3, width: f32) -> bool {
        let (cols, n) = (obs.cols, obs.points.len());
        let col = i % cols;
        let reach = (SUPPORT_CELLS * width).max(SUPPORT_MIN_M);
        [
            col.checked_sub(1).map(|_| i - 1),
            (col + 1 < cols).then_some(i + 1),
            i.checked_sub(cols),
            (i + cols < n).then_some(i + cols),
        ]
        .into_iter()
        .flatten()
        .any(|j| {
            !self.blend[j] && obs.points[j].is_some_and(|q| q.distance_squared(p) <= reach * reach)
        })
    }

    /// Whether point `i` (at `p`, range `r`) is on a top: a neighbor above
    /// or below in the grid rises little across, none lies nearly straight
    /// above or below, and a neighbor left or right agrees in range (a top
    /// runs along the row). Blends are not neighbors.
    fn on_top(&self, obs: &Observation<'_>, i: usize, p: Vec3, r: f32) -> bool {
        let cols = obs.cols;
        let near = AGREE_FRAC * r + 3.0 * noise_sigma(self.config.noise_k, r);
        let col = i % cols;
        let beside = [
            col.checked_sub(1).map(|_| i - 1),
            (col + 1 < cols).then_some(i + 1),
        ];
        let along = beside
            .into_iter()
            .flatten()
            .any(|j| !self.blend[j] && self.range[j] > 0.0 && (self.range[j] - r).abs() <= near);
        let (mut flat, mut steep) = (false, false);
        let above = (i >= cols).then(|| i - cols);
        let below = (i + cols < obs.points.len()).then_some(i + cols);
        for j in [above, below].into_iter().flatten() {
            if self.range[j] <= 0.0 || self.blend[j] {
                continue;
            }
            let Some(q) = obs.points[j] else {
                continue;
            };
            let d = q - p;
            if d.length_squared() > NEIGHBOR_M * NEIGHBOR_M {
                continue;
            }
            let across = Vec2::new(d.x, d.z).length();
            let rise = d.y.abs();
            flat |= rise <= FLAT_SLOPE * across;
            steep |= across <= STEEP_SLOPE * rise;
        }
        along && flat && !steep
    }

    /// The level segment a stuff point `i` (at `p`) stands for across the
    /// view: half way to its neighbors left and right on the same surface
    /// (within 3 grid cells' width of it, not blends), so the segment
    /// follows the surface however it is turned. At a silhouette it ends
    /// at the point: the true edge lies up to a grid cell beyond, but
    /// reaching for it (by mirroring the gap, or by half a cell across the
    /// ray) put the measured widths 7 cm over on average and pushed a
    /// turned box's corners out, against 2 cm under without. With no
    /// neighbor at all, `across` (half a grid cell across the ray) each way.
    fn sides(&self, obs: &Observation<'_>, i: usize, p: Vec3, across: Vec2) -> [Vec2; 2] {
        let col = i % obs.cols;
        let reach = (6.0 * across.length()).max(0.05);
        let half = |j: Option<usize>| {
            let j = j.filter(|&j| !self.blend[j])?;
            let q = obs.points[j].filter(|q| q.distance_squared(p) <= reach * reach)?;
            Some(0.5 * Vec2::new(q.x - p.x, q.z - p.z))
        };
        let left = half(col.checked_sub(1).map(|_| i - 1));
        let right = half((col + 1 < obs.cols).then_some(i + 1));
        match (left, right) {
            (Some(l), Some(r)) => [l, r],
            (Some(l), None) => [l, Vec2::ZERO],
            (None, Some(r)) => [Vec2::ZERO, r],
            (None, None) => [-across, across],
        }
    }

    /// How far a top's strip at point `i` (at `p`) reaches toward and away
    /// from the camera (level direction `away`), given where its cell's
    /// edges meet the top (`near`, `far`, signed from `p`): never past the
    /// points the grid cells above and below see (the face under a top's
    /// front edge, the next row of the top). On a side with no such point
    /// the top ends somewhere before the next cell's ray, so the cell's
    /// edge is its expected end; there it reaches at most
    /// [`SILHOUETTE_MAX_M`], since at a grazing view that end is far and
    /// uncertain.
    fn strip_ends(
        &self,
        obs: &Observation<'_>,
        i: usize,
        p: Vec3,
        away: Vec2,
        (near, far): (f32, f32),
    ) -> (f32, f32) {
        let cols = obs.cols;
        let (mut lo, mut hi) = (near.max(-SILHOUETTE_MAX_M), far.min(SILHOUETTE_MAX_M));
        let above = (i >= cols).then(|| i - cols);
        let below = (i + cols < obs.points.len()).then_some(i + cols);
        for j in [above, below].into_iter().flatten() {
            if self.blend[j] {
                continue;
            }
            let Some(q) =
                obs.points[j].filter(|q| q.distance_squared(p) <= NEIGHBOR_M * NEIGHBOR_M)
            else {
                continue;
            };
            let s = Vec2::new(q.x - p.x, q.z - p.z).dot(away);
            if s < 0.0 {
                lo = lo.min(near.max(s));
            } else {
                hi = hi.max(far.min(s));
            }
        }
        (lo, hi)
    }

    /// The cell at level position (`x`, `z`), made if `make`, with this
    /// update's fields started.
    fn cell_at(&mut self, x: f32, z: f32, make: bool) -> Option<usize> {
        let cell = self.config.cell_m;
        let (ix, iz) = ((x / cell).floor() as i32, (z / cell).floor() as i32);
        let key = cell_key(ix, iz);
        let i = if let Some(&i) = self.index.get(&key) {
            i as usize
        } else {
            if !make {
                return None;
            }
            let i = if let Some(i) = self.free.pop() {
                self.cells[i as usize] = Cell::new(ix, iz);
                i as usize
            } else {
                self.cells.push(Cell::new(ix, iz));
                self.cells.len() - 1
            };
            self.index.insert(key, i as u32);
            i
        };
        let c = &mut self.cells[i];
        if c.stamp != self.stamp {
            c.start(self.stamp);
            self.touched.push(i as u32);
        }
        Some(i)
    }

    /// A stuff sample at `p`, measured or on a top's strip, standing for
    /// the level segment from `p + side[0]` to `p + side[1]`.
    fn add_stuff(&mut self, p: Vec3, side: [Vec2; 2], measured: bool) {
        let Some(i) = self.cell_at(p.x, p.z, true) else {
            return;
        };
        let c = &mut self.cells[i];
        let base = Vec2::new(p.x, p.z);
        if c.n_stuff == 0 {
            c.u_ext = [base + side[0]; 8];
        }
        c.n_stuff += 1;
        c.u_lo = c.u_lo.min(p.y);
        c.u_hi = c.u_hi.max(p.y);
        for s in side {
            let q = base + s;
            for (e, d) in c.u_ext.iter_mut().zip(EXTREMES) {
                if q.dot(d) > e.dot(d) {
                    *e = q;
                }
            }
        }
        if measured || !c.u_probe_measured {
            c.u_probe = p;
            c.u_probe_measured = measured;
        }
    }

    /// The update's points into the cells.
    fn observe(&mut self, obs: &Observation<'_>, lens: &Lens, floor: f32, stats: &mut Stats) {
        let cfg = self.config;
        let cam = obs.camera.position;
        let rot = obs.camera.rotation;
        let (cols, rows) = (obs.cols, obs.rows);
        self.range.clear();
        self.range.extend(obs.points.iter().map(|p| match p {
            Some(p) if p.is_finite() => p.distance(cam),
            _ => -1.0,
        }));
        self.walls(obs, floor);
        let mut blend = std::mem::take(&mut self.blend);
        blend.clear();
        let low = floor + 2.0 * self.config.low_m;
        blend.extend((0..cols * rows).map(|i| {
            let r = self.range[i];
            r > 0.0 && self.mixed(i, cols, rows, r, obs.points[i].is_some_and(|p| p.y < low))
        }));
        self.blend = blend;
        let angle = lens.cell_angle();
        let step = 0.5 * cfg.cell_m;
        for i in 0..cols * rows {
            let r = self.range[i];
            if !(r > 0.0 && r <= cfg.max_range_m) {
                continue;
            }
            let Some(p) = obs.points[i] else {
                continue;
            };
            if self.blend[i] {
                stats.mixed += 1;
                continue;
            }
            let (col, row) = (i % cols, i / cols);
            let h = p.y - floor;
            let level = Vec2::new(p.x - cam.x, p.z - cam.z);
            let dist = level.length();
            // The noise is along the ray: its vertical part.
            let lift = 3.0 * noise_sigma(cfg.noise_k, r) * (p.y - cam.y).abs() / r;
            if h.abs() <= cfg.on_floor_m + lift {
                stats.bare += 1;
                // The ray's height a cell's width before it reached the
                // floor: stuff in the cell lower than that would have
                // stopped it.
                let y = p.y + (cam.y - p.y).max(0.0) / dist.max(1e-3) * cfg.cell_m;
                let base = Vec2::new(p.x, p.z);
                let away = level / dist.max(1e-3);
                match strip(p, cam, rot, lens, col, row, (-SPREAD_MAX_M, SPREAD_MAX_M))
                    .map(|(n, f)| (base + away * n, base + away * f))
                {
                    Some((a, b)) if a.distance(b) > cfg.cell_m => {
                        let n = (a.distance(b) / step).ceil() as usize;
                        for k in 0..=n {
                            let q = a.lerp(b, k as f32 / n as f32);
                            self.add_bare(q, y);
                        }
                    }
                    _ => self.add_bare(Vec2::new(p.x, p.z), y),
                }
                continue;
            }
            if self.wall[i] {
                stats.wall += 1;
                if let Some(k) = self.cell_at(p.x, p.z, true) {
                    self.cells[k].saw_tall = true;
                }
                continue;
            }
            if h < cfg.low_m + lift || h > cfg.high_m {
                continue;
            }
            if dist < cfg.self_m {
                stats.own += 1;
                continue;
            }
            if self.on_plane(p, r) {
                stats.on_plane += 1;
                if let Some(k) = self.cell_at(p.x, p.z, false) {
                    self.cells[k].n_plane += 1;
                }
                continue;
            }
            if !self.supported(obs, i, p, r * angle) {
                stats.lone += 1;
                continue;
            }
            stats.stuff += 1;
            let away = level / dist;
            let side = self.sides(obs, i, p, Vec2::new(-away.y, away.x) * (0.5 * r * angle));
            self.add_stuff(p, side, true);
            if self.on_top(obs, i, p, r)
                && let Some((near, far)) =
                    strip(p, cam, rot, lens, col, row, (-SPREAD_MAX_M, SPREAD_MAX_M))
            {
                let (near, far) = self.strip_ends(obs, i, p, away, (near, far));
                let base = Vec2::new(p.x, p.z);
                let (a, b) = (base + away * near, base + away * far);
                let n = ((a.distance(b) / step).ceil() as usize).max(1);
                for k in 0..=n {
                    let q = a.lerp(b, k as f32 / n as f32);
                    self.add_stuff(Vec3::new(q.x, p.y, q.y), side, false);
                }
            }
        }
    }

    /// Bare floor at level `q`, the ray over it at height `y`: only a cell
    /// that exists holds anything to clear.
    fn add_bare(&mut self, q: Vec2, y: f32) {
        if let Some(i) = self.cell_at(q.x, q.y, false) {
            let c = &mut self.cells[i];
            c.bare_y = c.bare_y.max(y);
        }
    }

    /// Cells holding a score but no stuff this update: is their probe seen
    /// through on all four grid cells around it?
    fn probe(&mut self, obs: &Observation<'_>, lens: &Lens) {
        let cam = obs.camera.position;
        let inv = obs.camera.rotation.inverse();
        let (cols, rows) = (obs.cols as i64, obs.rows as i64);
        for i in 0..self.cells.len() {
            let c = &self.cells[i];
            if !c.alive
                || c.score <= 0.0
                || !c.has_probe
                || (c.stamp == self.stamp && c.n_stuff > 0)
            {
                continue;
            }
            let d = c.probe - cam;
            let rp = d.length();
            if rp > self.config.max_range_m {
                continue;
            }
            let Some(g) = lens.project(inv * d) else {
                continue;
            };
            let (c0, r0) = ((g.x - 0.5).floor() as i64, (g.y - 0.5).floor() as i64);
            if c0 < 0 || r0 < 0 || c0 + 1 >= cols || r0 + 1 >= rows {
                continue;
            }
            let tol = (3.0 * noise_sigma(self.config.noise_k, rp)).max(self.config.see_tol_m);
            let (mut far, mut on) = (0, 0);
            for (dc, dr) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let rj = self.range[((r0 + dr) * cols + c0 + dc) as usize];
                if rj <= 0.0 {
                    continue;
                }
                far += u32::from(rj > rp + tol);
                on += u32::from((rj - rp).abs() <= tol);
            }
            let own = (g.y.floor() as i64).clamp(0, rows - 1) * cols
                + (g.x.floor() as i64).clamp(0, cols - 1);
            // All four farther: seen through. Some farther and none on it:
            // the probe hangs in the air at a silhouette (a blend that got
            // through), weak evidence, unless its own grid cell holds a
            // blend this update: a top seen in one row that is blended now
            // and then would lose its cells.
            if far == 4 || (far > 0 && on == 0 && !self.blend[own as usize]) {
                let c = &mut self.cells[i];
                if c.stamp != self.stamp {
                    c.start(self.stamp);
                    self.touched.push(i as u32);
                }
                if far == 4 {
                    c.seen_through = true;
                } else {
                    c.in_air = true;
                }
            }
        }
    }

    /// Whether a neighbor of cell (`ix`, `iz`) is occupied.
    fn near_occupied(&self, ix: i32, iz: i32) -> bool {
        RING[..8].iter().any(|&(dx, dz)| {
            self.index
                .get(&cell_key(ix + dx, iz + dz))
                .is_some_and(|&j| self.cells[j as usize].occupied)
        })
    }

    /// This update's evidence into each touched cell's state.
    fn settle(&mut self, now_s: f32, dt: f32) {
        // A moving footprint's leading edge: stuff beside an occupied cell
        // near it is occupied at once, and so is stuff beside that.
        let cell = self.config.cell_m;
        for _ in 0..4 {
            let mut changed = false;
            for k in 0..self.touched.len() {
                let i = self.touched[k] as usize;
                let c = self.cells[i];
                if c.n_stuff > 0
                    && !c.occupied
                    && self
                        .fast
                        .iter()
                        .any(|r| r.contains(c.center(cell), GROW_NEAR_M))
                    && self.near_occupied(c.ix, c.iz)
                {
                    self.cells[i].grow = true;
                    self.cells[i].occupied = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let on = self.config.on_s;
        for &i in &self.touched {
            let c = &mut self.cells[i as usize];
            if c.saw_tall {
                c.tall = true;
                c.last_stuff_s = now_s;
            }
            if c.n_stuff > 0 {
                if c.score <= 0.0 {
                    c.lo = c.u_lo;
                    c.hi = c.u_hi;
                } else {
                    c.lo = c.u_lo.min(c.lo + RELAX_M_S * dt);
                    c.hi = c.u_hi.max(c.hi - RELAX_M_S * dt);
                }
                c.score = (c.score + dt).min(SCORE_MAX_S);
                c.ext = c.u_ext;
                c.bounds = bounds_of(&c.u_ext);
                c.probe = c.u_probe;
                c.has_probe = true;
                c.last_stuff_s = now_s;
                if c.score >= on || c.grow {
                    c.occupied = true;
                }
            } else if c.score > 0.0 && (c.seen_through || c.bare_y >= c.lo - BARE_MARGIN_M) {
                c.score = 0.0;
                c.occupied = false;
                c.tall = false;
            } else if c.n_plane > 0 || c.in_air {
                c.score = (c.score - dt).max(0.0);
                if c.score <= 0.0 {
                    c.occupied = false;
                }
            }
        }
    }

    /// Drop cells far from the camera, and free cells with no stuff for a
    /// while.
    fn forget(&mut self, now_s: f32, cam: Vec3) {
        let cell = self.config.cell_m;
        let far = self.config.forget_m;
        let at = Vec2::new(cam.x, cam.z);
        for i in 0..self.cells.len() {
            let c = &self.cells[i];
            if !c.alive {
                continue;
            }
            let stale = c.score <= 0.0 && now_s - c.last_stuff_s > RECYCLE_S;
            if stale || c.center(cell).distance(at) > far {
                self.index.remove(&cell_key(c.ix, c.iz));
                self.cells[i].alive = false;
                self.cells[i].occupied = false;
                self.free.push(i as u32);
            }
        }
    }

    /// Grow groups from occupied cells touching the floor.
    fn group(&mut self, floor: f32) {
        self.groups.clear();
        self.members.clear();
        let cfg = self.config;
        let seed_y = floor + cfg.touch_m;
        for s in 0..self.cells.len() {
            let c = &self.cells[s];
            if !c.alive || !c.occupied || c.tall || c.visit == self.stamp || c.lo >= seed_y {
                continue;
            }
            let start = self.members.len();
            let mut hi = f32::NEG_INFINITY;
            self.cells[s].visit = self.stamp;
            self.stack.push(s as u32);
            while let Some(k) = self.stack.pop() {
                self.members.push(k);
                let ck = self.cells[k as usize];
                hi = hi.max(ck.hi);
                for &(dx, dz) in &RING {
                    let Some(&j) = self.index.get(&cell_key(ck.ix + dx, ck.iz + dz)) else {
                        continue;
                    };
                    let cj = &self.cells[j as usize];
                    if !cj.occupied
                        || cj.tall
                        || cj.visit == self.stamp
                        || cj.lo > ck.hi + STEP_M
                        || ck.lo > cj.hi + STEP_M
                        || bounds_gap(ck.bounds, cj.bounds) >= cfg.join_m
                    {
                        continue;
                    }
                    self.cells[j as usize].visit = self.stamp;
                    self.stack.push(j);
                }
            }
            self.groups.push(Group {
                start,
                len: self.members.len() - start,
                hi,
                owner: None,
            });
        }
    }

    /// The extreme stuff samples of the cells of every group owned by
    /// track `t` (or of group `g` alone) into `pts`.
    /// Only cells of at least `min_score` count, unless none does.
    fn gather(&mut self, owner: Option<usize>, g: usize, min_score: f32) {
        self.pts.clear();
        for pass in [min_score, 0.0] {
            for (k, gr) in self.groups.iter().enumerate() {
                let mine = match owner {
                    Some(t) => gr.owner == Some(t),
                    None => k == g,
                };
                if !mine {
                    continue;
                }
                for &m in &self.members[gr.start..gr.start + gr.len] {
                    let c = &self.cells[m as usize];
                    if c.score >= pass {
                        self.pts.extend_from_slice(&c.ext);
                    }
                }
            }
            if !self.pts.is_empty() {
                break;
            }
        }
    }

    /// Groups to footprints: follow, appear, vanish.
    fn track(&mut self, now_s: f32, dt: f32, cam_moving: bool, floor: f32) {
        let cfg = self.config;
        let cell = cfg.cell_m;
        // How many of each group's cells each footprint's estimate holds.
        self.overlaps.clear();
        for (g, gr) in self.groups.iter().enumerate() {
            for (t, tr) in self.tracks.iter().enumerate() {
                // Where its estimate should be now, by its velocity.
                let at = Rect2 {
                    c: tr.est.c + tr.vel * dt,
                    ..tr.est
                };
                let n = self.members[gr.start..gr.start + gr.len]
                    .iter()
                    .filter(|&&m| at.contains(self.cells[m as usize].center(cell), MATCH_M))
                    .count();
                if n > 0 {
                    self.overlaps.push((n, g, t));
                }
            }
        }
        let tracks = &self.tracks;
        self.overlaps.sort_unstable_by(|a, b| {
            b.0.cmp(&a.0)
                .then(tracks[a.2].id.cmp(&tracks[b.2].id))
                .then(a.1.cmp(&b.1))
        });
        for t in &mut self.tracks {
            t.got = false;
        }
        for &(_, g, t) in &self.overlaps {
            if self.groups[g].owner.is_none() && !self.tracks[t].got {
                self.groups[g].owner = Some(t);
                self.tracks[t].got = true;
            }
        }
        // A second group mostly on a footprint that has one is a piece of it.
        for &(n, g, t) in &self.overlaps {
            if self.groups[g].owner.is_none()
                && self.tracks[t].got
                && n as f32 >= FRAGMENT_FRAC * self.groups[g].len as f32
            {
                self.groups[g].owner = Some(t);
            }
        }
        // A footprint whose cells went into another's group merged into it.
        for &(n, g, t) in &self.overlaps {
            let tr = self.tracks[t];
            let Some(o) = self.groups[g].owner else {
                continue;
            };
            if !tr.got && tr.alive && o != t && n as f32 >= ABSORB_FRAC * tr.cells.max(1) as f32 {
                self.tracks[t].alive = false;
                #[cfg(test)]
                self.events.push(format!(
                    "{now_s:.2} {} merged into {}",
                    tr.id, self.tracks[o].id
                ));
            }
        }
        // A moving footprint that found no group takes a group no one took
        // near where it should be: a moving thing's cells come and go
        // between updates, and a second id would follow.
        for g in 0..self.groups.len() {
            if self.groups[g].owner.is_some() {
                continue;
            }
            let gr = self.groups[g];
            let near = self
                .tracks
                .iter()
                .enumerate()
                .filter(|(_, t)| t.alive && !t.got && t.moving)
                .map(|(t, tr)| {
                    let at = Rect2 {
                        c: tr.est.c + tr.vel * dt,
                        ..tr.est
                    };
                    let gap = self.members[gr.start..gr.start + gr.len]
                        .iter()
                        .map(|&m| at.distance(self.cells[m as usize].center(cell)))
                        .fold(f32::INFINITY, f32::min);
                    (t, gap - tr.vel.length() * dt)
                })
                .filter(|&(_, gap)| gap <= REACQUIRE_M)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((t, _)) = near {
                self.groups[g].owner = Some(t);
                self.tracks[t].got = true;
            }
        }
        // Estimates.
        for t in 0..self.tracks.len() {
            if !self.tracks[t].got {
                continue;
            }
            // A reported footprint's estimate, unless it moves, takes the
            // cells seen for a while: a blend at a silhouette is rarely seen
            // three updates running. One not yet reported takes all its
            // cells, and appears with them.
            let (fast, appeared) = (now_s < self.tracks[t].fast_until, self.tracks[t].appeared);
            let prev = self.tracks[t].est.yaw;
            self.gather(Some(t), 0, if fast || !appeared { 0.0 } else { MATURE_S });
            let Some(est) = fit_rect(&mut self.pts, &mut self.hull, Some(prev)) else {
                continue;
            };
            let (mut hi, mut cells) = (f32::NEG_INFINITY, 0);
            for gr in self.groups.iter().filter(|gr| gr.owner == Some(t)) {
                hi = hi.max(gr.hi);
                cells += gr.len;
            }
            let tr = &mut self.tracks[t];
            let v = (est.c - tr.last_c) / dt;
            tr.vel += (v - tr.vel) * (1.0 - (-dt / SPEED_TAU_S).exp());
            tr.last_c = est.c;
            tr.est = est;
            tr.height = hi - floor;
            tr.cells = cells;
        }
        // New footprints from groups no footprint took.
        for g in 0..self.groups.len() {
            if self.groups[g].owner.is_some() {
                continue;
            }
            self.gather(None, g, 0.0);
            let Some(est) = fit_rect(&mut self.pts, &mut self.hull, None) else {
                continue;
            };
            if est.presence() < cfg.min_area_m2 {
                continue;
            }
            if self.tracks.iter().filter(|t| t.alive).count() >= MAX_TRACKED {
                // Make room by the smallest footprint not yet reported, if any
                // (dropped with the others at the end, so indices hold).
                let weakest = self
                    .tracks
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.alive && !t.appeared)
                    .min_by(|a, b| a.1.est.area().total_cmp(&b.1.est.area()))
                    .map(|(i, _)| i);
                let Some(i) = weakest else {
                    continue;
                };
                self.tracks[i].alive = false;
            }
            let mut t = Track::new(self.next_id, est, now_s);
            self.next_id += 1;
            t.height = self.groups[g].hi - floor;
            t.cells = self.groups[g].len;
            self.groups[g].owner = Some(self.tracks.len());
            self.tracks.push(t);
        }
        // Life.
        for t in &mut self.tracks {
            if !t.alive {
                continue;
            }
            if t.got {
                t.miss_s = 0.0;
                t.last_hit_s = now_s;
                t.groups += 1;
            } else {
                t.miss_s += dt;
            }
            if !t.appeared {
                t.updates += 1;
                t.hits += u32::from(t.got && t.est.presence() >= cfg.min_area_m2);
                if now_s - t.last_hit_s > CANDIDATE_TTL_S {
                    t.alive = false;
                } else if t.got
                    && now_s - t.first_s >= cfg.appear_s - 1e-3
                    && t.hits as f32 >= APPEAR_FRAC * t.updates as f32
                    && t.est.presence() >= cfg.min_area_m2
                {
                    t.appeared = true;
                    t.rep = t.est;
                    t.since_s = now_s;
                    #[cfg(test)]
                    self.events.push(format!("{now_s:.2} {} appeared", t.id));
                }
            } else if t.miss_s >= cfg.vanish_s {
                t.alive = false;
                #[cfg(test)]
                self.events.push(format!("{now_s:.2} {} vanished", t.id));
            } else if t.got {
                t.follow(now_s, dt, cam_moving, &cfg);
            }
            t.confidence = (1.0 - (-(t.groups as f32) / CONF_HITS).exp())
                * (1.0 - t.miss_s / cfg.vanish_s).clamp(0.0, 1.0);
        }
        self.tracks.retain(|t| t.alive);
        self.fast.clear();
        self.fast.extend(
            self.tracks
                .iter()
                .filter(|t| t.appeared && now_s < t.fast_until)
                .map(|t| t.est),
        );
    }

    /// Draw the reported set within the budget and fill the output.
    fn select(&mut self, cam: Vec2, floor: f32) {
        self.entries.clear();
        for t in &mut self.tracks {
            t.shown_as = t.id;
        }
        for t in self.tracks.iter().filter(|t| t.appeared) {
            let i = self.entries.len();
            self.entries.push(Entry {
                parent: i,
                rect: t.rep,
                area: t.rep.area(),
                big: t.rep.area(),
                id: t.id,
                members: 1,
                height: t.height,
                confidence: t.confidence,
                moving: t.moving,
                since_s: t.since_s,
            });
        }
        let max = self.config.max_footprints;
        let mut count = self.entries.len();
        std::mem::swap(&mut self.merged, &mut self.merged_before);
        self.merged.clear();
        // Over the budget: merge the nearest small neighbors first.
        while count > max {
            let mut best: Option<(f32, usize, usize)> = None;
            for a in 0..self.entries.len() {
                if self.entries[a].parent != a {
                    continue;
                }
                for b in a + 1..self.entries.len() {
                    if self.entries[b].parent != b {
                        continue;
                    }
                    let (ea, eb) = (&self.entries[a], &self.entries[b]);
                    let gap = ea.rect.gap(&eb.rect);
                    if gap > MERGE_GAP_M {
                        continue;
                    }
                    let pair = (ea.id.min(eb.id), ea.id.max(eb.id));
                    let keep = if self.merged_before.contains(&pair) {
                        MERGE_KEEP
                    } else {
                        1.0
                    };
                    let cost = keep * (gap + 0.1) * (ea.area + eb.area);
                    if best.is_none_or(|x| cost < x.0) {
                        best = Some((cost, a, b));
                    }
                }
            }
            let Some((_, a, b)) = best else {
                break;
            };
            let (ea, eb) = (self.entries[a], self.entries[b]);
            self.merged.push((ea.id.min(eb.id), ea.id.max(eb.id)));
            let (big, small) = if (ea.big, eb.id) >= (eb.big, ea.id) {
                (ea, eb)
            } else {
                (eb, ea)
            };
            self.pts.clear();
            self.pts.extend_from_slice(&big.rect.corners());
            self.pts.extend_from_slice(&small.rect.corners());
            let rect =
                fit_rect(&mut self.pts, &mut self.hull, Some(big.rect.yaw)).unwrap_or(big.rect);
            self.entries[b].parent = a;
            self.entries[a] = Entry {
                parent: a,
                rect,
                area: ea.area + eb.area,
                big: big.big,
                id: big.id,
                members: ea.members + eb.members,
                height: ea.height.max(eb.height),
                confidence: ea.confidence.max(eb.confidence),
                moving: ea.moving || eb.moving,
                since_s: ea.since_s.min(eb.since_s),
            };
            count -= 1;
        }
        // Each track's root entry.
        for i in 0..self.entries.len() {
            let mut r = i;
            while self.entries[r].parent != r {
                r = self.entries[r].parent;
            }
            self.entries[i].parent = r;
        }
        // Still over: rank by area and nearness, the reported ones favored.
        self.ranked.clear();
        let mut k = 0;
        for t in &self.tracks {
            if !t.appeared {
                continue;
            }
            let root = self.entries[k].parent;
            if root == k {
                let e = &self.entries[k];
                let bonus = if t.reported { 1.0 + KEEP_BONUS } else { 1.0 };
                let score =
                    bonus * (0.3 + e.area.min(6.0).sqrt()) / (1.0 + e.rect.distance(cam) / 3.0);
                self.ranked.push((score, k));
            }
            k += 1;
        }
        self.ranked
            .sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        self.ranked.truncate(max);
        self.out.clear();
        for &(_, k) in &self.ranked {
            let e = &self.entries[k];
            self.out.push(Footprint {
                id: e.id,
                center: Vec3::new(e.rect.c.x, floor, e.rect.c.y),
                yaw: e.rect.yaw,
                half: e.rect.half,
                height: e.height,
                confidence: e.confidence,
                moving: e.moving,
                since_s: e.since_s,
            });
        }
        self.out.sort_unstable_by_key(|f| f.id);
        let mut k = 0;
        for t in &mut self.tracks {
            if !t.appeared {
                t.reported = false;
                continue;
            }
            let e = &self.entries[self.entries[k].parent];
            t.shown_as = e.id;
            t.reported = self.out.iter().any(|f| f.id == e.id);
            k += 1;
        }
    }

    /// Elements the store's buffers hold room for, for the no-growth test.
    #[cfg(test)]
    fn capacity(&self) -> usize {
        self.cells.capacity()
            + self.index.capacity()
            + self.free.capacity()
            + self.touched.capacity()
            + self.planes.capacity()
            + self.range.capacity()
            + self.wall.capacity()
            + self.blend.capacity()
            + self.fast.capacity()
            + self.stack.capacity()
            + self.members.capacity()
            + self.groups.capacity()
            + self.pts.capacity()
            + self.hull.capacity()
            + self.overlaps.capacity()
            + self.tracks.capacity()
            + self.entries.capacity()
            + self.merged.capacity()
            + self.merged_before.capacity()
            + self.ranked.capacity()
            + self.out.capacity()
    }
}

#[cfg(test)]
impl Footprints {
    /// The occupied cells' centers (x, z).
    fn occupied(&self) -> Vec<Vec2> {
        let cell = self.config.cell_m;
        self.cells
            .iter()
            .filter(|c| c.alive && c.occupied)
            .map(|c| c.center(cell))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live_planes::{DepthCamera, LivePlanes, plane_id, plane_uuid};
    use crate::surfaces::{KIND_TABLE, KIND_WALL, PLANE_HALF_THICKNESS_M, is_plane};

    // ---- A deterministic generator (splitmix64) --------------------------

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// Uniform in 0..1.
        fn uniform(&mut self) -> f32 {
            (self.next() >> 40) as f32 / (1u64 << 24) as f32
        }

        /// Standard normal (Box-Muller).
        fn gauss(&mut self) -> f32 {
            let a = self.uniform().max(1e-7);
            let b = self.uniform();
            (-2.0 * a.ln()).sqrt() * (std::f32::consts::TAU * b).cos()
        }
    }

    // ---- Synthetic scenes ------------------------------------------------

    #[derive(Debug, Clone, Copy)]
    enum Shape {
        /// A box: center, turn about +Y (rad), half extents.
        Cuboid {
            c: Vec3,
            yaw: f32,
            half: Vec3,
        },
        /// A vertical cylinder standing on `base`.
        Cylinder {
            base: Vec3,
            radius: f32,
            height: f32,
        },
        Gone,
    }

    impl Shape {
        fn hit(&self, o: Vec3, d: Vec3) -> Option<f32> {
            match *self {
                Shape::Cuboid { c, yaw, half } => {
                    let q = Quat::from_rotation_y(-yaw);
                    let (lo, ld) = (q * (o - c), q * d);
                    let (mut t0, mut t1) = (f32::NEG_INFINITY, f32::INFINITY);
                    for a in 0..3 {
                        if ld[a].abs() < 1e-12 {
                            if lo[a].abs() > half[a] {
                                return None;
                            }
                        } else {
                            let p = (-half[a] - lo[a]) / ld[a];
                            let q = (half[a] - lo[a]) / ld[a];
                            t0 = t0.max(p.min(q));
                            t1 = t1.min(p.max(q));
                        }
                    }
                    (t1 >= t0 && t0 > 1e-6).then_some(t0)
                }
                Shape::Cylinder {
                    base,
                    radius,
                    height,
                } => {
                    let mut best: Option<f32> = None;
                    let (ox, oz) = (o.x - base.x, o.z - base.z);
                    let a = d.x * d.x + d.z * d.z;
                    let b = 2.0 * (ox * d.x + oz * d.z);
                    let c = ox * ox + oz * oz - radius * radius;
                    let disc = b * b - 4.0 * a * c;
                    if a > 1e-12 && disc >= 0.0 {
                        let t = (-b - disc.sqrt()) / (2.0 * a);
                        let y = o.y + d.y * t;
                        if t > 1e-6 && (base.y..=base.y + height).contains(&y) {
                            best = Some(t);
                        }
                    }
                    if d.y.abs() > 1e-12 {
                        for y in [base.y + height, base.y] {
                            let t = (y - o.y) / d.y;
                            let p = o + d * t;
                            let r2 = (p.x - base.x).powi(2) + (p.z - base.z).powi(2);
                            if t > 1e-6 && r2 <= radius * radius && best.is_none_or(|b| t < b) {
                                best = Some(t);
                            }
                        }
                    }
                    best
                }
                Shape::Gone => None,
            }
        }
    }

    #[derive(Debug, Clone, Default)]
    struct Scene {
        shapes: Vec<Shape>,
    }

    impl Scene {
        fn add(&mut self, s: Shape) -> usize {
            self.shapes.push(s);
            self.shapes.len() - 1
        }

        fn hit(&self, o: Vec3, d: Vec3) -> Option<f32> {
            self.shapes
                .iter()
                .filter_map(|s| s.hit(o, d))
                .min_by(f32::total_cmp)
        }
    }

    /// A box standing on the floor at (`x`, `z`): `size` is width (local
    /// X), height and depth (local Z); turned `yaw_deg`.
    fn thing(x: f32, z: f32, size: [f32; 3], yaw_deg: f32) -> Shape {
        Shape::Cuboid {
            c: Vec3::new(x, 0.5 * size[1], z),
            yaw: yaw_deg.to_radians(),
            half: Vec3::from_array(size) * 0.5,
        }
    }

    /// A box from `lo` to `hi` (axis-aligned).
    fn block(lo: Vec3, hi: Vec3) -> Shape {
        Shape::Cuboid {
            c: 0.5 * (lo + hi),
            yaw: 0.0,
            half: 0.5 * (hi - lo),
        }
    }

    fn post(x: f32, z: f32, radius: f32, height: f32) -> Shape {
        Shape::Cylinder {
            base: Vec3::new(x, 0.0, z),
            radius,
            height,
        }
    }

    /// A wall `h` high on the floor line from `a` to `b` (x, z), facing
    /// left of the walk from `a` to `b`, 10 cm thick behind its face; and
    /// its face as the plane finder reports it.
    fn wall(scene: &mut Scene, a: [f32; 2], b: [f32; 2], h: f32) -> Plane {
        let d = Vec2::new(b[0] - a[0], b[1] - a[1]);
        let len = d.length();
        let u = d / len;
        let yaw = (-u.y).atan2(u.x);
        let n = Vec3::new(-u.y, 0.0, u.x);
        let mid = Vec3::new(0.5 * (a[0] + b[0]), 0.5 * h, 0.5 * (a[1] + b[1]));
        scene.add(Shape::Cuboid {
            c: mid - n * 0.05,
            yaw,
            half: Vec3::new(0.5 * len, 0.5 * h, 0.05),
        });
        Plane {
            center: mid,
            rot: Quat::from_rotation_y(yaw),
            half: Vec3::new(0.5 * len, 0.5 * h, PLANE_HALF_THICKNESS_M),
            kind: KIND_WALL,
        }
    }

    /// A room over `x`, `z`, `h` high: floor, ceiling and four walls facing
    /// in; the walls as reported planes.
    fn room_in(scene: &mut Scene, x: [f32; 2], z: [f32; 2], h: f32) -> Vec<Plane> {
        scene.add(block(
            Vec3::new(x[0] - 0.2, -0.1, z[0] - 0.2),
            Vec3::new(x[1] + 0.2, 0.0, z[1] + 0.2),
        ));
        scene.add(block(
            Vec3::new(x[0] - 0.2, h, z[0] - 0.2),
            Vec3::new(x[1] + 0.2, h + 0.1, z[1] + 0.2),
        ));
        vec![
            wall(scene, [x[0], z[0]], [x[1], z[0]], h),
            wall(scene, [x[1], z[0]], [x[1], z[1]], h),
            wall(scene, [x[1], z[1]], [x[0], z[1]], h),
            wall(scene, [x[0], z[1]], [x[0], z[0]], h),
        ]
    }

    /// The test room: 6 m by 6 m, 2.6 m high, the camera's spot (0, 0)
    /// 1.5 m from the back wall, 4.5 m from the front one.
    fn room() -> (Scene, Vec<Plane>) {
        let mut scene = Scene::default();
        let planes = room_in(&mut scene, [-3.0, 3.0], [-4.5, 1.5], 2.6);
        (scene, planes)
    }

    /// About the depth camera's: 100 degrees wide.
    const FOV: Fov = Fov {
        left: -0.873,
        right: 0.873,
        up: 0.785,
        down: -0.873,
    };

    fn cam(pos: Vec3, yaw_deg: f32, pitch_deg: f32) -> DepthCamera {
        DepthCamera {
            position: pos,
            rotation: Quat::from_rotation_y(yaw_deg.to_radians())
                * Quat::from_rotation_x(pitch_deg.to_radians()),
            fov: FOV,
        }
    }

    /// The yaw and pitch (degrees) that look from `from` at `at`.
    fn aim(from: Vec3, at: Vec3) -> (f32, f32) {
        let d = at - from;
        let yaw = (-d.x).atan2(-d.z).to_degrees();
        let pitch = d.y.atan2(Vec2::new(d.x, d.z).length()).to_degrees();
        (yaw, pitch)
    }

    fn look(from: Vec3, at: Vec3) -> DepthCamera {
        let (yaw, pitch) = aim(from, at);
        cam(from, yaw, pitch)
    }

    // ---- The sensor ------------------------------------------------------

    /// The synthetic sensor: the error along the ray as measured on the
    /// headset (see [`sensor_bias`], [`sensor_spread`]), spatially smooth
    /// (white noise blurred over 5x5 grid cells, so neighbors move
    /// together as the runtime's filter makes them), holes, and mixed cells
    /// at depth steps (a blend of the two depths, as `live_planes`' tests
    /// make them).
    #[derive(Debug, Clone, Copy)]
    struct Sensor {
        noise: bool,
        holes: f32,
        mixed: f32,
    }

    const SENSOR: Sensor = Sensor {
        noise: true,
        holes: 0.02,
        mixed: 0.5,
    };

    /// The median error along the ray (m): +2 cm at 2.5 m and nearer, +0.9
    /// cm at 3.5 m and farther, linear between.
    fn sensor_bias(r: f32) -> f32 {
        let f = (r - 2.5).clamp(0.0, 1.0);
        0.02 + f * (0.009 - 0.02)
    }

    /// The spread (one deviation) of the error along the ray (m): 0.14 cm
    /// at 2.5 m and nearer, 3 cm at 3.5 m, growing at that rate beyond
    /// (5.9 cm at 4.5 m).
    fn sensor_spread(r: f32) -> f32 {
        0.0014 + (r - 2.5).max(0.0) * (0.03 - 0.0014)
    }

    /// Unit-deviation noise smooth over the grid: white noise averaged
    /// over 5x5 cells and scaled back.
    fn smooth_field(grid: usize, rng: &mut Rng, white: &mut Vec<f32>, out: &mut Vec<f32>) {
        white.clear();
        white.extend((0..grid * grid).map(|_| rng.gauss()));
        out.clear();
        out.resize(grid * grid, 0.0);
        let at = |v: i64| v.clamp(0, grid as i64 - 1) as usize;
        for row in 0..grid {
            for col in 0..grid {
                let mut s = 0.0;
                for dr in -2..=2i64 {
                    for dc in -2..=2i64 {
                        s += white[at(row as i64 + dr) * grid + at(col as i64 + dc)];
                    }
                }
                out[row * grid + col] = s / 5.0;
            }
        }
    }

    fn render(
        scene: &Scene,
        cam: &DepthCamera,
        grid: usize,
        sensor: Sensor,
        rng: &mut Rng,
        scratch: &mut (Vec<f32>, Vec<f32>),
        out: &mut Vec<Option<Vec3>>,
    ) {
        let lens = Lens::new(cam.fov, grid, grid);
        let (white, field) = scratch;
        smooth_field(grid, rng, white, field);
        out.clear();
        let o = cam.position;
        for row in 0..grid {
            for col in 0..grid {
                let (x, y) = (col as f32 + 0.5, row as f32 + 0.5);
                let dir = cam.rotation * lens.ray(x, y);
                let Some(mut t) = scene.hit(o, dir) else {
                    out.push(None);
                    continue;
                };
                if sensor.mixed > 0.0 {
                    let mut other = None;
                    for (sx, sy) in [(-0.35, -0.35), (0.35, -0.35), (-0.35, 0.35), (0.35, 0.35)] {
                        let d = cam.rotation * lens.ray(x + sx, y + sy);
                        if let Some(s) = scene.hit(o, d)
                            && (s - t).abs() > 0.05 * t
                        {
                            other = Some(s);
                        }
                    }
                    let (mix, at) = (rng.uniform(), rng.uniform());
                    if let Some(s) = other
                        && mix < sensor.mixed
                    {
                        t += (s - t) * at;
                    }
                }
                let hole = rng.uniform() < sensor.holes;
                let len = dir.length();
                let mut r = t * len;
                if sensor.noise {
                    r += sensor_bias(r) + sensor_spread(r) * field[row * grid + col];
                }
                out.push((!hole && r > 0.0).then(|| o + dir * (r / len)));
            }
        }
    }

    /// A scripted run: a scene, the reported planes, the store and the
    /// sensor, updated at `hz` on a `grid` x `grid` read-back.
    struct Sim {
        scene: Scene,
        planes: Vec<Plane>,
        fp: Footprints,
        rng: Rng,
        sensor: Sensor,
        grid: usize,
        hz: f32,
        t: f32,
        floor: Option<f32>,
        fallback: f32,
        stats: Stats,
        buf: Vec<Option<Vec3>>,
        scratch: (Vec<f32>, Vec<f32>),
    }

    impl Sim {
        /// `FOOTPRINTS_SEED=n` shifts every run's seed, to check that a
        /// pass is not one lucky draw of the noise.
        fn new(scene: Scene, planes: Vec<Plane>, seed: u64) -> Self {
            let off: u64 = std::env::var("FOOTPRINTS_SEED")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            Self {
                scene,
                planes,
                fp: Footprints::default(),
                rng: Rng(seed + 1000 * off),
                sensor: SENSOR,
                grid: 64,
                hz: 4.0,
                t: 0.0,
                floor: Some(0.0),
                fallback: 0.0,
                stats: Stats::default(),
                buf: Vec::new(),
                scratch: (Vec::new(), Vec::new()),
            }
        }

        fn step(&mut self, cam: &DepthCamera) {
            render(
                &self.scene,
                cam,
                self.grid,
                self.sensor,
                &mut self.rng,
                &mut self.scratch,
                &mut self.buf,
            );
            let obs = Observation {
                camera: *cam,
                cols: self.grid,
                rows: self.grid,
                points: &self.buf,
            };
            self.stats = self
                .fp
                .update(self.t, &obs, self.floor, self.fallback, &self.planes);
            self.t += 1.0 / self.hz;
        }

        fn footprints(&self) -> &[Footprint] {
            self.fp.footprints()
        }
    }

    fn level(v: Vec3) -> Vec2 {
        Vec2::new(v.x, v.z)
    }

    /// One line per footprint, and what happened, for failure messages.
    fn dump(fp: &Footprints) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for f in fp.footprints() {
            let _ = write!(
                out,
                "\n  id {:3} c ({:5.2} {:5.2}) {:.2} x {:.2} m yaw {:5.1} h {:.2} conf {:.2}{}",
                f.id,
                f.center.x,
                f.center.z,
                2.0 * f.half.x,
                2.0 * f.half.y,
                f.yaw.to_degrees(),
                f.height,
                f.confidence,
                if f.moving { " moving" } else { "" }
            );
        }
        let _ = write!(out, "\n  events: {:?}", fp.events);
        out
    }

    /// The angle between two yaws as rectangles, 0 to 45 degrees.
    fn yaw_error_deg(a: f32, b: f32) -> f32 {
        let d = (a - b) / FRAC_PI_2;
        ((d - d.round()) * 90.0).abs()
    }

    /// Width and depth of `f` along a box turned `yaw_deg`.
    fn sides(f: &Footprint, yaw_deg: f32) -> (f32, f32) {
        let r = f.rect().aligned(yaw_deg.to_radians());
        (2.0 * r.half.x, 2.0 * r.half.y)
    }

    // ---- The scripted timelines -----------------------------------------

    /// A 0.5 m cube turned `yaw_deg` on the floor 2 m in front of eyes
    /// `eye` high, looked at, for 1 s at 4 Hz: when the footprint was first
    /// reported, and the footprints at 1 s.
    fn cube_at(eye: f32, yaw_deg: f32, floor: Option<f32>, seed: u64) -> (Option<f32>, Sim) {
        let (mut scene, planes) = room();
        scene.add(thing(0.0, -2.0, [0.5, 0.5, 0.5], yaw_deg));
        let mut sim = Sim::new(scene, planes, seed);
        sim.floor = floor;
        let camera = look(Vec3::new(0.0, eye, 0.0), Vec3::new(0.0, 0.25, -2.0));
        let mut first = None;
        // Updates at 0, 0.25, ... up to 1 s itself.
        for _ in 0..=4 {
            let t = sim.t;
            sim.step(&camera);
            if first.is_none() && !sim.footprints().is_empty() {
                first = Some(t);
            }
        }
        (first, sim)
    }

    fn check_cube(eye: f32, yaw_deg: f32, floor: Option<f32>, seed: u64) {
        let (first, sim) = cube_at(eye, yaw_deg, floor, seed);
        let fps = sim.footprints();
        assert_eq!(fps.len(), 1, "eye {eye}, yaw {yaw_deg}: {}", dump(&sim.fp));
        let f = fps[0];
        let (w, d) = sides(&f, yaw_deg);
        let yaw_err = yaw_error_deg(f.yaw, yaw_deg.to_radians());
        let off = level(f.center).distance(Vec2::new(0.0, -2.0));
        eprintln!(
            "  cube, eye {eye:.2} m, turned {yaw_deg:2} deg: first at {:.2} s, {w:.3} x {d:.3} m, yaw off {yaw_err:.1} deg, center off {:.1} cm, height {:.2}",
            first.unwrap_or(f32::NAN),
            off * 100.0,
            f.height
        );
        assert!(first.is_some_and(|t| t <= 1.0 + 1e-3));
        assert!(
            (w - 0.5).abs() <= CELL_M && (d - 0.5).abs() <= CELL_M,
            "eye {eye}, yaw {yaw_deg}: {w} x {d}: {}",
            dump(&sim.fp)
        );
        assert!(
            yaw_err <= 10.0,
            "eye {eye}, yaw {yaw_deg}: yaw off {yaw_err}"
        );
        assert!(off <= CELL_M, "eye {eye}, yaw {yaw_deg}: center off {off}");
    }

    #[test]
    fn a_cube_at_two_meters_is_one_footprint_within_a_second() {
        let mut failed = Vec::new();
        for (k, eye) in [0.76, 1.2, 1.7].into_iter().enumerate() {
            for (j, yaw) in [0.0, 30.0].into_iter().enumerate() {
                let seed = 10 + 2 * k as u64 + j as u64;
                if std::panic::catch_unwind(|| check_cube(eye, yaw, Some(0.0), seed)).is_err() {
                    failed.push((eye, yaw));
                }
            }
        }
        assert!(failed.is_empty(), "failed (eye, yaw): {failed:?}");
    }

    #[test]
    fn with_no_floor_the_fallback_is_used_and_the_cube_still_passes() {
        for (k, eye) in [0.76, 1.2, 1.7].into_iter().enumerate() {
            check_cube(eye, 0.0, None, 20 + k as u64);
            let (_, sim) = cube_at(eye, 0.0, None, 20 + k as u64);
            assert!(!sim.stats.floor_found);
            assert_close!(sim.stats.floor_y, 0.0);
        }
    }

    #[test]
    fn an_l_shaped_pile_is_one_footprint_covering_it() {
        let (mut scene, planes) = room();
        // A long low box, a deeper one going back from its right end, and
        // a small one on top of the first.
        let a = (Vec3::new(-0.7, 0.0, -2.2), Vec3::new(0.3, 0.3, -1.8));
        let b = (Vec3::new(0.0, 0.0, -2.8), Vec3::new(0.3, 0.25, -2.2));
        scene.add(block(a.0, a.1));
        scene.add(block(b.0, b.1));
        scene.add(block(
            Vec3::new(-0.5, 0.3, -2.1),
            Vec3::new(-0.3, 0.5, -1.9),
        ));
        let mut sim = Sim::new(scene, planes, 30);
        let camera = look(Vec3::new(0.3, 1.6, 0.3), Vec3::new(-0.2, 0.2, -2.3));
        for _ in 0..6 {
            sim.step(&camera);
        }
        let fps = sim.footprints();
        assert_eq!(fps.len(), 1, "{}", dump(&sim.fp));
        let r = fps[0].rect();
        let corners = [
            (a.0.x, a.0.z),
            (a.1.x, a.0.z),
            (a.1.x, a.1.z),
            (a.0.x, a.1.z),
            (b.0.x, b.0.z),
            (b.1.x, b.0.z),
            (b.1.x, b.1.z),
            (b.0.x, b.1.z),
        ];
        let worst = corners
            .iter()
            .map(|&(x, z)| r.distance(Vec2::new(x, z)))
            .fold(0.0, f32::max);
        let (w, d) = sides(&fps[0], 0.0);
        eprintln!(
            "  L pile (bounding 1.00 x 1.00 m): {w:.2} x {d:.2} m, a corner at most {:.1} cm outside",
            worst * 100.0
        );
        assert!(worst <= CELL_M, "{}", dump(&sim.fp));
    }

    /// Two 0.4 m cubes `gap` apart side by side, the gap starting at `x0`:
    /// the footprints after 1.5 s.
    fn two_things(gap: f32, x0: f32, seed: u64) -> Sim {
        let (mut scene, planes) = room();
        scene.add(block(
            Vec3::new(x0 - 0.4, 0.0, -2.2),
            Vec3::new(x0, 0.4, -1.8),
        ));
        scene.add(block(
            Vec3::new(x0 + gap, 0.0, -2.2),
            Vec3::new(x0 + gap + 0.4, 0.4, -1.8),
        ));
        let mut sim = Sim::new(scene, planes, seed);
        let camera = look(Vec3::new(0.0, 1.4, 0.0), Vec3::new(x0, 0.2, -2.0));
        for _ in 0..6 {
            sim.step(&camera);
        }
        sim
    }

    #[test]
    fn things_half_a_meter_apart_stay_two_and_ten_centimeters_apart_become_one() {
        for (k, x0) in [0.0, 0.05, -0.03].into_iter().enumerate() {
            let sim = two_things(0.5, x0, 40 + k as u64);
            assert_eq!(
                sim.footprints().len(),
                2,
                "0.5 m at {x0}: {}",
                dump(&sim.fp)
            );
            let sim = two_things(0.1, x0, 50 + k as u64);
            let fps = sim.footprints();
            assert_eq!(fps.len(), 1, "0.1 m at {x0}: {}", dump(&sim.fp));
            let (w, d) = sides(&fps[0], 0.0);
            eprintln!(
                "  two 0.4 m cubes 0.1 m apart at {x0:5.2}: one, {w:.2} x {d:.2} m (0.90 x 0.40)"
            );
            assert!((w - 0.9).abs() <= CELL_M && (d - 0.4).abs() <= CELL_M);
        }
    }

    /// The test room with a cube, a person and a low pile.
    fn three_things() -> (Scene, Vec<Plane>, [Vec2; 3]) {
        let (mut scene, planes) = room();
        scene.add(thing(-0.9, -2.2, [0.5, 0.5, 0.5], 15.0));
        scene.add(post(0.6, -2.0, 0.18, 1.7));
        scene.add(block(Vec3::new(-0.3, 0.0, -3.2), Vec3::new(0.4, 0.2, -2.8)));
        scene.add(block(Vec3::new(-0.2, 0.2, -3.1), Vec3::new(0.1, 0.4, -2.9)));
        let at = [
            Vec2::new(-0.9, -2.2),
            Vec2::new(0.6, -2.0),
            Vec2::new(0.05, -3.0),
        ];
        (scene, planes, at)
    }

    #[test]
    fn noise_alone_keeps_ids_and_centers_still() {
        let (scene, planes, at) = three_things();
        let mut sim = Sim::new(scene, planes, 60);
        let camera = look(Vec3::new(0.0, 1.6, 0.0), Vec3::new(0.0, 0.3, -2.4));
        let mut ids: Option<Vec<u32>> = None;
        let mut last: Vec<(u32, Vec2)> = Vec::new();
        let mut fastest = 0.0f32;
        for _ in 0..40 {
            let t = sim.t;
            sim.step(&camera);
            let now: Vec<(u32, Vec2)> = sim
                .footprints()
                .iter()
                .map(|f| (f.id, level(f.center)))
                .collect();
            if t >= 1.0 - 1e-3 {
                let set: Vec<u32> = now.iter().map(|x| x.0).collect();
                let first = ids.get_or_insert_with(|| set.clone());
                assert_eq!(*first, set, "at {t:.2} s: {}", dump(&sim.fp));
                for (id, c) in &now {
                    if let Some((_, p)) = last.iter().find(|x| x.0 == *id) {
                        fastest = fastest.max(c.distance(*p) * sim.hz);
                    }
                }
            }
            last = now;
        }
        let fps = sim.footprints();
        eprintln!(
            "  noise alone, 10 s: {} footprints, ids {:?} from 1 s on, fastest center {:.2} cm/s",
            fps.len(),
            ids,
            fastest * 100.0
        );
        assert_eq!(fps.len(), 3, "{}", dump(&sim.fp));
        // Seen from one side, a footprint is the visible part: within the
        // thing's own half size of its center.
        for (p, half) in at.into_iter().zip([0.25, 0.18, 0.2]) {
            assert!(
                fps.iter().any(|f| f.rect().distance(p) < half),
                "nothing at {p}: {}",
                dump(&sim.fp)
            );
        }
        assert!(fastest <= 0.01, "a center moved {fastest} m/s");
    }

    /// A 0.4 m box sliding along x at `speed` for `secs`, centered on the
    /// view: (ids ever reported, the largest center error once reported,
    /// the farthest occupied cell behind it after 1 s).
    fn slide(speed: f32, secs: f32, seed: u64) -> (Vec<u32>, f32, f32) {
        let (mut scene, planes) = room();
        let x0 = -0.5 * speed * secs;
        let k = scene.add(thing(x0, -2.5, [0.4, 0.4, 0.4], 0.0));
        let mut sim = Sim::new(scene, planes, seed);
        let camera = look(Vec3::new(0.0, 1.6, 0.0), Vec3::new(0.0, 0.2, -2.5));
        let (mut ids, mut err, mut trail) = (Vec::new(), 0.0f32, 0.0f32);
        for _ in 0..(secs * sim.hz).round() as usize {
            let t = sim.t;
            let x = x0 + speed * t;
            sim.scene.shapes[k] = thing(x, -2.5, [0.4, 0.4, 0.4], 0.0);
            sim.step(&camera);
            for f in sim.footprints() {
                if !ids.contains(&f.id) {
                    ids.push(f.id);
                }
                err = err.max(level(f.center).distance(Vec2::new(x, -2.5)));
            }
            if t >= 1.0 {
                for c in sim.fp.occupied() {
                    trail = trail.max((x - 0.2) - c.x);
                }
            }
            assert!(ids.len() <= 1, "a second id at {t:.2} s: {}", dump(&sim.fp));
        }
        (ids, err, trail)
    }

    #[test]
    fn a_sliding_thing_keeps_one_id_and_leaves_no_trail() {
        let (ids, err, trail) = slide(0.5, 4.0, 70);
        eprintln!(
            "  sliding at 0.5 m/s for 4 s: ids {ids:?}, center within {:.1} cm, farthest cell behind {:.1} cm",
            err * 100.0,
            trail * 100.0
        );
        assert_eq!(ids.len(), 1);
        assert!(err <= 0.25, "center {err} m off");
        assert!(trail <= 0.6, "a cell {trail} m behind");
        let (ids, err, trail) = slide(1.0, 3.0, 71);
        eprintln!(
            "  sliding at 1 m/s for 3 s: ids {ids:?}, center within {:.1} cm, farthest cell behind {:.1} cm",
            err * 100.0,
            trail * 100.0
        );
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn a_removed_thing_is_dropped_in_view_and_kept_out_of_view() {
        // In view: removed at 2 s.
        let (mut scene, planes) = room();
        let k = scene.add(thing(0.0, -2.0, [0.5, 0.5, 0.5], 0.0));
        let mut sim = Sim::new(scene.clone(), planes.clone(), 80);
        let camera = look(Vec3::new(0.0, 1.6, 0.0), Vec3::new(0.0, 0.25, -2.0));
        let mut gone = None;
        for _ in 0..20 {
            let t = sim.t;
            if t >= 2.0 {
                sim.scene.shapes[k] = Shape::Gone;
            }
            sim.step(&camera);
            if (1.0..2.0).contains(&t) {
                assert_eq!(sim.footprints().len(), 1, "at {t:.2} s: {}", dump(&sim.fp));
            } else if t >= 2.0 && gone.is_none() && sim.footprints().is_empty() {
                gone = Some(t - 2.0);
            }
        }
        eprintln!(
            "  removed in view: gone after {:.2} s",
            gone.unwrap_or(f32::NAN)
        );
        assert!(gone.is_some_and(|s| s <= 1.5), "{}", dump(&sim.fp));

        // Out of view: turned away over 2..3 s, removed at 3.5 s, turned
        // back over 5..6 s.
        let mut sim = Sim::new(scene, planes, 81);
        let pos = Vec3::new(0.0, 1.6, 0.0);
        let (_, pitch) = aim(pos, Vec3::new(0.0, 0.25, -2.0));
        let yaw = |t: f32| 180.0 * ((t - 2.0).clamp(0.0, 1.0) - (t - 5.0).clamp(0.0, 1.0));
        let (mut id, mut gone) = (None, None);
        for _ in 0..36 {
            let t = sim.t;
            if t >= 3.5 {
                sim.scene.shapes[k] = Shape::Gone;
            }
            sim.step(&cam(pos, yaw(t), pitch));
            let fps = sim.footprints();
            if (1.0..6.0).contains(&t) {
                assert_eq!(fps.len(), 1, "at {t:.2} s: {}", dump(&sim.fp));
                assert_eq!(*id.get_or_insert(fps[0].id), fps[0].id);
            } else if t >= 6.0 && gone.is_none() && fps.is_empty() {
                gone = Some(t - 6.0);
            }
        }
        eprintln!(
            "  removed out of view: kept while away, gone {:.2} s after the camera was back",
            gone.unwrap_or(f32::NAN)
        );
        assert!(gone.is_some_and(|s| s <= 1.5), "{}", dump(&sim.fp));
    }

    /// `secs` of a still camera on `scene`: the most footprints at once
    /// and the most occupied cells.
    fn run_still(
        scene: Scene,
        planes: Vec<Plane>,
        camera: &DepthCamera,
        secs: f32,
        seed: u64,
    ) -> (usize, usize, Sim) {
        let mut sim = Sim::new(scene, planes, seed);
        let (mut most, mut cells) = (0, 0);
        for _ in 0..(secs * sim.hz).round() as usize {
            sim.step(camera);
            most = most.max(sim.footprints().len());
            cells = cells.max(sim.stats.occupied);
        }
        (most, cells, sim)
    }

    #[test]
    fn hanging_things_and_wall_shelves_make_no_footprint() {
        let mut scene = Scene::default();
        let planes = room_in(&mut scene, [-3.0, 3.0], [-2.5, 1.5], 2.6);
        // A 0.3 m cube hanging from 1.4 to 1.7 m.
        scene.add(block(
            Vec3::new(-0.75, 1.4, -1.85),
            Vec3::new(-0.45, 1.7, -1.55),
        ));
        // A shelf on the front wall at 1.2 m with books on it.
        scene.add(block(
            Vec3::new(0.2, 1.2, -2.5),
            Vec3::new(1.0, 1.23, -2.25),
        ));
        scene.add(block(
            Vec3::new(0.3, 1.23, -2.48),
            Vec3::new(0.8, 1.45, -2.3),
        ));
        for (k, eye) in [1.6, 1.0].into_iter().enumerate() {
            let camera = look(Vec3::new(0.0, eye, 0.3), Vec3::new(0.0, 1.0, -2.0));
            let (most, cells, sim) =
                run_still(scene.clone(), planes.clone(), &camera, 3.0, 90 + k as u64);
            eprintln!(
                "  hanging cube and wall shelf, eye {eye}: {most} footprints, {cells} cells occupied"
            );
            assert_eq!(most, 0, "{}", dump(&sim.fp));
            assert!(cells > 0);
        }
    }

    /// A table: a top at 0.72 to 0.75 m, 1.2 by 0.7 m, on four 5 cm legs;
    /// the top as the plane finder reports it.
    fn table(scene: &mut Scene, with_top: bool) -> Plane {
        let (cx, cz) = (0.0, -2.0);
        if with_top {
            scene.add(block(
                Vec3::new(cx - 0.6, 0.72, cz - 0.35),
                Vec3::new(cx + 0.6, 0.75, cz + 0.35),
            ));
        }
        for sx in [-1.0, 1.0] {
            for sz in [-1.0, 1.0] {
                let (x, z) = (cx + sx * 0.55, cz + sz * 0.3);
                scene.add(block(
                    Vec3::new(x - 0.025, 0.0, z - 0.025),
                    Vec3::new(x + 0.025, 0.72, z + 0.025),
                ));
            }
        }
        Plane {
            center: Vec3::new(cx, 0.75, cz),
            rot: Quat::from_rotation_x(-FRAC_PI_2),
            half: Vec3::new(0.6, 0.35, PLANE_HALF_THICKNESS_M),
            kind: KIND_TABLE,
        }
    }

    #[test]
    fn a_table_whose_top_is_a_plane_and_bare_legs_make_no_footprint() {
        for (k, eye) in [1.6, 1.0].into_iter().enumerate() {
            let camera = look(Vec3::new(0.3, eye, 0.0), Vec3::new(0.0, 0.4, -2.0));
            let (mut scene, mut planes) = room();
            planes.push(table(&mut scene, true));
            let (most, cells, sim) = run_still(scene, planes, &camera, 3.0, 100 + k as u64);
            eprintln!(
                "  table, its top a plane, eye {eye}: {most} footprints, {cells} cells occupied"
            );
            assert_eq!(most, 0, "{}", dump(&sim.fp));
            let (mut scene, planes) = room();
            table(&mut scene, false);
            let (most, cells, sim) = run_still(scene, planes, &camera, 3.0, 110 + k as u64);
            eprintln!("  four bare legs, eye {eye}: {most} footprints, {cells} cells occupied");
            assert_eq!(most, 0, "{}", dump(&sim.fp));
        }
    }

    #[test]
    fn a_bookcase_whose_front_is_a_plane_makes_no_footprint() {
        let mut scene = Scene::default();
        let mut planes = room_in(&mut scene, [-3.0, 3.0], [-2.5, 1.5], 2.6);
        scene.add(block(Vec3::new(-0.4, 0.0, -2.5), Vec3::new(0.4, 1.8, -2.2)));
        planes.push(Plane {
            center: Vec3::new(0.0, 0.9, -2.2),
            rot: Quat::IDENTITY,
            half: Vec3::new(0.4, 0.9, PLANE_HALF_THICKNESS_M),
            kind: KIND_WALL,
        });
        for (k, eye) in [1.6, 1.0].into_iter().enumerate() {
            let camera = look(Vec3::new(0.0, eye, 0.0), Vec3::new(0.0, 0.8, -2.2));
            let (most, cells, sim) =
                run_still(scene.clone(), planes.clone(), &camera, 3.0, 120 + k as u64);
            eprintln!(
                "  bookcase, its front a plane, eye {eye}: {most} footprints, {cells} cells occupied"
            );
            assert_eq!(most, 0, "{}", dump(&sim.fp));
        }
    }

    #[test]
    fn looking_down_at_ones_own_feet_makes_no_footprint() {
        let (mut scene, planes) = room();
        // Legs, shoes and a hand, the eyes 10 cm in front of the body.
        for x in [-0.1, 0.1] {
            scene.add(Shape::Cylinder {
                base: Vec3::new(x, 0.08, 0.1),
                radius: 0.07,
                height: 0.8,
            });
            scene.add(block(
                Vec3::new(x - 0.05, 0.0, -0.15),
                Vec3::new(x + 0.05, 0.09, 0.15),
            ));
        }
        scene.add(block(
            Vec3::new(0.2, 0.85, -0.4),
            Vec3::new(0.28, 0.95, -0.3),
        ));
        let mut sim = Sim::new(scene, planes, 130);
        let mut most = 0;
        for k in 0..16 {
            let sway = 5.0 * (k as f32 * 0.7).sin();
            sim.step(&cam(Vec3::new(0.0, 1.6, 0.0), sway, -75.0 + sway));
            most = most.max(sim.footprints().len());
        }
        eprintln!(
            "  looking down at the feet: {most} footprints, {} own points in the last update",
            sim.stats.own
        );
        assert_eq!(most, 0, "{}", dump(&sim.fp));
        assert!(sim.stats.own > 0);
    }

    #[test]
    fn turning_away_and_back_keeps_the_footprint_and_nearest_answers() {
        let (mut scene, planes) = room();
        scene.add(thing(0.0, -2.0, [0.5, 0.5, 0.5], 0.0));
        let mut sim = Sim::new(scene, planes, 140);
        let pos = Vec3::new(0.0, 1.6, 0.0);
        // Look, turn away over 2 s, hold 2 s, turn back over 2 s, hold 1 s.
        let yaw = |t: f32| 90.0 * ((t - 1.5).clamp(0.0, 2.0) - (t - 5.5).clamp(0.0, 2.0));
        let mut id = None;
        let spot = Vec3::new(0.0, 0.0, -2.0);
        for _ in 0..34 {
            let t = sim.t;
            sim.step(&cam(pos, yaw(t), -25.0));
            let fps = sim.footprints();
            if t >= 1.0 {
                assert_eq!(
                    fps.len(),
                    1,
                    "at {t:.2} s, yaw {}: {}",
                    yaw(t),
                    dump(&sim.fp)
                );
                let id = *id.get_or_insert(fps[0].id);
                assert_eq!(fps[0].id, id, "at {t:.2} s: {}", dump(&sim.fp));
                let near = sim.fp.nearest(spot);
                assert!(
                    near.is_some_and(|(n, d)| n == id && d < 0.05),
                    "at {t:.2} s: nearest {near:?}"
                );
            }
        }
        eprintln!("  turned away and back: id {id:?} held throughout");
    }

    #[test]
    fn ten_things_in_view_report_at_most_eight_stably() {
        let (mut scene, planes) = room();
        let mut at = Vec::new();
        // 0.4 m boxes 0.5 m apart in two rows 0.5 m apart, 1.8 to 3.2 m
        // away.
        for z in [-1.5, -2.4] {
            for x in [-1.8, -0.9, 0.0, 0.9, 1.8] {
                scene.add(thing(x, z, [0.4, 0.4, 0.4], 0.0));
                at.push(Vec2::new(x, z));
            }
        }
        let mut sim = Sim::new(scene, planes, 150);
        let camera = look(Vec3::new(0.0, 1.6, 0.3), Vec3::new(0.0, 0.2, -1.95));
        // (The camera sees x = ±1.8 at 45 degrees off its axis.)
        let mut ids: Option<Vec<u32>> = None;
        for _ in 0..40 {
            let t = sim.t;
            sim.step(&camera);
            let fps = sim.footprints();
            assert!(
                fps.len() <= MAX_FOOTPRINTS,
                "at {t:.2} s: {}",
                dump(&sim.fp)
            );
            if t >= 2.0 {
                let set: Vec<u32> = fps.iter().map(|f| f.id).collect();
                assert_eq!(
                    *ids.get_or_insert_with(|| set.clone()),
                    set,
                    "at {t:.2} s: {}",
                    dump(&sim.fp)
                );
                for p in &at {
                    assert!(
                        fps.iter().any(|f| f.rect().distance(*p) < 0.2),
                        "at {t:.2} s nothing covers {p}: {}",
                        dump(&sim.fp)
                    );
                }
            }
        }
        eprintln!(
            "  ten things: {} tracked, {} reported, {} of them merged pairs",
            sim.stats.tracked, sim.stats.reported, sim.stats.merged
        );
        assert!(sim.stats.tracked >= 10);
        assert_eq!(sim.stats.reported, MAX_FOOTPRINTS);
    }

    #[test]
    fn walls_without_planes_make_no_footprint() {
        let mut scene = Scene::default();
        room_in(&mut scene, [-2.5, 2.5], [-3.0, 2.0], 2.6);
        for (k, eye) in [1.6, 1.2].into_iter().enumerate() {
            let mut sim = Sim::new(scene.clone(), Vec::new(), 160 + k as u64);
            let mut most = 0;
            for _ in 0..32 {
                let t = sim.t;
                sim.step(&cam(Vec3::new(0.0, eye, -0.5), 45.0 * t, -10.0));
                most = most.max(sim.footprints().len());
            }
            eprintln!("  walls, no planes, eye {eye}, a full turn: {most} footprints");
            assert_eq!(most, 0, "{}", dump(&sim.fp));
        }
    }

    #[test]
    fn with_the_plane_finder_a_person_is_a_footprint_and_walls_are_not() {
        let (mut scene, _) = room();
        scene.add(post(0.5, -2.0, 0.18, 1.7));
        // A pile of small boxes, no face big enough for a plane.
        for (k, x) in [-0.9, -0.75, -0.6].into_iter().enumerate() {
            let h = 0.12 + 0.06 * k as f32;
            scene.add(block(
                Vec3::new(x - 0.07, 0.0, -2.3),
                Vec3::new(x + 0.07, h, -2.1),
            ));
        }
        let mut sim = Sim::new(scene, Vec::new(), 170);
        let mut finder = LivePlanes::default();
        let camera = look(Vec3::new(0.0, 1.6, 0.0), Vec3::new(-0.2, 0.4, -2.2));
        let things = [Vec2::new(0.5, -2.0), Vec2::new(-0.75, -2.2)];
        let mut stray = 0;
        for _ in 0..20 {
            render(
                &sim.scene,
                &camera,
                sim.grid,
                sim.sensor,
                &mut sim.rng,
                &mut sim.scratch,
                &mut sim.buf,
            );
            let obs = Observation {
                camera,
                cols: sim.grid,
                rows: sim.grid,
                points: &sim.buf,
            };
            finder.update(sim.t, &obs);
            sim.planes = finder.planes().iter().map(Plane::from_live).collect();
            sim.stats = sim
                .fp
                .update(sim.t, &obs, finder.floor_y(), 0.0, &sim.planes);
            sim.t += 0.25;
            stray += sim
                .footprints()
                .iter()
                .filter(|f| things.iter().all(|&p| f.rect().distance(p) > 0.3))
                .count();
        }
        let fps = sim.footprints();
        eprintln!(
            "  with the plane finder ({} planes): {} footprints, {stray} away from the things over 5 s{}",
            finder.planes().len(),
            fps.len(),
            dump(&sim.fp)
        );
        // Seen from the front, the person's footprint is its visible front
        // half: it covers the cylinder's front, not its center.
        let front = things[0] + Vec2::new(0.0, 0.18);
        assert!(
            fps.iter().any(|f| f.rect().distance(front) < 0.05),
            "{}",
            dump(&sim.fp)
        );
        assert_eq!(stray, 0);
    }

    // ---- Parts ------------------------------------------------------------

    #[test]
    fn uuids_are_tagged_and_round_trip() {
        let u = footprint_uuid(0x0102_0304);
        assert_eq!(&u[..8], b"fsfoot\0\0");
        assert_eq!(footprint_id(&u), Some(0x0102_0304));
        assert_eq!(plane_id(&u), None);
        assert_eq!(footprint_id(&plane_uuid(7)), None);
        assert_eq!(footprint_id(&[0; 16]), None);
        assert_ne!(footprint_uuid(0), [0; 16]);
        // Version nibble and variant bits zero: no runtime's UUID.
        assert_eq!(u[6] >> 4, 0);
        assert_eq!(u[8] >> 6, 0);
    }

    #[test]
    fn an_obstacle_box_is_a_curb_on_the_floor() {
        let f = Footprint {
            id: 9,
            center: Vec3::new(1.0, -0.03, -2.0),
            yaw: 0.3,
            half: Vec2::new(0.4, 0.2),
            height: 0.8,
            confidence: 0.9,
            moving: false,
            since_s: 1.0,
        };
        let b = to_obstacle_box(&f);
        assert_close!(b.center, [1.0, -0.03 + 0.5 * CURB_M, -2.0]);
        assert_close!(b.half, [0.4, 0.5 * CURB_M, 0.2]);
        assert_close!(b.rot, Quat::from_rotation_y(0.3).to_array());
        assert_eq!(b.kind, KIND_OTHER);
        assert_eq!(b.uuid, footprint_uuid(9));
        assert!(!is_plane(Vec3::from_array(b.half)));
        // Its corners are the footprint's.
        let q = Quat::from_array(b.rot);
        let corner = Vec3::from_array(b.center) + q * Vec3::new(b.half[0], -b.half[1], b.half[2]);
        assert!(f.corners().iter().any(|c| c.distance(corner) < 1e-5));
    }

    #[test]
    fn the_rectangle_fit_finds_the_yaw_and_keeps_a_square_still() {
        let (mut pts, mut hull) = (Vec::new(), Vec::new());
        let truth = Rect2 {
            c: Vec2::new(1.0, -2.0),
            yaw: 20f32.to_radians(),
            half: Vec2::new(0.5, 0.2),
        };
        pts.extend_from_slice(&truth.corners());
        pts.push(truth.c);
        let r = fit_rect(&mut pts, &mut hull, None).unwrap();
        assert!(yaw_error_deg(r.yaw, truth.yaw) < 0.01);
        assert!(r.c.distance(truth.c) < 1e-4);
        assert!((r.area() - truth.area()).abs() < 1e-4);
        // A square keeps a yaw it was given when that costs little area...
        let square = Rect2 {
            half: Vec2::splat(0.25),
            yaw: 0.0,
            ..truth
        };
        pts.clear();
        pts.extend_from_slice(&square.corners());
        let r = fit_rect(&mut pts, &mut hull, Some(2f32.to_radians())).unwrap();
        assert!(yaw_error_deg(r.yaw, 2f32.to_radians()) < 0.01);
        // ...and not when it costs much.
        pts.clear();
        pts.extend_from_slice(&square.corners());
        let r = fit_rect(&mut pts, &mut hull, Some(20f32.to_radians())).unwrap();
        assert!(yaw_error_deg(r.yaw, 0.0) < 0.01);
        assert!(fit_rect(&mut [], &mut hull, None).is_none());
    }

    #[test]
    fn follow_holds_in_its_dead_band_and_keeps_to_its_speed() {
        assert_close!(follow(0.05, 0.1, 1.0, 0.25, FOLLOW_GAIN), 0.0);
        let s = follow(1.0, 0.1, 0.008, 0.25, FOLLOW_GAIN);
        assert!(s > 0.0 && s <= 0.008 * 0.25 + 1e-7);
        // 0.9 past the band at gain 8 over 0.25 s would be 0.78: the
        // limit, 2 m/s, holds it to 0.5.
        assert_close!(follow(-1.0, 0.1, 2.0, 0.25, MOVE_GAIN), -0.5);
    }

    /// The synthetic sensor gives the measured figures back: median and
    /// spread (1.4826 times the median absolute deviation) of the error
    /// along the ray at 2.5 and 3.5 m.
    #[test]
    fn the_synthetic_sensor_matches_the_measured_noise() {
        let mut rng = Rng(5);
        let (mut white, mut field) = (Vec::new(), Vec::new());
        for (r, med, sd) in [(2.5, 0.02, 0.0014), (3.5, 0.009, 0.03)] {
            let mut errs = Vec::new();
            for _ in 0..8 {
                smooth_field(64, &mut rng, &mut white, &mut field);
                errs.extend(field.iter().map(|n| sensor_bias(r) + sensor_spread(r) * n));
            }
            errs.sort_unstable_by(f32::total_cmp);
            let m = errs[errs.len() / 2];
            let mut dev: Vec<f32> = errs.iter().map(|e| (e - m).abs()).collect();
            dev.sort_unstable_by(f32::total_cmp);
            let s = 1.4826 * dev[dev.len() / 2];
            eprintln!(
                "  sensor at {r} m: median {:+.2} cm, spread {:.2} cm",
                m * 100.0,
                s * 100.0
            );
            assert!((m - med).abs() < 0.1 * sd + 1e-4);
            assert!((s / sd - 1.0).abs() < 0.1);
        }
    }

    #[test]
    fn no_heap_growth_after_warm_up() {
        let (scene, planes, _) = three_things();
        let mut sim = Sim::new(scene, planes, 180);
        let pos = Vec3::new(0.0, 1.6, 0.0);
        let sweep = |t: f32| cam(pos, 40.0 * (t * std::f32::consts::FRAC_PI_2).sin(), -25.0);
        for _ in 0..32 {
            let t = sim.t;
            sim.step(&sweep(t));
        }
        let warm = sim.fp.capacity();
        for _ in 0..64 {
            let t = sim.t;
            sim.step(&sweep(t));
        }
        assert_eq!(sim.fp.capacity(), warm);
    }

    /// The cost of an update, rendering left out. `cargo test --release -p
    /// fosfora-xr footprints::tests::update_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "a timing run; read its output"]
    fn update_cost() {
        let (scene, planes, _) = three_things();
        let pos = Vec3::new(0.0, 1.6, 0.0);
        for grid in [40, 64, 80] {
            let mut sim = Sim::new(scene.clone(), planes.clone(), 190);
            sim.grid = grid;
            let frames: Vec<(DepthCamera, Vec<Option<Vec3>>)> = (0..200)
                .map(|k| {
                    let camera = cam(pos, 30.0 * (k as f32 * 0.05).sin(), -25.0);
                    let mut buf = Vec::new();
                    render(
                        &scene,
                        &camera,
                        grid,
                        SENSOR,
                        &mut sim.rng,
                        &mut sim.scratch,
                        &mut buf,
                    );
                    (camera, buf)
                })
                .collect();
            let mut fp = Footprints::default();
            let mut times = Vec::with_capacity(frames.len());
            for (k, (camera, points)) in frames.iter().enumerate() {
                let obs = Observation {
                    camera: *camera,
                    cols: grid,
                    rows: grid,
                    points,
                };
                let t0 = std::time::Instant::now();
                std::hint::black_box(fp.update(k as f32 * 0.25, &obs, Some(0.0), 0.0, &planes));
                times.push(t0.elapsed().as_secs_f64() * 1e3);
            }
            let warm = &mut times[20..];
            warm.sort_by(f64::total_cmp);
            let mean = warm.iter().sum::<f64>() / warm.len() as f64;
            eprintln!(
                "{grid}x{grid}: mean {mean:.3} ms, median {:.3}, max {:.3} over {} updates, {} footprints",
                warm[warm.len() / 2],
                warm[warm.len() - 1],
                warm.len(),
                fp.footprints().len()
            );
        }
    }

    /// The plane finder and the footprints on a recording made on the
    /// headset, as the worker would run them: `LIVE_PLANES_REC` names the
    /// file. Recordings are of real rooms and are never committed.
    #[test]
    #[ignore = "needs a recording: LIVE_PLANES_REC=<file.lprec>"]
    fn replay_footprints_in_live_planes_rec() {
        use crate::live_planes::unproject_grid;
        use crate::live_wire::{Fitter, Job};

        let path = std::env::var("LIVE_PLANES_REC")
            .unwrap_or_else(|_| panic!("set LIVE_PLANES_REC to a recording's path"));
        let rec =
            crate::live_rec::read(std::path::Path::new(&path)).unwrap_or_else(|e| panic!("{e:#}"));
        println!(
            "{path}: {} read-backs, grid {}x{}",
            rec.records.len(),
            rec.cols,
            rec.rows
        );
        let mut fitter = Fitter::default();
        let mut fp = Footprints::default();
        let (mut points, mut planes) = (Vec::new(), Vec::new());
        let mut seen: Vec<Footprint> = Vec::new();
        let mut ms = Vec::new();
        for r in &rec.records {
            let job = Job {
                epoch: 1,
                time_s: r.time_s,
                grid: r.grid.clone(),
                reading: rec.reading,
                fit: true,
                noise: None,
                record: None,
            };
            let Some(snap) = fitter.run(&job) else {
                continue;
            };
            let g = &r.grid;
            unproject_grid(
                &g.depth,
                g.n,
                g.n,
                g.size,
                &g.view,
                g.near,
                g.far,
                rec.reading,
                &mut points,
            );
            let obs = Observation {
                camera: DepthCamera::from_view(&g.view),
                cols: g.n,
                rows: g.n,
                points: &points,
            };
            planes.clear();
            planes.extend(snap.planes.iter().map(Plane::from_live));
            // The stage floor's height until the finder has a floor: 0.
            let t0 = std::time::Instant::now();
            let s = fp.update(r.time_s, &obs, snap.floor_y, 0.0, &planes);
            ms.push(t0.elapsed().as_secs_f64() * 1e3);
            let ids: Vec<u32> = fp.footprints().iter().map(|f| f.id).collect();
            println!(
                "{:7.2} s  planes {:2}  floor {:6.2}{}  stuff {:4} bare {:4} on planes {:4}  cells {:4} occupied {:3} groups {:2} tracked {:2}  footprints {} {ids:?}",
                r.time_s,
                snap.planes.len(),
                s.floor_y,
                if s.floor_found { "" } else { "?" },
                s.stuff,
                s.bare,
                s.on_plane,
                s.cells,
                s.occupied,
                s.groups,
                s.tracked,
                s.reported,
            );
            for f in fp.footprints() {
                match seen.iter_mut().find(|x| x.id == f.id) {
                    Some(x) => *x = *f,
                    None => seen.push(*f),
                }
            }
        }
        let mean = ms.iter().sum::<f64>() / ms.len().max(1) as f64;
        let max = ms.iter().copied().fold(0.0, f64::max);
        println!("update: mean {mean:.3} ms, max {max:.3} ms");
        println!("footprints ever reported (last state):");
        for f in &seen {
            println!(
                "  id {:3}  center ({:6.2} {:6.2} {:6.2})  {:.2} x {:.2} m  yaw {:5.1} deg  height {:.2} m  first seen {:.2} s",
                f.id,
                f.center.x,
                f.center.y,
                f.center.z,
                2.0 * f.half.x,
                2.0 * f.half.y,
                f.yaw.to_degrees(),
                f.height,
                f.since_s
            );
        }
    }
}
