//! Live planes against the headset's own depth (board #5400): a recording
//! of what the finder is given, and its replay on the desktop.
//!
//! **Why.** The finder's tests (`live_planes.rs`) feed it a synthetic
//! sensor. The headset's depth is filtered by the runtime and far quieter
//! than the finder's noise model, and on the headset the set it reported
//! did not settle. A recording is the finder's input on the headset, and
//! [`replay`] runs it through the worker's own code, so the finder can be
//! tuned against the room it failed in.
//!
//! **The knob** ([`RecKnob`]). `debug.fosfora.liveplanesrec <seconds>` is
//! read at launch and polled once a second. A new value above 0 asks for a
//! recording of that many seconds (at most [`MAX_S`]); unset or 0 stops
//! one. Read at launch, a recording starts with the finder's first update.
//! A finished recording does not start again while the knob keeps its
//! value: set another value, or clear it first.
//!
//! **The writer** ([`Recorder`]). The worker (`live_wire::Fitter`) appends
//! each read-back it is given, before the finder takes it, to one file in
//! the asked folder: `debug/` beside the app's own `Music/`
//! (`Android/data/<package>/files/debug`), which a USB pull reaches without
//! `run-as`. The count down starts at the first read-back written. The file
//! closes when the time is up, when the knob changes, or when the finder
//! starts over (a recording is of one run of the finder), and its path and
//! count are logged. The frame loop never writes. With the knob unset the
//! jobs carry no ask and the worker does nothing more than check that.
//!
//! **The file** (little-endian). A header of [`HEADER_BYTES`]: the magic
//! [`MAGIC`], the version [`VERSION`] (u32), the grid's columns and rows
//! (u32 each) and the reading ([`reading_code`], u32: 0 rows top-down, 1
//! rows bottom-up as on the Quest 3, 2 columns mirrored). Then one record
//! per read-back: [`RECORD_HEAD_BYTES`] of the time (f32 s, the acquire on
//! the app's clock), the layer's acquired position (3 f32) and orientation
//! (`x, y, z, w`, 4 f32), its fov (left, right, up, down, 4 f32 radians),
//! near and far (f32 each; far is infinite for an infinite far plane) and
//! the depth map's size (2 u32), then `cols * rows` f32: the stored depth
//! values exactly as read back, in the read-back's layout
//! (`depth[gy * cols + gx]`, see `live_wire::DepthGrid`). Each record is
//! written in one call, so a file cut short ends in at most one partial
//! record, which [`decode`] leaves out and counts.
//!
//! **The replay** ([`replay`]). Each record becomes a `live_wire::Job` of
//! one epoch and goes through `Fitter::run`, the worker's own path
//! (`unproject_grid`, then `LivePlanes::update`). [`doubles`] finds pairs
//! of reported planes that are one surface twice. The `#[ignore]`d test
//! `replay_the_recording_in_live_planes_rec` prints a recording's run;
//! recordings are of real rooms and stay out of the repository.

use std::fmt;
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use glam::{Quat, Vec2, Vec3};
use log::{info, warn};

use crate::env_depth::{DepthView, Reading, decode_distance};
use crate::live_planes::{LivePlane, Stats};
use crate::live_wire::{DepthGrid, Fitter, Job, area, clipped_area};
use crate::math::Fov;
use crate::surfaces::kind_name;

// ---- The knob ----------------------------------------------------------

/// The knob that asks for a recording, in seconds.
pub const KNOB: &str = "debug.fosfora.liveplanesrec";
/// The longest recording (s): 64x64 at 4 Hz is 64 KiB a second, so 600 s
/// is about 39 MiB, and 80x80 at 10 Hz about 150 MiB.
pub const MAX_S: f32 = 600.0;
/// The folder recordings go to, beside the app's own `Music/`.
pub const DIR: &str = "debug";

/// The knob's seconds: `None` unset, unreadable or not above 0, else at
/// most [`MAX_S`].
pub fn rec_knob(value: Option<&str>) -> Option<f32> {
    value
        .and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|s| s.is_finite() && *s > 0.0)
        .map(|s| s.min(MAX_S))
}

/// A recording the frame loop asks the worker for.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    /// A new value of the knob is a new ask.
    pub id: u32,
    pub seconds: f32,
    /// The folder the file goes in, created when it opens.
    pub dir: PathBuf,
}

/// The frame loop's side of [`KNOB`]: the ask its value makes, handed to
/// every job (`live_wire::Job::record`).
#[derive(Debug, Default)]
pub struct RecKnob {
    last: Option<String>,
    next_id: u32,
    ask: Option<Arc<Ask>>,
}

impl RecKnob {
    /// Take the knob's `value` (`None` unset), recordings going to `dir`
    /// (`None` when the app has no external files folder). Returns a line
    /// to log when the ask changes.
    pub fn poll(&mut self, value: Option<&str>, dir: Option<&Path>) -> Option<String> {
        if value == self.last.as_deref() {
            return None;
        }
        self.last = value.map(str::to_owned);
        let had = self.ask.take().is_some();
        match (rec_knob(value), dir) {
            (Some(seconds), Some(dir)) => {
                self.next_id += 1;
                self.ask = Some(Arc::new(Ask {
                    id: self.next_id,
                    seconds,
                    dir: dir.to_owned(),
                }));
                Some(format!(
                    "live planes rec: {seconds} s asked for ({KNOB}), from the next read-back the finder is given, into {}",
                    dir.display()
                ))
            }
            (Some(_), None) => Some(format!(
                "live planes rec: {KNOB} set, but the app has no external files folder to record into"
            )),
            (None, _) if had => Some(format!(
                "live planes rec: off ({KNOB} {})",
                value.unwrap_or("unset")
            )),
            (None, _) => None,
        }
    }

    /// The ask for the next job, `None` while there is none.
    pub fn ask(&self) -> Option<Arc<Ask>> {
        self.ask.clone()
    }
}

// ---- The format --------------------------------------------------------

/// The first eight bytes of a recording.
pub const MAGIC: [u8; 8] = *b"fslivrec";
pub const VERSION: u32 = 1;
/// The header: magic, version, columns, rows, reading.
pub const HEADER_BYTES: usize = 24;
/// A record before its depth values: time, position, orientation, fov,
/// near, far, size.
pub const RECORD_HEAD_BYTES: usize = 64;
/// Cells a decoded grid may have at most (a corrupt header names no
/// absurd size).
const MAX_CELLS: usize = 1 << 20;

/// The reading's code in the header.
pub fn reading_code(r: Reading) -> u32 {
    match r {
        Reading::RowTop => 0,
        Reading::RowBottom => 1,
        Reading::ColsMirrored => 2,
    }
}

fn reading_of(code: u32) -> Option<Reading> {
    match code {
        0 => Some(Reading::RowTop),
        1 => Some(Reading::RowBottom),
        2 => Some(Reading::ColsMirrored),
        _ => None,
    }
}

/// The reading's name for a log line.
pub fn reading_name(r: Reading) -> &'static str {
    match r {
        Reading::RowTop => "rows top-down",
        Reading::RowBottom => "rows bottom-up",
        Reading::ColsMirrored => "columns mirrored",
    }
}

/// Append the header of a recording of `cols` x `rows` grids read as
/// `reading`.
pub fn encode_header(cols: usize, rows: usize, reading: Reading, out: &mut Vec<u8>) {
    out.extend_from_slice(&MAGIC);
    for v in [VERSION, cols as u32, rows as u32, reading_code(reading)] {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

/// Append one record: `grid` acquired at `time_s`. Its depth values are
/// written as they are, all of them.
pub fn encode_record(time_s: f32, grid: &DepthGrid, out: &mut Vec<u8>) {
    let v = &grid.view;
    let [px, py, pz] = v.position;
    let [qx, qy, qz, qw] = v.orientation;
    let f = v.fov;
    for x in [
        time_s, px, py, pz, qx, qy, qz, qw, f.left, f.right, f.up, f.down, grid.near, grid.far,
    ] {
        out.extend_from_slice(&x.to_le_bytes());
    }
    for s in grid.size {
        out.extend_from_slice(&s.to_le_bytes());
    }
    for d in &grid.depth {
        out.extend_from_slice(&d.to_le_bytes());
    }
}

/// One read-back of a recording.
#[derive(Debug, Clone)]
pub struct Record {
    /// The acquire's time on the app's clock (s).
    pub time_s: f32,
    /// The read-back. Its `acquired` is not recorded: it is the decode's
    /// time.
    pub grid: DepthGrid,
}

/// A recording read back.
#[derive(Debug, Clone)]
pub struct Recording {
    pub cols: usize,
    pub rows: usize,
    pub reading: Reading,
    pub records: Vec<Record>,
    /// Bytes of a last record cut short, left out.
    pub cut_bytes: usize,
}

/// Why a recording does not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecError {
    /// Shorter than the header (its length).
    Short(usize),
    Magic,
    Version(u32),
    /// Columns and rows: none, too many, or not square (the read-back's
    /// grid is).
    Grid(u32, u32),
    Reading(u32),
}

impl fmt::Display for RecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Short(n) => write!(f, "{n} bytes, shorter than the {HEADER_BYTES}-byte header"),
            Self::Magic => write!(f, "not a live planes recording (no fslivrec magic)"),
            Self::Version(v) => write!(f, "version {v}, this build reads {VERSION}"),
            Self::Grid(c, r) => write!(f, "a {c} x {r} grid, not one the read-back makes"),
            Self::Reading(code) => write!(f, "reading code {code}, not 0, 1 or 2"),
        }
    }
}

impl std::error::Error for RecError {}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_bits(u32_at(b, at))
}

/// Read a recording from its bytes.
pub fn decode(bytes: &[u8]) -> Result<Recording, RecError> {
    if bytes.len() < HEADER_BYTES {
        return Err(RecError::Short(bytes.len()));
    }
    if bytes[..8] != MAGIC {
        return Err(RecError::Magic);
    }
    let version = u32_at(bytes, 8);
    if version != VERSION {
        return Err(RecError::Version(version));
    }
    let (c, r) = (u32_at(bytes, 12), u32_at(bytes, 16));
    let (cols, rows) = (c as usize, r as usize);
    if cols == 0 || cols != rows || cols.saturating_mul(rows) > MAX_CELLS {
        return Err(RecError::Grid(c, r));
    }
    let code = u32_at(bytes, 20);
    let reading = reading_of(code).ok_or(RecError::Reading(code))?;
    let len = RECORD_HEAD_BYTES + 4 * cols * rows;
    let body = &bytes[HEADER_BYTES..];
    let acquired = Instant::now();
    let records = body
        .chunks_exact(len)
        .map(|b| {
            let f = |k: usize| f32_at(b, 4 * k);
            let depth = (0..cols * rows)
                .map(|i| f32_at(b, RECORD_HEAD_BYTES + 4 * i))
                .collect();
            Record {
                time_s: f(0),
                grid: DepthGrid {
                    n: cols,
                    depth,
                    size: [u32_at(b, 56), u32_at(b, 60)],
                    view: DepthView {
                        orientation: [f(4), f(5), f(6), f(7)],
                        position: [f(1), f(2), f(3)],
                        fov: Fov {
                            left: f(8),
                            right: f(9),
                            up: f(10),
                            down: f(11),
                        },
                    },
                    near: f(12),
                    far: f(13),
                    acquired,
                },
            }
        })
        .collect();
    Ok(Recording {
        cols,
        rows,
        reading,
        records,
        cut_bytes: body.len() % len,
    })
}

/// Read the recording at `path`.
pub fn read(path: &Path) -> anyhow::Result<Recording> {
    use anyhow::Context as _;
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    decode(&bytes).with_context(|| format!("decoding {}", path.display()))
}

/// The file name of ask `id`'s recording opened at `unix_s`.
pub fn file_name(unix_s: u64, id: u32) -> String {
    format!("liveplanes-{unix_s}-{id}.lprec")
}

// ---- The writer (the worker's side) ------------------------------------

/// A recording being written.
#[derive(Debug)]
struct Open {
    ask: u32,
    path: PathBuf,
    file: File,
    n: usize,
    reading: Reading,
    count: u32,
    bytes: u64,
    first_s: f32,
    last_s: f32,
    until_s: f32,
}

impl Open {
    /// Create the file for `ask` and write the header for `job`'s grid.
    fn create(ask: &Ask, job: &Job, buf: &mut Vec<u8>) -> std::io::Result<Self> {
        std::fs::create_dir_all(&ask.dir)?;
        let unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let path = ask.dir.join(file_name(unix, ask.id));
        let mut file = File::create_new(&path)?;
        let n = job.grid.n;
        buf.clear();
        encode_header(n, n, job.reading, buf);
        file.write_all(buf)?;
        Ok(Self {
            ask: ask.id,
            path,
            file,
            n,
            reading: job.reading,
            count: 0,
            bytes: buf.len() as u64,
            first_s: job.time_s,
            last_s: job.time_s,
            until_s: job.time_s + ask.seconds,
        })
    }

    /// Append `job`'s read-back in one write.
    fn append(&mut self, job: &Job, buf: &mut Vec<u8>) -> std::io::Result<()> {
        buf.clear();
        encode_record(job.time_s, &job.grid, buf);
        self.file.write_all(buf)?;
        self.count += 1;
        self.bytes += buf.len() as u64;
        self.last_s = job.time_s;
        Ok(())
    }
}

/// The worker's recordings: at most one open, and the ask it last
/// finished, so a finished one does not start again while the knob keeps
/// its value.
#[derive(Debug, Default)]
pub struct Recorder {
    open: Option<Open>,
    done: Option<u32>,
    buf: Vec<u8>,
}

impl Recorder {
    /// Before the finder takes `job`: close a recording no longer asked
    /// for, whose time is up or whose grid changed shape, then append the
    /// job's read-back to the recording its ask is for, opening it on the
    /// first. A job with no ask and nothing open does nothing.
    pub fn step(&mut self, job: &Job) {
        let ask = job.record.as_deref();
        if let Some(o) = &self.open {
            let why = match ask {
                None => Some("the knob was cleared"),
                Some(a) if a.id != o.ask => Some("a new recording was asked for"),
                Some(_) if job.time_s >= o.until_s => Some("its time is up"),
                Some(_)
                    if job.grid.n != o.n
                        || job.grid.depth.len() != o.n * o.n
                        || job.reading != o.reading =>
                {
                    Some("the read-back changed shape")
                }
                Some(_) => None,
            };
            if let Some(why) = why {
                self.close(why);
            }
        }
        let Some(ask) = ask else {
            return;
        };
        if self.open.is_none() {
            if self.done == Some(ask.id) {
                return;
            }
            if job.grid.depth.len() != job.grid.n * job.grid.n {
                warn!(
                    "live planes rec: a read-back of {} values for a {} grid; not recorded",
                    job.grid.depth.len(),
                    job.grid.n
                );
                return;
            }
            match Open::create(ask, job, &mut self.buf) {
                Ok(o) => {
                    info!(
                        "live planes rec: recording {} s to {} (grid {}, {})",
                        ask.seconds,
                        o.path.display(),
                        o.n,
                        reading_name(o.reading)
                    );
                    self.open = Some(o);
                }
                Err(e) => {
                    warn!(
                        "live planes rec: creating a file in {}: {e}; not recording",
                        ask.dir.display()
                    );
                    self.done = Some(ask.id);
                    return;
                }
            }
        }
        if let Some(o) = &mut self.open
            && let Err(e) = o.append(job, &mut self.buf)
        {
            warn!("live planes rec: writing {}: {e}", o.path.display());
            self.close("a write failed");
        }
    }

    /// Close the open recording, if any, and log its path and count.
    pub fn close(&mut self, why: &str) {
        let Some(o) = self.open.take() else {
            return;
        };
        self.done = Some(o.ask);
        info!(
            "live planes rec: closed {} · {} read-backs over {:.1} s, {:.1} MiB ({why})",
            o.path.display(),
            o.count,
            o.last_s - o.first_s,
            o.bytes as f32 / (1024.0 * 1024.0)
        );
    }

    /// Whether a recording is open.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.close("the worker stopped");
    }
}

// ---- The replay --------------------------------------------------------

/// One update of a replay.
#[derive(Debug, Clone)]
pub struct Step {
    /// The read-back's time (s).
    pub time_s: f32,
    pub stats: Stats,
    /// The update's time on this machine (ms).
    pub update_ms: f32,
    /// The planes reported after it, in id order.
    pub planes: Vec<LivePlane>,
}

/// What a replay did.
#[derive(Debug, Clone, Default)]
pub struct Replay {
    pub steps: Vec<Step>,
    /// The reported planes after the last update.
    pub planes: Vec<LivePlane>,
    pub floor_y: Option<f32>,
    /// Each id ever reported and when it first was (s), in that order.
    pub first_reported: Vec<(u32, f32)>,
}

/// Run `rec` through the finder as the worker runs it: each record a
/// [`Job`] of one epoch through [`Fitter::run`] (`unproject_grid`, then
/// `LivePlanes::update`).
pub fn replay(rec: &Recording) -> Replay {
    let mut fitter = Fitter::default();
    let mut out = Replay::default();
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
        for p in &snap.planes {
            if !out.first_reported.iter().any(|&(id, _)| id == p.id) {
                out.first_reported.push((p.id, r.time_s));
            }
        }
        out.steps.push(Step {
            time_s: r.time_s,
            stats: snap.stats,
            update_ms: snap.update_ms,
            planes: snap.planes.clone(),
        });
        out.planes = snap.planes;
        out.floor_y = snap.floor_y;
    }
    out
}

/// How a read-back differs from another: the camera's move (m) and turn
/// (degrees), and the median change of the depth along -Z (m) over the
/// cells with depth in both, and how many those are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Change {
    pub moved_m: f32,
    pub turned_deg: f32,
    pub depth_m: f32,
    pub cells: usize,
}

/// How `b` differs from `a` (same grid).
pub fn change(a: &DepthGrid, b: &DepthGrid) -> Change {
    let q = |v: &DepthView| {
        let [x, y, z, w] = v.orientation;
        Quat::from_xyzw(x, y, z, w).normalize()
    };
    let mut diffs: Vec<f32> = a
        .depth
        .iter()
        .zip(&b.depth)
        .filter(|&(da, db)| (0.0..1.0).contains(da) && (0.0..1.0).contains(db))
        .map(|(&da, &db)| {
            (decode_distance(db, b.near, b.far) - decode_distance(da, a.near, a.far)).abs()
        })
        .filter(|d| d.is_finite())
        .collect();
    let cells = diffs.len();
    let depth_m = if diffs.is_empty() {
        0.0
    } else {
        let mid = diffs.len() / 2;
        *diffs.select_nth_unstable_by(mid, f32::total_cmp).1
    };
    // The turn from the relative rotation's vector part, which stays exact
    // for the hundredths of a degree a still headset turns (an angle from
    // the dot product's arc cosine has a floor of about 0.04 degrees).
    let d = q(&a.view).inverse() * q(&b.view);
    let turned = 2.0 * Vec3::new(d.x, d.y, d.z).length().atan2(d.w.abs());
    Change {
        moved_m: Vec3::from(a.view.position).distance(Vec3::from(b.view.position)),
        turned_deg: turned.to_degrees(),
        depth_m,
        cells,
    }
}

// ---- Doubles -----------------------------------------------------------

/// Two reported planes are one surface twice when their normals are within
/// this (degrees)...
pub const DOUBLE_DEG: f32 = 15.0;
/// ...each center is within this of the other's plane (m)...
pub const DOUBLE_OFFSET_M: f32 = 0.20;
/// ...and their rectangles overlap by more than this share of the
/// smaller's area.
pub const DOUBLE_OVERLAP: f32 = 0.3;

/// A pair of reported planes on one surface: their ids (the lower first),
/// the angle between their normals (degrees), the farther of each center
/// from the other's plane (m), and the overlap as a share of the smaller
/// rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Double {
    pub a: u32,
    pub b: u32,
    pub angle_deg: f32,
    pub offset_m: f32,
    pub overlap: f32,
}

/// Whether `a` and `b` are one surface twice ([`DOUBLE_DEG`],
/// [`DOUBLE_OFFSET_M`], [`DOUBLE_OVERLAP`]). The overlap is measured in
/// `a`'s plane, with `b`'s rectangle seen along `a`'s normal.
pub fn double(a: &LivePlane, b: &LivePlane) -> Option<Double> {
    let (na, nb) = (a.normal(), b.normal());
    let cos = na.dot(nb).clamp(-1.0, 1.0);
    if cos < DOUBLE_DEG.to_radians().cos() {
        return None;
    }
    let d = b.center - a.center;
    let offset_m = na.dot(d).abs().max(nb.dot(d).abs());
    if offset_m >= DOUBLE_OFFSET_M {
        return None;
    }
    let (ua, va) = (a.rot * Vec3::X, a.rot * Vec3::Y);
    let (ub, vb) = (b.rot * Vec3::X * b.half.x, b.rot * Vec3::Y * b.half.y);
    let quad = [
        b.center - ub - vb,
        b.center + ub - vb,
        b.center + ub + vb,
        b.center - ub + vb,
    ]
    .map(|q| Vec2::new((q - a.center).dot(ua), (q - a.center).dot(va)));
    let smaller = (4.0 * a.half.x * a.half.y).min(area(&quad));
    if smaller <= 0.0 {
        return None;
    }
    let overlap = clipped_area(&quad, a.half.x, a.half.y) / smaller;
    (overlap > DOUBLE_OVERLAP).then(|| Double {
        a: a.id.min(b.id),
        b: a.id.max(b.id),
        angle_deg: cos.acos().to_degrees(),
        offset_m,
        overlap,
    })
}

/// Every pair of `planes` that is one surface twice, by the lower id then
/// the higher.
pub fn doubles(planes: &[LivePlane]) -> Vec<Double> {
    let mut out = Vec::new();
    for (i, a) in planes.iter().enumerate() {
        for b in &planes[i + 1..] {
            out.extend(double(a, b));
        }
    }
    out.sort_by_key(|d| (d.a, d.b));
    out
}

// ---- The report --------------------------------------------------------

/// The summary also lists the planes reported at this time (s), so two
/// runs of one scene can be compared.
pub const LIST_AT_S: f32 = 10.0;

/// One reported plane: its id, kind, world plane and rectangle, its center
/// and normal in the frame of the camera `view` (so runs whose tracking
/// origins differ compare), and when it was first reported.
fn plane_line(s: &mut String, p: &LivePlane, run: &Replay, view: Option<&DepthView>) {
    use std::fmt::Write as _;
    let n = p.normal();
    let first = run
        .first_reported
        .iter()
        .find(|&&(id, _)| id == p.id)
        .map_or(f32::NAN, |&(_, t)| t);
    let (c, nc) = view.map_or((p.center, n), |v| {
        let [x, y, z, w] = v.orientation;
        let inv = Quat::from_xyzw(x, y, z, w).normalize().inverse();
        (inv * (p.center - Vec3::from(v.position)), inv * n)
    });
    let _ = writeln!(
        s,
        "  #{:<3} {:7} {:.2} x {:.2} m · center ({:5.2} {:5.2} {:5.2}) n ({:5.2} {:5.2} {:5.2}) · world offset {:6.2} m · first reported {first:.2} s",
        p.id,
        kind_name(p.kind),
        2.0 * p.half.x,
        2.0 * p.half.y,
        c.x,
        c.y,
        c.z,
        nc.x,
        nc.y,
        nc.z,
        n.dot(p.center)
    );
}

/// A replay as text: the recording, one line per update (the camera's
/// move and the depth's change since the update before and against the
/// last read-back, the fit's counts, the reported and tracked planes, the
/// ids that came and went and any doubles), then the planes reported at
/// [`LIST_AT_S`] and at the end, the most doubles any update had and the
/// doubles at the end.
pub fn summary(rec: &Recording, run: &Replay) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let span = match (rec.records.first(), rec.records.last()) {
        (Some(a), Some(b)) => b.time_s - a.time_s,
        _ => 0.0,
    };
    let _ = write!(
        s,
        "recording: {} read-backs over {span:.1} s, grid {} x {}, {}",
        rec.records.len(),
        rec.cols,
        rec.rows,
        reading_name(rec.reading)
    );
    if let Some(r) = rec.records.first() {
        let g = &r.grid;
        let _ = write!(
            s,
            ", map {} x {}, near {} far {}",
            g.size[0], g.size[1], g.near, g.far
        );
    }
    if rec.cut_bytes > 0 {
        let _ = write!(s, ", a last record cut short ({} bytes)", rec.cut_bytes);
    }
    s.push('\n');
    let last = rec.records.last().map(|r| &r.grid);
    let mut before: Vec<u32> = Vec::new();
    let mut worst: (usize, f32) = (0, 0.0);
    for (k, step) in run.steps.iter().enumerate() {
        let st = step.stats;
        let _ = write!(s, "t {:6.2}", step.time_s);
        if k > 0
            && let (Some(a), Some(b)) = (rec.records.get(k - 1), rec.records.get(k))
        {
            let c = change(&a.grid, &b.grid);
            let _ = write!(
                s,
                " · moved {:.1} mm turned {:.2}° depth Δ {:.2} cm",
                c.moved_m * 1000.0,
                c.turned_deg,
                c.depth_m * 100.0
            );
        }
        if let (Some(r), Some(z)) = (rec.records.get(k), last) {
            let _ = write!(
                s,
                " (to the last {:.2} cm)",
                change(&r.grid, z).depth_m * 100.0
            );
        }
        let _ = write!(
            s,
            " · valid {} patches {} merged {} created {} dropped {} · tracked {} · reported {}",
            st.valid, st.patches, st.merged, st.created, st.dropped, st.tracked, st.reported
        );
        let ids: Vec<u32> = step.planes.iter().map(|p| p.id).collect();
        for id in ids.iter().filter(|id| !before.contains(id)) {
            let _ = write!(s, " +#{id}");
        }
        for id in before.iter().filter(|id| !ids.contains(id)) {
            let _ = write!(s, " -#{id}");
        }
        if st.corrected {
            let _ = write!(s, " · the tracking origin moved, the planes with it");
        }
        if st.odd {
            let _ = write!(s, " · left out (explained neither still nor moved)");
        }
        let ds = doubles(&step.planes).len();
        if ds > 0 {
            let _ = write!(s, " · {ds} doubles");
        }
        if ds > worst.0 {
            worst = (ds, step.time_s);
        }
        let _ = writeln!(s, " · {:.2} ms", step.update_ms);
        before = ids;
    }
    if !run.steps.is_empty() {
        let n = run.steps.len() as f32;
        let mean = run.steps.iter().map(|x| x.update_ms).sum::<f32>() / n;
        let max = run.steps.iter().map(|x| x.update_ms).fold(0.0, f32::max);
        let _ = writeln!(
            s,
            "updates: {} · {mean:.2} ms mean, {max:.2} max (this machine) · floor y {}",
            run.steps.len(),
            run.floor_y.map_or("-".to_owned(), |y| format!("{y:.2}"))
        );
    }
    if let Some(k) = run.steps.iter().position(|x| x.time_s >= LIST_AT_S) {
        let step = &run.steps[k];
        let _ = writeln!(
            s,
            "reported at {:.2} s: {} (center and normal in the camera's frame: x right, y up, z back)",
            step.time_s,
            step.planes.len()
        );
        let view = rec.records.get(k).map(|r| &r.grid.view);
        for p in &step.planes {
            plane_line(&mut s, p, run, view);
        }
    }
    let _ = writeln!(s, "reported at the end: {}", run.planes.len());
    let view = rec.records.last().map(|r| &r.grid.view);
    for p in &run.planes {
        plane_line(&mut s, p, run, view);
    }
    let ds = doubles(&run.planes);
    let _ = writeln!(
        s,
        "doubles at any update: at most {}{}",
        worst.0,
        if worst.0 > 0 {
            format!(" (first at {:.2} s)", worst.1)
        } else {
            String::new()
        }
    );
    let _ = writeln!(s, "doubles: {}", ds.len());
    for d in &ds {
        let _ = writeln!(
            s,
            "  #{} and #{}: {:.1}° apart, {:.1} cm, {:.0}% of the smaller",
            d.a,
            d.b,
            d.angle_deg,
            d.offset_m * 100.0,
            d.overlap * 100.0
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live_wire::tests::{FOV, floor_and_wall, grid_of, view};
    use crate::surfaces::{KIND_CEILING, KIND_WALL, PLANE_HALF_THICKNESS_M};

    /// A fresh folder under the system's temporary one.
    fn temp(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fosfora-live-rec-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A read-back whose every value says which record and cell it is,
    /// with an infinite far plane and a NaN among the depths.
    fn odd_grid(n: usize, seed: f32) -> DepthGrid {
        let mut depth: Vec<f32> = (0..n * n).map(|i| seed + i as f32 * 1e-4).collect();
        depth[1] = f32::NAN;
        depth[2] = 1.0;
        DepthGrid {
            n,
            depth,
            size: [320, 288],
            view: DepthView {
                orientation: [0.1, -0.2, 0.3, 0.927],
                position: [seed, 1.6, -0.25],
                fov: Fov {
                    left: -0.9,
                    right: 0.8,
                    up: 0.7,
                    down: -0.85,
                },
            },
            near: 0.1,
            far: f32::INFINITY,
            acquired: Instant::now(),
        }
    }

    fn same_bits(a: &DepthGrid, b: &DepthGrid) {
        assert_eq!(a.n, b.n);
        assert_eq!(a.size, b.size);
        let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&a.depth), bits(&b.depth));
        assert_eq!(bits(&a.view.position), bits(&b.view.position));
        assert_eq!(bits(&a.view.orientation), bits(&b.view.orientation));
        let fov = |f: Fov| bits(&[f.left, f.right, f.up, f.down]);
        assert_eq!(fov(a.view.fov), fov(b.view.fov));
        assert_eq!(bits(&[a.near, a.far]), bits(&[b.near, b.far]));
    }

    #[test]
    fn a_recording_reads_back_bit_for_bit() {
        let n = 5;
        let grids = [odd_grid(n, 0.25), odd_grid(n, 0.5)];
        let mut bytes = Vec::new();
        encode_header(n, n, Reading::RowBottom, &mut bytes);
        assert_eq!(bytes.len(), HEADER_BYTES);
        encode_record(1.25, &grids[0], &mut bytes);
        assert_eq!(bytes.len(), HEADER_BYTES + RECORD_HEAD_BYTES + 4 * n * n);
        encode_record(1.5, &grids[1], &mut bytes);
        let rec = decode(&bytes).expect("a recording");
        assert_eq!(
            (rec.cols, rec.rows, rec.reading),
            (n, n, Reading::RowBottom)
        );
        assert_eq!(rec.records.len(), 2);
        assert_eq!(rec.cut_bytes, 0);
        assert_close!(rec.records[0].time_s, 1.25);
        assert_close!(rec.records[1].time_s, 1.5);
        for (r, g) in rec.records.iter().zip(&grids) {
            same_bits(&r.grid, g);
        }
        // Cut short: the partial last record is left out and counted.
        let cut = decode(&bytes[..bytes.len() - 10]).expect("a recording");
        assert_eq!(cut.records.len(), 1);
        assert_eq!(cut.cut_bytes, RECORD_HEAD_BYTES + 4 * n * n - 10);
        // A header alone is an empty recording.
        assert!(
            decode(&bytes[..HEADER_BYTES])
                .expect("empty")
                .records
                .is_empty()
        );
    }

    #[test]
    fn a_bad_header_is_named() {
        let mut bytes = Vec::new();
        encode_header(4, 4, Reading::RowTop, &mut bytes);
        assert_eq!(decode(&bytes[..10]).unwrap_err(), RecError::Short(10));
        let mut bad = bytes.clone();
        bad[0] = b'x';
        assert_eq!(decode(&bad).unwrap_err(), RecError::Magic);
        let mut bad = bytes.clone();
        bad[8] = 2;
        assert_eq!(decode(&bad).unwrap_err(), RecError::Version(2));
        let mut bad = bytes.clone();
        bad[16] = 5;
        assert_eq!(decode(&bad).unwrap_err(), RecError::Grid(4, 5));
        let mut bad = bytes.clone();
        bad[20] = 7;
        assert_eq!(decode(&bad).unwrap_err(), RecError::Reading(7));
        for r in [Reading::RowTop, Reading::RowBottom, Reading::ColsMirrored] {
            assert_eq!(reading_of(reading_code(r)), Some(r));
        }
    }

    #[test]
    fn the_knob_asks_once_per_value() {
        let dir = Path::new("/data/files/debug");
        let mut knob = RecKnob::default();
        assert_eq!(knob.poll(None, Some(dir)), None);
        assert!(knob.ask().is_none());
        let line = knob.poll(Some("30"), Some(dir)).expect("a line");
        assert!(line.contains("30 s asked for"), "{line}");
        let ask = knob.ask().expect("an ask");
        assert_eq!(
            *ask,
            Ask {
                id: 1,
                seconds: 30.0,
                dir: dir.to_owned()
            }
        );
        // The same value again is the same ask.
        assert_eq!(knob.poll(Some("30"), Some(dir)), None);
        assert_eq!(knob.ask().expect("an ask").id, 1);
        // A new value is a new ask; more than the most is the most.
        assert!(knob.poll(Some("5000"), Some(dir)).is_some());
        let ask = knob.ask().expect("an ask");
        assert_eq!((ask.id, ask.seconds), (2, MAX_S));
        // Cleared or 0: off, logged once.
        assert!(
            knob.poll(None, Some(dir))
                .is_some_and(|l| l.contains("off"))
        );
        assert!(knob.ask().is_none());
        assert_eq!(knob.poll(Some("0"), Some(dir)), None);
        // No folder: no ask, and a line saying why.
        assert!(knob.poll(Some("10"), None).is_some());
        assert!(knob.ask().is_none());
        assert_eq!(rec_knob(Some(" 12.5 ")), Some(12.5));
        assert_eq!(rec_knob(Some("-3")), None);
        assert_eq!(rec_knob(Some("x")), None);
        assert_eq!(rec_knob(Some("inf")), None);
    }

    fn job(epoch: u64, time_s: f32, grid: DepthGrid, record: Option<&Arc<Ask>>) -> Job {
        Job {
            epoch,
            time_s,
            grid,
            reading: Reading::RowBottom,
            fit: true,
            noise: None,
            record: record.cloned(),
        }
    }

    fn files(dir: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    #[test]
    fn the_worker_records_what_it_is_given_until_the_time_is_up_and_the_replay_matches_it() {
        let dir = temp("worker");
        let grid = grid_of(
            floor_and_wall,
            &view(Vec3::new(0.0, 1.6, 0.0), -25.0, FOV),
            40,
            0.0,
        );
        let ask = Arc::new(Ask {
            id: 1,
            seconds: 2.0,
            dir: dir.clone(),
        });
        // Without an ask the worker writes nothing.
        let mut fitter = Fitter::default();
        fitter.run(&job(1, 0.0, grid.clone(), None));
        assert!(files(&dir).is_empty());
        // With one: every job from the first it carries, until 2 s later.
        let mut fitter = Fitter::default();
        let mut snaps = Vec::new();
        for i in 0..12 {
            let t = i as f32 * 0.25;
            snaps.push(
                fitter
                    .run(&job(1, t, grid.clone(), Some(&ask)))
                    .expect("a snapshot"),
            );
        }
        let found = files(&dir);
        assert_eq!(found.len(), 1, "{found:?}");
        let name = found[0].file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert!(
            name.starts_with("liveplanes-") && name.ends_with("-1.lprec"),
            "{name}"
        );
        let rec = read(&found[0]).expect("the recording");
        assert_eq!(rec.records.len(), 8);
        assert_eq!((rec.cols, rec.reading), (40, Reading::RowBottom));
        same_bits(&rec.records[3].grid, &grid);
        // The replay is the worker's run, plane for plane.
        let run = replay(&rec);
        assert_eq!(run.steps.len(), 8);
        let direct = &snaps[7];
        assert!(!direct.planes.is_empty());
        assert_eq!(run.planes, direct.planes);
        assert_eq!(run.steps[7].stats, direct.stats);
        assert_eq!(run.floor_y, direct.floor_y);
        let text = summary(&rec, &run);
        assert!(
            text.starts_with("recording: 8 read-backs over 1.8 s, grid 40 x 40"),
            "{text}"
        );
        assert!(text.contains("doubles: 0"), "{text}");
        // The same ask does not record again, and a new run of the finder
        // closes an open one.
        let ask2 = Arc::new(Ask {
            id: 2,
            seconds: 60.0,
            dir: dir.clone(),
        });
        fitter.run(&job(1, 3.0, grid.clone(), Some(&ask2)));
        fitter.run(&job(2, 3.25, grid.clone(), Some(&ask2)));
        fitter.run(&job(2, 3.5, grid, Some(&ask2)));
        let found = files(&dir);
        assert_eq!(found.len(), 2, "{found:?}");
        let second = found
            .iter()
            .find(|p| p.to_string_lossy().ends_with("-2.lprec"))
            .expect("the second");
        assert_eq!(read(second).expect("the second").records.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recorder_without_a_folder_gives_up_once() {
        // A file where the folder should be.
        let blocker = temp("blocked");
        std::fs::write(&blocker, b"x").expect("a file");
        let ask = Arc::new(Ask {
            id: 4,
            seconds: 10.0,
            dir: blocker.clone(),
        });
        let grid = odd_grid(4, 0.5);
        let mut rec = Recorder::default();
        rec.step(&job(1, 0.0, grid.clone(), Some(&ask)));
        assert!(!rec.is_open());
        rec.step(&job(1, 0.25, grid, Some(&ask)));
        assert!(!rec.is_open());
        let _ = std::fs::remove_file(&blocker);
    }

    #[test]
    fn the_change_between_read_backs_is_the_pose_and_the_median_depth() {
        let a = odd_grid(4, 0.5);
        let mut b = a.clone();
        b.view.position[0] += 0.003;
        b.depth = a.depth.iter().map(|d| d + 1e-3).collect();
        let c = change(&a, &b);
        assert_close!(c.moved_m, 0.003);
        assert!(c.turned_deg < 1e-3, "{}", c.turned_deg);
        // NaN and 1.0 (no depth) left out.
        assert_eq!(c.cells, 14);
        assert!(c.depth_m > 0.0 && c.depth_m < 0.01, "{}", c.depth_m);
        // A turn of a fiftieth of a degree reads as one.
        let [x, y, z, w] = a.view.orientation;
        let turned =
            Quat::from_xyzw(x, y, z, w).normalize() * Quat::from_rotation_y(0.02f32.to_radians());
        b.view.orientation = turned.to_array();
        let c = change(&a, &b);
        assert!((c.turned_deg - 0.02).abs() < 1e-3, "{}", c.turned_deg);
    }

    // ---- Doubles ---------------------------------------------------------

    /// A ceiling at `y` facing down, centered at (`x`, `z`), `w` x `d`.
    fn ceiling(id: u32, x: f32, y: f32, z: f32, w: f32, d: f32) -> LivePlane {
        LivePlane {
            id,
            kind: KIND_CEILING,
            center: Vec3::new(x, y, z),
            rot: Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            half: Vec3::new(0.5 * w, 0.5 * d, PLANE_HALF_THICKNESS_M),
            confidence: 1.0,
            last_seen_s: 0.0,
        }
    }

    /// A wall facing +z at `z`, turned `deg` about the vertical.
    fn wall(id: u32, x: f32, z: f32, deg: f32, w: f32) -> LivePlane {
        LivePlane {
            id,
            kind: KIND_WALL,
            center: Vec3::new(x, 1.3, z),
            rot: Quat::from_rotation_y(deg.to_radians()),
            half: Vec3::new(0.5 * w, 1.0, PLANE_HALF_THICKNESS_M),
            confidence: 1.0,
            last_seen_s: 0.0,
        }
    }

    #[test]
    fn nested_offset_ceilings_are_a_double() {
        let a = ceiling(1, 0.0, 2.70, 0.0, 4.15, 3.19);
        assert!(a.normal().dot(Vec3::NEG_Y) > 0.999);
        let b = ceiling(34, 0.05, 2.73, -0.04, 4.05, 3.10);
        let ds = doubles(&[b, a]);
        assert_eq!(ds.len(), 1);
        let d = ds[0];
        assert_eq!((d.a, d.b), (1, 34));
        assert!(d.angle_deg < 0.1, "{}", d.angle_deg);
        assert_close!(d.offset_m, 0.03);
        assert!(d.overlap > 0.99, "{}", d.overlap);
        // A small one inside a large one: all of the smaller.
        let small = ceiling(5, 1.0, 2.75, 0.5, 0.5, 0.5);
        let d = double(&a, &small).expect("a double");
        assert_close!(d.overlap, 1.0);
        let e = double(&small, &a).expect("either way round");
        assert_eq!((e.a, e.b), (1, 5));
        assert_close!(
            [e.angle_deg, e.offset_m, e.overlap],
            [d.angle_deg, d.offset_m, d.overlap]
        );
    }

    #[test]
    fn the_double_test_holds_its_angle_offset_and_overlap_limits() {
        let a = wall(1, 0.0, -2.0, 0.0, 2.0);
        // Normals 14 and 16 degrees apart, about the same center.
        assert!(double(&a, &wall(2, 0.0, -2.0, 14.0, 2.0)).is_some());
        assert!(double(&a, &wall(2, 0.0, -2.0, 16.0, 2.0)).is_none());
        // 19 and 21 cm in front.
        assert!(double(&a, &wall(2, 0.0, -1.81, 0.0, 2.0)).is_some());
        assert!(double(&a, &wall(2, 0.0, -1.79, 0.0, 2.0)).is_none());
        // Side by side, 1 m apart, 2 m wide each: overlap 1 of 2.
        assert!(double(&a, &wall(2, 1.0, -2.0, 0.0, 2.0)).is_some());
        // 1.5 m apart: 0.5 of 2 is 25%; 1.3 m apart: 35%.
        assert!(double(&a, &wall(2, 1.5, -2.0, 0.0, 2.0)).is_none());
        assert!(double(&a, &wall(2, 1.3, -2.0, 0.0, 2.0)).is_some());
        // Beside it, not over it.
        assert!(double(&a, &wall(2, 2.5, -2.0, 0.0, 2.0)).is_none());
        // The two faces of a board: opposite normals.
        assert!(double(&a, &wall(2, 0.0, -2.02, 180.0, 2.0)).is_none());
        // A rectangle of no size overlaps nothing.
        let flat = LivePlane {
            half: Vec3::new(0.0, 1.0, PLANE_HALF_THICKNESS_M),
            ..wall(2, 0.0, -2.0, 0.0, 2.0)
        };
        assert!(double(&a, &flat).is_none());
        // Every pair, by id.
        let set = [
            wall(9, 0.0, -2.01, 0.0, 2.0),
            a,
            wall(3, 0.1, -2.02, 1.0, 1.0),
            wall(4, 5.0, -2.0, 0.0, 1.0),
        ];
        let pairs: Vec<(u32, u32)> = doubles(&set).iter().map(|d| (d.a, d.b)).collect();
        assert_eq!(pairs, vec![(1, 3), (1, 9), (3, 9)]);
    }

    /// The replay of a recording made on the headset: `LIVE_PLANES_REC`
    /// names the file. Recordings are of real rooms and are never
    /// committed.
    #[test]
    #[ignore = "needs a recording: LIVE_PLANES_REC=<file.lprec>"]
    fn replay_the_recording_in_live_planes_rec() {
        let path = std::env::var("LIVE_PLANES_REC")
            .unwrap_or_else(|_| panic!("set LIVE_PLANES_REC to a recording's path"));
        let rec = read(Path::new(&path)).unwrap_or_else(|e| panic!("{e:#}"));
        let run = replay(&rec);
        println!("{path}");
        print!("{}", summary(&rec, &run));
    }
}
