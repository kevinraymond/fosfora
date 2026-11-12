//! Live planes from the depth map (board #5391): a small, stable set of
//! planar surfaces in world space, kept from the depth samples of each
//! update, so the surface effects have something to draw on where the
//! room scan has nothing. Plain numbers in (glam only), so it builds and
//! tests on the desktop; wiring it to the depth read-back is a later step.
//!
//! **Input.** One [`Observation`] per update: the depth camera's pose and
//! fov, and a row-major grid of world points (row 0 at the top of the
//! view), `None` where the map has no depth. [`unproject_grid`] makes that
//! grid from stored depth values the way `env_depth`'s read-back samples
//! them. Points nearer than [`NEAR_M`] (hands, the wearer's body) or past
//! the configured range ([`MAX_RANGE_M`] by default) are ignored.
//!
//! **Fit, per update: region growing over the grid.** A cell holding a
//! blend of two depths at a silhouette is left out. Each cell gets a normal
//! from a least-squares plane through a window of its neighbors, as wide as
//! the noise needs ([`WINDOW_NOISE_FRACS`]): the centered window when it is
//! flat, else the flattest of four with the cell at a corner, so a cell
//! beside a crease or a silhouette takes its own surface's. Seeds are taken
//! flattest first; a region takes a 4-neighbor whose normal is within
//! [`GROW_ANGLE_DEG`] of the region's and whose point is within 2.5 noise
//! deviations of the region's plane, refitting that plane as it grows.
//! Region growing rather than sequential RANSAC: the grid already says
//! which samples touch, so two coplanar surfaces with a gap between them
//! stay apart, a hand in front of a wall does not join it, the cost is one
//! pass over the cells with no random sampling, and the result is the same
//! every time for the same input. A region is a patch when it has
//! [`MIN_CELLS`] cells, [`MIN_AREA_M2`] of surface and is
//! [`MIN_SPREAD_CELLS`] wide; its plane is a fit weighted by each point's
//! inverse noise variance, with the noise's own scatter (along each ray)
//! taken out, which otherwise tilts a small far plane toward the rays.
//!
//! **Noise model.** One standard deviation along the ray is [`NOISE_K`]
//! times the squared range: 1% of the range at 1 m, 4% at 4 m. Every
//! distance threshold scales with it.
//!
//! **Persist and merge.** A patch merges into a tracked plane whose normal
//! is within [`MERGE_ANGLE_DEG`], whose plane passes within
//! [`MERGE_OFFSET_M`] of the patch's centroid, and whose rectangle overlaps
//! or touches ([`TOUCH_M`]) the patch's. A tracked plane keeps the
//! weighted moments of every point merged into it, so its plane is the fit
//! of all of them (a confidence-weighted average: near points count more
//! than far ones). Its extent is an oriented rectangle on the plane, wall
//! rectangles upright; each edge follows the median of its last
//! [`EDGE_HISTORY`] observations, moves only by more than a cell, and
//! shrinks only while it is in view. Tracked planes that come to overlap
//! merge too. A patch that merges nowhere starts a new plane, unless it lies
//! over one just outside the merge angle ([`DUPLICATE_SCALE`]). Ids are
//! never reused.
//!
//! **Stability.** A plane is reported once it has had patches in
//! [`APPEAR_FRAC`] of its updates over [`APPEAR_S`], one of them strong
//! ([`CREATE_NORMAL_SD_DEG`]); one that misses is held back until the view
//! changes, so a surface found only now and then does not pop in while the
//! wearer stands still. A reported plane's rectangle follows the estimate
//! with a dead band and a rate limit on its normal, its offset and each
//! edge: fast while the camera moves, about a centimeter a second while it
//! is still. Each update, a 4x4 grid of points on every tracked plane is
//! checked against the depth map: a plane in view whose points the map sees
//! through, with no patch, for [`VANISH_S`] is dropped. A plane out of view,
//! out of range or hidden behind something nearer is kept as it was. A
//! plane farther than [`FORGET_M`] from the camera is dropped.
//!
//! **Kinds.** Normal up: the floor near the floor estimate, a table 0.3 to
//! 1.3 m above it, else other. Normal down above [`CEILING_MIN_M`]: the
//! ceiling. Normal within [`WALL_DEG`] of horizontal and reaching
//! [`WALL_MIN_TOP_M`] above the floor: a wall (a box side or a cabinet
//! front is other). Anything else: other. The floor estimate is a slow
//! filter of the lowest large horizontal plane well below the camera,
//! since the tracking origin's `y = 0` need not be the floor.
//!
//! **Budget.** At most [`LivePlanesConfig::max_planes`] are reported, by a
//! score of confidence, area and nearness, with the reported ones favored
//! by [`KEEP_BONUS`] so the set does not thrash at the limit. At most
//! [`MAX_TRACKED`] are tracked.

use glam::{DVec3, Mat3, Quat, Vec3};

use crate::env_depth::{DepthView, Reading, decode_distance, texel_dir};
use crate::math::Fov;
use crate::surfaces::{
    KIND_CEILING, KIND_FLOOR, KIND_OTHER, KIND_TABLE, KIND_WALL, PLANE_HALF_THICKNESS_M,
};

// ---- Input -------------------------------------------------------------

/// Samples nearer than this to the camera are ignored (m): hands and body.
pub const NEAR_M: f32 = 0.3;
/// Samples farther than this are ignored by default (m).
pub const MAX_RANGE_M: f32 = 4.5;
/// The noise the fit assumes: one standard deviation along the ray is this
/// times the squared range (m⁻¹), 1% of the range at 1 m and 4% at 4 m.
pub const NOISE_K: f32 = 0.01;
/// The noise never assumed below this (m).
pub const NOISE_MIN_M: f32 = 0.005;

// ---- Per-update fit ----------------------------------------------------

/// A cell's normal comes from a window as wide as the noise needs: 3x3
/// while the noise deviation is under the first of these fractions of a
/// cell's width at its range, 5x5 under the second, 7x7 under the third,
/// else 9x9. At each limit the normal's error is 9 degrees on average and
/// under 18 in 19 cells of 20 (simulated), well inside
/// [`GROW_ANGLE_DEG`]; smaller windows keep small surfaces. On the 40x40
/// grid: 3x3 to 1.6 m, 5x5 to 3.8 m, 7x7 beyond.
pub const WINDOW_NOISE_FRACS: [f32; 3] = [0.3, 0.7, 1.2];
/// A window is well filled with points in at least this fraction of its
/// cells.
pub const WINDOW_FILL: f64 = 0.6;
/// A region does not take a neighbor across a depth jump: a range more
/// than this fraction of the cell's apart, plus 3 noise deviations.
pub const JUMP_FRAC: f32 = 0.12;
/// A seed's flatness: the variance off its local plane over the noise
/// variance, at most this.
pub const SEED_FLATNESS: f32 = 2.0;
/// A region takes a neighbor whose normal is within this of the region's.
pub const GROW_ANGLE_DEG: f32 = 30.0;
/// ...and whose point is within this many noise deviations of the region's
/// plane, never under [`GROW_TOL_MIN_M`]. The noise is along the ray, so
/// off the plane it is the deviation times the cosine of the incidence,
/// taken as at least [`NOISE_MIN_COS`].
pub const GROW_SIGMAS: f32 = 2.5;
pub const GROW_TOL_MIN_M: f32 = 0.02;
pub const NOISE_MIN_COS: f32 = 0.35;
/// A growing region refits its plane at 12, 24, 48... cells, when the
/// fit's normal has a standard error under this (degrees).
pub const REFIT_NORMAL_SD_DEG: f32 = 3.0;
/// A patch needs this many cells...
pub const MIN_CELLS: usize = 10;
/// ...and this much surface (m²): a hand is about 0.02.
pub const MIN_AREA_M2: f32 = 0.05;
/// A patch starts a new plane only when its normal's standard error is
/// under this (degrees) out to [`CREATE_NEAR_M`], and has [`MIN_CELLS`]
/// cells; past that range each sample is worth less, so the limit falls by
/// [`CREATE_SD_PER_M`] a meter, to at least [`CREATE_SD_MIN_DEG`], and the
/// cells needed rise by [`CREATE_CELLS_PER_M`]. A plane appears only after
/// one such patch; a looser one supports a tracked plane and starts one,
/// decided like any other (see [`APPEAR_S`]).
pub const CREATE_NORMAL_SD_DEG: f32 = 6.0;
pub const CREATE_NEAR_M: f32 = 2.0;
pub const CREATE_SD_PER_M: f32 = 2.0;
pub const CREATE_SD_MIN_DEG: f32 = 2.0;
pub const CREATE_CELLS_PER_M: f32 = 10.0;
/// A patch's narrower in-plane spread (standard deviation) must be at least
/// this many cell widths at its range (about 3 cells across): a strip along
/// a crease or the edge of the view fits a plane at any tilt.
pub const MIN_SPREAD_CELLS: f32 = 0.9;
/// A patch's extent along an axis drops this fraction of its outermost
/// points on each side (1 in 50), so a stray point does not stretch it.
pub const TRIM_DIV: usize = 50;
/// See `extents`.
pub const EXTENT_SIGMAS: f32 = 2.5;

// ---- Merging -----------------------------------------------------------

/// A patch merges into a tracked plane whose normal is within this...
pub const MERGE_ANGLE_DEG: f32 = 10.0;
/// ...whose plane passes within this of the patch's centroid (m)...
pub const MERGE_OFFSET_M: f32 = 0.08;
/// ...and whose rectangle overlaps the patch's or comes within this (m).
pub const TOUCH_M: f32 = 0.05;
/// A patch overlapping or touching a tracked plane within this many times
/// the merge angle (and within the merge offset), but not within the angle
/// itself, starts no new plane.
pub const DUPLICATE_SCALE: f32 = 1.5;
/// An edge of a tracked rectangle follows the median of the last this many
/// observations of it (see `Tracked::apply_seen`)...
pub const EDGE_HISTORY: usize = 8;
/// ...when that is more than this many cell widths (at its range) from
/// where the edge is. While the camera turns faster than [`MOVING_DEG_S`]
/// or moves faster than [`MOVING_M_S`], an edge also grows on two updates
/// in a row.
pub const EDGE_BAND_CELLS: f32 = 1.0;
pub const MOVING_DEG_S: f32 = 5.0;
pub const MOVING_M_S: f32 = 0.1;
/// A plane's accumulated weight is held under this (a 1 m sample weighs
/// 1), so it can still follow a slow change.
pub const WEIGHT_CAP: f64 = 5000.0;

// ---- Life --------------------------------------------------------------

/// A plane is reported once it has had this many patches, one of them
/// strong (see [`CREATE_NORMAL_SD_DEG`])...
pub const APPEAR_HITS: u32 = 2;
/// ...in at least this fraction of the updates since it was first seen...
pub const APPEAR_FRAC: f32 = 0.7;
/// ...over at least this long (s). A candidate that misses this when its
/// time is up is held back, still taking patches, and decided again only
/// once the camera has moved [`REDECIDE_MOVE_M`] or turned
/// [`REDECIDE_TURN_DEG`] from where it was decided: a surface found in
/// some updates and not others does not pop in at a random moment while
/// the wearer stands still.
pub const APPEAR_S: f32 = 0.5;
pub const REDECIDE_MOVE_M: f32 = 0.5;
pub const REDECIDE_TURN_DEG: f32 = 30.0;
/// A plane first seen after the camera has been still for
/// [`STILL_GRACE_S`] was missed by every update before it, so it is either
/// new in the room or found only now and then: it needs patches in
/// [`APPEAR_STILL_FRAC`] of its updates over [`APPEAR_STILL_S`].
pub const STILL_GRACE_S: f32 = 0.5;
pub const APPEAR_STILL_S: f32 = 2.0;
pub const APPEAR_STILL_FRAC: f32 = 0.9;
/// A plane in view whose samples the map does not see for this long is
/// dropped (s).
pub const VANISH_S: f32 = 2.0;
/// A plane not yet reported with no patch for this long is dropped (s);
/// a held one (see [`APPEAR_S`]) with nothing seen of it for this long.
pub const CANDIDATE_TTL_S: f32 = 1.0;
/// Planes farther than this from the camera are dropped (m).
pub const FORGET_M: f32 = 8.0;
/// An update's time step never counts for more than this (s), so a pause
/// does not age every plane at once.
pub const MAX_DT_S: f32 = 1.0;
/// The plane-sample check: a sample counts only when the view meets the
/// plane at least this steeply (the cosine of the incidence)...
pub const EVIDENCE_MIN_COS: f32 = 0.15;
/// ...a sample agrees when the map's range along its cell's ray is within
/// this many noise deviations of the plane's, never under
/// [`EVIDENCE_TOL_MIN_M`]; farther, the map sees through the plane; nearer,
/// something hides it and the sample says nothing...
pub const EVIDENCE_SIGMAS: f32 = 3.0;
pub const EVIDENCE_TOL_MIN_M: f32 = 0.04;
/// ...a plane is supported without a patch when at least this many of its
/// samples agree and no more see through it...
pub const AGREE_MIN: u32 = 2;
/// ...and is in view when at least this many samples were checked.
pub const MIN_EVIDENCE: u32 = 3;
/// Patches for the confidence to reach 1 - 1/e.
pub const CONF_HITS: f32 = 3.0;

// ---- Kinds -------------------------------------------------------------

/// A plane is horizontal when its normal is within this of vertical
/// (degrees).
pub const HORIZONTAL_DEG: f32 = 15.0;
/// A plane is vertical when its normal is within this of horizontal.
pub const WALL_DEG: f32 = 15.0;
/// A vertical plane is a wall when its top reaches this above the floor
/// (m); lower, a box side or a cabinet front, it is other.
pub const WALL_MIN_TOP_M: f32 = 1.2;
/// An upward plane within this of the floor estimate is floor (m).
pub const FLOOR_TOL_M: f32 = 0.15;
/// An upward plane this high above the floor is a table (m).
pub const TABLE_MIN_M: f32 = 0.3;
pub const TABLE_MAX_M: f32 = 1.3;
/// A downward plane this high above the floor is the ceiling (m).
pub const CEILING_MIN_M: f32 = 1.9;
/// A kind boundary a plane already sits on one side of is moved this much
/// further out (m), so a plane near a boundary does not flip.
pub const KIND_HYSTERESIS_M: f32 = 0.05;
/// A reported plane changes kind only after the new kind has held this
/// long (s).
pub const KIND_HOLD_S: f32 = 1.0;
/// The floor estimate takes upward planes of at least this area (m²; the
/// first from a single patch, then from planes seen twice)...
pub const FLOOR_MIN_AREA_M2: f32 = 1.0;
/// ...at least this far below the camera (m): a seated head is about
/// 1.2 m up, a table top about 0.5 m below it.
pub const FLOOR_BELOW_EYE_M: f32 = 0.9;
/// ...follows a candidate up to this much above it (m; a step) or any
/// amount below...
pub const FLOOR_TRACK_M: f32 = 0.3;
/// ...with this time constant (s).
pub const FLOOR_TAU_S: f32 = 2.0;
/// Before any floor is seen, it is taken this far below the camera (m).
pub const FLOOR_GUESS_BELOW_EYE_M: f32 = 1.4;

// ---- Output ------------------------------------------------------------

/// The reported planes by default: room in the 32-box budget for the
/// scanned boxes, the hands and the stage floor.
pub const MAX_PLANES: usize = 24;
/// Planes tracked at most, reported or not.
pub const MAX_TRACKED: usize = 64;
/// A reported plane's score is raised by this fraction when the budget is
/// redrawn, so a newcomer has to beat it clearly: twice the score, as a
/// plane at a third of the distance has. A rectangle still growing to its
/// surface's size after it appeared raises its score by less.
pub const KEEP_BONUS: f32 = 1.0;
/// See `Tracked::score`.
pub const SCORE_CONF_CAP: f32 = 0.5;
/// A reported normal turns only past this dead band (degrees)...
pub const TURN_DEADBAND_DEG: f32 = 0.5;
/// ...at most this fast (degrees per second) while the camera moves, and
/// [`TURN_STILL_DEG_S`] while it is still.
pub const TURN_MAX_DEG_S: f32 = 10.0;
pub const TURN_STILL_DEG_S: f32 = 0.5;
/// A reported plane's offset moves only past this dead band (m)...
pub const OFFSET_DEADBAND_M: f32 = 0.005;
/// ...at most this fast (m/s) while the camera moves, and
/// [`OFFSET_STILL_M_S`] while it is still.
pub const OFFSET_MAX_M_S: f32 = 0.1;
pub const OFFSET_STILL_M_S: f32 = 0.003;
/// A reported edge moves only past this dead band (m), or past this many
/// cell widths at the edge's range if more: which cells belong to a patch
/// at its rim changes from update to update, and an edge should not
/// follow that...
pub const EDGE_DEADBAND_M: f32 = 0.03;
pub const EDGE_DEADBAND_CELLS: f32 = 1.0;
/// ...at most this fast (m/s) while the camera moves (see
/// [`MOVING_DEG_S`]), and [`EDGE_STILL_M_S`] while it is still: a plane
/// still settling after it appeared then creeps instead of sliding, so
/// a still wearer sees nothing move faster than about a centimeter a
/// second.
pub const EDGE_MAX_M_S: f32 = 1.0;
pub const EDGE_STILL_M_S: f32 = 0.008;
/// Past its dead band, a reported value closes this fraction of the gap
/// per second (as `1 - exp(-gain * dt)`), within its rate limit.
pub const FOLLOW_GAIN: f32 = 4.0;
/// The first eight bytes of every live plane's UUID: `fslive` and two
/// zero bytes. The zero version nibble (byte 6) and the zero variant bits
/// (byte 8, the id's high byte) are a layout no runtime's random UUID has,
/// and the prefix keeps it off the stage floor's all-zero UUID.
pub const UUID_PREFIX: [u8; 8] = *b"fslive\0\0";

/// The noise the fit assumes at range `r` (m): one standard deviation.
pub fn noise_sigma(r: f32) -> f32 {
    (NOISE_K * r * r).max(NOISE_MIN_M)
}

/// How far off a plane a sample at range `r` may lie and still be on it,
/// seen at incidence cosine `cos` (m).
fn plane_tol(r: f32, cos: f32) -> f32 {
    (GROW_SIGMAS * noise_sigma(r) * cos.abs().max(NOISE_MIN_COS)).max(GROW_TOL_MIN_M)
}

/// A sample's weight in a fit: its inverse noise variance, 1 at 1 m.
fn weight(r: f32) -> f32 {
    (noise_sigma(1.0) / noise_sigma(r)).powi(2)
}

/// The depth camera: its pose in the app's world space and its fov.
#[derive(Debug, Clone, Copy)]
pub struct DepthCamera {
    pub position: Vec3,
    pub rotation: Quat,
    pub fov: Fov,
}

impl DepthCamera {
    /// The camera of one acquired depth layer.
    pub fn from_view(view: &DepthView) -> Self {
        let [x, y, z, w] = view.orientation;
        Self {
            position: view.position.into(),
            rotation: Quat::from_xyzw(x, y, z, w),
            fov: view.fov,
        }
    }

    /// Where the camera looks.
    pub fn forward(&self) -> Vec3 {
        self.rotation * Vec3::NEG_Z
    }
}

/// One update's input: the depth camera and its points.
#[derive(Debug, Clone, Copy)]
pub struct Observation<'a> {
    pub camera: DepthCamera,
    pub cols: usize,
    pub rows: usize,
    /// `cols * rows` world points, row-major, row 0 at the top of the view
    /// and column 0 at its left; cell (`col`, `row`) is the point the ray
    /// through the middle of that cell of the fov met. `None` for no depth.
    pub points: &'a [Option<Vec3>],
}

/// The texel cell `i` of a grid of `n` cells along a side of `dim` texels
/// samples: the middle of its block, as `env_depth::check_texel` reads the
/// 40x40 grid.
pub fn grid_texel(i: usize, n: usize, dim: u32) -> u32 {
    let (i, n) = (i as u32, n.max(1) as u32);
    i * dim / n + dim / (2 * n)
}

/// World points from a grid of stored depth values, in [`Observation`]
/// order. `depth` is `cols * rows` values, row-major, cell (`gx`, `gy`)
/// holding the value at texel (`grid_texel(gx)`, `grid_texel(gy)`) of the
/// layer `view` describes, the way the read-back lays the 40x40 grid out
/// (`env_depth::CHECK_WGSL`). `reading` says which way the texture's rows
/// and columns run (`Reading::RowBottom` on the Quest 3); each texel's ray
/// is `env_depth::texel_dir` and its distance `decode_distance`, so the
/// conventions are the occluder's. A value outside `0..1` is `None`.
pub fn unproject_grid(
    depth: &[f32],
    cols: usize,
    rows: usize,
    size: [u32; 2],
    view: &DepthView,
    near: f32,
    far: f32,
    reading: Reading,
    out: &mut Vec<Option<Vec3>>,
) {
    out.clear();
    out.resize(cols * rows, None);
    if depth.len() < cols * rows {
        return;
    }
    let cam = DepthCamera::from_view(view);
    for row in 0..rows {
        for col in 0..cols {
            let (gx, gy) = match reading {
                Reading::RowTop => (col, row),
                Reading::RowBottom => (col, rows - 1 - row),
                Reading::ColsMirrored => (cols - 1 - col, row),
            };
            let d = depth[gy * cols + gx];
            if !(0.0..1.0).contains(&d) {
                continue;
            }
            let tx = grid_texel(gx, cols, size[0]);
            let ty = grid_texel(gy, rows, size[1]);
            let z = decode_distance(d, near, far);
            if !(z.is_finite() && z > 0.0) {
                continue;
            }
            let dir = texel_dir(view.fov, size, tx, ty, reading);
            out[row * cols + col] = Some(cam.position + cam.rotation * (dir * z));
        }
    }
}

/// What the store's size and range limits are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LivePlanesConfig {
    /// Samples farther than this are ignored (m).
    pub max_range_m: f32,
    /// Planes reported at most.
    pub max_planes: usize,
}

impl Default for LivePlanesConfig {
    fn default() -> Self {
        Self {
            max_range_m: MAX_RANGE_M,
            max_planes: MAX_PLANES,
        }
    }
}

/// A reported plane: a thin oriented box on a real surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LivePlane {
    /// Unique in a session, never reused.
    pub id: u32,
    /// `surfaces::KIND_*`.
    pub kind: u32,
    /// The rectangle's center, on the plane.
    pub center: Vec3,
    /// Plane axes to world: local X and Y in the plane (Y up the slope on a
    /// wall or a ramp), local Z the normal, toward the side the plane was
    /// seen from.
    pub rot: Quat,
    /// Half extents along local X, Y and Z; Z is
    /// [`PLANE_HALF_THICKNESS_M`], as `room.rs` gives a scene plane.
    pub half: Vec3,
    /// 0 to 1: rises with repeated support, falls while the plane is in
    /// view and unseen.
    pub confidence: f32,
    /// The last update the plane was seen in (s).
    pub last_seen_s: f32,
}

impl LivePlane {
    pub fn normal(&self) -> Vec3 {
        self.rot * Vec3::Z
    }
}

/// The fields of an obstacle box (`particles3d::ObstacleBox`, which is
/// Android-only, so this is a plain mirror the wiring copies across).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaneBox {
    pub center: [f32; 3],
    /// Rotation box to world, `(x, y, z, w)`.
    pub rot: [f32; 4],
    pub half: [f32; 3],
    pub kind: u32,
    pub emit: f32,
    pub hidden: bool,
    pub uuid: [u8; 16],
}

/// A live plane as an obstacle box: a scene plane's shape (local Z the
/// normal, half thickness [`PLANE_HALF_THICKNESS_M`]), an emitter like a
/// room box, never hidden, with [`plane_uuid`].
pub fn to_obstacle_box(p: &LivePlane) -> PlaneBox {
    PlaneBox {
        center: p.center.to_array(),
        rot: p.rot.to_array(),
        half: p.half.to_array(),
        kind: p.kind,
        emit: 1.0,
        hidden: false,
        uuid: plane_uuid(p.id),
    }
}

/// The synthetic UUID of live plane `id`: [`UUID_PREFIX`] then the id as a
/// big-endian u64.
pub fn plane_uuid(id: u32) -> [u8; 16] {
    let mut u = [0; 16];
    u[..8].copy_from_slice(&UUID_PREFIX);
    u[8..].copy_from_slice(&u64::from(id).to_be_bytes());
    u
}

/// The live plane id a UUID carries, `None` for any other UUID.
pub fn plane_id(uuid: &[u8; 16]) -> Option<u32> {
    if uuid[..8] != UUID_PREFIX || uuid[8..12] != [0; 4] {
        return None;
    }
    Some(u32::from_be_bytes([uuid[12], uuid[13], uuid[14], uuid[15]]))
}

/// What one update did, for a log line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Cells with a point in range, blends at silhouettes left out.
    pub valid: usize,
    /// Patches the fit found.
    pub patches: usize,
    /// Of those, merged into a tracked plane, and starting a new one.
    pub merged: usize,
    pub created: usize,
    /// Planes dropped: vanished, forgotten, merged into another, evicted.
    pub dropped: usize,
    pub tracked: usize,
    pub reported: usize,
}

// ---- Geometry helpers --------------------------------------------------

/// A fov as tangents, for a grid of `cols` x `rows`.
#[derive(Debug, Clone, Copy)]
struct Lens {
    l: f32,
    r: f32,
    u: f32,
    d: f32,
    cols: usize,
    rows: usize,
}

impl Lens {
    fn new(fov: Fov, cols: usize, rows: usize) -> Self {
        Self {
            l: fov.left.tan(),
            r: fov.right.tan(),
            u: fov.up.tan(),
            d: fov.down.tan(),
            cols,
            rows,
        }
    }

    /// The ray through the middle of a cell, camera space, z = -1.
    fn dir(&self, col: usize, row: usize) -> Vec3 {
        let fx = (col as f32 + 0.5) / self.cols as f32;
        let fy = (row as f32 + 0.5) / self.rows as f32;
        Vec3::new(
            self.l + fx * (self.r - self.l),
            self.u - fy * (self.u - self.d),
            -1.0,
        )
    }

    /// The cell a camera-space point falls in.
    fn cell(&self, local: Vec3) -> Option<usize> {
        if local.z > -1e-4 {
            return None;
        }
        let x = local.x / -local.z;
        let y = local.y / -local.z;
        let fx = (x - self.l) / (self.r - self.l);
        let fy = (self.u - y) / (self.u - self.d);
        if !((0.0..1.0).contains(&fx) && (0.0..1.0).contains(&fy)) {
            return None;
        }
        let col = ((fx * self.cols as f32) as usize).min(self.cols - 1);
        let row = ((fy * self.rows as f32) as usize).min(self.rows - 1);
        Some(row * self.cols + col)
    }

    /// A cell's width near the middle of the view (radians; in tangents,
    /// which match there).
    fn cell_angle(&self) -> f32 {
        ((self.r - self.l) / self.cols as f32).min((self.u - self.d) / self.rows as f32)
    }

    /// The solid angle of a cell (steradians).
    fn solid_angle(&self, col: usize, row: usize) -> f32 {
        let a = (self.r - self.l) * (self.u - self.d) / (self.cols * self.rows) as f32;
        a / self.dir(col, row).length().powi(3)
    }
}

/// Weighted first and second moments of points about `origin` (f64: a
/// plane can hold thousands of points meters from the origin), and the
/// weighted covariance of their noise.
#[derive(Debug, Clone, Copy)]
struct Moments {
    origin: Vec3,
    w: f64,
    w2: f64,
    s: DVec3,
    /// xx, xy, xz, yy, yz, zz.
    ss: [f64; 6],
    /// The same of each point's noise: one deviation along its ray. The
    /// fit takes it out of the scatter, since noise along a slanted ray
    /// tilts a plain least-squares plane toward the ray: about 14 degrees
    /// for a 0.5 m panel at 3.3 m seen at 45 degrees.
    noise: [f64; 6],
}

/// The symmetric outer product `v vᵀ` scaled by `w`, as xx, xy, xz, yy, yz,
/// zz.
fn outer(v: DVec3, w: f64) -> [f64; 6] {
    [
        w * v.x * v.x,
        w * v.x * v.y,
        w * v.x * v.z,
        w * v.y * v.y,
        w * v.y * v.z,
        w * v.z * v.z,
    ]
}

/// A plane fit: the centroid, the normal, the major in-plane axis, the
/// noise-corrected covariance's eigenvalues (ascending), the points'
/// variance off the plane as measured (noise and all) and the effective
/// sample count.
#[derive(Debug, Clone, Copy)]
struct Fit {
    centroid: Vec3,
    normal: Vec3,
    major: Vec3,
    lambda: [f64; 3],
    off_var: f64,
    n_eff: f64,
}

impl Fit {
    /// The normal's standard error (radians).
    fn normal_sd(&self) -> f32 {
        let spread = self.n_eff * self.lambda[1];
        if spread <= 0.0 {
            return std::f32::consts::FRAC_PI_2;
        }
        (self.off_var.max(0.0) / spread).sqrt().atan() as f32
    }
}

impl Moments {
    fn new(origin: Vec3) -> Self {
        Self {
            origin,
            w: 0.0,
            w2: 0.0,
            s: DVec3::ZERO,
            ss: [0.0; 6],
            noise: [0.0; 6],
        }
    }

    /// Add point `p` seen from `cam` with weight `w`.
    fn add(&mut self, p: Vec3, cam: Vec3, w: f32) {
        let x = (p - self.origin).as_dvec3();
        let w = f64::from(w);
        self.w += w;
        self.w2 += w * w;
        self.s += x * w;
        let ray = (p - cam).as_dvec3();
        let r = ray.length();
        let along = ray * (f64::from(noise_sigma(r as f32)) / r.max(1e-9));
        let (xx, nn) = (outer(x, w), outer(along, w));
        for k in 0..6 {
            self.ss[k] += xx[k];
            self.noise[k] += nn[k];
        }
    }

    /// Add `o`'s points, moved to this origin.
    fn merge(&mut self, o: &Moments) {
        let d = (o.origin - self.origin).as_dvec3();
        let (s, w) = (o.s, o.w);
        self.ss[0] += o.ss[0] + 2.0 * d.x * s.x + w * d.x * d.x;
        self.ss[1] += o.ss[1] + d.x * s.y + s.x * d.y + w * d.x * d.y;
        self.ss[2] += o.ss[2] + d.x * s.z + s.x * d.z + w * d.x * d.z;
        self.ss[3] += o.ss[3] + 2.0 * d.y * s.y + w * d.y * d.y;
        self.ss[4] += o.ss[4] + d.y * s.z + s.y * d.z + w * d.y * d.z;
        self.ss[5] += o.ss[5] + 2.0 * d.z * s.z + w * d.z * d.z;
        self.s += s + d * w;
        self.w += w;
        self.w2 += o.w2;
        for (a, b) in self.noise.iter_mut().zip(o.noise) {
            *a += b;
        }
    }

    fn scale(&mut self, k: f64) {
        self.w *= k;
        self.w2 *= k * k;
        self.s *= k;
        for v in self.ss.iter_mut().chain(self.noise.iter_mut()) {
            *v *= k;
        }
    }

    fn fit(&self) -> Option<Fit> {
        if self.w <= 0.0 {
            return None;
        }
        let m = self.s / self.w;
        let mm = outer(m, 1.0);
        let (mut c, mut raw) = ([0.0; 6], [0.0; 6]);
        for k in 0..6 {
            raw[k] = self.ss[k] / self.w - mm[k];
            c[k] = raw[k] - self.noise[k] / self.w;
        }
        let (lambda, normal, major) = sym3_eigen(c)?;
        let [xx, xy, xz, yy, yz, zz] = raw;
        let n = normal;
        let off_var = xx * n.x * n.x
            + yy * n.y * n.y
            + zz * n.z * n.z
            + 2.0 * (xy * n.x * n.y + xz * n.x * n.z + yz * n.y * n.z);
        Some(Fit {
            centroid: self.origin + m.as_vec3(),
            normal: normal.as_vec3(),
            major: major.as_vec3(),
            lambda,
            off_var,
            n_eff: self.w * self.w / self.w2.max(1e-30),
        })
    }
}

/// The eigenvalues (ascending) of the symmetric 3x3 matrix `[xx xy xz; xy
/// yy yz; xz yz zz]`, the unit eigenvector of the smallest and one of the
/// largest (any vector across the smallest when the two largest are
/// equal). `None` when the smallest is not distinct (a line of points, or
/// none).
fn sym3_eigen(a: [f64; 6]) -> Option<([f64; 3], DVec3, DVec3)> {
    let [a00, a01, a02, a11, a12, a22] = a;
    let p1 = a01 * a01 + a02 * a02 + a12 * a12;
    let q = (a00 + a11 + a22) / 3.0;
    let p2 = (a00 - q).powi(2) + (a11 - q).powi(2) + (a22 - q).powi(2) + 2.0 * p1;
    let p = (p2 / 6.0).sqrt();
    if p.is_nan() || p <= 1e-30 {
        return None;
    }
    let (b00, b11, b22) = ((a00 - q) / p, (a11 - q) / p, (a22 - q) / p);
    let (b01, b02, b12) = (a01 / p, a02 / p, a12 / p);
    let det = b00 * (b11 * b22 - b12 * b12) - b01 * (b01 * b22 - b12 * b02)
        + b02 * (b01 * b12 - b11 * b02);
    let phi = (det / 2.0).clamp(-1.0, 1.0).acos() / 3.0;
    let hi = q + 2.0 * p * phi.cos();
    let lo = q + 2.0 * p * (phi + 2.0 * std::f64::consts::FRAC_PI_3).cos();
    let mid = 3.0 * q - hi - lo;
    let vector = |l: f64| {
        let r0 = DVec3::new(a00 - l, a01, a02);
        let r1 = DVec3::new(a01, a11 - l, a12);
        let r2 = DVec3::new(a02, a12, a22 - l);
        let c = [r0.cross(r1), r0.cross(r2), r1.cross(r2)];
        let best = c
            .into_iter()
            .max_by(|x, y| x.length_squared().total_cmp(&y.length_squared()))?;
        let len2 = best.length_squared();
        // Relative to the matrix's scale: a repeated eigenvalue leaves the
        // rows parallel and every cross product near zero.
        (len2 > 1e-18 * (p * p).powi(2)).then(|| best / len2.sqrt())
    };
    let normal = vector(lo)?;
    let major = vector(hi)
        .map(|m| (m - normal * m.dot(normal)).normalize_or_zero())
        .filter(|m| m.length_squared() > 0.5)
        .unwrap_or_else(|| normal.any_orthonormal_vector());
    Some(([lo, mid, hi], normal, major))
}

/// Move `err` toward zero: nothing inside `deadband`, past it the gap
/// beyond the band closes at [`FOLLOW_GAIN`], at most `max_speed`.
fn follow(err: f32, deadband: f32, max_speed: f32, dt: f32) -> f32 {
    let excess = err.abs() - deadband;
    if excess <= 0.0 {
        return 0.0;
    }
    let step = (excess * (1.0 - (-FOLLOW_GAIN * dt).exp())).min(max_speed * dt);
    step.copysign(err)
}

/// Whether two rectangles `[u0, u1, v0, v1]` overlap or come within `gap`.
fn overlaps(a: [f32; 4], b: [f32; 4], gap: f32) -> bool {
    a[0] <= b[1] + gap && b[0] <= a[1] + gap && a[2] <= b[3] + gap && b[2] <= a[3] + gap
}

fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[0].min(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].max(b[3]),
    ]
}

/// A plane frame: origin, in-plane axes and normal.
#[derive(Debug, Clone, Copy)]
struct Frame {
    o: Vec3,
    u: Vec3,
    v: Vec3,
    n: Vec3,
}

/// The extent of `cells` in `f`: along each axis the [`TRIM_DIV`]-trimmed
/// extremes, out by `pad`. Each cell counts where its ray from `cam` meets
/// the plane, not where its noisy point is, so the noise along the ray does
/// not stretch the extent; only which cells belong does. A cell whose point
/// is more than [`EXTENT_SIGMAS`] noise deviations from that meeting point
/// along the ray (a cell of the next surface over at a crease), whose ray
/// runs nearly along the plane, or whose meeting point is past `max_range`
/// (so the far edge is the range, not where noise put the last cells), does
/// not count.
fn extents(
    pts: &[Vec3],
    cells: &[u32],
    cam: Vec3,
    f: Frame,
    pad: f32,
    max_range: f32,
    coords: &mut (Vec<f32>, Vec<f32>),
) -> [f32; 4] {
    let (a, b) = coords;
    a.clear();
    b.clear();
    let height = f.n.dot(f.o - cam);
    for &c in cells {
        let ray = pts[c as usize] - cam;
        let r = ray.length();
        let den = f.n.dot(ray);
        let t = height / den;
        if den.abs() <= 0.1 * r
            || t <= 0.0
            || t * r > max_range
            || (t - 1.0).abs() * r > EXTENT_SIGMAS * noise_sigma(r)
        {
            continue;
        }
        let d = cam + ray * t - f.o;
        a.push(d.dot(f.u));
        b.push(d.dot(f.v));
    }
    if a.is_empty() {
        for &c in cells {
            let d = pts[c as usize] - f.o;
            a.push(d.dot(f.u));
            b.push(d.dot(f.v));
        }
    }
    let k = a.len() / TRIM_DIV;
    let lo_hi = |x: &mut Vec<f32>| -> (f32, f32) {
        if x.is_empty() {
            return (0.0, 0.0);
        }
        if k == 0 {
            let lo = x.iter().copied().fold(f32::INFINITY, f32::min);
            let hi = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            return (lo, hi);
        }
        let n = x.len();
        let lo = *x.select_nth_unstable_by(k, f32::total_cmp).1;
        let hi = *x.select_nth_unstable_by(n - 1 - k, f32::total_cmp).1;
        (lo, hi)
    };
    let (u0, u1) = lo_hi(a);
    let (v0, v1) = lo_hi(b);
    [u0 - pad, u1 + pad, v0 - pad, v1 + pad]
}

/// The in-plane axes for a normal: upright (`v` the uphill direction, `u`
/// level) or about a fixed horizontal reference. `u x v = n`.
fn plane_axes(n: Vec3, upright: bool, yaw_ref: Vec3) -> (Vec3, Vec3) {
    if upright {
        let v = (Vec3::Y - n * n.y).normalize_or(Vec3::Z);
        (v.cross(n), v)
    } else {
        let u = (yaw_ref - n * n.dot(yaw_ref)).normalize_or(n.any_orthonormal_vector());
        (u, n.cross(u))
    }
}

// ---- Per-update scratch ------------------------------------------------

/// A patch the fit found this update: its cells are
/// `cells[start..start + len]`.
#[derive(Debug, Clone, Copy)]
struct Patch {
    mom: Moments,
    fit: Fit,
    start: usize,
    len: usize,
    mean_range: f32,
}

/// Buffers sized to the grid, kept between updates.
#[derive(Debug, Default)]
struct Scratch {
    /// Summed-area tables of the valid points' moments about the camera:
    /// count, x, y, z, xx, xy, xz, yy, yz, zz.
    sat: Vec<[f64; 10]>,
    pts: Vec<Vec3>,
    range: Vec<f32>,
    valid: Vec<bool>,
    normal: Vec<Vec3>,
    flat: Vec<f32>,
    has_normal: Vec<bool>,
    label: Vec<u32>,
    order: Vec<u32>,
    cells: Vec<u32>,
    patches: Vec<Patch>,
}

impl Scratch {
    /// Copy the points in range, leaving out blends at silhouettes; returns
    /// how many there are.
    fn prepare(&mut self, obs: &Observation<'_>, max_range: f32) -> usize {
        let n = obs.cols * obs.rows;
        if self.pts.len() != n {
            self.pts.resize(n, Vec3::ZERO);
            self.range.resize(n, 0.0);
            self.valid.resize(n, false);
            self.normal.resize(n, Vec3::ZERO);
            self.flat.resize(n, 0.0);
            self.has_normal.resize(n, false);
            self.label.resize(n, 0);
            self.order.reserve(n);
            self.cells.reserve(n);
            self.patches.reserve(n / MIN_CELLS + 1);
            self.sat.reserve((obs.cols + 1) * (obs.rows + 1));
        }
        let cam = obs.camera.position;
        for (i, p) in obs.points.iter().enumerate() {
            let (pt, r) = match p {
                Some(p) if p.is_finite() => (*p, p.distance(cam)),
                _ => (Vec3::ZERO, 0.0),
            };
            self.pts[i] = pt;
            self.range[i] = r;
            self.valid[i] = (NEAR_M..=max_range).contains(&r);
        }
        // A cell at a silhouette often holds a blend of the two depths:
        // between two opposite neighbors that differ by a jump, in the
        // middle half of it. Such cells line up into slanted sheets that
        // are no surface; leave them out. (An edge cell of a slanted
        // surface sits a few centimeters from its own side, so it stays.)
        // Ranges as the sensor gave them, before the range cut, so a
        // blend against the far side counts too.
        let (cols, rows) = (obs.cols, obs.rows);
        let raw = |i: usize| obs.points[i].map(|q| q.distance(cam));
        let mut count = 0;
        for i in 0..cols * rows {
            if !self.valid[i] {
                continue;
            }
            let (col, row) = (i % cols, i / cols);
            let r = self.range[i];
            let near = 3.0 * noise_sigma(r);
            let jump = 2.0 * (JUMP_FRAC * r + near);
            let pairs = [
                (col > 0 && col + 1 < cols).then(|| (i - 1, i + 1)),
                (row > 0 && row + 1 < rows).then(|| (i - cols, i + cols)),
            ];
            let mixed = pairs.into_iter().flatten().any(|(a, b)| {
                let (Some(ra), Some(rb)) = (raw(a), raw(b)) else {
                    return false;
                };
                let (lo, hi) = (ra.min(rb), ra.max(rb));
                let margin = (0.25 * (hi - lo)).max(near);
                hi - lo > jump && r > lo + margin && r < hi - margin
            });
            self.valid[i] = !mixed;
            count += usize::from(!mixed);
        }
        count
    }

    /// Each valid cell's normal (toward the camera) and flatness, from one
    /// of five windows holding the cell: the centered one when it is filled
    /// and flat for the noise, else the flattest of the others, a filled one
    /// before a sparse one. Beside a crease or a silhouette one of them lies
    /// on the cell's own surface, so the normal does not blend two. The
    /// windows' moments come from summed-area tables, so each costs the
    /// same whatever its size.
    fn normals(&mut self, lens: &Lens, cam: Vec3) {
        let (cols, rows) = (lens.cols, lens.rows);
        let w = cols + 1;
        self.sat.clear();
        self.sat.resize(w * (rows + 1), [0.0; 10]);
        for row in 0..rows {
            for col in 0..cols {
                let i = row * cols + col;
                let mut v = [0.0f64; 10];
                if self.valid[i] {
                    let x = (self.pts[i] - cam).as_dvec3();
                    v = [
                        1.0,
                        x.x,
                        x.y,
                        x.z,
                        x.x * x.x,
                        x.x * x.y,
                        x.x * x.z,
                        x.y * x.y,
                        x.y * x.z,
                        x.z * x.z,
                    ];
                }
                let (a, b, c) = (
                    self.sat[row * w + col + 1],
                    self.sat[(row + 1) * w + col],
                    self.sat[row * w + col],
                );
                let out = &mut self.sat[(row + 1) * w + col + 1];
                for k in 0..10 {
                    out[k] = v[k] + a[k] + b[k] - c[k];
                }
            }
        }
        let spacing = lens.cell_angle();
        for i in 0..cols * rows {
            self.has_normal[i] = false;
            if !self.valid[i] {
                continue;
            }
            let (col, row) = (i % cols, i / cols);
            let (p, r) = (self.pts[i], self.range[i]);
            let sigma = noise_sigma(r);
            let ratio = sigma / (r * spacing);
            let reach = 1 + WINDOW_NOISE_FRACS.iter().filter(|&&f| ratio > f).count() as isize;
            let full = ((2 * reach + 1) * (2 * reach + 1)) as f64;
            let step = f64::from(r * spacing);
            let noise_var = f64::from(sigma * sigma);
            // (flatness, filled, covariance) of the window chosen so far.
            let mut best: Option<(f64, bool, [f64; 6])> = None;
            for (dr, dc) in [(0, 0), (-1, -1), (-1, 1), (1, -1), (1, 1)] {
                let (cr, cc) = (row as isize + dr * reach, col as isize + dc * reach);
                let r0 = (cr - reach).clamp(0, rows as isize) as usize;
                let r1 = (cr + reach + 1).clamp(0, rows as isize) as usize;
                let c0 = (cc - reach).clamp(0, cols as isize) as usize;
                let c1 = (cc + reach + 1).clamp(0, cols as isize) as usize;
                let (a, b, c, d) = (
                    self.sat[r1 * w + c1],
                    self.sat[r0 * w + c1],
                    self.sat[r1 * w + c0],
                    self.sat[r0 * w + c0],
                );
                let sum = |k: usize| a[k] - b[k] - c[k] + d[k];
                let n = sum(0);
                if n < 6.0 {
                    continue;
                }
                let m = DVec3::new(sum(1), sum(2), sum(3)) / n;
                let cov = [
                    sum(4) / n - m.x * m.x,
                    sum(5) / n - m.x * m.y,
                    sum(6) / n - m.x * m.z,
                    sum(7) / n - m.y * m.y,
                    sum(8) / n - m.y * m.z,
                    sum(9) / n - m.z * m.z,
                ];
                // The smallest eigenvalue, near enough: det over the sum of
                // the principal 2x2 minors; the middle one about the minors
                // over the trace. A window whose middle one is small is a
                // line of points, with no plane.
                let [xx, xy, xz, yy, yz, zz] = cov;
                let det =
                    xx * (yy * zz - yz * yz) - xy * (xy * zz - yz * xz) + xz * (xy * yz - yy * xz);
                let minors = xx * yy - xy * xy + xx * zz - xz * xz + yy * zz - yz * yz;
                let trace = xx + yy + zz;
                if minors <= 0.0 || minors / trace < 0.1 * step * step {
                    continue;
                }
                let flat = det / minors;
                // The centered window when it is flat for the noise; else
                // the flattest, a well-filled one before a sparse one (a
                // few points fit a plane at any tilt and look flat by luck).
                let filled = n >= WINDOW_FILL * full;
                if dr == 0 && dc == 0 && filled && flat <= SEED_FLATNESS as f64 * noise_var {
                    best = Some((flat, filled, cov));
                    break;
                }
                if best.is_none_or(|b| (filled, -flat) > (b.1, -b.0)) {
                    best = Some((flat, filled, cov));
                }
            }
            let Some((_, _, cov)) = best else {
                continue;
            };
            let Some((lambda, normal, _)) = sym3_eigen(cov) else {
                continue;
            };
            // A line of points (a strip one cell wide) has no plane.
            if lambda[1] < 0.06 * step * step {
                continue;
            }
            let mut nv = normal.as_vec3();
            if nv.dot(cam - p) < 0.0 {
                nv = -nv;
            }
            self.normal[i] = nv;
            self.flat[i] = (lambda[0] / f64::from(sigma * sigma)) as f32;
            self.has_normal[i] = true;
        }
    }

    /// Grow regions from the flattest seeds; keep those that make patches.
    fn grow(&mut self, lens: &Lens, cam: Vec3) {
        let Self {
            pts,
            range,
            normal,
            flat,
            has_normal,
            label,
            order,
            cells,
            patches,
            ..
        } = self;
        let (cols, rows) = (lens.cols, lens.rows);
        label.fill(0);
        order.clear();
        cells.clear();
        patches.clear();
        order.extend(
            (0..cols * rows)
                .filter(|&i| has_normal[i] && flat[i] <= SEED_FLATNESS)
                .map(|i| i as u32),
        );
        order.sort_unstable_by(|&a, &b| flat[a as usize].total_cmp(&flat[b as usize]));
        let cos_grow = GROW_ANGLE_DEG.to_radians().cos();
        let mut tag: u32 = 0;
        for k in 0..order.len() {
            let seed = order[k] as usize;
            if label[seed] != 0 {
                continue;
            }
            tag += 1;
            label[seed] = tag;
            let start = cells.len();
            cells.push(seed as u32);
            let mut mom = Moments::new(pts[seed]);
            mom.add(pts[seed], cam, weight(range[seed]));
            let mut n = normal[seed];
            let mut d = n.dot(pts[seed]);
            let mut refit_at = 12;
            let mut head = start;
            while head < cells.len() {
                let i = cells[head] as usize;
                head += 1;
                let (col, row) = (i % cols, i / cols);
                let neighbors = [
                    (col > 0).then(|| i - 1),
                    (col + 1 < cols).then(|| i + 1),
                    (row > 0).then(|| i - cols),
                    (row + 1 < rows).then(|| i + cols),
                ];
                for j in neighbors.into_iter().flatten() {
                    if label[j] != 0 || !has_normal[j] {
                        continue;
                    }
                    let rj = range[j];
                    let cos = n.dot(pts[j] - cam) / rj;
                    if (rj - range[i]).abs() > JUMP_FRAC * range[i] + 3.0 * noise_sigma(range[i])
                        || normal[j].dot(n) < cos_grow
                        || (n.dot(pts[j]) - d).abs() > plane_tol(rj, cos)
                    {
                        continue;
                    }
                    label[j] = tag;
                    cells.push(j as u32);
                    mom.add(pts[j], cam, weight(rj));
                }
                let count = cells.len() - start;
                if count >= refit_at {
                    refit_at *= 2;
                    // Only a well-determined fit replaces the plane: a few
                    // cells close together can tilt any way and would cut
                    // the region short.
                    if let Some(f) = mom.fit()
                        && f.normal_sd() <= REFIT_NORMAL_SD_DEG.to_radians()
                    {
                        n = if f.normal.dot(n) < 0.0 {
                            -f.normal
                        } else {
                            f.normal
                        };
                        d = n.dot(f.centroid);
                    }
                }
            }
            if cells.len() - start < MIN_CELLS {
                cells.truncate(start);
                continue;
            }
            // Refit on the inliers of the region's final plane.
            let Some(f) = mom.fit() else {
                cells.truncate(start);
                continue;
            };
            let (fnorm, fd) = (f.normal, f.normal.dot(f.centroid));
            let mut mom = Moments::new(f.centroid);
            let mut kept = start;
            let (mut area, mut range_sum) = (0.0f32, 0.0f32);
            for c in start..cells.len() {
                let i = cells[c] as usize;
                let r = range[i];
                let cos = fnorm.dot(pts[i] - cam) / r;
                let off = fnorm.dot(pts[i]) - fd;
                if off.abs() > plane_tol(r, cos) {
                    continue;
                }
                cells[kept] = cells[c];
                kept += 1;
                mom.add(pts[i], cam, weight(r));
                area += lens.solid_angle(i % cols, i / cols) * r * r / cos.abs().max(0.25);
                range_sum += r;
            }
            cells.truncate(kept);
            let len = kept - start;
            if len < MIN_CELLS || area < MIN_AREA_M2 {
                cells.truncate(start);
                continue;
            }
            let Some(mut fit) = mom.fit() else {
                cells.truncate(start);
                continue;
            };
            let cell = range_sum / len as f32 * lens.cell_angle();
            if fit.lambda[1] < f64::from(MIN_SPREAD_CELLS * cell).powi(2) {
                cells.truncate(start);
                continue;
            }
            if fit.normal.dot(cam - fit.centroid) < 0.0 {
                fit.normal = -fit.normal;
            }
            patches.push(Patch {
                mom,
                fit,
                start,
                len,
                mean_range: range_sum / len as f32,
            });
        }
    }
}

// ---- Tracked planes ----------------------------------------------------

/// What the map says at a point of a tracked plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Probe {
    /// Out of the view or the range, seen edge-on, or no depth there.
    Out,
    /// Something nearer is in the way.
    Hidden,
    /// The map sees the plane there.
    Agree,
    /// The map sees past where the plane would be.
    Through,
}

/// A plane the store tracks, reported or not.
#[derive(Debug, Clone, Copy)]
struct Tracked {
    id: u32,
    alive: bool,
    mom: Moments,
    /// The plane's frame origin is this point projected onto the plane: it
    /// stays put in the plane while the fit moves.
    anchor: Vec3,
    centroid: Vec3,
    normal: Vec3,
    /// Upright axes (walls, slopes), or about `yaw_ref` (horizontal).
    upright: bool,
    yaw_ref: Vec3,
    /// The rectangle `[u0, u1, v0, v1]` in the plane's frame.
    edges: [f32; 4],
    /// The rectangles the last [`EDGE_HISTORY`] updates with patches
    /// observed, a ring, and how many updates have filled it.
    hist: [[f32; 4]; EDGE_HISTORY],
    /// Which of each observation's edges were in view.
    hist_seen: [[bool; 4]; EDGE_HISTORY],
    hist_len: u32,
    // This update.
    merged: bool,
    seen_edges: Option<[f32; 4]>,
    /// Which edges of the rectangle are in view this update.
    edge_seen: [bool; 4],
    agrees: bool,
    evidence: u32,
    // Life.
    first_s: f32,
    last_hit_s: f32,
    last_seen_s: f32,
    hits: u32,
    /// Updates since it was first seen, while not yet reported.
    updates: u32,
    /// Held back at its decision, from this camera position and forward.
    held: Option<(Vec3, Vec3)>,
    /// Patches that met the creation limits: a plane needs one to appear.
    strong: u32,
    /// First seen after the camera had been still a while (see
    /// [`STILL_GRACE_S`]).
    late: bool,
    miss_s: f32,
    appeared: bool,
    reported: bool,
    kind: u32,
    pending: Option<(u32, f32)>,
    confidence: f32,
    // What is reported, following the estimate.
    rep_center: Vec3,
    rep_rot: Quat,
    rep_half: [f32; 2],
}

impl Tracked {
    fn new(id: u32, p: &Patch, edges_of: impl FnOnce(Frame) -> [f32; 4]) -> Self {
        let n = p.fit.normal;
        let upright = n.y.abs() < 10.0f32.to_radians().cos();
        let yaw_ref = Vec3::new(p.fit.major.x, 0.0, p.fit.major.z).normalize_or(Vec3::X);
        let (u, v) = plane_axes(n, upright, yaw_ref);
        let edges = edges_of(Frame {
            o: p.fit.centroid,
            u,
            v,
            n,
        });
        Self {
            id,
            alive: true,
            mom: p.mom,
            anchor: p.fit.centroid,
            centroid: p.fit.centroid,
            normal: n,
            upright,
            yaw_ref,
            edges,
            hist: [edges; EDGE_HISTORY],
            hist_seen: [[false; 4]; EDGE_HISTORY],
            hist_len: 1,
            merged: false,
            seen_edges: None,
            edge_seen: [false; 4],
            agrees: false,
            evidence: 0,
            first_s: 0.0,
            last_hit_s: 0.0,
            last_seen_s: 0.0,
            hits: 1,
            updates: 1,
            held: None,
            strong: 0,
            late: false,
            miss_s: 0.0,
            appeared: false,
            reported: false,
            kind: KIND_OTHER,
            pending: None,
            confidence: 0.0,
            rep_center: Vec3::ZERO,
            rep_rot: Quat::IDENTITY,
            rep_half: [0.0; 2],
        }
    }

    fn offset(&self) -> f32 {
        self.normal.dot(self.centroid)
    }

    fn axes(&self) -> (Vec3, Vec3) {
        plane_axes(self.normal, self.upright, self.yaw_ref)
    }

    fn frame(&self) -> Frame {
        let (u, v) = self.axes();
        Frame {
            o: self.origin(),
            u,
            v,
            n: self.normal,
        }
    }

    fn origin(&self) -> Vec3 {
        self.anchor - self.normal * (self.normal.dot(self.anchor) - self.offset())
    }

    fn rot(&self) -> Quat {
        let (u, v) = self.axes();
        Quat::from_mat3(&Mat3::from_cols(u, v, self.normal)).normalize()
    }

    /// The estimate's rectangle: center and half extents.
    fn center_half(&self) -> (Vec3, [f32; 2]) {
        let (u, v) = self.axes();
        let e = self.edges;
        let c = self.origin() + u * (0.5 * (e[0] + e[1])) + v * (0.5 * (e[2] + e[3]));
        (c, [0.5 * (e[1] - e[0]), 0.5 * (e[3] - e[2])])
    }

    fn area(&self) -> f32 {
        (self.edges[1] - self.edges[0]) * (self.edges[3] - self.edges[2])
    }

    fn corners(&self) -> [Vec3; 4] {
        let (u, v) = self.axes();
        let (o, e) = (self.origin(), self.edges);
        [
            o + u * e[0] + v * e[2],
            o + u * e[1] + v * e[2],
            o + u * e[0] + v * e[3],
            o + u * e[1] + v * e[3],
        ]
    }

    fn top_y(&self) -> f32 {
        self.corners().iter().map(|c| c.y).fold(f32::MIN, f32::max)
    }

    /// The distance from `p` to the nearest point of the rectangle.
    fn distance_to(&self, p: Vec3) -> f32 {
        let (u, v) = self.axes();
        let d = p - self.origin();
        let (a, b) = (d.dot(u), d.dot(v));
        let e = self.edges;
        let da = (e[0] - a).max(a - e[1]).max(0.0);
        let db = (e[2] - b).max(b - e[3]).max(0.0);
        (da * da + db * db + d.dot(self.normal).powi(2)).sqrt()
    }

    fn refit(&mut self) {
        if let Some(f) = self.mom.fit() {
            self.normal = if f.normal.dot(self.normal) < 0.0 {
                -f.normal
            } else {
                f.normal
            };
            self.centroid = f.centroid;
        }
    }

    fn absorb(&mut self, m: &Moments) {
        self.mom.merge(m);
        if self.mom.w > WEIGHT_CAP {
            self.mom.scale(WEIGHT_CAP / self.mom.w);
        }
        self.refit();
    }

    /// Take in another tracked plane: its points, its rectangle and its
    /// history.
    fn absorb_plane(&mut self, o: &Tracked) {
        self.absorb(&o.mom);
        let (u, v) = self.axes();
        let origin = self.origin();
        let mut e = self.edges;
        for c in o.corners() {
            let d = c - origin;
            let (a, b) = (d.dot(u), d.dot(v));
            e = union(e, [a, a, b, b]);
        }
        self.edges = e;
        // The observations so far are of the smaller rectangle: start again
        // from the union, so an edge in view is not pulled back to them.
        self.hist = [e; EDGE_HISTORY];
        self.hist_seen = [[false; 4]; EDGE_HISTORY];
        self.hist_len = 1;
        self.hits += o.hits;
        self.strong += o.strong;
        self.updates = self.updates.max(o.updates);
        self.first_s = self.first_s.min(o.first_s);
        self.last_hit_s = self.last_hit_s.max(o.last_hit_s);
        self.last_seen_s = self.last_seen_s.max(o.last_seen_s);
        self.miss_s = self.miss_s.min(o.miss_s);
        self.merged |= o.merged;
        self.agrees |= o.agrees;
    }

    /// Check a 4x4 grid of points on the rectangle, and three points just
    /// inside each edge, against the map: sets `agrees`, `evidence` and
    /// `edge_seen`.
    fn check(&mut self, obs: &Observation<'_>, lens: &Lens, max_range: f32) {
        self.merged = false;
        self.seen_edges = None;
        self.agrees = false;
        self.evidence = 0;
        self.edge_seen = [false; 4];
        let cam = obs.camera.position;
        let o = self.origin();
        if (cam - o).dot(self.normal) <= 0.0 {
            return;
        }
        let (u, v) = self.axes();
        let inv = obs.camera.rotation.inverse();
        let e = self.edges;
        let n = self.normal;
        let offset = self.offset();
        let probe = |a: f32, b: f32| -> Probe {
            let p = o + u * a + v * b;
            let r = p.distance(cam);
            let steep = (p - cam).dot(n).abs() >= EVIDENCE_MIN_COS * r;
            let Some(cell) = lens.cell(inv * (p - cam)) else {
                return Probe::Out;
            };
            if !(steep && (NEAR_M..=max_range).contains(&r)) {
                return Probe::Out;
            }
            let Some(q) = obs.points[cell].filter(|q| q.is_finite()) else {
                return Probe::Out;
            };
            // Along the cell's ray: where the plane would be against where
            // the map saw something.
            let ray = q - cam;
            let den = n.dot(ray);
            let t = (offset - n.dot(cam)) / den;
            if den.abs() < 1e-6 || t <= 0.0 {
                return Probe::Out;
            }
            let expected = t * ray.length();
            let diff = ray.length() - expected;
            let tol = (EVIDENCE_SIGMAS * noise_sigma(expected)).max(EVIDENCE_TOL_MIN_M);
            if diff < -tol {
                Probe::Hidden
            } else if diff > tol {
                Probe::Through
            } else {
                Probe::Agree
            }
        };
        let (mut agree, mut through) = (0u32, 0u32);
        for i in 0..4 {
            for j in 0..4 {
                let a = e[0] + (e[1] - e[0]) * (i as f32 + 0.5) / 4.0;
                let b = e[2] + (e[3] - e[2]) * (j as f32 + 0.5) / 4.0;
                match probe(a, b) {
                    Probe::Agree => agree += 1,
                    Probe::Through => through += 1,
                    Probe::Out | Probe::Hidden => {}
                }
            }
        }
        self.evidence = agree + through;
        self.agrees = agree >= AGREE_MIN && agree >= through;
        // An edge is in view when three points just inside it are: in the
        // frustum and in range, and nothing nearer hides them.
        let inset = |lo: f32, hi: f32| (0.05 * (hi - lo), 0.5 * (lo + hi));
        let (du, mu) = inset(e[0], e[1]);
        let (dv, mv) = inset(e[2], e[3]);
        let seen = |pts: [(f32, f32); 3]| {
            pts.iter()
                .all(|&(a, b)| matches!(probe(a, b), Probe::Agree | Probe::Through))
        };
        self.edge_seen = [
            seen([
                (e[0] + du, e[2] + dv),
                (e[0] + du, mv),
                (e[0] + du, e[3] - dv),
            ]),
            seen([
                (e[1] - du, e[2] + dv),
                (e[1] - du, mv),
                (e[1] - du, e[3] - dv),
            ]),
            seen([
                (e[0] + du, e[2] + dv),
                (mu, e[2] + dv),
                (e[1] - du, e[2] + dv),
            ]),
            seen([
                (e[0] + du, e[3] - dv),
                (mu, e[3] - dv),
                (e[1] - du, e[3] - dv),
            ]),
        ];
    }

    /// Fold this update's merged patches into the rectangle. An edge moves
    /// only by more than a cell's width at its range ([`EDGE_BAND_CELLS`]).
    /// It grows to the median of where the plane's last [`EDGE_HISTORY`]
    /// observations put it, when that is past it. It shrinks to the median
    /// of the observations made while it was itself in view (at least 3),
    /// and only while it is in view now: an edge out of view is where the
    /// view ended, not where the surface does. Which cells make the rim of a
    /// patch changes from update to update; the median of eight does not, so
    /// a plane in a still view stays put, and a stray strip of cells seen
    /// once does not stretch it. While the camera moves, new parts of a
    /// surface come into view and leave it within a couple of seconds, so an
    /// edge also grows when this update and the last both reach more than
    /// the band past it, to the nearer of the two.
    fn apply_seen(&mut self, cam: Vec3, cell_angle: f32, moving: bool) {
        let Some(s) = self.seen_edges else {
            return;
        };
        let at = self.hist_len as usize % EDGE_HISTORY;
        self.hist[at] = s;
        self.hist_seen[at] = self.edge_seen;
        self.hist_len = self.hist_len.saturating_add(1);
        let len = (self.hist_len as usize).min(EDGE_HISTORY);
        let prev = (self.hist_len as usize + EDGE_HISTORY - 2) % EDGE_HISTORY;
        let (u, v) = self.axes();
        let o = self.origin();
        let e = self.edges;
        let mids = [
            o + u * e[0] + v * (0.5 * (e[2] + e[3])),
            o + u * e[1] + v * (0.5 * (e[2] + e[3])),
            o + u * (0.5 * (e[0] + e[1])) + v * e[2],
            o + u * (0.5 * (e[0] + e[1])) + v * e[3],
        ];
        for k in 0..4 {
            let band = EDGE_BAND_CELLS * cell_angle * mids[k].distance(cam);
            // Outward positive; lower medians, so two observations give the
            // nearer.
            let sign = if k % 2 == 0 { -1.0 } else { 1.0 };
            let edge = sign * self.edges[k];
            let (mut all, mut in_view) = ([0.0f32; EDGE_HISTORY], [0.0f32; EDGE_HISTORY]);
            let mut n_view = 0;
            for (i, h) in self.hist[..len].iter().enumerate() {
                all[i] = sign * h[k];
                if self.hist_seen[i][k] {
                    in_view[n_view] = sign * h[k];
                    n_view += 1;
                }
            }
            let median = |x: &mut [f32]| {
                x.sort_unstable_by(f32::total_cmp);
                x[(x.len() - 1) / 2]
            };
            let mut out = median(&mut all[..len]);
            if moving && len >= 2 {
                out = out.max((sign * s[k]).min(sign * self.hist[prev][k]));
            }
            if out > edge + band {
                self.edges[k] = sign * out;
            } else if self.edge_seen[k] && n_view >= 3 {
                let inner = median(&mut in_view[..n_view]);
                if inner < edge - band {
                    self.edges[k] = sign * inner;
                }
            }
        }
    }

    /// Report the estimate as it is (a plane appearing).
    fn snap(&mut self) {
        let (c, h) = self.center_half();
        self.rep_center = c;
        self.rep_rot = self.rot();
        self.rep_half = h;
    }

    /// Move the reported rectangle toward the estimate, within the dead
    /// bands and rate limits.
    fn follow(&mut self, dt: f32, cam: Vec3, cell_angle: f32, moving: bool) {
        let (turn_max, offset_max, edge_max) = if moving {
            (TURN_MAX_DEG_S, OFFSET_MAX_M_S, EDGE_MAX_M_S)
        } else {
            (TURN_STILL_DEG_S, OFFSET_STILL_M_S, EDGE_STILL_M_S)
        };
        let (c, h) = self.center_half();
        let q = self.rot();
        let angle = self.rep_rot.angle_between(q);
        let step = follow(
            angle,
            TURN_DEADBAND_DEG.to_radians(),
            turn_max.to_radians(),
            dt,
        );
        if step > 0.0 && angle > 0.0 {
            self.rep_rot = self.rep_rot.slerp(q, (step / angle).min(1.0)).normalize();
        }
        let (u, v, n) = (
            self.rep_rot * Vec3::X,
            self.rep_rot * Vec3::Y,
            self.rep_rot * Vec3::Z,
        );
        self.rep_center += n * follow(
            (c - self.rep_center).dot(n),
            OFFSET_DEADBAND_M,
            offset_max,
            dt,
        );
        let d = c - self.rep_center;
        let (cu, cv) = (d.dot(u), d.dot(v));
        let target = [cu - h[0], cu + h[0], cv - h[1], cv + h[1]];
        let [hu, hv] = self.rep_half;
        let mut e = [-hu, hu, -hv, hv];
        let center = self.rep_center;
        let mids = [
            center + u * e[0],
            center + u * e[1],
            center + v * e[2],
            center + v * e[3],
        ];
        for k in 0..4 {
            let band =
                (EDGE_DEADBAND_CELLS * cell_angle * mids[k].distance(cam)).max(EDGE_DEADBAND_M);
            e[k] += follow(target[k] - e[k], band, edge_max, dt);
        }
        for k in [0, 2] {
            if e[k + 1] < e[k] {
                let m = 0.5 * (e[k] + e[k + 1]);
                e[k] = m;
                e[k + 1] = m;
            }
        }
        self.rep_center += u * (0.5 * (e[0] + e[1])) + v * (0.5 * (e[2] + e[3]));
        self.rep_half = [0.5 * (e[1] - e[0]), 0.5 * (e[3] - e[2])];
    }

    fn classify(&self, floor: f32) -> u32 {
        let n = self.normal;
        let cos_h = HORIZONTAL_DEG.to_radians().cos();
        let hy = |k: u32| {
            if self.kind == k {
                KIND_HYSTERESIS_M
            } else {
                0.0
            }
        };
        let h = self.centroid.y - floor;
        if n.y >= cos_h {
            if h.abs() <= FLOOR_TOL_M + hy(KIND_FLOOR) {
                KIND_FLOOR
            } else if (TABLE_MIN_M - hy(KIND_TABLE)..=TABLE_MAX_M + hy(KIND_TABLE)).contains(&h) {
                KIND_TABLE
            } else {
                KIND_OTHER
            }
        } else if n.y <= -cos_h {
            if h >= CEILING_MIN_M - hy(KIND_CEILING) {
                KIND_CEILING
            } else {
                KIND_OTHER
            }
        } else if n.y.abs() <= WALL_DEG.to_radians().sin() {
            if self.top_y() - floor >= WALL_MIN_TOP_M - hy(KIND_WALL) {
                KIND_WALL
            } else {
                KIND_OTHER
            }
        } else {
            KIND_OTHER
        }
    }

    /// The budget's score: confidence, area and nearness. Confidence counts
    /// only up to [`SCORE_CONF_CAP`], about where a plane has it when it
    /// appears, so a plane gaining confidence after it appeared does not
    /// later push out one that was reported first; a plane losing it while
    /// unseen does lose its place.
    fn score(&self, cam: Vec3) -> f32 {
        let area = 4.0 * self.rep_half[0] * self.rep_half[1];
        let bonus = if self.reported { 1.0 + KEEP_BONUS } else { 1.0 };
        bonus * self.confidence.min(SCORE_CONF_CAP) * (0.3 + area.min(6.0).sqrt())
            / (1.0 + self.distance_to(cam) / 3.0)
    }

    fn live(&self) -> LivePlane {
        LivePlane {
            id: self.id,
            kind: self.kind,
            center: self.rep_center,
            rot: self.rep_rot,
            half: Vec3::new(self.rep_half[0], self.rep_half[1], PLANE_HALF_THICKNESS_M),
            confidence: self.confidence,
            last_seen_s: self.last_seen_s,
        }
    }
}

/// Whether two tracked planes are one surface: normals within
/// [`MERGE_ANGLE_DEG`], the smaller's center within [`MERGE_OFFSET_M`] of
/// the larger's plane, rectangles overlapping or touching. While one of
/// them has not appeared, normals within [`DUPLICATE_SCALE`] times that: a
/// second plane started on a surface from a noisy patch joins the first
/// before it can appear beside it.
fn same_surface(a: &Tracked, b: &Tracked) -> bool {
    let scale = if a.appeared && b.appeared {
        1.0
    } else {
        DUPLICATE_SCALE
    };
    if a.normal.dot(b.normal) < (scale * MERGE_ANGLE_DEG).to_radians().cos() {
        return false;
    }
    let (big, small) = if a.area() >= b.area() { (a, b) } else { (b, a) };
    let (c, _) = small.center_half();
    if (big.normal.dot(c) - big.offset()).abs() > MERGE_OFFSET_M {
        return false;
    }
    let (u, v) = big.axes();
    let o = big.origin();
    let mut r = [f32::MAX, f32::MIN, f32::MAX, f32::MIN];
    for p in small.corners() {
        let (x, y) = ((p - o).dot(u), (p - o).dot(v));
        r = union(r, [x, x, y, y]);
    }
    overlaps(r, big.edges, TOUCH_M)
}

// ---- The store ---------------------------------------------------------

/// The live planes: feed it an [`Observation`] per update, read
/// [`Self::planes`].
#[derive(Debug)]
pub struct LivePlanes {
    config: LivePlanesConfig,
    next_id: u32,
    last_s: Option<f32>,
    /// The camera's position and forward at the last update.
    last_view: Option<(Vec3, Vec3)>,
    /// Since when the camera has been still, while it is.
    still_since: Option<f32>,
    floor_y: Option<f32>,
    tracked: Vec<Tracked>,
    out: Vec<LivePlane>,
    scratch: Scratch,
    coords: (Vec<f32>, Vec<f32>),
    ranked: Vec<(f32, usize)>,
    /// What happened to planes, for the tests' failure messages.
    #[cfg(test)]
    events: Vec<String>,
}

impl Default for LivePlanes {
    fn default() -> Self {
        Self::new(LivePlanesConfig::default())
    }
}

impl LivePlanes {
    pub fn new(config: LivePlanesConfig) -> Self {
        Self {
            config,
            next_id: 1,
            last_s: None,
            last_view: None,
            still_since: None,
            floor_y: None,
            tracked: Vec::with_capacity(MAX_TRACKED + 1),
            out: Vec::with_capacity(config.max_planes),
            scratch: Scratch::default(),
            coords: (Vec::new(), Vec::new()),
            ranked: Vec::with_capacity(MAX_TRACKED + 1),
            #[cfg(test)]
            events: Vec::new(),
        }
    }

    pub fn config(&self) -> LivePlanesConfig {
        self.config
    }

    /// The reported planes, in id order.
    pub fn planes(&self) -> &[LivePlane] {
        &self.out
    }

    /// The floor estimate's height, once a floor has been seen.
    pub fn floor_y(&self) -> Option<f32> {
        self.floor_y
    }

    /// Planes tracked, reported or not.
    pub fn tracked_count(&self) -> usize {
        self.tracked.len()
    }

    /// Take one observation at `now_s` (seconds, any monotonic clock).
    pub fn update(&mut self, now_s: f32, obs: &Observation<'_>) -> Stats {
        let mut stats = Stats::default();
        let n = obs.cols * obs.rows;
        if n == 0 || obs.points.len() != n {
            return stats;
        }
        let dt = self
            .last_s
            .map_or(0.0, |last| (now_s - last).clamp(0.0, MAX_DT_S));
        self.last_s = Some(now_s);
        let cam = obs.camera.position;
        let lens = Lens::new(obs.camera.fov, obs.cols, obs.rows);
        let max_range = self.config.max_range_m;
        if self.coords.0.capacity() < n {
            self.coords.0.reserve(n);
            self.coords.1.reserve(n);
        }

        stats.valid = self.scratch.prepare(obs, max_range);
        self.scratch.normals(&lens, cam);
        self.scratch.grow(&lens, cam);
        stats.patches = self.scratch.patches.len();

        for t in &mut self.tracked {
            t.check(obs, &lens, max_range);
        }
        for k in 0..self.scratch.patches.len() {
            self.place(k, now_s, &lens, cam, &mut stats);
        }
        // Moving: turning faster than MOVING_DEG_S or walking faster than
        // MOVING_M_S since the last update.
        let forward = obs.camera.forward();
        let moving = self.last_view.is_some_and(|(at, fwd)| {
            dt > 0.0
                && (at.distance(cam) > MOVING_M_S * dt
                    || fwd.angle_between(forward) > MOVING_DEG_S.to_radians() * dt)
        });
        self.last_view = Some((cam, forward));
        if moving {
            self.still_since = None;
        } else if self.still_since.is_none() {
            self.still_since = Some(now_s);
        }
        for t in &mut self.tracked {
            t.apply_seen(cam, lens.cell_angle(), moving);
        }
        self.merge_tracked(&mut stats);
        self.life(now_s, dt, &obs.camera, &mut stats);
        self.update_floor(dt, cam);

        let floor = self.floor_y.unwrap_or(cam.y - FLOOR_GUESS_BELOW_EYE_M);
        for t in &mut self.tracked {
            let k = t.classify(floor);
            // `reported` is still last update's: a plane appearing now
            // takes its kind at once.
            if !t.reported || k == t.kind {
                t.kind = k;
                t.pending = None;
            } else {
                match t.pending {
                    Some((p, since)) if p == k => {
                        if now_s - since >= KIND_HOLD_S {
                            t.kind = k;
                            t.pending = None;
                        }
                    }
                    _ => t.pending = Some((k, now_s)),
                }
            }
            // Out of view (or hidden), a plane is kept as it was.
            if t.appeared && (t.merged || t.evidence >= MIN_EVIDENCE) {
                t.follow(dt, cam, lens.cell_angle(), moving);
            }
        }
        self.select(cam);
        stats.tracked = self.tracked.len();
        stats.reported = self.out.len();
        stats
    }

    /// Merge patch `k` into the tracked plane it belongs to, or start one.
    fn place(&mut self, k: usize, now_s: f32, lens: &Lens, cam: Vec3, stats: &mut Stats) {
        let max_range = self.config.max_range_m;
        let late = self
            .still_since
            .is_some_and(|since| now_s - since >= STILL_GRACE_S);
        let Self {
            scratch,
            tracked,
            coords,
            next_id,
            ..
        } = self;
        let p = scratch.patches[k];
        let cells = &scratch.cells[p.start..p.start + p.len];
        let pad = 0.5 * p.mean_range * lens.cell_angle();
        let far = (p.mean_range - CREATE_NEAR_M).max(0.0);
        let max_sd = (CREATE_NORMAL_SD_DEG - CREATE_SD_PER_M * far).max(CREATE_SD_MIN_DEG);
        let min_cells = MIN_CELLS as f32 + CREATE_CELLS_PER_M * far;
        let strong = p.fit.normal_sd() <= max_sd.to_radians() && p.len as f32 >= min_cells;
        let cos_merge = MERGE_ANGLE_DEG.to_radians().cos();
        let cos_near = (DUPLICATE_SCALE * MERGE_ANGLE_DEG).to_radians().cos();
        let mut best: Option<(usize, f32, [f32; 4])> = None;
        let mut duplicate = false;
        for (i, t) in tracked.iter().enumerate() {
            let cos = t.normal.dot(p.fit.normal);
            if cos < cos_near {
                continue;
            }
            let off = (t.normal.dot(p.fit.centroid) - t.offset()).abs();
            if off > MERGE_OFFSET_M {
                continue;
            }
            let ext = extents(&scratch.pts, cells, cam, t.frame(), pad, max_range, coords);
            if cos < cos_merge {
                duplicate |= overlaps(ext, t.edges, TOUCH_M);
                continue;
            }
            if !overlaps(ext, t.edges, TOUCH_M) {
                continue;
            }
            let score =
                cos.clamp(-1.0, 1.0).acos() / MERGE_ANGLE_DEG.to_radians() + off / MERGE_OFFSET_M;
            if best.is_none_or(|b| score < b.1) {
                best = Some((i, score, ext));
            }
        }
        if let Some((i, _, ext)) = best {
            let t = &mut tracked[i];
            t.absorb(&p.mom);
            t.strong += u32::from(strong);
            t.seen_edges = Some(t.seen_edges.map_or(ext, |s| union(s, ext)));
            t.merged = true;
            stats.merged += 1;
            return;
        }
        // A patch over a plane already tracked, just outside the merge
        // limits, is a noisy view of it: it starts no second plane, which
        // would appear beside the first and merge into it later.
        if duplicate {
            return;
        }
        // A weak patch still starts a plane, so a surface whose patches
        // pass only now and then is decided once, at the end of its first
        // window, instead of popping in when a strong one comes.
        if tracked.len() >= MAX_TRACKED {
            // Make room by the weakest plane not reported, if any.
            let weakest = tracked
                .iter()
                .enumerate()
                .filter(|(_, t)| !t.reported)
                .min_by(|a, b| a.1.score(cam).total_cmp(&b.1.score(cam)))
                .map(|(i, _)| i);
            let Some(i) = weakest else {
                return;
            };
            tracked.remove(i);
            stats.dropped += 1;
        }
        let mut t = Tracked::new(*next_id, &p, |f| {
            extents(&scratch.pts, cells, cam, f, pad, max_range, coords)
        });
        *next_id += 1;
        t.first_s = now_s;
        t.last_hit_s = now_s;
        t.last_seen_s = now_s;
        t.strong = u32::from(strong);
        t.late = late;
        tracked.push(t);
        stats.created += 1;
    }

    /// Merge tracked planes that have become one surface. The survivor is
    /// the one reported if only one is, else the older.
    fn merge_tracked(&mut self, stats: &mut Stats) {
        let n = self.tracked.len();
        for i in 0..n {
            for j in i + 1..n {
                let (a, b) = (self.tracked[i], self.tracked[j]);
                if !(a.alive && b.alive && same_surface(&a, &b)) {
                    continue;
                }
                let keep_j = b.appeared && !a.appeared;
                let (keep, gone) = if keep_j { (j, i) } else { (i, j) };
                let other = self.tracked[gone];
                self.tracked[keep].absorb_plane(&other);
                self.tracked[gone].alive = false;
                #[cfg(test)]
                self.events.push(format!(
                    "{:.1} {} merged into {}",
                    self.last_s.unwrap_or(0.0),
                    other.id,
                    self.tracked[keep].id
                ));
                stats.dropped += 1;
                if keep_j {
                    break;
                }
            }
        }
        self.tracked.retain(|t| t.alive);
    }

    /// Hits, misses, appearing, vanishing, forgetting, confidence.
    fn life(&mut self, now_s: f32, dt: f32, camera: &DepthCamera, stats: &mut Stats) {
        let cam = camera.position;
        for t in &mut self.tracked {
            if t.merged {
                t.hits += 1;
                t.last_hit_s = now_s;
                t.last_seen_s = now_s;
                t.miss_s = 0.0;
            } else if t.agrees {
                t.last_seen_s = now_s;
                t.miss_s = 0.0;
            } else if t.evidence >= MIN_EVIDENCE {
                t.miss_s += dt;
            }
            // A candidate in its decision window lives on patches; a held
            // one on the map agreeing with it too, so it is not dropped and
            // found again as a new candidate with a new decision.
            let last = if t.held.is_some() {
                t.last_seen_s
            } else {
                t.last_hit_s
            };
            if t.miss_s >= VANISH_S || (!t.appeared && now_s - last > CANDIDATE_TTL_S) {
                t.alive = false;
                #[cfg(test)]
                if t.appeared {
                    self.events.push(format!("{now_s:.1} {} vanished", t.id));
                }
            }
            // Keep the rectangle within reach of the camera.
            let (u, v) = t.axes();
            let d = cam - t.origin();
            let (cu, cv) = (d.dot(u), d.dot(v));
            let e = &mut t.edges;
            e[0] = e[0].max(cu - FORGET_M);
            e[1] = e[1].min(cu + FORGET_M);
            e[2] = e[2].max(cv - FORGET_M);
            e[3] = e[3].min(cv + FORGET_M);
            if e[0] > e[1] || e[2] > e[3] || t.distance_to(cam) > FORGET_M {
                t.alive = false;
                #[cfg(test)]
                self.events.push(format!("{now_s:.1} {} forgotten", t.id));
            }
            if !t.appeared && t.first_s < now_s {
                t.updates += 1;
            }
            if let Some((at, fwd)) = t.held
                && (at.distance(cam) > REDECIDE_MOVE_M
                    || fwd.angle_between(camera.forward()) > REDECIDE_TURN_DEG.to_radians())
            {
                t.held = None;
                t.late = false;
                t.first_s = now_s;
                t.hits = u32::from(t.merged);
                t.updates = 1;
            }
            let (window, frac) = if t.late {
                (APPEAR_STILL_S, APPEAR_STILL_FRAC)
            } else {
                (APPEAR_S, APPEAR_FRAC)
            };
            if t.alive && !t.appeared && t.held.is_none() && now_s - t.first_s >= window - 1e-3 {
                if t.hits >= APPEAR_HITS && t.hits as f32 >= frac * t.updates as f32 && t.strong > 0
                {
                    t.appeared = true;
                    t.snap();
                    #[cfg(test)]
                    self.events.push(format!("{now_s:.1} {} appeared", t.id));
                } else {
                    t.held = Some((cam, camera.forward()));
                }
            }
            t.confidence = (1.0 - (-(t.hits as f32) / CONF_HITS).exp())
                * (1.0 - t.miss_s / VANISH_S).clamp(0.0, 1.0);
        }
        let before = self.tracked.len();
        self.tracked.retain(|t| t.alive);
        stats.dropped += before - self.tracked.len();
    }

    /// Follow the lowest large upward plane well below the camera.
    fn update_floor(&mut self, dt: f32, cam: Vec3) {
        let cos_h = HORIZONTAL_DEG.to_radians().cos();
        let lowest = self
            .tracked
            .iter()
            .filter(|t| {
                (t.hits >= APPEAR_HITS || self.floor_y.is_none())
                    && t.normal.y >= cos_h
                    && t.area() >= FLOOR_MIN_AREA_M2
                    && t.centroid.y <= cam.y - FLOOR_BELOW_EYE_M
            })
            .map(|t| t.centroid.y)
            .min_by(f32::total_cmp);
        match (self.floor_y, lowest) {
            (None, Some(z)) => self.floor_y = Some(z),
            (Some(f), Some(z)) if z < f + FLOOR_TRACK_M => {
                self.floor_y = Some(f + (z - f) * (1.0 - (-dt / FLOOR_TAU_S).exp()));
            }
            _ => {}
        }
    }

    /// Redraw the reported set within the budget and fill the output.
    fn select(&mut self, cam: Vec3) {
        self.ranked.clear();
        for (i, t) in self.tracked.iter().enumerate() {
            if t.appeared {
                self.ranked.push((t.score(cam), i));
            }
        }
        self.ranked
            .sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        #[cfg(test)]
        let before: Vec<u32> = self
            .tracked
            .iter()
            .filter(|t| t.reported)
            .map(|t| t.id)
            .collect();
        for t in &mut self.tracked {
            t.reported = false;
        }
        for &(_, i) in self.ranked.iter().take(self.config.max_planes) {
            self.tracked[i].reported = true;
        }
        #[cfg(test)]
        for &(score, i) in &self.ranked {
            let t = &self.tracked[i];
            if t.reported != before.contains(&t.id) {
                self.events.push(format!(
                    "{:.1} {} {} at score {score:.3} (conf {:.2})",
                    self.last_s.unwrap_or(0.0),
                    t.id,
                    if t.reported { "in" } else { "out" },
                    t.confidence
                ));
            }
        }
        self.out.clear();
        self.out.extend(
            self.tracked
                .iter()
                .filter(|t| t.reported)
                .map(Tracked::live),
        );
    }

    /// Elements the store's buffers hold room for, for the no-growth test.
    #[cfg(test)]
    fn capacity(&self) -> usize {
        let s = &self.scratch;
        self.tracked.capacity()
            + self.out.capacity()
            + self.ranked.capacity()
            + self.coords.0.capacity()
            + self.coords.1.capacity()
            + s.sat.capacity()
            + s.pts.capacity()
            + s.range.capacity()
            + s.valid.capacity()
            + s.normal.capacity()
            + s.flat.capacity()
            + s.has_normal.capacity()
            + s.label.capacity()
            + s.order.capacity()
            + s.cells.capacity()
            + s.patches.capacity()
    }
}

#[cfg(test)]
impl LivePlanes {
    /// The farthest tracked plane's nearest point from `cam` (m).
    fn farthest_tracked(&self, cam: Vec3) -> f32 {
        self.tracked
            .iter()
            .map(|t| t.distance_to(cam))
            .fold(0.0, f32::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A rectangle: center, unit axes, half extents; the normal is `u x v`.
    #[derive(Debug, Clone, Copy)]
    struct Rect {
        c: Vec3,
        u: Vec3,
        v: Vec3,
        hu: f32,
        hv: f32,
    }

    impl Rect {
        fn normal(&self) -> Vec3 {
            self.u.cross(self.v)
        }

        fn hit(&self, o: Vec3, d: Vec3) -> Option<f32> {
            let n = self.normal();
            let den = n.dot(d);
            if den.abs() < 1e-9 {
                return None;
            }
            let t = n.dot(self.c - o) / den;
            if t <= 0.0 {
                return None;
            }
            let p = o + d * t - self.c;
            (p.dot(self.u).abs() <= self.hu && p.dot(self.v).abs() <= self.hv).then_some(t)
        }
    }

    /// A horizontal rectangle at `y` over `x0..x1`, `z0..z1`, facing up or
    /// down.
    fn level(y: f32, x: [f32; 2], z: [f32; 2], up: bool) -> Rect {
        Rect {
            c: Vec3::new(0.5 * (x[0] + x[1]), y, 0.5 * (z[0] + z[1])),
            u: Vec3::X,
            v: if up { Vec3::NEG_Z } else { Vec3::Z },
            hu: 0.5 * (x[1] - x[0]),
            hv: 0.5 * (z[1] - z[0]),
        }
    }

    /// A vertical rectangle standing on the ground line from `a` to `b`
    /// (`x`, `z`), `y0..y1` high, facing left of the walk from `a` to `b`.
    fn upright(a: [f32; 2], b: [f32; 2], y: [f32; 2]) -> Rect {
        let d = Vec3::new(b[0] - a[0], 0.0, b[1] - a[1]);
        Rect {
            c: Vec3::new(
                0.5 * (a[0] + b[0]),
                0.5 * (y[0] + y[1]),
                0.5 * (a[1] + b[1]),
            ),
            u: d.normalize(),
            v: Vec3::Y,
            hu: 0.5 * d.length(),
            hv: 0.5 * (y[1] - y[0]),
        }
    }

    #[derive(Debug, Clone, Copy)]
    enum Shape {
        Rect(Rect),
        Sphere(Vec3, f32),
        Gone,
    }

    #[derive(Debug, Clone, Default)]
    struct Scene {
        shapes: Vec<Shape>,
    }

    impl Scene {
        fn add(&mut self, r: Rect) -> usize {
            self.shapes.push(Shape::Rect(r));
            self.shapes.len() - 1
        }

        fn rect(&self, i: usize) -> Rect {
            match self.shapes[i] {
                Shape::Rect(r) => r,
                _ => panic!("shape {i} is not a rectangle"),
            }
        }

        fn remove(&mut self, ids: &[usize]) {
            for &i in ids {
                self.shapes[i] = Shape::Gone;
            }
        }

        fn hit(&self, o: Vec3, d: Vec3) -> Option<f32> {
            self.shapes
                .iter()
                .filter_map(|s| match *s {
                    Shape::Rect(r) => r.hit(o, d),
                    Shape::Sphere(c, rad) => {
                        let dd = d.length_squared();
                        let oc = o - c;
                        let b = oc.dot(d);
                        let disc = b * b - dd * (oc.length_squared() - rad * rad);
                        (disc >= 0.0)
                            .then(|| (-b - disc.sqrt()) / dd)
                            .filter(|&t| t > 0.0)
                    }
                    Shape::Gone => None,
                })
                .min_by(f32::total_cmp)
        }

        /// A room over `x`, `z`, `h` high: floor, ceiling and four walls
        /// facing in. Returns floor, ceiling, then the walls at `z[0]`
        /// (front), `x[0]` (left), `x[1]` (right) and `z[1]` (back).
        fn room(&mut self, x: [f32; 2], z: [f32; 2], h: f32) -> [usize; 6] {
            [
                self.add(level(0.0, x, z, true)),
                self.add(level(h, x, z, false)),
                self.add(upright([x[0], z[0]], [x[1], z[0]], [0.0, h])),
                self.add(upright([x[0], z[1]], [x[0], z[0]], [0.0, h])),
                self.add(upright([x[1], z[0]], [x[1], z[1]], [0.0, h])),
                self.add(upright([x[1], z[1]], [x[0], z[1]], [0.0, h])),
            ]
        }

        /// A box on the floor over `x`, `z`, `h` high: top, then the sides
        /// facing -z, +z... as `room` orders walls (front first).
        fn cuboid(&mut self, x: [f32; 2], z: [f32; 2], h: f32) -> [usize; 5] {
            [
                self.add(level(h, x, z, true)),
                self.add(upright([x[1], z[1]], [x[0], z[1]], [0.0, h])),
                self.add(upright([x[0], z[0]], [x[0], z[1]], [0.0, h])),
                self.add(upright([x[1], z[1]], [x[1], z[0]], [0.0, h])),
                self.add(upright([x[0], z[0]], [x[1], z[0]], [0.0, h])),
            ]
        }
    }

    /// The test room: 4 m by 4.5 m, 2.6 m high, a table (top at 0.75 m)
    /// and a 0.6 m box on the floor.
    struct Room {
        scene: Scene,
        floor: usize,
        ceiling: usize,
        walls: [usize; 4],
        table: usize,
        cuboid: [usize; 5],
    }

    fn room() -> Room {
        let mut scene = Scene::default();
        let [floor, ceiling, front, left, right, back] = scene.room([-2.0, 2.0], [-2.0, 2.5], 2.6);
        let table = scene.add(level(0.75, [-0.2, 1.0], [-0.5, 0.2], true));
        let cuboid = scene.cuboid([-1.3, -0.7], [-1.3, -0.7], 0.6);
        Room {
            scene,
            floor,
            ceiling,
            walls: [front, left, right, back],
            table,
            cuboid,
        }
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

    /// Standing in the test room, looking at the front wall and down.
    fn standing() -> DepthCamera {
        cam(Vec3::new(0.0, 1.6, 1.0), 0.0, -20.0)
    }

    /// The sensor model: error along the ray of `k` times the squared range
    /// (1% at 1 m, 4% at 4 m), D16 quantization, holes, and mixed cells at
    /// silhouettes (a blend of the two depths).
    #[derive(Debug, Clone, Copy)]
    struct Noise {
        k: f32,
        holes: f32,
        mixed: f32,
        quantize: bool,
    }

    const SENSOR: Noise = Noise {
        k: NOISE_K,
        holes: 0.03,
        mixed: 0.5,
        quantize: true,
    };

    const CLEAN: Noise = Noise {
        k: 0.0,
        holes: 0.0,
        mixed: 0.0,
        quantize: false,
    };

    /// The depth projection's near plane in the quantization (m).
    const D16_NEAR: f32 = 0.1;

    fn render(
        scene: &Scene,
        cam: &DepthCamera,
        grid: usize,
        noise: Noise,
        rng: &mut Rng,
        out: &mut Vec<Option<Vec3>>,
    ) {
        let lens = Lens::new(cam.fov, grid, grid);
        let du = (lens.r - lens.l) / grid as f32;
        let dv = (lens.u - lens.d) / grid as f32;
        out.clear();
        for row in 0..grid {
            for col in 0..grid {
                let local = lens.dir(col, row);
                let dir = cam.rotation * local;
                let Some(mut t) = scene.hit(cam.position, dir) else {
                    out.push(None);
                    continue;
                };
                if noise.mixed > 0.0 {
                    let mut other = None;
                    for (sx, sy) in [(-0.35, -0.35), (0.35, -0.35), (-0.35, 0.35), (0.35, 0.35)] {
                        let d = cam.rotation * (local + Vec3::new(sx * du, sy * dv, 0.0));
                        if let Some(s) = scene.hit(cam.position, d)
                            && (s - t).abs() > 0.05 * t
                        {
                            other = Some(s);
                        }
                    }
                    let mix = rng.uniform();
                    let at = rng.uniform();
                    if let Some(s) = other
                        && mix < noise.mixed
                    {
                        t += (s - t) * at;
                    }
                }
                let hole = rng.uniform() < noise.holes;
                let len = local.length();
                let r = t * len;
                t = (r + noise.k * r * r * rng.gauss()) / len;
                if noise.quantize {
                    let d = 1.0 - D16_NEAR / t;
                    let q = (d * 65535.0).round() / 65535.0;
                    t = if q < 1.0 {
                        decode_distance(q, D16_NEAR, f32::INFINITY)
                    } else {
                        f32::INFINITY
                    };
                }
                out.push((!hole && t.is_finite() && t > 0.0).then(|| cam.position + dir * t));
            }
        }
    }

    /// A scripted run: a scene, the store and a sensor, updated at `hz`.
    struct Sim {
        scene: Scene,
        planes: LivePlanes,
        rng: Rng,
        noise: Noise,
        grid: usize,
        hz: f32,
        t: f32,
        buf: Vec<Option<Vec3>>,
    }

    impl Sim {
        /// `LIVE_PLANES_SEED=n` shifts every run's seed, to check that a
        /// pass is not one lucky draw of the noise.
        fn new(scene: Scene, noise: Noise, seed: u64) -> Self {
            let off: u64 = std::env::var("LIVE_PLANES_SEED")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            Self {
                scene,
                planes: LivePlanes::default(),
                rng: Rng(seed + 1000 * off),
                noise,
                grid: 40,
                hz: 5.0,
                t: 0.0,
                buf: Vec::new(),
            }
        }

        fn step(&mut self, cam: &DepthCamera) -> Stats {
            render(
                &self.scene,
                cam,
                self.grid,
                self.noise,
                &mut self.rng,
                &mut self.buf,
            );
            let obs = Observation {
                camera: *cam,
                cols: self.grid,
                rows: self.grid,
                points: &self.buf,
            };
            let stats = self.planes.update(self.t, &obs);
            self.t += 1.0 / self.hz;
            stats
        }

        /// `secs` of updates with the camera `cam(t)`; `each(t, planes)`
        /// after every one.
        fn run(
            &mut self,
            secs: f32,
            cam: impl Fn(f32) -> DepthCamera,
            mut each: impl FnMut(f32, &LivePlanes),
        ) {
            let steps = (secs * self.hz).round() as usize;
            for _ in 0..steps {
                let t = self.t;
                self.step(&cam(t));
                each(t, &self.planes);
            }
        }
    }

    /// The reported plane on `truth`: normal within 10 degrees, center
    /// within 10 cm of its plane and over it (30 cm margin); the closest.
    /// With its normal's error (degrees) and its center's distance from the
    /// true plane (m).
    fn on<'a>(planes: &'a [LivePlane], truth: &Rect) -> Option<(&'a LivePlane, f32, f32)> {
        let n = truth.normal();
        planes
            .iter()
            .filter_map(|p| {
                let angle = p.normal().dot(n).abs().min(1.0).acos().to_degrees();
                let d = p.center - truth.c;
                let off = d.dot(n).abs();
                let over = d.dot(truth.u).abs() <= truth.hu + 0.3
                    && d.dot(truth.v).abs() <= truth.hv + 0.3;
                (angle < 10.0 && off < 0.1 && over).then_some((p, angle, off))
            })
            .min_by(|a, b| (a.1 / 3.0 + a.2 / 0.03).total_cmp(&(b.1 / 3.0 + b.2 / 0.03)))
    }

    /// Whether every reported plane lies on some surface of `scene`.
    fn ghosts(planes: &[LivePlane], scene: &Scene) -> Vec<LivePlane> {
        planes
            .iter()
            .filter(|p| {
                !scene.shapes.iter().any(|s| match s {
                    Shape::Rect(r) => on(std::slice::from_ref(p), r).is_some(),
                    _ => false,
                })
            })
            .copied()
            .collect()
    }

    /// One line per plane, for failure messages.
    fn dump(planes: &[LivePlane]) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for p in planes {
            let (c, n) = (p.center, p.normal());
            let _ = write!(
                out,
                "\n  id {:3} {:7} c ({:5.2} {:5.2} {:5.2}) n ({:5.2} {:5.2} {:5.2}) half {:.2} x {:.2} conf {:.2}",
                p.id,
                crate::surfaces::kind_name(p.kind),
                c.x,
                c.y,
                c.z,
                n.x,
                n.y,
                n.z,
                p.half.x,
                p.half.y,
                p.confidence
            );
        }
        out
    }

    fn assert_surface(planes: &[LivePlane], truth: &Rect, kind: u32, name: &str) -> u32 {
        let Some((p, angle, off)) = on(planes, truth) else {
            panic!("no plane on the {name}: {}", dump(planes));
        };
        eprintln!(
            "  {name}: id {} {} angle {angle:.2} deg, offset {:.1} cm, half {:.2} x {:.2} m",
            p.id,
            crate::surfaces::kind_name(p.kind),
            off * 100.0,
            p.half.x,
            p.half.y
        );
        assert_eq!(p.kind, kind, "{name}: {}", dump(std::slice::from_ref(p)));
        assert!(angle < 3.0, "{name}: {angle} deg");
        assert!(off < 0.03, "{name}: {off} m");
        p.id
    }

    // ---- The scripted timelines -----------------------------------------

    fn standing_still_at(hz: f32, seed: u64) {
        let r = room();
        let mut sim = Sim::new(r.scene.clone(), SENSOR, seed);
        sim.hz = hz;
        // Updates at 0 s, 1/hz, ... up to 1 s itself.
        sim.run(1.0 + 0.5 / hz, |_| standing(), |_, _| {});
        let planes = sim.planes.planes();
        eprintln!(
            "standing still at {hz} Hz, after 1 s: {} planes",
            planes.len()
        );
        assert_surface(planes, &r.scene.rect(r.floor), KIND_FLOOR, "floor");
        assert_surface(planes, &r.scene.rect(r.walls[0]), KIND_WALL, "front wall");
        assert_surface(planes, &r.scene.rect(r.walls[1]), KIND_WALL, "left wall");
        assert_surface(planes, &r.scene.rect(r.walls[2]), KIND_WALL, "right wall");
        assert_surface(planes, &r.scene.rect(r.table), KIND_TABLE, "table");
        let ghosts = ghosts(planes, &r.scene);
        assert!(ghosts.is_empty(), "planes on no surface: {}", dump(&ghosts));
    }

    #[test]
    fn standing_still_reports_floor_walls_and_table_within_a_second() {
        standing_still_at(5.0, 1);
        standing_still_at(2.0, 2);
        standing_still_at(1.0, 12);
    }

    #[test]
    fn turning_full_circle_tracks_every_wall_and_keeps_the_first() {
        let r = room();
        let mut sim = Sim::new(r.scene.clone(), SENSOR, 3);
        let pos = Vec3::new(0.0, 1.6, 0.25);
        let turn = |t: f32| cam(pos, (360.0 * t / 8.0).min(360.0), -5.0);
        let front = r.scene.rect(r.walls[0]);
        let left = r.scene.rect(r.walls[1]);
        // Each wall as it was once the camera had turned away from it.
        // The front wall spans yaw -41.6..41.6 degrees from here and the
        // left 41.6..138.4: out of the 100 degree view from 2.04 to 5.96 s
        // and from 4.19 to 7.81 s.
        let mut away: Vec<(&str, Rect, f32, f32, Option<LivePlane>)> = vec![
            ("front", front, 2.2, 5.8, None),
            ("left", left, 4.4, 7.6, None),
        ];
        let mut max_drift = 0.0f32;
        sim.run(9.6, turn, |t, planes| {
            for (_, truth, from, until, snap) in &mut away {
                if snap.is_none() && t >= *from {
                    *snap = on(planes.planes(), truth).map(|x| *x.0);
                }
                // Out of view: kept as it was.
                if let Some(s) = snap
                    && t <= *until
                {
                    let now = planes.planes().iter().find(|p| p.id == s.id);
                    let now = now.unwrap_or_else(|| panic!("wall {} dropped at {t}", s.id));
                    max_drift = max_drift.max(now.center.distance(s.center));
                }
            }
        });
        let planes = sim.planes.planes();
        eprintln!(
            "turning: {} planes, out-of-view drift {:.1} cm",
            planes.len(),
            max_drift * 100.0
        );
        assert!(max_drift < 0.02, "{max_drift}");
        for (name, truth, _, _, snap) in &away {
            let s = snap.unwrap_or_else(|| panic!("no {name} wall at its snapshot"));
            let (p, _, _) = on(planes, truth).unwrap_or_else(|| panic!("{name} wall gone"));
            let moved = p.center.distance(s.center);
            let turned = p.normal().angle_between(s.normal()).to_degrees();
            eprintln!(
                "  {name} wall back in view: id {} (was {}), moved {:.1} cm, turned {turned:.2} deg",
                p.id,
                s.id,
                moved * 100.0
            );
            assert_eq!(p.id, s.id, "{name}");
            assert!(
                moved < 0.05 && turned < 1.0,
                "{name}: {moved} m, {turned} deg"
            );
        }
        assert_surface(planes, &r.scene.rect(r.floor), KIND_FLOOR, "floor");
        assert_surface(planes, &r.scene.rect(r.ceiling), KIND_CEILING, "ceiling");
        for (k, name) in ["front", "left", "right", "back"].iter().enumerate() {
            assert_surface(planes, &r.scene.rect(r.walls[k]), KIND_WALL, name);
        }
        let ghosts = ghosts(planes, &r.scene);
        assert!(ghosts.is_empty(), "planes on no surface: {}", dump(&ghosts));
    }

    #[test]
    fn noise_alone_keeps_ids_and_centers_still() {
        let r = room();
        let mut sim = Sim::new(r.scene.clone(), SENSOR, 4);
        let mut history: Vec<(f32, Vec<LivePlane>)> = Vec::new();
        sim.run(
            10.0,
            |_| standing(),
            |t, planes| {
                history.push((t, planes.planes().to_vec()));
            },
        );
        let ids = |ps: &[LivePlane]| ps.iter().map(|p| p.id).collect::<Vec<_>>();
        let first = history.iter().position(|(t, _)| *t >= 1.0 - 1e-3).unwrap();
        let settled = ids(&history[first].1);
        let per_s = sim.hz.round() as usize;
        let mut worst = (0.0f32, 0u32, 0.0f32);
        for k in first..history.len() {
            let (t, ps) = &history[k];
            assert_eq!(
                ids(ps),
                settled,
                "ids changed at {t}: {}\n{:?}",
                dump(ps),
                sim.planes.events
            );
            if k >= first + per_s {
                for p in ps {
                    let before = history[k - per_s].1.iter().find(|q| q.id == p.id).unwrap();
                    let moved = p.center.distance(before.center);
                    if moved > worst.0 {
                        worst = (moved, p.id, *t);
                    }
                }
            }
        }
        eprintln!(
            "noise only: ids {settled:?} from 1 s; fastest center {:.2} cm/s (id {} at {:.1} s)",
            worst.0 * 100.0,
            worst.1,
            worst.2
        );
        assert!(worst.0 <= 0.01, "{worst:?}");
        let planes = &history.last().unwrap().1;
        for (truth, kind, name) in [
            (r.floor, KIND_FLOOR, "floor"),
            (r.walls[0], KIND_WALL, "front wall"),
            (r.table, KIND_TABLE, "table"),
        ] {
            assert_surface(planes, &r.scene.rect(truth), kind, name);
        }
    }

    /// Whether any reported plane lies on one of the box's faces.
    fn box_planes(planes: &[LivePlane], scene: &Scene, faces: &[Rect]) -> Vec<u32> {
        let _ = scene;
        faces
            .iter()
            .filter_map(|f| on(planes, f).map(|x| x.0.id))
            .collect()
    }

    #[test]
    fn a_removed_box_is_dropped_in_view_and_kept_out_of_view() {
        let r = room();
        let faces: Vec<Rect> = [r.cuboid[0], r.cuboid[1]]
            .iter()
            .map(|&i| r.scene.rect(i))
            .collect();
        let at = Vec3::new(-1.0, 1.6, 0.0);
        let look = |yaw: f32| cam(at, yaw, -50.0);

        // In view: dropped within about 3 s.
        let mut sim = Sim::new(r.scene.clone(), SENSOR, 5);
        sim.run(2.0, |_| look(0.0), |_, _| {});
        let seen = box_planes(sim.planes.planes(), &sim.scene, &faces);
        eprintln!("box seen as {seen:?}");
        assert_eq!(seen.len(), 2, "{}", dump(sim.planes.planes()));
        sim.scene.remove(&r.cuboid);
        let removed = sim.t;
        let mut gone_at = None;
        sim.run(
            4.0,
            |_| look(0.0),
            |t, planes| {
                let left = planes
                    .planes()
                    .iter()
                    .filter(|p| seen.contains(&p.id))
                    .count();
                if left == 0 && gone_at.is_none() {
                    gone_at = Some(t - removed);
                }
            },
        );
        eprintln!("box removed in view: dropped after {gone_at:?} s");
        assert!(gone_at.is_some_and(|s| s <= 3.0), "{gone_at:?}");
        let ghosts = ghosts(sim.planes.planes(), &sim.scene);
        assert!(ghosts.is_empty(), "planes on no surface: {}", dump(&ghosts));

        // Out of view: kept, then dropped once back in view.
        let mut sim = Sim::new(r.scene.clone(), SENSOR, 6);
        sim.run(2.0, |_| look(0.0), |_, _| {});
        let seen = box_planes(sim.planes.planes(), &sim.scene, &faces);
        assert_eq!(seen.len(), 2);
        let t0 = sim.t;
        sim.run(1.0, |t| look(180.0 * (t - t0)), |_, _| {});
        sim.scene.remove(&r.cuboid);
        sim.run(
            5.0,
            |_| look(180.0),
            |t, planes| {
                let kept = planes
                    .planes()
                    .iter()
                    .filter(|p| seen.contains(&p.id))
                    .count();
                assert_eq!(kept, 2, "box dropped out of view at {t}");
            },
        );
        let t1 = sim.t;
        sim.run(1.0, |t| look(180.0 - 180.0 * (t - t1)), |_, _| {});
        let back = sim.t;
        let mut gone_at = None;
        sim.run(
            4.0,
            |_| look(0.0),
            |t, planes| {
                let left = planes
                    .planes()
                    .iter()
                    .filter(|p| seen.contains(&p.id))
                    .count();
                if left == 0 && gone_at.is_none() {
                    gone_at = Some(t - back);
                }
            },
        );
        eprintln!("box removed out of view: kept 5 s, dropped {gone_at:?} s after it came back");
        assert!(gone_at.is_some_and(|s| s <= 3.0), "{gone_at:?}");
    }

    /// A corridor 2 m wide and 2.5 m high along -z, with pillars 0.3 m
    /// proud of the walls every 3 m on each side.
    fn corridor() -> (Scene, usize) {
        let mut scene = Scene::default();
        let [floor, ..] = scene.room([-1.0, 1.0], [-24.0, 3.0], 2.5);
        for k in 0..8 {
            let z = -1.5 - 3.0 * k as f32;
            for (x0, x1, zc) in [(-1.0, -0.7, z), (0.7, 1.0, z - 1.5)] {
                scene.cuboid([x0, x1], [zc - 0.2, zc + 0.2], 2.5);
            }
        }
        (scene, floor)
    }

    #[test]
    fn walking_a_corridor_forgets_far_planes_and_keeps_one_floor() {
        let (scene, floor) = corridor();
        let floor = scene.rect(floor);
        let mut sim = Sim::new(scene, SENSOR, 7);
        let walk = |t: f32| cam(Vec3::new(0.0, 1.6, -t.min(12.0)), 0.0, -10.0);
        let mut floor_ids = Vec::new();
        let (mut most, mut most_tracked, mut most_floors) = (0, 0, 0);
        sim.run(12.4, walk, |t, planes| {
            let ps = planes.planes();
            most = most.max(ps.len());
            most_tracked = most_tracked.max(planes.tracked_count());
            assert!(ps.len() <= MAX_PLANES, "{} planes at {t}", ps.len());
            let floors: Vec<u32> = ps
                .iter()
                .filter(|p| p.kind == KIND_FLOOR)
                .map(|p| p.id)
                .collect();
            most_floors = most_floors.max(floors.len());
            for id in floors {
                if !floor_ids.contains(&id) {
                    floor_ids.push(id);
                }
            }
            let c = walk(t).position;
            let far = planes.farthest_tracked(c);
            assert!(far <= FORGET_M, "a plane {far} m away at {t}");
        });
        let c = walk(sim.t).position;
        let (p, angle, off) = on(sim.planes.planes(), &floor).expect("no floor");
        let near = c.z - p.center.z;
        eprintln!(
            "corridor: at most {most} reported, {most_tracked} tracked, {most_floors} floors at once, \
             floor ids {floor_ids:?}; the floor now {:.1} m long, centered {near:.1} m ahead, \
             angle {angle:.2} deg, offset {:.1} cm",
            2.0 * p.half.y,
            off * 100.0
        );
        assert!(most_floors <= 2 && floor_ids.len() <= 3, "{floor_ids:?}");
        assert_eq!(p.kind, KIND_FLOOR);
    }

    #[test]
    fn parallel_surfaces_twenty_cm_apart_stay_two_planes() {
        let mut scene = Scene::default();
        scene.room([-2.0, 2.0], [-2.5, 2.0], 2.6);
        let table = scene.add(level(0.75, [-0.6, 0.6], [-1.8, -0.8], true));
        let shelf = scene.add(level(0.95, [-0.6, 0.6], [-1.8, -1.4], true));
        let (table, shelf) = (scene.rect(table), scene.rect(shelf));
        let mut sim = Sim::new(scene, SENSOR, 8);
        let view = |_| cam(Vec3::new(0.0, 1.8, -0.6), 0.0, -50.0);
        let mut ids = None;
        sim.run(4.0, view, |t, planes| {
            if t >= 2.0 {
                let a = on(planes.planes(), &table).map(|x| x.0.id);
                let b = on(planes.planes(), &shelf).map(|x| x.0.id);
                let now = (a, b);
                assert!(ids.is_none_or(|i| i == now), "{now:?} at {t}");
                ids = Some(now);
            }
        });
        let planes = sim.planes.planes();
        eprintln!("table top and shelf 20 cm above it:");
        let a = assert_surface(planes, &table, KIND_TABLE, "table top");
        let b = assert_surface(planes, &shelf, KIND_TABLE, "shelf");
        assert_ne!(a, b);
    }

    #[test]
    fn a_thirty_degree_ramp_is_other() {
        let mut scene = Scene::default();
        scene.room([-2.0, 2.0], [-3.0, 2.0], 2.6);
        let (s, c) = (30.0f32.to_radians().sin(), 30.0f32.to_radians().cos());
        let ramp = Rect {
            c: Vec3::new(0.0, 0.8 * s, -0.6 - 0.8 * c),
            u: Vec3::X,
            v: Vec3::new(0.0, s, -c),
            hu: 0.6,
            hv: 0.8,
        };
        scene.add(ramp);
        let mut sim = Sim::new(scene.clone(), SENSOR, 9);
        sim.run(
            3.0,
            |_| cam(Vec3::new(0.0, 1.6, 1.0), 0.0, -30.0),
            |_, _| {},
        );
        let planes = sim.planes.planes();
        eprintln!("ramp:");
        let id = assert_surface(planes, &ramp, KIND_OTHER, "30 degree ramp");
        // Nothing else on the ramp's plane claims to be a wall or a floor.
        for p in planes.iter().filter(|p| p.id != id) {
            let angle = p.normal().angle_between(ramp.normal()).to_degrees();
            assert!(angle > 10.0 || p.kind == KIND_OTHER, "{p:?}");
        }
        let ghosts = ghosts(planes, &scene);
        assert!(ghosts.is_empty(), "planes on no surface: {}", dump(&ghosts));
    }

    #[test]
    fn a_hand_near_the_camera_makes_no_plane_and_does_not_split_the_wall() {
        let mut scene = Scene::default();
        let [_, _, front, ..] = scene.room([-2.0, 2.0], [-1.5, 2.0], 2.6);
        let wall = scene.rect(front);
        let eye = Vec3::new(0.0, 1.5, 0.0);
        // A flat palm (the worst case: planar) and a fist, 0.4 m out.
        scene.add(Rect {
            c: eye + Vec3::new(-0.08, -0.1, -0.4),
            u: Vec3::X,
            v: Vec3::Y,
            hu: 0.05,
            hv: 0.09,
        });
        scene
            .shapes
            .push(Shape::Sphere(eye + Vec3::new(0.12, -0.05, -0.4), 0.05));
        let mut sim = Sim::new(scene, SENSOR, 10);
        let mut wall_id = None;
        sim.run(
            4.0,
            |_| cam(eye, 0.0, 0.0),
            |t, planes| {
                let ps = planes.planes();
                for p in ps {
                    assert!(
                        p.center.distance(eye) > 0.6,
                        "a plane at the hand at {t}: {p:?}"
                    );
                }
                if t >= 1.0 {
                    let on_wall: Vec<u32> = ps
                        .iter()
                        .filter(|p| on(std::slice::from_ref(p), &wall).is_some())
                        .map(|p| p.id)
                        .collect();
                    assert_eq!(on_wall.len(), 1, "the wall as {on_wall:?} at {t}");
                    assert!(wall_id.is_none_or(|w| w == on_wall[0]));
                    wall_id = Some(on_wall[0]);
                }
            },
        );
        eprintln!("hand in front of a wall:");
        assert_surface(
            sim.planes.planes(),
            &wall,
            KIND_WALL,
            "wall behind the hand",
        );
    }

    /// 40 panels facing a camera at (0, 1.6, 0) looking along -z: 8 by 5,
    /// tiling the view with a gap between them, 1.5 and 1.8 m away in a
    /// checkerboard so no two neighbors are one plane. Nothing behind them:
    /// this is about the budget, not the blends at a deep step.
    fn panels() -> Scene {
        let mut scene = Scene::default();
        let (l, r, u, d) = (
            FOV.left.tan(),
            FOV.right.tan(),
            FOV.up.tan(),
            FOV.down.tan(),
        );
        let (pw, ph) = ((r - l) / 8.0, (u - d) / 5.0);
        for i in 0..8 {
            for j in 0..5 {
                let depth = if (i + j) % 2 == 0 { 1.5 } else { 1.8 };
                let x = depth * (l + (i as f32 + 0.5) * pw);
                let y = 1.6 + depth * (d + (j as f32 + 0.5) * ph);
                let (hw, hh) = (0.4 * depth * pw, 0.4 * depth * ph);
                scene.add(upright(
                    [x - hw, -depth],
                    [x + hw, -depth],
                    [y - hh, y + hh],
                ));
            }
        }
        scene
    }

    #[test]
    fn forty_patches_stay_within_the_budget() {
        let scene = panels();
        let mut sim = Sim::new(scene.clone(), SENSOR, 11);
        // 40 panels in one view are 4 cells across on the 40x40 grid, under
        // what a patch needs; this test is about the budget, so it runs
        // the grid a later step may read.
        sim.grid = 80;
        let view = |_| cam(Vec3::new(0.0, 1.6, 0.0), 0.0, 0.0);
        let mut settled: Option<Vec<u32>> = None;
        let mut most_tracked = 0;
        sim.run(8.0, view, |t, planes| {
            let ps = planes.planes();
            assert!(ps.len() <= MAX_PLANES, "{} at {t}", ps.len());
            most_tracked = most_tracked.max(planes.tracked_count());
            if t >= 3.0 {
                let ids: Vec<u32> = ps.iter().map(|p| p.id).collect();
                if let Some(s) = &settled {
                    assert_eq!(&ids, s, "the set changed at {t}: {:?}", planes.events);
                }
                settled = Some(ids);
            }
        });
        let ps = sim.planes.planes();
        eprintln!(
            "40 panels: {} reported, {} tracked (at most {most_tracked}), stable from 3 s",
            ps.len(),
            sim.planes.tracked_count()
        );
        assert_eq!(ps.len(), MAX_PLANES);
        assert!(sim.planes.tracked_count() > MAX_PLANES + 4);
        let ghosts = ghosts(ps, &scene);
        assert!(ghosts.is_empty(), "planes on no surface: {}", dump(&ghosts));
    }

    // ---- The pieces ------------------------------------------------------

    #[test]
    fn unproject_grid_lands_each_point_in_its_own_cell() {
        // A camera 1.5 m over a floor, turned and tilted; the read-back's
        // 40x40 grid of a 320x320 map, each texel's depth where its ray
        // meets the floor, stored as the runtime stores it.
        let (cols, rows, size) = (40, 40, [320u32, 320]);
        let rot = Quat::from_rotation_y(0.5) * Quat::from_rotation_x(-0.3);
        let view = DepthView {
            orientation: rot.to_array(),
            position: [1.0, 1.5, -0.5],
            fov: FOV,
        };
        let near = 0.1;
        for far in [f32::INFINITY, 20.0] {
            for reading in [Reading::RowBottom, Reading::RowTop, Reading::ColsMirrored] {
                let mut depth = vec![1.0f32; cols * rows];
                for gy in 0..rows {
                    for gx in 0..cols {
                        let (tx, ty) =
                            (grid_texel(gx, cols, size[0]), grid_texel(gy, rows, size[1]));
                        let dir = rot * texel_dir(FOV, size, tx, ty, reading);
                        if dir.y < -1e-3 {
                            // Depth along -Z: the ray is scaled to z = -1.
                            let z = 1.5 / -dir.y;
                            depth[gy * cols + gx] = if far.is_finite() {
                                (far + near - 2.0 * near * far / z) / (2.0 * (far - near)) + 0.5
                            } else {
                                1.0 - near / z
                            };
                        }
                    }
                }
                let mut out = Vec::new();
                unproject_grid(
                    &depth, cols, rows, size, &view, near, far, reading, &mut out,
                );
                let cam = DepthCamera::from_view(&view);
                let lens = Lens::new(FOV, cols, rows);
                let inv = cam.rotation.inverse();
                let mut seen = 0;
                for (i, p) in out.iter().enumerate() {
                    let Some(p) = p else {
                        continue;
                    };
                    seen += 1;
                    assert!(p.y.abs() < 1e-3, "{reading:?} {far}: {p} off the floor");
                    assert_eq!(
                        lens.cell(inv * (*p - cam.position)),
                        Some(i),
                        "{reading:?} {far}"
                    );
                }
                // The floor fills the lower part of the view: row 0 is the
                // top, so the last row has points and the first has none.
                assert!(seen > 400, "{reading:?}: {seen}");
                assert!(out[(rows - 1) * cols..].iter().all(Option::is_some));
                assert!(out[..cols].iter().all(Option::is_none));
            }
        }
    }

    #[test]
    fn uuids_are_tagged_and_round_trip() {
        let u = plane_uuid(7);
        assert_eq!(&u[..6], b"fslive");
        assert_ne!(u, crate::room_file::STAGE_FLOOR_UUID);
        assert_eq!(u[6] >> 4, 0, "version nibble");
        assert_eq!(u[8] >> 6, 0, "variant bits");
        assert_eq!(plane_id(&u), Some(7));
        assert_eq!(plane_id(&plane_uuid(u32::MAX)), Some(u32::MAX));
        assert_eq!(plane_id(&crate::room_file::STAGE_FLOOR_UUID), None);
        // A runtime anchor's UUID (version 4, RFC 4122 variant).
        let anchor = [
            0x3f, 0x2a, 0x91, 0x0c, 0x5d, 0x11, 0x4e, 0x2b, 0x9a, 0x7c, 0x01, 0x23, 0x45, 0x67,
            0x89, 0xab,
        ];
        assert_eq!(plane_id(&anchor), None);
    }

    #[test]
    fn an_obstacle_box_is_the_reported_plane() {
        let p = LivePlane {
            id: 42,
            kind: KIND_WALL,
            center: Vec3::new(1.0, 1.2, -2.0),
            rot: Quat::from_rotation_y(0.3),
            half: Vec3::new(0.9, 1.1, PLANE_HALF_THICKNESS_M),
            confidence: 0.8,
            last_seen_s: 3.0,
        };
        let b = to_obstacle_box(&p);
        assert_close!(b.center, [1.0, 1.2, -2.0]);
        assert_close!(b.rot, p.rot.to_array());
        assert_close!(b.half, [0.9, 1.1, PLANE_HALF_THICKNESS_M]);
        assert_eq!(b.kind, KIND_WALL);
        assert_close!(b.emit, 1.0);
        assert!(!b.hidden);
        assert_eq!(plane_id(&b.uuid), Some(42));
        assert!(crate::surfaces::is_plane(Vec3::from(b.half)));
    }

    /// The covariance of `pts`, as `sym3_eigen` takes it.
    fn covariance(pts: &[Vec3]) -> [f64; 6] {
        let n = pts.len() as f64;
        let m = pts.iter().map(|p| p.as_dvec3()).sum::<DVec3>() / n;
        let mut c = [0.0; 6];
        for p in pts {
            for (a, b) in c.iter_mut().zip(outer(p.as_dvec3() - m, 1.0 / n)) {
                *a += b;
            }
        }
        c
    }

    #[test]
    fn sym3_eigen_finds_a_planes_normal_and_refuses_a_line() {
        let n = Vec3::new(0.3, 0.9, -0.2).normalize();
        let (u, v) = plane_axes(n, true, Vec3::X);
        let c = Vec3::new(5.0, 1.0, -3.0);
        let mut pts = Vec::new();
        for i in -3..=3 {
            for j in -2..=2 {
                pts.push(c + u * (0.1 * i as f32) + v * (0.1 * j as f32));
            }
        }
        let (lambda, normal, major) = sym3_eigen(covariance(&pts)).unwrap();
        assert!(normal.as_vec3().dot(n).abs() > 0.99999, "{normal}");
        // The plane is longer along u.
        assert!(major.as_vec3().dot(u).abs() > 0.99999, "{major}");
        assert!(lambda[0].abs() < 1e-9 && lambda[1] > 0.0 && lambda[2] > lambda[1]);
        // A square: the two in-plane eigenvalues are equal, any major will do.
        let square: Vec<Vec3> = (-2..=2)
            .flat_map(|i| (-2..=2).map(move |j| Vec3::new(i as f32, 0.0, j as f32) * 0.1))
            .collect();
        let (_, normal, major) = sym3_eigen(covariance(&square)).unwrap();
        assert!(normal.y.abs() > 0.99999 && major.y.abs() < 1e-6);
        let line: Vec<Vec3> = (0..10).map(|i| Vec3::X * (0.1 * i as f32)).collect();
        assert!(sym3_eigen(covariance(&line)).is_none());
    }

    #[test]
    fn follow_holds_in_its_dead_band_and_keeps_to_its_speed() {
        assert_close!(follow(0.02, 0.03, 1.0, 0.2), 0.0);
        assert_close!(follow(-0.02, 0.03, 1.0, 0.2), 0.0);
        let step = follow(1.0, 0.03, 0.5, 0.2);
        assert!(step > 0.0 && step <= 0.1 + 1e-6, "{step}");
        assert!(follow(-1.0, 0.03, 0.5, 0.2) < 0.0);
        // Small excess: a fraction of it, not the rate limit.
        let step = follow(0.05, 0.03, 1.0, 0.2);
        assert!(step > 0.0 && step < 0.02, "{step}");
    }

    /// A tracked plane through `c` with normal `n`, `half` in its axes.
    fn plane_at(c: Vec3, n: Vec3, half: [f32; 2]) -> Tracked {
        let n = n.normalize();
        let (u, v) = plane_axes(n, n.y.abs() < 10.0f32.to_radians().cos(), Vec3::X);
        let mut mom = Moments::new(c);
        for i in -4..=4 {
            for j in -4..=4 {
                let p = c + u * (half[0] * i as f32 / 4.0) + v * (half[1] * j as f32 / 4.0);
                mom.add(p, c + n * 2.0, 1.0);
            }
        }
        let mut fit = mom.fit().unwrap();
        if fit.normal.dot(n) < 0.0 {
            fit.normal = -fit.normal;
        }
        let patch = Patch {
            mom,
            fit,
            start: 0,
            len: 81,
            mean_range: 2.0,
        };
        Tracked::new(1, &patch, |_| [-half[0], half[0], -half[1], half[1]])
    }

    #[test]
    fn kinds_by_normal_and_height() {
        let kind = |c: Vec3, n: Vec3, half: [f32; 2]| plane_at(c, n, half).classify(0.0);
        assert_eq!(
            kind(Vec3::new(0.0, 0.02, -1.0), Vec3::Y, [1.0, 1.0]),
            KIND_FLOOR
        );
        assert_eq!(
            kind(Vec3::new(0.0, 0.75, -1.0), Vec3::Y, [0.5, 0.4]),
            KIND_TABLE
        );
        assert_eq!(
            kind(Vec3::new(0.0, 1.25, -1.0), Vec3::Y, [0.5, 0.4]),
            KIND_TABLE
        );
        // A step, and a high shelf: neither floor nor table.
        assert_eq!(
            kind(Vec3::new(0.0, 0.2, -1.0), Vec3::Y, [0.5, 0.4]),
            KIND_OTHER
        );
        assert_eq!(
            kind(Vec3::new(0.0, 1.6, -1.0), Vec3::Y, [0.5, 0.4]),
            KIND_OTHER
        );
        assert_eq!(
            kind(Vec3::new(0.0, 2.6, -1.0), Vec3::NEG_Y, [2.0, 2.0]),
            KIND_CEILING
        );
        // A table's underside.
        assert_eq!(
            kind(Vec3::new(0.0, 0.7, -1.0), Vec3::NEG_Y, [0.5, 0.4]),
            KIND_OTHER
        );
        assert_eq!(
            kind(Vec3::new(0.0, 1.3, -2.0), Vec3::Z, [2.0, 1.2]),
            KIND_WALL
        );
        // Within 15 degrees of vertical, still a wall.
        let tilted = Vec3::new(0.0, 12.0f32.to_radians().sin(), 12.0f32.to_radians().cos());
        assert_eq!(
            kind(Vec3::new(0.0, 1.3, -2.0), tilted, [2.0, 1.2]),
            KIND_WALL
        );
        // A box side reaches 0.5 m: other.
        assert_eq!(
            kind(Vec3::new(0.0, 0.25, -2.0), Vec3::Z, [0.25, 0.25]),
            KIND_OTHER
        );
        // A 30 degree ramp: other.
        let ramp = Vec3::new(0.0, 30.0f32.to_radians().cos(), 30.0f32.to_radians().sin());
        assert_eq!(
            kind(Vec3::new(0.0, 0.4, -1.5), ramp, [0.6, 0.8]),
            KIND_OTHER
        );
        // A table at 1.33 m stays a table; a new plane there is other.
        let mut t = plane_at(Vec3::new(0.0, 1.33, -1.0), Vec3::Y, [0.5, 0.4]);
        assert_eq!(t.classify(0.0), KIND_OTHER);
        t.kind = KIND_TABLE;
        assert_eq!(t.classify(0.0), KIND_TABLE);
    }

    #[test]
    fn the_noise_correction_untilts_a_small_far_plane() {
        // A 0.5 m panel facing +z, 3.3 m from the camera at 45 degrees,
        // with the sensor's noise along each ray: a plain fit leans toward
        // the rays, the corrected one does not.
        let cam = Vec3::ZERO;
        let c = Vec3::new(-2.33, 0.0, -2.33);
        let mut rng = Rng(5);
        // The normals averaged over the trials, so what is left is the bias.
        let (mut plain, mut corrected) = (Vec3::ZERO, Vec3::ZERO);
        let trials = 40;
        for _ in 0..trials {
            let mut m = Moments::new(c);
            for i in -4..=4 {
                for j in -4..=4 {
                    let p = c + Vec3::new(0.06 * i as f32, 0.06 * j as f32, 0.0);
                    let ray = (p - cam).normalize();
                    let q = p + ray * (noise_sigma(p.length()) * rng.gauss());
                    m.add(q, cam, weight(q.length()));
                }
            }
            let w = m.w;
            let mm = outer(m.s / w, 1.0);
            let mut raw = [0.0; 6];
            for k in 0..6 {
                raw[k] = m.ss[k] / w - mm[k];
            }
            let (_, n_plain, _) = sym3_eigen(raw).unwrap();
            let toward = |n: Vec3| if n.z < 0.0 { -n } else { n };
            plain += toward(n_plain.as_vec3());
            corrected += toward(m.fit().unwrap().normal);
        }
        let off = |n: Vec3| n.normalize().dot(Vec3::Z).min(1.0).acos().to_degrees();
        let (plain, corrected) = (off(plain), off(corrected));
        eprintln!(
            "a 0.5 m panel at 3.3 m, 45 degrees: plain fit leans {plain:.1} deg, corrected {corrected:.1}"
        );
        assert!(plain > 8.0, "{plain}");
        assert!(corrected < 2.0, "{corrected}");
    }

    /// With no noise at all, the fit still takes out the noise its model
    /// expects, so walls seen at a slant 3 m off lean by about 2.5 degrees
    /// the other way: the model has to match the sensor (the device step
    /// measures it). Within the brief's 3 degrees and 1 cm all the same.
    #[test]
    fn clean_depth_gives_planes_within_the_bar() {
        let r = room();
        let mut sim = Sim::new(r.scene.clone(), CLEAN, 0);
        sim.run(1.0, |_| standing(), |_, _| {});
        let planes = sim.planes.planes();
        for (truth, name) in [
            (r.floor, "floor"),
            (r.walls[0], "front wall"),
            (r.walls[1], "left wall"),
            (r.walls[2], "right wall"),
            (r.table, "table"),
        ] {
            let (_, angle, off) =
                on(planes, &r.scene.rect(truth)).unwrap_or_else(|| panic!("{name}"));
            eprintln!("  clean {name}: {angle:.2} deg, {:.2} cm", off * 100.0);
            assert!(angle < 3.0 && off < 0.01, "{name}: {angle} deg, {off} m");
        }
        let ghosts = ghosts(planes, &r.scene);
        assert!(ghosts.is_empty(), "planes on no surface: {}", dump(&ghosts));
    }

    #[test]
    fn no_heap_growth_after_warm_up() {
        let r = room();
        let mut sim = Sim::new(r.scene.clone(), SENSOR, 13);
        let pos = Vec3::new(0.0, 1.6, 0.25);
        sim.run(2.0, |_| cam(pos, 0.0, -5.0), |_, _| {});
        let warm = sim.planes.capacity();
        sim.run(8.0, |t| cam(pos, 45.0 * (t - 2.0), -5.0), |_, _| {});
        assert_eq!(sim.planes.capacity(), warm);
    }

    /// The cost of an update, rendering left out. `cargo test --release -p
    /// fosfora-xr live_planes::tests::update_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "a timing run; read its output"]
    fn update_cost() {
        let r = room();
        for grid in [40, 80] {
            let mut sim = Sim::new(r.scene.clone(), SENSOR, 14);
            sim.grid = grid;
            let frames: Vec<Vec<Option<Vec3>>> = (0..200)
                .map(|_| {
                    let mut buf = Vec::new();
                    render(&r.scene, &standing(), grid, SENSOR, &mut sim.rng, &mut buf);
                    buf
                })
                .collect();
            let mut times = Vec::with_capacity(frames.len());
            for (k, points) in frames.iter().enumerate() {
                let obs = Observation {
                    camera: standing(),
                    cols: grid,
                    rows: grid,
                    points,
                };
                let t0 = std::time::Instant::now();
                std::hint::black_box(sim.planes.update(k as f32 * 0.2, &obs));
                times.push(t0.elapsed().as_secs_f64() * 1e3);
            }
            let warm = &mut times[20..];
            warm.sort_by(f64::total_cmp);
            let mean = warm.iter().sum::<f64>() / warm.len() as f64;
            eprintln!(
                "{grid}x{grid}: mean {mean:.3} ms, median {:.3}, max {:.3} over {} updates",
                warm[warm.len() / 2],
                warm[warm.len() - 1],
                warm.len()
            );
        }
    }
}
