//! Live planes in the show (board #5397): the wiring between the depth
//! map's read-back (`env_depth.rs`), the plane finder (`live_planes.rs`)
//! and the room's boxes. The rules are plain numbers, and the worker is
//! plain threads, so all of it builds and tests on the desktop; `app.rs`
//! holds one of each and applies what they return.
//!
//! **The read-back** ([`DepthGrid`]). The self-check's shape, for the
//! finder: a compute pass loads a [`GRID`] x [`GRID`] grid of the left
//! layer ([`LAYER`]) into a buffer, recorded in the frame's own submission,
//! and the buffer is mapped a few frames later without waiting. It runs
//! [`LIVE_HZ`] times a second while on, whatever room boxes there are. The
//! grid carries the layer's pose and fov, its near and far and the time it
//! was acquired: the finder unprojects with those, not the frame's.
//!
//! **The worker** ([`Worker`]). The frame loop hands each read-back to a
//! thread through a slot that holds one job: a newer one replaces one not
//! yet taken. The thread runs `unproject_grid` and `LivePlanes::update`
//! ([`Fitter`]) and sends back a [`Snapshot`]: the reported planes,
//! `floor_y`, the `Stats` and the update's time with a running mean and
//! max. The frame loop never waits: it hands a job over only when the slot
//! is free of the thread (`try_lock`, else it keeps the job a frame), and
//! drains the results with `try_recv`.
//!
//! **When it runs** ([`runs`]). While roam is on and the depth provider
//! runs; `debug.fosfora.liveplanes 0|1` overrides, 1 also with roam off (a
//! scanned room, for measurement). Off, nothing is read back and the
//! thread sleeps. Each time it turns on, the finder starts fresh (a new
//! epoch: snapshots of an older one are dropped).
//!
//! **Into the show** ([`LiveShow`]). The reported planes become obstacle
//! boxes after the room's (`live_planes::to_obstacle_box`), in the finder's
//! order (by id), each with its kind's default behavior. A live plane that
//! lies on a scanned box ([`on_box`]: normals within [`MATCH_DEG`], the
//! plane within [`MATCH_M`] of the box's face, more than [`MATCH_OVERLAP`]
//! of the live rectangle over the face) is left out, so a scanned wall is
//! not lit twice; the stage floor counts as such a box, since in roam it
//! stands at the finder's floor height and is already that surface. The
//! room's boxes come first and live boxes take what is left of the 32
//! ([`room_for_live`], the stage floor's slot kept). A live plane fades in
//! over [`FADE_S`] from when it is first shown and out over the same time
//! when it leaves: the strength in its lane row is scaled ([`fade_rows`]),
//! which the surfaces pass and the sim's emitters already apply.
//!
//! **Never saved.** A live plane's UUID (`live_planes::plane_id`) is never
//! written to a room file and never enters the room id (`room_file.rs`),
//! the lanes' writers refuse it (`lanes.rs`), and the room editor, the
//! voice path's vocabulary ([`offered`]) and the surface labels do not
//! offer it.
//!
//! **The floor** ([`stage_floor_y`]). In roam the stage floor's slab and
//! rings sit at the finder's floor height, following it at up to
//! [`FLOOR_FOLLOW_M_S`]; seated, at y = 0.
//!
//! **Noise calibration** ([`NoiseBins`]). With
//! `debug.fosfora.liveplanesnoise 1` in a scanned room, each read-back's
//! texels are cast against the room's boxes as the self-check casts them,
//! and the depth error along the ray is binned by range for the texels
//! whose ray meets a face more than [`NOISE_OFF_GRAZING_DEG`] off grazing.
//! Every [`NOISE_EVERY_S`] one line reports each bin's count, median and
//! robust spread (1.4826 x the median absolute deviation), and the
//! finder's noise constant that spread implies.

use std::ops::Range;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, TryLockError};
use std::thread::JoinHandle;
use std::time::Instant;

use glam::{Quat, Vec2, Vec3};
use log::info;

use crate::env_depth::{DepthView, Obb, Reading, decode_distance, ray_obb, texel_dir};
use crate::intent::Surface;
use crate::lanes::LaneBox;
use crate::live_planes::{
    DepthCamera, LivePlane, LivePlanes, Observation, PlaneBox, Stats, grid_texel, plane_id,
    to_obstacle_box, unproject_grid,
};
use crate::surfaces::{
    KIND_CEILING, KIND_FLOOR, KIND_OTHER, KIND_TABLE, KIND_WALL, SURFACE_LANE_ROWS, kind_name,
};

// ---- Knobs -------------------------------------------------------------

/// Cells per side of the read-back grid (`debug.fosfora.liveplanesgrid`).
pub const GRID: usize = 64;
/// The grid knob's range.
pub const GRID_MIN: usize = 40;
pub const GRID_MAX: usize = 80;
/// Read-backs a second while on (`debug.fosfora.liveplaneshz`).
pub const LIVE_HZ: f32 = 4.0;
/// The rate knob's range.
pub const HZ_MIN: f32 = 1.0;
pub const HZ_MAX: f32 = 10.0;
/// The depth layer read back: 0, the left. One layer is half the cost of
/// two, the two see nearly the same room, and the left is the one the
/// self-check's reading was verified on first.
pub const LAYER: usize = 0;

/// `debug.fosfora.liveplanesgrid`: [`GRID`] unset or unreadable, else
/// clamped to [`GRID_MIN`]..=[`GRID_MAX`].
pub fn grid_knob(value: Option<&str>) -> usize {
    value
        .and_then(|v| v.trim().parse::<usize>().ok())
        .map_or(GRID, |n| n.clamp(GRID_MIN, GRID_MAX))
}

/// `debug.fosfora.liveplaneshz`: [`LIVE_HZ`] unset or unreadable, else
/// clamped to [`HZ_MIN`]..=[`HZ_MAX`].
pub fn hz_knob(value: Option<&str>) -> f32 {
    value
        .and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .map_or(LIVE_HZ, |v| v.clamp(HZ_MIN, HZ_MAX))
}

// ---- When it runs ------------------------------------------------------

/// Whether the finder runs: with roam on, unless `debug.fosfora.liveplanes`
/// (`knob`) says otherwise. A depth provider has to be running too; the
/// caller checks that.
pub fn runs(roam: bool, knob: Option<bool>) -> bool {
    knob.unwrap_or(roam)
}

/// Why it runs or not, for the log.
pub fn why(roam: bool, knob: Option<bool>) -> &'static str {
    match knob {
        Some(true) => "debug.fosfora.liveplanes 1",
        Some(false) => "debug.fosfora.liveplanes 0",
        None if roam => "roam on",
        None => "roam off",
    }
}

/// The line logged when it turns on.
pub fn start_line(grid: usize, hz: f32) -> String {
    format!(
        "live planes: on · grid {grid} · {hz} Hz · layer {}",
        if LAYER == 0 { "left" } else { "right" }
    )
}

// ---- A live plane on a scanned box -------------------------------------

/// A live plane and a box face it may lie on: normals within this (deg).
pub const MATCH_DEG: f32 = 15.0;
/// The live plane's center within this of the face's plane (m).
pub const MATCH_M: f32 = 0.10;
/// More than this share of the live rectangle over the face.
pub const MATCH_OVERLAP: f32 = 0.5;

/// Whether the live plane `p` lies on a face of `b`: a face whose outward
/// normal is within [`MATCH_DEG`] of the plane's, whose plane the live
/// plane's center is within [`MATCH_M`] of, and over which more than
/// [`MATCH_OVERLAP`] of the live rectangle lies (both seen along the
/// face's normal). Every face of `b` is tried, both sides of a scanned
/// plane among them.
pub fn on_box(p: &PlaneBox, b: &Obb) -> bool {
    let rot = Quat::from_array(p.rot);
    let (n, c) = (rot * Vec3::Z, Vec3::from(p.center));
    let (ux, uy) = (rot * Vec3::X * p.half[0], rot * Vec3::Y * p.half[1]);
    let corners = [c - ux - uy, c + ux - uy, c + ux + uy, c - ux + uy];
    let cos = MATCH_DEG.to_radians().cos();
    let axes = [b.rot * Vec3::X, b.rot * Vec3::Y, b.rot * Vec3::Z];
    let half = b.half.to_array();
    for k in 0..3 {
        for sign in [1.0f32, -1.0] {
            let out = axes[k] * sign;
            if n.dot(out) < cos {
                continue;
            }
            let face = b.center + out * half[k];
            if (c - face).dot(out).abs() > MATCH_M {
                continue;
            }
            let (i, j) = ((k + 1) % 3, (k + 2) % 3);
            let quad = corners.map(|q| Vec2::new((q - face).dot(axes[i]), (q - face).dot(axes[j])));
            let whole = area(&quad);
            if whole <= 0.0 {
                continue;
            }
            if clipped_area(&quad, half[i], half[j]) > MATCH_OVERLAP * whole {
                return true;
            }
        }
    }
    false
}

/// A polygon's area (either winding).
fn area(poly: &[Vec2]) -> f32 {
    let n = poly.len();
    (0..n)
        .map(|i| poly[i].perp_dot(poly[(i + 1) % n]))
        .sum::<f32>()
        .abs()
        * 0.5
}

/// The area of the convex polygon `poly` inside the rectangle
/// `[-hx, hx] x [-hy, hy]` (Sutherland-Hodgman, one edge at a time).
fn clipped_area(poly: &[Vec2], hx: f32, hy: f32) -> f32 {
    let mut cur: Vec<Vec2> = poly.to_vec();
    // Each edge: the inside test's axis (0 x, 1 y), its sign and limit.
    for (axis, sign, limit) in [(0, 1.0, hx), (0, -1.0, hx), (1, 1.0, hy), (1, -1.0, hy)] {
        if cur.is_empty() {
            break;
        }
        let inside = |v: Vec2| sign * v[axis] <= limit;
        let mut next = Vec::with_capacity(cur.len() + 2);
        for i in 0..cur.len() {
            let (a, b) = (cur[i], cur[(i + 1) % cur.len()]);
            let (ia, ib) = (inside(a), inside(b));
            if ia {
                next.push(a);
            }
            if ia != ib {
                let t = (limit - sign * a[axis]) / (sign * (b[axis] - a[axis]));
                next.push(a + (b - a) * t);
            }
        }
        cur = next;
    }
    if cur.len() < 3 { 0.0 } else { area(&cur) }
}

// ---- Into the show -----------------------------------------------------

/// The boxes live planes may take: what the room's `scanned` boxes and the
/// stage floor (`floor`, when it is in the show) leave of the obstacle
/// block's 32 (`particles3d::MAX_BOXES`, one lane row each). The stage
/// floor's slot is kept for it: it is pushed after the live boxes and
/// would otherwise be the one dropped.
pub fn room_for_live(scanned: usize, floor: bool) -> usize {
    SURFACE_LANE_ROWS.saturating_sub(scanned + usize::from(floor))
}

/// How long a live plane takes to come to full strength, and to go (s).
pub const FADE_S: f32 = 0.6;

/// A live plane in the show this frame: its id, its box and its strength
/// (0..1, the fade).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shown {
    pub id: u32,
    pub b: PlaneBox,
    pub level: f32,
}

/// What the last [`LiveShow::step`] did with the reported planes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub reported: usize,
    /// In the show, fading ones included.
    pub shown: usize,
    /// Left out: on a scanned box, on the stage floor.
    pub on_scanned: usize,
    pub on_floor: usize,
    /// Reported and free, but past the boxes left.
    pub over_budget: usize,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    id: u32,
    b: PlaneBox,
    level: f32,
    /// Reported (and not on a scanned box) this frame.
    present: bool,
}

/// The live planes in the show across frames: which are, and each one's
/// fade.
#[derive(Debug, Default)]
pub struct LiveShow {
    entries: Vec<Entry>,
    shown: Vec<Shown>,
    counts: Counts,
}

impl LiveShow {
    /// Once a frame: `planes` the finder's reported set (empty while it is
    /// off), `scanned` the room's boxes, `floor` the stage floor while it
    /// is in the show, `room` the boxes left ([`room_for_live`]). Returns
    /// the planes in the show, by id: those reported and on no scanned box
    /// fading in, those gone fading out, at most `room` of them. One past
    /// the budget waits at strength 0 and fades in once it has a box.
    pub fn step(
        &mut self,
        dt: f32,
        planes: &[LivePlane],
        scanned: &[Obb],
        floor: Option<&Obb>,
        room: usize,
    ) -> &[Shown] {
        let step = if dt.is_finite() {
            dt.max(0.0) / FADE_S
        } else {
            0.0
        };
        let mut counts = Counts {
            reported: planes.len(),
            ..Counts::default()
        };
        for e in &mut self.entries {
            e.present = false;
        }
        for p in planes {
            let b = to_obstacle_box(p);
            if scanned.iter().any(|s| on_box(&b, s)) {
                counts.on_scanned += 1;
                continue;
            }
            if floor.is_some_and(|f| on_box(&b, f)) {
                counts.on_floor += 1;
                continue;
            }
            match self.entries.iter_mut().find(|e| e.id == p.id) {
                Some(e) => {
                    e.b = b;
                    e.present = true;
                }
                None => self.entries.push(Entry {
                    id: p.id,
                    b,
                    level: 0.0,
                    present: true,
                }),
            }
        }
        self.entries.sort_unstable_by_key(|e| e.id);
        self.shown.clear();
        for e in &mut self.entries {
            if self.shown.len() < room {
                e.level = if e.present {
                    (e.level + step).min(1.0)
                } else {
                    (e.level - step).max(0.0)
                };
                if e.present || e.level > 0.0 {
                    self.shown.push(Shown {
                        id: e.id,
                        b: e.b,
                        level: e.level,
                    });
                }
            } else {
                counts.over_budget += usize::from(e.present);
                e.level = 0.0;
            }
        }
        self.entries.retain(|e| e.present || e.level > 0.0);
        counts.shown = self.shown.len();
        self.counts = counts;
        &self.shown
    }

    /// The last step's planes in the show.
    pub fn shown(&self) -> &[Shown] {
        &self.shown
    }

    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// Forget every plane at once (a fresh finder reuses ids).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.shown.clear();
        self.counts = Counts::default();
    }
}

/// The lane rows with the live planes' strengths faded: row `first + i`'s
/// strength (`y`) times `levels[i]`. The behavior, the color and the band
/// are left alone, so only how strongly a face lights and its emitter
/// spawns follow the fade.
pub fn fade_rows(
    rows: &[[f32; 4]; SURFACE_LANE_ROWS],
    first: usize,
    levels: impl IntoIterator<Item = f32>,
) -> [[f32; 4]; SURFACE_LANE_ROWS] {
    let mut out = *rows;
    for (row, level) in out.iter_mut().skip(first).zip(levels) {
        row[1] *= level.clamp(0.0, 1.0);
    }
    out
}

/// A live box's label for the lanes' log and its name
/// (`surfaces::friendly_name`): "live wall" and so on, which no anchor's
/// label is, so a scanned surface's name never changes for it.
pub fn label(kind: u32) -> &'static str {
    match kind {
        KIND_FLOOR => "live floor",
        KIND_WALL => "live wall",
        KIND_TABLE => "live table",
        KIND_CEILING => "live ceiling",
        _ => "live surface",
    }
}

/// The surfaces the voice path, its agent and the surface labels offer:
/// `intent::surfaces` without the live planes, named among the others
/// alone, each keeping its lane index.
pub fn offered(boxes: &[LaneBox<'_>]) -> Vec<Surface> {
    let keep: Vec<usize> = (0..boxes.len())
        .filter(|&k| plane_id(&boxes[k].uuid).is_none())
        .collect();
    if keep.len() == boxes.len() {
        return crate::intent::surfaces(boxes);
    }
    let kept: Vec<LaneBox<'_>> = keep.iter().map(|&k| boxes[k]).collect();
    crate::intent::surfaces(&kept)
        .into_iter()
        .map(|mut s| {
            s.index = keep[s.index];
            s
        })
        .collect()
}

/// The indices of `boxes` the room editor may not pick: the live planes,
/// which sit together after the room's boxes.
pub fn unpickable(scanned: usize, live: usize) -> Range<usize> {
    scanned..scanned + live
}

// ---- The floor ---------------------------------------------------------

/// The fastest the stage floor follows the finder's floor height (m/s).
/// The height is already filtered; this only turns its first value, which
/// may sit well off y = 0, into a glide instead of a jump under the
/// particles resting on the floor.
pub const FLOOR_FOLLOW_M_S: f32 = 0.25;

/// The stage floor's height this frame from `current`: in roam toward the
/// finder's `floor_y` (held while there is none), seated y = 0.
pub fn stage_floor_y(current: f32, roam: bool, floor_y: Option<f32>, dt: f32) -> f32 {
    if !roam {
        return 0.0;
    }
    let Some(target) = floor_y.filter(|y| y.is_finite()) else {
        return current;
    };
    let max = FLOOR_FOLLOW_M_S * if dt.is_finite() { dt.max(0.0) } else { 0.0 };
    current + (target - current).clamp(-max, max)
}

// ---- Logs --------------------------------------------------------------

/// What the once-a-second line reports.
#[derive(Debug, Clone, Copy)]
pub struct Status<'a> {
    pub planes: &'a [LivePlane],
    pub tracked: usize,
    pub floor_y: Option<f32>,
    pub mean_ms: f32,
    pub max_ms: f32,
    pub hz: f32,
    pub counts: Counts,
}

/// "live planes: 7 reported (floor 1, wall 3, table 1, other 2) · 12
/// tracked · floor y -0.02 · update 1.9 ms (max 3.4) · 4.0 Hz · 3 shown, 2
/// on scanned boxes, 0 on the stage floor, 0 over budget".
pub fn status_line(s: &Status<'_>) -> String {
    let kinds: Vec<String> = [KIND_FLOOR, KIND_WALL, KIND_TABLE, KIND_CEILING, KIND_OTHER]
        .into_iter()
        .map(|k| {
            let n = s
                .planes
                .iter()
                .filter(|p| p.kind == k || (k == KIND_OTHER && !known_kind(p.kind)))
                .count();
            (k, n)
        })
        .filter(|&(_, n)| n > 0)
        .map(|(k, n)| format!("{} {n}", kind_name(k)))
        .collect();
    let c = s.counts;
    format!(
        "live planes: {} reported{} · {} tracked · floor y {} · update {:.1} ms (max {:.1}) · {:.1} Hz · {} shown, {} on scanned boxes, {} on the stage floor, {} over budget",
        s.planes.len(),
        if kinds.is_empty() {
            String::new()
        } else {
            format!(" ({})", kinds.join(", "))
        },
        s.tracked,
        s.floor_y.map_or("-".to_owned(), |y| format!("{y:.2}")),
        s.mean_ms,
        s.max_ms,
        s.hz,
        c.shown,
        c.on_scanned,
        c.on_floor,
        c.over_budget
    )
}

fn known_kind(kind: u32) -> bool {
    matches!(kind, KIND_FLOOR | KIND_WALL | KIND_TABLE | KIND_CEILING)
}

/// How far `head` is from plane `p`'s rectangle (m).
pub fn distance(p: &LivePlane, head: Vec3) -> f32 {
    let local = p.rot.inverse() * (head - p.center);
    let on = Vec3::new(
        local.x.clamp(-p.half.x, p.half.x),
        local.y.clamp(-p.half.y, p.half.y),
        0.0,
    );
    local.distance(on)
}

/// The lines for the planes that appeared in `now` and left since
/// `before`: "live planes: + #12 wall 1.20 x 2.10 m, 1.8 m away", and "-"
/// for one that left (its last size and place).
pub fn change_lines(before: &[LivePlane], now: &[LivePlane], head: Vec3) -> Vec<String> {
    let line = |sign: char, p: &LivePlane| {
        format!(
            "live planes: {sign} #{} {} {:.2} x {:.2} m, {:.1} m away",
            p.id,
            kind_name(p.kind),
            2.0 * p.half.x,
            2.0 * p.half.y,
            distance(p, head)
        )
    };
    let appeared = now
        .iter()
        .filter(|p| !before.iter().any(|b| b.id == p.id))
        .map(|p| line('+', p));
    let left = before
        .iter()
        .filter(|b| !now.iter().any(|p| p.id == b.id))
        .map(|p| line('-', p));
    appeared.chain(left).collect()
}

// ---- Noise calibration -------------------------------------------------

/// The range bins: [`NOISE_BINS`] of [`NOISE_BIN_M`] from
/// [`NOISE_FROM_M`] (0.5 to 4.5 m).
pub const NOISE_FROM_M: f32 = 0.5;
pub const NOISE_BIN_M: f32 = 0.5;
pub const NOISE_BINS: usize = 8;
/// A texel counts when its ray meets the face more than this off grazing
/// (deg).
pub const NOISE_OFF_GRAZING_DEG: f32 = 30.0;
/// Seconds between two reports.
pub const NOISE_EVERY_S: f32 = 5.0;
/// The median absolute deviation to a normal distribution's deviation.
pub const MAD_TO_SD: f32 = 1.4826;
/// A bin needs this many samples for its noise constant to be shown.
pub const NOISE_MIN_SAMPLES: usize = 10;

/// The bin of a texel at `range` (m), `None` outside them.
pub fn noise_bin(range: f32) -> Option<usize> {
    let i = ((range - NOISE_FROM_M) / NOISE_BIN_M).floor();
    (range.is_finite() && i >= 0.0 && i < NOISE_BINS as f32).then_some(i as usize)
}

/// Where the ray `o + t d` enters `b` (`env_depth::ray_obb`, the
/// self-check's cast) and the outward normal of the face it enters.
pub fn hit_face(o: Vec3, d: Vec3, b: &Obb) -> Option<(f32, Vec3)> {
    let t = ray_obb(o, d, b)?;
    let local = b.rot.inverse() * (o + d * t - b.center);
    let half = b.half.to_array();
    let k = (0..3)
        .max_by(|&i, &j| {
            let r = |a: usize| local[a].abs() / half[a].max(1e-6);
            r(i).total_cmp(&r(j))
        })
        .unwrap_or(2);
    let mut axis = Vec3::ZERO;
    axis[k] = local[k].signum();
    Some((t, b.rot * axis))
}

/// The median and the robust spread ([`MAD_TO_SD`] x the median absolute
/// deviation) of `v`, which it reorders; `None` when empty.
pub fn robust(v: &mut [f32]) -> Option<(f32, f32)> {
    if v.is_empty() {
        return None;
    }
    let median = median(v);
    let mut dev: Vec<f32> = v.iter().map(|e| (e - median).abs()).collect();
    Some((median, MAD_TO_SD * self::median(&mut dev)))
}

/// The median of `v` (reordered; the mean of the middle two when even).
fn median(v: &mut [f32]) -> f32 {
    let n = v.len();
    let (_, &mut mid, _) = v.select_nth_unstable_by(n / 2, f32::total_cmp);
    if n % 2 == 1 {
        mid
    } else {
        let below = v[..n / 2].iter().copied().fold(f32::NEG_INFINITY, f32::max);
        0.5 * (below + mid)
    }
}

/// The depth error along the ray, by range, across the read-backs of one
/// report.
#[derive(Debug, Default)]
pub struct NoiseBins {
    bins: [Vec<f32>; NOISE_BINS],
    reads: u32,
    since: Option<f32>,
}

impl NoiseBins {
    /// One sample: a texel at true `range` (m) whose map distance along
    /// the ray is `error` off (m).
    pub fn add(&mut self, range: f32, error: f32) {
        if let Some(i) = noise_bin(range)
            && error.is_finite()
        {
            self.bins[i].push(error);
        }
    }

    /// One read-back against the room's `boxes` (hidden walls left out by
    /// the caller), read as `reading`: each texel with depth whose nearest
    /// box face its ray meets more than [`NOISE_OFF_GRAZING_DEG`] off
    /// grazing. The range is the box's along the ray, the error the map's
    /// minus it along the ray. Returns the samples taken.
    pub fn add_grid(&mut self, grid: &DepthGrid, reading: Reading, boxes: &[Obb]) -> usize {
        self.reads += 1;
        let n = grid.n;
        if boxes.is_empty() || grid.depth.len() < n * n {
            return 0;
        }
        let [x, y, z, w] = grid.view.orientation;
        let rot = Quat::from_xyzw(x, y, z, w);
        let origin = Vec3::from(grid.view.position);
        let min_cos = NOISE_OFF_GRAZING_DEG.to_radians().sin();
        let mut taken = 0;
        for gy in 0..n {
            for gx in 0..n {
                let d = grid.depth[gy * n + gx];
                if !(0.0..1.0).contains(&d) {
                    continue;
                }
                let tx = grid_texel(gx, n, grid.size[0]);
                let ty = grid_texel(gy, n, grid.size[1]);
                // z = -1 in the camera: t is the depth along -Z.
                let dir = rot * texel_dir(grid.view.fov, grid.size, tx, ty, reading);
                let Some((t, normal)) = boxes
                    .iter()
                    .filter_map(|b| hit_face(origin, dir, b))
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                else {
                    continue;
                };
                let len = dir.length();
                if (dir / len).dot(normal).abs() <= min_cos {
                    continue;
                }
                let map = decode_distance(d, grid.near, grid.far);
                if !map.is_finite() {
                    continue;
                }
                self.add(t * len, (map - t) * len);
                taken += 1;
            }
        }
        taken
    }

    /// Whether a report is due at `now_s`: [`NOISE_EVERY_S`] after the
    /// first call, then after the last report.
    pub fn due(&mut self, now_s: f32) -> bool {
        match self.since {
            None => {
                self.since = Some(now_s);
                false
            }
            Some(s) if now_s - s >= NOISE_EVERY_S || now_s < s => {
                self.since = Some(now_s);
                true
            }
            Some(_) => false,
        }
    }

    /// The report: per bin its count, median and spread (cm), and the
    /// noise constant the spread implies at the bin's middle range
    /// (`live_planes::NOISE_K`: one deviation is k times the squared
    /// range), then cleared for the next.
    pub fn report(&mut self) -> String {
        let cells: Vec<String> = self
            .bins
            .iter_mut()
            .enumerate()
            .map(|(i, errors)| {
                let from = NOISE_FROM_M + NOISE_BIN_M * i as f32;
                let n = errors.len();
                match robust(errors) {
                    Some((median, spread)) => {
                        let mid = from + 0.5 * NOISE_BIN_M;
                        let k = if n >= NOISE_MIN_SAMPLES {
                            format!(" k {:.4}", spread / (mid * mid))
                        } else {
                            String::new()
                        };
                        format!(
                            "{from:.1} n {n} med {:+.2} sd {:.2}{k}",
                            100.0 * median,
                            100.0 * spread
                        )
                    }
                    None => format!("{from:.1} n 0"),
                }
            })
            .collect();
        let line = format!(
            "live planes noise: {} reads, error along the ray (cm) by range (m), over {NOISE_OFF_GRAZING_DEG}° off grazing: {}",
            self.reads,
            cells.join(" · ")
        );
        for b in &mut self.bins {
            b.clear();
        }
        self.reads = 0;
        line
    }

    /// Samples held in bin `i`.
    pub fn count(&self, i: usize) -> usize {
        self.bins.get(i).map_or(0, Vec::len)
    }
}

// ---- The worker --------------------------------------------------------

/// One read-back of the depth map's layer [`LAYER`]: `n * n` stored values
/// in the self-check's layout (`depth[gy * n + gx]` the value at texel
/// (`grid_texel(gx)`, `grid_texel(gy)`)) of a `size` map, with the pose and
/// fov the image was acquired with, its near and far, and when.
#[derive(Debug, Clone)]
pub struct DepthGrid {
    pub n: usize,
    pub depth: Vec<f32>,
    pub size: [u32; 2],
    pub view: DepthView,
    pub near: f32,
    pub far: f32,
    pub acquired: Instant,
}

/// What the worker does with one read-back.
#[derive(Debug, Clone)]
pub struct Job {
    /// The finder's run: a new one starts it fresh.
    pub epoch: u64,
    /// The acquire's time on the app's clock (s).
    pub time_s: f32,
    pub grid: DepthGrid,
    pub reading: Reading,
    /// Run the finder (live planes on).
    pub fit: bool,
    /// The room's boxes for the noise calibration, when it is on.
    pub noise: Option<Vec<Obb>>,
}

/// What one update of the finder reported.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub epoch: u64,
    pub planes: Vec<LivePlane>,
    pub floor_y: Option<f32>,
    pub stats: Stats,
    /// This update's time, and the mean and max of the last [`TIMES`] (ms).
    pub update_ms: f32,
    pub mean_ms: f32,
    pub max_ms: f32,
    /// Updates in this epoch so far, this one included.
    pub updates: u64,
}

/// Updates the running mean and max cover.
pub const TIMES: usize = 16;

/// The worker's state: the finder of the current epoch, its buffers, its
/// timings and the noise bins. Plain, so the desktop tests drive it
/// without a thread.
#[derive(Debug, Default)]
pub struct Fitter {
    epoch: Option<u64>,
    planes: LivePlanes,
    points: Vec<Option<Vec3>>,
    times: [f32; TIMES],
    updates: u64,
    noise: NoiseBins,
}

impl Fitter {
    /// Run `job`: a new epoch first starts the finder (and the noise bins)
    /// fresh; the noise bins take the read-back when it carries boxes, and
    /// log their report when due; with `fit`, the finder takes it and the
    /// snapshot is returned.
    pub fn run(&mut self, job: &Job) -> Option<Snapshot> {
        if self.epoch != Some(job.epoch) {
            *self = Self {
                epoch: Some(job.epoch),
                points: std::mem::take(&mut self.points),
                ..Self::default()
            };
        }
        if let Some(boxes) = &job.noise {
            self.noise.add_grid(&job.grid, job.reading, boxes);
            if self.noise.due(job.time_s) {
                info!("{}", self.noise.report());
            }
        }
        if !job.fit {
            return None;
        }
        let g = &job.grid;
        unproject_grid(
            &g.depth,
            g.n,
            g.n,
            g.size,
            &g.view,
            g.near,
            g.far,
            job.reading,
            &mut self.points,
        );
        let obs = Observation {
            camera: DepthCamera::from_view(&g.view),
            cols: g.n,
            rows: g.n,
            points: &self.points,
        };
        let start = Instant::now();
        let stats = self.planes.update(job.time_s, &obs);
        let ms = start.elapsed().as_secs_f32() * 1000.0;
        self.times[(self.updates % TIMES as u64) as usize] = ms;
        self.updates += 1;
        let held = &self.times[..(self.updates as usize).min(TIMES)];
        Some(Snapshot {
            epoch: job.epoch,
            planes: self.planes.planes().to_vec(),
            floor_y: self.planes.floor_y(),
            stats,
            update_ms: ms,
            mean_ms: held.iter().sum::<f32>() / held.len() as f32,
            max_ms: held.iter().copied().fold(0.0, f32::max),
            updates: self.updates,
        })
    }
}

/// The slot the frame loop and the thread share: one job at most.
#[derive(Debug, Default)]
struct Slot {
    job: Option<Job>,
    /// Jobs replaced before the thread took them.
    replaced: u64,
    quit: bool,
}

impl Slot {
    /// Put `job` in, replacing one not yet taken.
    fn put(&mut self, job: Job) {
        if self.job.replace(job).is_some() {
            self.replaced += 1;
        }
    }
}

#[derive(Debug, Default)]
struct Shared {
    slot: Mutex<Slot>,
    wake: Condvar,
}

/// The finder's thread, and the frame loop's end of it.
#[derive(Debug)]
pub struct Worker {
    shared: Arc<Shared>,
    results: Receiver<Snapshot>,
    /// A job the slot was busy for, handed over on the next call.
    waiting: Option<Job>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    /// Start the thread.
    pub fn spawn() -> std::io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let (tx, results) = channel();
        let inner = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("fosfora-live-planes".to_owned())
            .spawn(move || work(&inner, &tx))?;
        Ok(Self {
            shared,
            results,
            waiting: None,
            thread: Some(thread),
        })
    }

    /// Hand `job` over, replacing any the thread has not taken. Never
    /// waits: while the thread holds the slot the job is kept and handed
    /// over on the next call.
    pub fn submit(&mut self, job: Job) {
        self.waiting = Some(job);
        self.hand_over();
    }

    fn hand_over(&mut self) {
        let Some(job) = self.waiting.take() else {
            return;
        };
        match self.shared.slot.try_lock() {
            Ok(mut slot) => {
                slot.put(job);
                drop(slot);
                self.shared.wake.notify_one();
            }
            Err(TryLockError::WouldBlock) => self.waiting = Some(job),
            // The thread panicked: nothing takes jobs any more.
            Err(TryLockError::Poisoned(_)) => {}
        }
    }

    /// The newest snapshot since the last call, if any; hands over a job
    /// kept from before. Never waits.
    pub fn poll(&mut self) -> Option<Snapshot> {
        self.hand_over();
        let mut last = None;
        while let Ok(s) = self.results.try_recv() {
            last = Some(s);
        }
        last
    }

    /// Jobs replaced before the thread took them, when the slot is free to
    /// read.
    pub fn replaced(&self) -> Option<u64> {
        self.shared.slot.try_lock().ok().map(|s| s.replaced)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.shared.slot.lock() {
            slot.quit = true;
        }
        self.shared.wake.notify_one();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The thread: take a job when there is one, run it, send the snapshot.
fn work(shared: &Shared, tx: &Sender<Snapshot>) {
    let mut fitter = Fitter::default();
    loop {
        let job = {
            let Ok(mut slot) = shared.slot.lock() else {
                return;
            };
            loop {
                if slot.quit {
                    return;
                }
                if let Some(job) = slot.job.take() {
                    break job;
                }
                slot = match shared.wake.wait(slot) {
                    Ok(s) => s,
                    Err(_) => return,
                };
            }
        };
        if let Some(snapshot) = fitter.run(&job)
            && tx.send(snapshot).is_err()
        {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::env_depth::is_infinite;
    use crate::live_planes::plane_uuid;
    use crate::math::Fov;
    use crate::surfaces::PLANE_HALF_THICKNESS_M;

    /// A live plane as the finder reports one: center, rotation (local Z
    /// the normal), half extents.
    fn plane(id: u32, kind: u32, center: Vec3, rot: Quat, half: [f32; 2]) -> LivePlane {
        LivePlane {
            id,
            kind,
            center,
            rot,
            half: Vec3::new(half[0], half[1], PLANE_HALF_THICKNESS_M),
            confidence: 1.0,
            last_seen_s: 0.0,
        }
    }

    /// A wall plane facing +z (into the room) at z = `z`.
    fn wall_at(id: u32, x: f32, z: f32, half: [f32; 2]) -> LivePlane {
        plane(id, KIND_WALL, Vec3::new(x, 1.3, z), Quat::IDENTITY, half)
    }

    /// The scanned wall: 4 m wide, 2.6 m high at z = -2, a scene plane
    /// (2 cm half thickness), +Z into the room.
    fn scanned_wall() -> Obb {
        Obb {
            center: Vec3::new(0.0, 1.3, -2.0),
            rot: Quat::IDENTITY,
            half: Vec3::new(2.0, 1.3, PLANE_HALF_THICKNESS_M),
        }
    }

    fn boxed(p: &LivePlane) -> PlaneBox {
        to_obstacle_box(p)
    }

    #[test]
    fn a_live_plane_on_a_scanned_wall_is_left_out_and_one_beside_it_is_kept() {
        let wall = scanned_wall();
        // On the wall's room face (1 cm off it), wholly over it.
        assert!(on_box(&boxed(&wall_at(1, 0.0, -1.97, [0.8, 1.0])), &wall));
        // The other side of the plane counts too: a scene plane's two
        // faces are 4 cm apart.
        let behind = plane(
            2,
            KIND_WALL,
            Vec3::new(0.0, 1.3, -2.03),
            Quat::from_rotation_y(std::f32::consts::PI),
            [0.8, 1.0],
        );
        assert!(on_box(&boxed(&behind), &wall));
        // Beside the wall (past its end): kept.
        assert!(!on_box(&boxed(&wall_at(3, 3.5, -1.98, [0.8, 1.0])), &wall));
        // In front of it by 30 cm (a cabinet front): kept.
        assert!(!on_box(&boxed(&wall_at(4, 0.0, -1.68, [0.5, 0.5])), &wall));
    }

    #[test]
    fn the_overlap_test_holds_its_angle_offset_and_area_limits() {
        let wall = scanned_wall();
        let tilted = |deg: f32| {
            plane(
                1,
                KIND_WALL,
                Vec3::new(0.0, 1.3, -1.98),
                Quat::from_rotation_y(deg.to_radians()),
                [0.5, 0.5],
            )
        };
        assert!(on_box(&boxed(&tilted(14.0)), &wall), "14 degrees off");
        assert!(!on_box(&boxed(&tilted(16.0)), &wall), "16 degrees off");
        // The plane's center within 10 cm of the face (at z = -1.98).
        assert!(on_box(&boxed(&wall_at(1, 0.0, -1.89, [0.5, 0.5])), &wall));
        assert!(!on_box(&boxed(&wall_at(1, 0.0, -1.87, [0.5, 0.5])), &wall));
        // Over the wall's end at x = 2: 1 m wide, 60 % over, then 40 %.
        assert!(on_box(&boxed(&wall_at(1, 1.9, -1.98, [0.5, 0.5])), &wall));
        assert!(!on_box(&boxed(&wall_at(1, 2.1, -1.98, [0.5, 0.5])), &wall));
        // A table top on a scanned desk volume's top face, turned 30
        // degrees about the vertical: 0.4 x 0.3 m over a 1.2 x 0.6 m top.
        let desk = Obb {
            center: Vec3::new(1.0, 0.375, -0.8),
            rot: Quat::from_rotation_y(0.3),
            half: Vec3::new(0.6, 0.375, 0.3),
        };
        let up = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        let top = plane(
            9,
            KIND_TABLE,
            Vec3::new(1.0, 0.76, -0.8),
            Quat::from_rotation_y(0.8) * up,
            [0.2, 0.15],
        );
        assert!(on_box(&boxed(&top), &desk));
        // The same top 20 cm above the desk: kept.
        let shelf = LivePlane {
            center: Vec3::new(1.0, 0.95, -0.8),
            ..top
        };
        assert!(!on_box(&boxed(&shelf), &desk));
    }

    #[test]
    fn the_clipped_area_is_the_overlap_of_a_turned_square() {
        // A unit square turned 45 degrees about the center of a unit
        // square: the overlap is 2 (sqrt 2 - 1) of each.
        let r = std::f32::consts::FRAC_1_SQRT_2;
        let diamond = [
            Vec2::new(0.0, -r),
            Vec2::new(r, 0.0),
            Vec2::new(0.0, r),
            Vec2::new(-r, 0.0),
        ];
        assert!((area(&diamond) - 1.0).abs() < 1e-5);
        let overlap = clipped_area(&diamond, 0.5, 0.5);
        assert!(
            (overlap - 2.0 * (2f32.sqrt() - 1.0)).abs() < 1e-5,
            "{overlap}"
        );
        // Disjoint: nothing.
        let far = diamond.map(|v| v + Vec2::new(3.0, 0.0));
        assert!(clipped_area(&far, 0.5, 0.5).abs() < 1e-6);
    }

    #[test]
    fn live_boxes_take_what_the_room_and_the_stage_floor_leave() {
        assert_eq!(room_for_live(0, false), 32);
        assert_eq!(room_for_live(0, true), 31);
        assert_eq!(room_for_live(17, true), 14);
        assert_eq!(room_for_live(31, true), 0);
        assert_eq!(room_for_live(40, true), 0);
    }

    /// `n` frames of `dt` with `planes` reported.
    fn run(show: &mut LiveShow, planes: &[LivePlane], n: usize, room: usize) -> Vec<Shown> {
        let mut out = Vec::new();
        for _ in 0..n {
            out = show.step(1.0 / 72.0, planes, &[], None, room).to_vec();
        }
        out
    }

    #[test]
    fn a_live_plane_fades_in_over_0_6_s_and_out_when_it_leaves() {
        let mut show = LiveShow::default();
        let p = [wall_at(5, 0.0, -2.0, [1.0, 1.0])];
        let first = run(&mut show, &p, 1, 32);
        assert_eq!(first.len(), 1);
        assert!(first[0].level < 0.03, "{}", first[0].level);
        // Half way at 0.3 s, full at 0.6 s and after.
        let half = run(&mut show, &p, 21, 32);
        assert!((half[0].level - 0.5).abs() < 0.03, "{}", half[0].level);
        let full = run(&mut show, &p, 22, 32);
        assert_close!(full[0].level, 1.0);
        assert_eq!(full[0].b.uuid, plane_uuid(5));
        // Gone: it fades out over the same time with its last box, then
        // leaves the show.
        let going = run(&mut show, &[], 22, 32);
        assert_eq!(going.len(), 1);
        assert!((going[0].level - 0.5).abs() < 0.03, "{}", going[0].level);
        assert_eq!(going[0].b, full[0].b);
        assert!(run(&mut show, &[], 22, 32).is_empty());
        assert_eq!(show.counts().shown, 0);
    }

    #[test]
    fn past_the_budget_a_plane_waits_at_zero_and_fades_in_when_a_box_frees() {
        let mut show = LiveShow::default();
        let a = wall_at(1, 0.0, -2.0, [1.0, 1.0]);
        let b = wall_at(2, 3.0, -2.0, [1.0, 1.0]);
        let shown = run(&mut show, &[a, b], 72, 1);
        assert_eq!(shown.iter().map(|s| s.id).collect::<Vec<_>>(), vec![1]);
        assert_eq!(show.counts().over_budget, 1);
        // The first leaves: it fades out in its box, then the second
        // comes in from nothing.
        let shown = run(&mut show, &[b], 1, 1);
        assert_eq!(shown[0].id, 1);
        let shown = run(&mut show, &[b], 44, 1);
        assert_eq!(shown[0].id, 2);
        assert!(shown[0].level < 0.1, "{}", shown[0].level);
        assert_eq!(show.counts().over_budget, 0);
    }

    #[test]
    fn planes_on_scanned_boxes_and_the_stage_floor_are_counted_and_left_out() {
        let mut show = LiveShow::default();
        let floor = Obb {
            center: Vec3::new(0.0, -0.05, 0.0),
            rot: Quat::IDENTITY,
            half: Vec3::new(10.0, 0.05, 10.0),
        };
        let up = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        let live_floor = plane(3, KIND_FLOOR, Vec3::new(0.0, 0.02, 0.5), up, [1.5, 1.0]);
        let on = wall_at(1, 0.0, -1.98, [0.8, 1.0]);
        let off = wall_at(2, 3.5, -1.98, [0.8, 1.0]);
        let shown = show
            .step(
                0.1,
                &[on, off, live_floor],
                &[scanned_wall()],
                Some(&floor),
                30,
            )
            .to_vec();
        assert_eq!(shown.iter().map(|s| s.id).collect::<Vec<_>>(), vec![2]);
        assert_eq!(
            show.counts(),
            Counts {
                reported: 3,
                shown: 1,
                on_scanned: 1,
                on_floor: 1,
                over_budget: 0
            }
        );
    }

    #[test]
    fn fading_scales_only_the_live_rows_strength() {
        let mut rows = [[0.0f32; 4]; SURFACE_LANE_ROWS];
        for (k, r) in rows.iter_mut().enumerate() {
            *r = [k as f32 + 1.0, 0.8, 3.0, 2.0];
        }
        let faded = fade_rows(&rows, 5, [0.5, 0.0, 1.0]);
        for (k, r) in faded.iter().enumerate() {
            let strength = match k {
                5 => 0.4,
                6 => 0.0,
                _ => 0.8,
            };
            assert_close!(*r, [k as f32 + 1.0, strength, 3.0, 2.0], "row {k}");
        }
        // No live planes: the rows as they are.
        assert_close!(fade_rows(&rows, 7, []), rows);
    }

    #[test]
    fn the_stage_floor_follows_the_finder_in_roam_only() {
        // Seated: y = 0 whatever the finder says.
        assert_close!(stage_floor_y(-0.3, false, Some(-0.4), 0.1), 0.0);
        // In roam without a floor yet: where it is.
        assert_close!(stage_floor_y(-0.3, true, None, 0.1), -0.3);
        // A small step lands at once; a large one glides.
        assert_close!(stage_floor_y(0.0, true, Some(-0.02), 0.1), -0.02);
        let mut y = 0.0;
        for _ in 0..10 {
            y = stage_floor_y(y, true, Some(-1.0), 0.1);
        }
        assert!((y + 0.25).abs() < 1e-5, "{y}");
    }

    fn lane(uuid: [u8; 16], kind: u32, label: &str) -> LaneBox<'_> {
        LaneBox { uuid, kind, label }
    }

    #[test]
    fn the_vocabulary_offers_no_live_plane_and_keeps_the_lane_indices() {
        use crate::room_file::STAGE_FLOOR_UUID;
        let scanned = |n: u8| [n; 16];
        let boxes = [
            lane(scanned(1), KIND_WALL, "WALL_FACE"),
            lane(scanned(2), KIND_TABLE, "TABLE"),
            lane(plane_uuid(7), KIND_WALL, label(KIND_WALL)),
            lane(plane_uuid(9), KIND_WALL, label(KIND_WALL)),
            lane(STAGE_FLOOR_UUID, KIND_FLOOR, ""),
        ];
        let list = offered(&boxes);
        let named: Vec<(String, usize)> = list.iter().map(|s| (s.name.clone(), s.index)).collect();
        assert_eq!(
            named,
            vec![
                ("wall".to_owned(), 0),
                ("table".to_owned(), 1),
                ("floor".to_owned(), 4)
            ]
        );
        // Without live planes it is the grammar's own list.
        let room = [boxes[0], boxes[1], boxes[4]];
        let plain: Vec<(String, usize)> = crate::intent::surfaces(&room)
            .into_iter()
            .map(|s| (s.name, s.index))
            .collect();
        let same: Vec<(String, usize)> = offered(&room)
            .into_iter()
            .map(|s| (s.name, s.index))
            .collect();
        assert_eq!(plain, same);
        // A live box's name never clashes with a scanned one's.
        assert_eq!(crate::surfaces::friendly_name(0, &boxes), "wall");
        assert_eq!(crate::surfaces::friendly_name(2, &boxes), "live wall 2");
        assert_eq!(unpickable(2, 2), 2..4);
    }

    #[test]
    fn the_knobs_parse_and_clamp() {
        assert_eq!(grid_knob(None), 64);
        assert_eq!(grid_knob(Some("48")), 48);
        assert_eq!(grid_knob(Some("20")), 40);
        assert_eq!(grid_knob(Some("200")), 80);
        assert_eq!(grid_knob(Some("x")), 64);
        assert_close!(hz_knob(None), 4.0);
        assert_close!(hz_knob(Some("2.5")), 2.5);
        assert_close!(hz_knob(Some("0")), 1.0);
        assert_close!(hz_knob(Some("30")), 10.0);
        assert!(runs(true, None));
        assert!(!runs(false, None));
        assert!(runs(false, Some(true)));
        assert!(!runs(true, Some(false)));
        assert_eq!(why(true, None), "roam on");
        assert_eq!(
            start_line(64, 4.0),
            "live planes: on · grid 64 · 4 Hz · layer left"
        );
    }

    #[test]
    fn the_status_line_reads_as_the_brief_writes_it() {
        let up = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        let planes = [
            plane(1, KIND_FLOOR, Vec3::ZERO, up, [1.0, 1.0]),
            wall_at(2, 0.0, -2.0, [1.0, 1.0]),
            wall_at(3, 1.0, -2.0, [1.0, 1.0]),
            plane(4, KIND_OTHER, Vec3::ZERO, Quat::IDENTITY, [0.3, 0.3]),
        ];
        let line = status_line(&Status {
            planes: &planes,
            tracked: 12,
            floor_y: Some(-0.02),
            mean_ms: 1.94,
            max_ms: 3.4,
            hz: 4.0,
            counts: Counts {
                reported: 4,
                shown: 3,
                on_scanned: 1,
                on_floor: 0,
                over_budget: 0,
            },
        });
        assert_eq!(
            line,
            "live planes: 4 reported (floor 1, wall 2, other 1) · 12 tracked · floor y -0.02 · update 1.9 ms (max 3.4) · 4.0 Hz · 3 shown, 1 on scanned boxes, 0 on the stage floor, 0 over budget"
        );
        let none = status_line(&Status {
            planes: &[],
            tracked: 0,
            floor_y: None,
            mean_ms: 0.0,
            max_ms: 0.0,
            hz: 0.0,
            counts: Counts::default(),
        });
        assert!(
            none.starts_with("live planes: 0 reported · 0 tracked · floor y - ·"),
            "{none}"
        );
    }

    #[test]
    fn the_change_lines_name_what_appeared_and_what_left() {
        let a = wall_at(4, 0.0, -2.0, [0.6, 1.05]);
        let b = wall_at(5, 3.0, -2.0, [0.5, 0.5]);
        let head = Vec3::new(0.0, 1.3, 0.0);
        assert_close!(distance(&a, head), 2.0);
        assert_close!(distance(&a, Vec3::new(2.6, 1.3, -2.0)), 2.0);
        let lines = change_lines(&[a], &[b], head);
        assert_eq!(
            lines,
            vec![
                "live planes: + #5 wall 1.00 x 1.00 m, 3.2 m away".to_owned(),
                "live planes: - #4 wall 1.20 x 2.10 m, 2.0 m away".to_owned(),
            ]
        );
        assert!(change_lines(&[a], &[a], head).is_empty());
    }

    // ---- Noise -----------------------------------------------------------

    #[test]
    fn the_noise_bins_run_from_half_a_meter_to_four_and_a_half() {
        assert_eq!(noise_bin(0.49), None);
        assert_eq!(noise_bin(0.5), Some(0));
        assert_eq!(noise_bin(1.74), Some(2));
        assert_eq!(noise_bin(4.49), Some(7));
        assert_eq!(noise_bin(4.5), None);
        assert_eq!(noise_bin(f32::NAN), None);
    }

    #[test]
    fn the_robust_spread_shrugs_off_outliers() {
        // splitmix64 and Box-Muller, as live_planes' tests draw noise.
        let mut s = 7u64;
        let mut uniform = || {
            s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
        };
        // One sample in 20 an outlier half a meter off, either way.
        let mut v: Vec<f32> = (0..20_000)
            .map(|i| {
                if i % 20 == 0 {
                    if i % 40 == 0 { 0.5 } else { -0.5 }
                } else {
                    let (a, b) = (uniform().max(1e-7), uniform());
                    0.003 + 0.02 * (-2.0 * a.ln()).sqrt() * (std::f32::consts::TAU * b).cos()
                }
            })
            .collect();
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        let sd = (v.iter().map(|e| (e - mean).powi(2)).sum::<f32>() / v.len() as f32).sqrt();
        assert!(sd > 0.1, "the plain deviation {sd}");
        let (median, spread) = robust(&mut v).expect("samples");
        assert!((median - 0.003).abs() < 0.002, "median {median}");
        assert!((spread - 0.02).abs() < 0.002, "spread {spread}");
        assert!(robust(&mut []).is_none());
        let mut even = [4.0, 1.0, 3.0, 2.0];
        assert_close!(median_of(&mut even), 2.5);
    }

    fn median_of(v: &mut [f32]) -> f32 {
        median(v)
    }

    #[test]
    fn hit_face_casts_as_the_self_check_and_names_the_face() {
        let wall = scanned_wall();
        let o = Vec3::new(0.3, 1.5, 0.0);
        let d = Vec3::new(0.1, -0.05, -1.0);
        let (t, n) = hit_face(o, d, &wall).expect("the wall");
        assert_close!(t, ray_obb(o, d, &wall).expect("hit"));
        assert_close!(n.to_array(), [0.0, 0.0, 1.0]);
        // A turned desk, hit from above: its top.
        let desk = Obb {
            center: Vec3::new(1.0, 0.375, -0.8),
            rot: Quat::from_rotation_y(0.3),
            half: Vec3::new(0.6, 0.375, 0.3),
        };
        let (_, n) = hit_face(Vec3::new(1.0, 1.6, -0.8), Vec3::NEG_Y, &desk).expect("top");
        assert!(n.dot(Vec3::Y) > 0.999, "{n}");
    }

    /// The test lens, about the depth camera's: 100 degrees wide.
    const FOV: Fov = Fov {
        left: -0.873,
        right: 0.873,
        up: 0.785,
        down: -0.873,
    };
    const SIZE: [u32; 2] = [320, 320];
    const NEAR: f32 = 0.1;

    fn view(position: Vec3, pitch_deg: f32, fov: Fov) -> DepthView {
        DepthView {
            orientation: Quat::from_rotation_x(pitch_deg.to_radians()).to_array(),
            position: position.to_array(),
            fov,
        }
    }

    /// The stored values the read-back would hold for `scene` (the depth
    /// along -Z of the nearest hit along each texel's ray, or none), in
    /// the self-check's layout, read bottom-up as on the Quest 3, encoded
    /// with an infinite far plane; `error` adds that much along the ray.
    fn grid_of(
        scene: impl Fn(Vec3, Vec3) -> Option<f32>,
        view: &DepthView,
        n: usize,
        error: f32,
    ) -> DepthGrid {
        let [x, y, z, w] = view.orientation;
        let rot = Quat::from_xyzw(x, y, z, w);
        let origin = Vec3::from(view.position);
        let mut depth = Vec::with_capacity(n * n);
        for gy in 0..n {
            for gx in 0..n {
                let (tx, ty) = (grid_texel(gx, n, SIZE[0]), grid_texel(gy, n, SIZE[1]));
                let dir = rot * texel_dir(view.fov, SIZE, tx, ty, Reading::RowBottom);
                depth.push(scene(origin, dir).map_or(1.0, |t| {
                    let t = t + error / dir.length();
                    1.0 - NEAR / t
                }));
            }
        }
        assert!(is_infinite(NEAR, f32::INFINITY));
        DepthGrid {
            n,
            depth,
            size: SIZE,
            view: *view,
            near: NEAR,
            far: f32::INFINITY,
            acquired: Instant::now(),
        }
    }

    /// A room's floor (y = 0) and front wall (z = -2.5): the depth along
    /// `d` (z = -1 in the camera) from `o`.
    fn floor_and_wall(o: Vec3, d: Vec3) -> Option<f32> {
        let floor = (d.y < -1e-6).then(|| -o.y / d.y).filter(|&t| {
            let p = o + d * t;
            t > 0.0 && p.x.abs() < 3.0 && p.z > -2.5 && p.z < 2.0
        });
        let wall = (d.z < -1e-6).then(|| (-2.5 - o.z) / d.z).filter(|&t| {
            let p = o + d * t;
            t > 0.0 && p.x.abs() < 3.0 && p.y > 0.0 && p.y < 2.6
        });
        floor.into_iter().chain(wall).min_by(f32::total_cmp)
    }

    #[test]
    fn the_noise_cast_recovers_the_error_along_the_ray_off_grazing_only() {
        // Head-on to a wall 2 m away through a narrow lens: every ray is
        // under 30 degrees from the normal, so every texel counts.
        let narrow = Fov {
            left: -0.4,
            right: 0.4,
            up: 0.4,
            down: -0.4,
        };
        let v = view(Vec3::new(0.0, 1.3, 0.0), 0.0, narrow);
        let wall = Obb {
            center: Vec3::new(0.0, 1.3, -2.02),
            rot: Quat::IDENTITY,
            half: Vec3::new(3.0, 3.0, 0.02),
        };
        let plane_hit = |o: Vec3, d: Vec3| ray_obb(o, d, &wall);
        let grid = grid_of(plane_hit, &v, 40, 0.012);
        let mut bins = NoiseBins::default();
        assert_eq!(bins.add_grid(&grid, Reading::RowBottom, &[wall]), 40 * 40);
        // 2.0 to 2.33 m along the rays: bins 3 and 4.
        assert_eq!(bins.count(3) + bins.count(4), 40 * 40);
        let line = bins.report();
        assert!(line.contains("2.0 n "), "{line}");
        assert!(line.contains("med +1.20 sd 0.00"), "{line}");
        assert!(line.starts_with("live planes noise: 1 reads"), "{line}");
        // Reported, then cleared.
        assert_eq!(bins.count(3), 0);

        // A floor seen from 1.5 m, looking level: a ray meets it under 30
        // degrees off grazing past 3 m of range, and those are left out.
        let v = view(Vec3::new(0.0, 1.5, 0.0), -10.0, FOV);
        let floor = Obb {
            center: Vec3::new(0.0, -0.05, 0.0),
            rot: Quat::IDENTITY,
            half: Vec3::new(20.0, 0.05, 20.0),
        };
        let grid = grid_of(|o, d| ray_obb(o, d, &floor), &v, 40, 0.0);
        let mut bins = NoiseBins::default();
        assert!(bins.add_grid(&grid, Reading::RowBottom, &[floor]) > 100);
        for i in 0..NOISE_BINS {
            if NOISE_FROM_M + NOISE_BIN_M * i as f32 >= 3.0 {
                assert_eq!(bins.count(i), 0, "bin {i}");
            }
        }
    }

    #[test]
    fn a_noise_report_is_due_every_five_seconds() {
        let mut bins = NoiseBins::default();
        assert!(!bins.due(10.0));
        assert!(!bins.due(14.9));
        assert!(bins.due(15.0));
        assert!(!bins.due(19.0));
        assert!(bins.due(20.1));
    }

    // ---- The worker ------------------------------------------------------

    fn job(epoch: u64, time_s: f32, grid: DepthGrid) -> Job {
        Job {
            epoch,
            time_s,
            grid,
            reading: Reading::RowBottom,
            fit: true,
            noise: None,
        }
    }

    #[test]
    fn the_fitter_finds_the_floor_and_the_wall_from_the_read_back_and_starts_fresh() {
        // Standing 1.6 m up, turned 30 degrees to the left and looking
        // down: the read-back carries this pose and the planes land in the
        // world, not in the camera.
        let mut v = view(Vec3::new(0.4, 1.6, 0.3), -25.0, FOV);
        let turn = Quat::from_rotation_y(30f32.to_radians()) * Quat::from_array(v.orientation);
        v.orientation = turn.to_array();
        let grid = grid_of(floor_and_wall, &v, GRID, 0.0);
        let mut fitter = Fitter::default();
        let mut last = None;
        for i in 0..8 {
            last = fitter.run(&job(1, i as f32 * 0.25, grid.clone()));
        }
        let s = last.expect("a snapshot");
        assert_eq!(s.epoch, 1);
        assert_eq!(s.updates, 8);
        let floor = s
            .planes
            .iter()
            .find(|p| p.kind == KIND_FLOOR)
            .unwrap_or_else(|| panic!("no floor in {:?}", s.planes));
        assert!(floor.normal().dot(Vec3::Y) > 0.99, "{:?}", floor);
        assert!(floor.center.y.abs() < 0.03, "{:?}", floor);
        let wall = s
            .planes
            .iter()
            .find(|p| p.kind == KIND_WALL)
            .unwrap_or_else(|| panic!("no wall in {:?}", s.planes));
        assert!(wall.normal().dot(Vec3::Z) > 0.99, "{:?}", wall);
        assert!((wall.center.z + 2.5).abs() < 0.05, "{:?}", wall);
        assert!(s.floor_y.is_some_and(|y| y.abs() < 0.03), "{:?}", s.floor_y);
        assert!(s.max_ms >= s.mean_ms && s.mean_ms > 0.0);
        // A new epoch: the finder starts over (nothing reported on its
        // first update, ids from 1 again).
        let fresh = fitter.run(&job(2, 2.0, grid.clone())).expect("a snapshot");
        assert_eq!(fresh.epoch, 2);
        assert_eq!(fresh.updates, 1);
        assert!(fresh.planes.is_empty(), "{:?}", fresh.planes);
        assert!(fresh.stats.tracked > 0);
        // Without `fit` the finder is left alone.
        let quiet = Job {
            fit: false,
            ..job(2, 2.25, grid)
        };
        assert!(fitter.run(&quiet).is_none());
    }

    #[test]
    fn a_newer_job_replaces_one_not_yet_taken() {
        let grid = grid_of(
            floor_and_wall,
            &view(Vec3::new(0.0, 1.6, 0.0), -25.0, FOV),
            40,
            0.0,
        );
        let mut slot = Slot::default();
        slot.put(job(1, 0.0, grid.clone()));
        slot.put(job(1, 0.25, grid));
        assert_eq!(slot.replaced, 1);
        let taken = slot.job.take().expect("a job");
        assert_close!(taken.time_s, 0.25);
    }

    /// Poll `w` until a snapshot comes, for up to 10 s.
    fn wait(w: &mut Worker) -> Snapshot {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(s) = w.poll() {
                return s;
            }
            assert!(Instant::now() < until, "no snapshot within 10 s");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn the_worker_never_waits_on_its_thread_and_answers_each_job() {
        let grid = grid_of(
            floor_and_wall,
            &view(Vec3::new(0.0, 1.6, 0.0), -25.0, FOV),
            40,
            0.0,
        );
        let mut w = Worker::spawn().expect("the thread");
        // While the thread cannot take the slot, a submit returns at once
        // and keeps the job; the newer replaces it.
        {
            let shared = Arc::clone(&w.shared);
            let held = shared.slot.lock().expect("the slot");
            let start = Instant::now();
            w.submit(job(1, 0.0, grid.clone()));
            w.submit(job(1, 0.25, grid.clone()));
            assert!(w.poll().is_none());
            assert!(start.elapsed() < Duration::from_millis(50));
            assert!(
                w.waiting
                    .as_ref()
                    .is_some_and(|j| (j.time_s - 0.25).abs() < 1e-6)
            );
            drop(held);
        }
        let s = wait(&mut w);
        assert_eq!((s.epoch, s.updates), (1, 1));
        w.submit(job(1, 0.5, grid));
        let s = wait(&mut w);
        assert_eq!(s.updates, 2);
        // The slot is free to read once the thread waits again.
        let replaced = (0..500).find_map(|_| {
            std::thread::sleep(Duration::from_millis(2));
            w.replaced()
        });
        assert_eq!(replaced, Some(0));
        // Dropping it joins the thread.
        drop(w);
    }
}
