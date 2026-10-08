//! The surface lanes (board #3326): each obstacle box's behavior, from the
//! room file and the boxes of the frame, as the rows the Flux sim reads
//! after the pour row (`surfaces::lane_row`) and the behavior per box the
//! weights, the wall spectrum and the surfaces pass follow. Plain data, so
//! it builds and tests on the desktop; `app.rs` feeds it the frame's
//! boxes, the `debug.fosfora.surface` knob and the room editor's pinches
//! (`room_edit.rs`), which go through the same typed calls the knob does
//! ([`RoomLanes::assign`], [`RoomLanes::assign_kind_of`],
//! [`RoomLanes::cycle`]): one path that writes, saves and logs. The hand
//! menu's surface rows (board #3472, D3) step the pointed surface's color,
//! band and strength through [`RoomLanes::set_params`], which writes the
//! same entry fields the knob's `:<color>:<band>@<strength>` does, through
//! the same path.
//!
//! The rows are rebuilt only when the box list (UUIDs and kinds, in order)
//! or an assignment changes: the boxes relocate once a second but keep
//! their identity, so a steady room costs one hash of its box list per
//! frame. The room file is loaded when the room id changes (the query
//! returned another anchor set) and saved on every change, never on load;
//! one that does not read is left alone until the next change.
//!
//! The knob, comma-separated assignments
//! `<target>=<behavior>[:<color>[:<band>]][@<strength>]` ([`parse_knob`]):
//! the target is a UUID (32 hex, or a prefix of at least 8 unique among
//! the room's anchors), a box index `#<k>` (this frame's order, the stage
//! floor too) or a kind name (the class assignment: the kind's default and
//! every anchor of that kind); the behavior one of `none embers sparks
//! spectrum rings streamlines curls pulse` (`ripple` reads as `rings`); the
//! color index 0 to 8 (0 the kind's own color, 1 to 7 the surface palette:
//! blue, violet, warm white, amber, green, teal, rose; 8 the key's tint)
//! and the audio band 0 to 3 (rms, bass, mid, high), board #3488, written
//! to the entries' `params` (a color alone keeps the band they have; with
//! neither, the params are left as they are; a kind's default carries no
//! params, so an anchor of the kind that the room gains later runs the
//! defaults); the strength 0..1, 1 by default. `clear` alone drops every
//! entry and puts the kind defaults back. A bad value applies nothing.

use std::path::PathBuf;

use log::{info, warn};

use crate::room_file::{Loaded, RoomFile, STAGE_FLOOR_UUID, room_id_hex, room_path, uuid_hex};
use crate::surface_fx::BANDS;
use crate::surfaces::{
    COLOR_KEY, RESERVED_BEHAVIORS, SURFACE_LANE_ROWS, SurfaceBehavior, kind_from_name, kind_name,
    lane_params, lane_row,
};

/// The shortest UUID prefix the knob takes.
pub const MIN_UUID_PREFIX: usize = 8;
/// The hand menu's Strength row steps 0..1 in this many steps (tenths,
/// [`next_strength`]), and a strength from it is stored at that precision.
pub const STRENGTH_STEPS: f32 = 10.0;

/// One of a surface's three parameters the hand menu steps (board #3472,
/// D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Param {
    /// The color index, 0..=[`COLOR_KEY`] (`surfaces::color_name`).
    Color,
    /// The audio band, 0..[`BANDS`] (`surface_fx::band_name`).
    Band,
    /// The strength, 0..1.
    Strength,
}

/// What the panel changes on one surface ([`RoomLanes::set_params`]).
/// `None` leaves a field alone.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ParamEdit {
    pub color: Option<u32>,
    pub band: Option<u32>,
    pub strength: Option<f32>,
}

impl ParamEdit {
    /// One step of `param` up or down from a surface's `current` (color,
    /// band, strength, as [`RoomLanes::params_of`] reports them): the
    /// color and the band wrap, the strength clamps.
    pub fn step(param: Param, up: bool, current: (u32, u32, f32)) -> Self {
        let (color, band, strength) = current;
        match param {
            Param::Color => Self {
                color: Some(next_color(color, up)),
                ..Self::default()
            },
            Param::Band => Self {
                band: Some(next_band(band, up)),
                ..Self::default()
            },
            Param::Strength => Self {
                strength: Some(next_strength(strength, up)),
                ..Self::default()
            },
        }
    }
}

/// The color index one step up or down from `color`, wrapping over
/// 0..=[`COLOR_KEY`] (the key's tint is one step below the kind's own).
pub fn next_color(color: u32, up: bool) -> u32 {
    step_wrapping(color, up, COLOR_KEY + 1)
}

/// The audio band one step up or down from `band`, wrapping over the
/// [`BANDS`] (rms, bass, mid, high).
pub fn next_band(band: u32, up: bool) -> u32 {
    step_wrapping(band, up, BANDS)
}

/// `v` (taken as the last value when past it) one step up or down in
/// `0..n`, wrapping.
fn step_wrapping(v: u32, up: bool, n: u32) -> u32 {
    let v = v.min(n - 1);
    if up { (v + 1) % n } else { (v + n - 1) % n }
}

/// The strength a tenth up or down from `strength`'s nearest tenth,
/// clamped to 0..1 with no wrap: dimming to nothing and then jumping to
/// full would surprise.
pub fn next_strength(strength: f32, up: bool) -> f32 {
    let tenths = (strength * STRENGTH_STEPS).round() + if up { 1.0 } else { -1.0 };
    tenths.clamp(0.0, STRENGTH_STEPS) / STRENGTH_STEPS
}

/// `strength` at one decimal, as a strength from the panel is stored (the
/// nearest float to the decimal, as the knob's `@0.7` reads).
pub fn round_strength(strength: f32) -> f32 {
    (strength * STRENGTH_STEPS).round() / STRENGTH_STEPS
}

/// One obstacle box of this frame as the lanes see it, in the obstacle
/// block's order: the room's boxes, then the stage floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneBox<'a> {
    /// All zero for the stage floor.
    pub uuid: [u8; 16],
    pub kind: u32,
    /// The anchor's semantic labels, for the log.
    pub label: &'a str,
}

/// What a knob assignment is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// An anchor by UUID or UUID prefix (lowercase hex, 8 to 32 digits).
    Uuid(String),
    /// A box by its index in this frame's order.
    Index(usize),
    /// Every surface of a kind, and the kind's default.
    Kind(u32),
}

/// One `<target>=<behavior>[:<color>[:<band>]][@<strength>]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub target: Target,
    pub behavior: SurfaceBehavior,
    pub strength: f32,
    /// The color index (0..=`surfaces::COLOR_KEY`), when given.
    pub color: Option<u32>,
    /// The audio band (0..`surface_fx::BANDS`), when given (only after a
    /// color).
    pub band: Option<u32>,
}

/// A parsed `debug.fosfora.surface` value.
#[derive(Debug, Clone, PartialEq)]
pub enum Knob {
    /// Drop every entry, the kind defaults back to the built-ins.
    Clear,
    Assign(Vec<Assignment>),
}

/// Parse a `debug.fosfora.surface` value (the grammar in the module docs).
pub fn parse_knob(value: &str) -> Result<Knob, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("empty".to_owned());
    }
    if value.eq_ignore_ascii_case("clear") {
        return Ok(Knob::Clear);
    }
    value
        .split(',')
        .map(|part| parse_assignment(part.trim()))
        .collect::<Result<Vec<_>, _>>()
        .map(Knob::Assign)
}

fn parse_assignment(part: &str) -> Result<Assignment, String> {
    if part.eq_ignore_ascii_case("clear") {
        return Err("'clear' stands alone".to_owned());
    }
    let (target, rhs) = part.split_once('=').ok_or_else(|| {
        format!("'{part}': expected <target>=<behavior>[:<color>[:<band>]][@<strength>]")
    })?;
    let (spec, strength) = match rhs.split_once('@') {
        Some((b, s)) => (b, Some(s)),
        None => (rhs, None),
    };
    let target = parse_target(target.trim())?;
    let mut fields = spec.split(':');
    let behavior = fields.next().unwrap_or("").trim();
    let color = fields
        .next()
        .map(|c| parse_index(c, crate::surfaces::COLOR_KEY, "color"))
        .transpose()?;
    let band = fields
        .next()
        .map(|b| parse_index(b, crate::surface_fx::BANDS - 1, "band"))
        .transpose()?;
    if fields.next().is_some() {
        return Err(format!("'{spec}': at most <behavior>:<color>:<band>"));
    }
    let behavior = SurfaceBehavior::from_name(behavior).ok_or_else(|| {
        if RESERVED_BEHAVIORS
            .iter()
            .any(|r| r.eq_ignore_ascii_case(behavior))
        {
            format!("'{behavior}' is reserved for a later step")
        } else {
            format!(
                "unknown behavior '{behavior}' (none embers sparks spectrum rings streamlines curls pulse \
                 aurora prism shards astrolabe bezel fenestra reticle tessera)"
            )
        }
    })?;
    let strength = match strength {
        None => 1.0,
        Some(s) => s
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|v| (0.0..=1.0).contains(v))
            .ok_or_else(|| format!("strength '{}' is not in 0..1", s.trim()))?,
    };
    Ok(Assignment {
        target,
        behavior,
        strength,
        color,
        band,
    })
}

/// A color index or a band: a whole number from 0 to `max`.
fn parse_index(v: &str, max: u32, what: &str) -> Result<u32, String> {
    let v = v.trim();
    v.parse::<u32>()
        .ok()
        .filter(|n| *n <= max)
        .ok_or_else(|| format!("{what} '{v}' is not a whole number from 0 to {max}"))
}

fn parse_target(t: &str) -> Result<Target, String> {
    if let Some(k) = t.strip_prefix('#') {
        return k
            .parse::<usize>()
            .map(Target::Index)
            .map_err(|_| format!("'{t}' is not a box index"));
    }
    if let Some(kind) = kind_from_name(t) {
        return Ok(Target::Kind(kind));
    }
    if (MIN_UUID_PREFIX..=32).contains(&t.len()) && t.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Ok(Target::Uuid(t.to_ascii_lowercase()));
    }
    Err(format!(
        "'{t}' is not a UUID (at least {MIN_UUID_PREFIX} hex digits), #<index> or a kind"
    ))
}

/// The boxes `target` names among `boxes`: one for a UUID or an index, a
/// UUID prefix only when one anchor has it; every box of a kind (maybe
/// none: the kind's default still takes).
pub fn resolve(target: &Target, boxes: &[LaneBox<'_>]) -> Result<Vec<usize>, String> {
    match target {
        Target::Index(k) if *k < boxes.len() => Ok(vec![*k]),
        Target::Index(k) => Err(format!("no box #{k} ({} boxes)", boxes.len())),
        Target::Kind(kind) => Ok(boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind == *kind)
            .map(|(k, _)| k)
            .collect()),
        Target::Uuid(prefix) => {
            let hits: Vec<usize> = boxes
                .iter()
                .enumerate()
                .filter(|(_, b)| {
                    b.uuid != STAGE_FLOOR_UUID && uuid_hex(&b.uuid).starts_with(prefix)
                })
                .map(|(k, _)| k)
                .collect();
            match hits.len() {
                1 => Ok(hits),
                0 => Err(format!("no anchor {prefix}")),
                n => Err(format!("{prefix} names {n} anchors")),
            }
        }
    }
}

/// The lane rows and the behavior of each of `boxes` under `file`. The
/// rows past the boxes stay zero; the behaviors cover every box, also one
/// past the obstacle block's capacity (the block drops it, the canvas can
/// still pick it).
pub fn lane_rows(
    file: &RoomFile,
    boxes: &[LaneBox<'_>],
) -> ([[f32; 4]; SURFACE_LANE_ROWS], Vec<SurfaceBehavior>) {
    let mut rows = [[0.0; 4]; SURFACE_LANE_ROWS];
    let mut behaviors = Vec::with_capacity(boxes.len());
    for (k, b) in boxes.iter().enumerate() {
        let (behavior, strength, params) = file.resolve(&b.uuid, b.kind);
        if let Some(row) = rows.get_mut(k) {
            *row = lane_row(behavior, strength, params);
        }
        behaviors.push(behavior);
    }
    (rows, behaviors)
}

/// A room id for the log: its hex, `-` without a room.
fn room_label(room: Option<u64>) -> String {
    room.map_or("-".to_owned(), room_id_hex)
}

/// A box for the log: its label (`stage floor` for the stage floor) and
/// the first 8 hex digits of its UUID.
pub fn box_label(b: &LaneBox<'_>) -> (String, String) {
    let label = if b.uuid == STAGE_FLOOR_UUID {
        "stage floor".to_owned()
    } else {
        b.label.to_owned()
    };
    (label, uuid_hex(&b.uuid)[..8].to_owned())
}

/// The app's lanes: the room file of the room in view and the rows built
/// from it for the frame's boxes.
#[derive(Debug)]
pub struct RoomLanes {
    /// The config dir the room files live under (`rooms/`).
    config: PathBuf,
    /// The room the file state is for; `None` without anchors.
    room: Option<u64>,
    /// Whether any frame has run yet (the first loads even without a room).
    started: bool,
    file: RoomFile,
    /// Bumped on every assignment, part of the build key.
    revision: u64,
    /// The key the rows were last built for.
    key: Option<u64>,
    rows: [[f32; 4]; SURFACE_LANE_ROWS],
    behaviors: Vec<SurfaceBehavior>,
    /// The knob value last applied (or refused), so it applies once.
    applied: Option<String>,
}

impl RoomLanes {
    pub fn new(config: PathBuf) -> Self {
        Self {
            config,
            room: None,
            started: false,
            file: RoomFile::default(),
            revision: 0,
            key: None,
            rows: [[0.0; 4]; SURFACE_LANE_ROWS],
            behaviors: Vec::new(),
            applied: None,
        }
    }

    /// This frame's boxes of `room`: load the room's file when the room
    /// changed, rebuild the rows when the boxes or the assignments did
    /// (logging the table once). True when the rows were rebuilt.
    pub fn update(&mut self, room: Option<u64>, boxes: &[LaneBox<'_>]) -> bool {
        if room != self.room || !self.started {
            self.started = true;
            self.room = room;
            self.load();
        }
        let key = self.key_of(boxes);
        if self.key == Some(key) {
            return false;
        }
        self.key = Some(key);
        (self.rows, self.behaviors) = lane_rows(&self.file, boxes);
        info!("{}", self.table(boxes));
        true
    }

    /// The rows for the aux block, one per box of the last update.
    pub fn rows(&self) -> &[[f32; 4]; SURFACE_LANE_ROWS] {
        &self.rows
    }

    /// The behavior box `k` of the last update runs.
    pub fn behavior(&self, k: usize) -> SurfaceBehavior {
        self.behaviors.get(k).copied().unwrap_or_default()
    }

    /// The knob's value this poll (`None` or empty: unset). A value is
    /// applied once, when it differs from the last one; while the room is
    /// on but has no anchors yet (`waiting`) it waits for them instead of
    /// failing on a UUID. Returns true when it changed an assignment.
    pub fn poll_knob(&mut self, value: Option<&str>, waiting: bool, boxes: &[LaneBox<'_>]) -> bool {
        let value = value.map(str::trim).filter(|v| !v.is_empty());
        if value == self.applied.as_deref() {
            return false;
        }
        let Some(value) = value else {
            self.applied = None;
            return false;
        };
        if waiting {
            return false;
        }
        self.applied = Some(value.to_owned());
        match self.apply(value, boxes) {
            Ok(()) => true,
            Err(e) => {
                warn!("debug.fosfora.surface '{value}': {e}; nothing applied");
                false
            }
        }
    }

    /// Apply a knob value to the room file (all of it or, on any error,
    /// none), save it and log each assignment.
    fn apply(&mut self, value: &str, boxes: &[LaneBox<'_>]) -> Result<(), String> {
        let assignments = match parse_knob(value)? {
            Knob::Clear => {
                self.file.clear();
                let saved = self.save();
                info!("room {}: cleared{saved}", room_label(self.room));
                self.revision += 1;
                return Ok(());
            }
            Knob::Assign(a) => a,
        };
        let targets = assignments
            .iter()
            .map(|a| resolve(&a.target, boxes))
            .collect::<Result<Vec<_>, _>>()?;
        let mut lines = Vec::new();
        for (a, hits) in assignments.iter().zip(&targets) {
            lines.extend(self.write(a, hits, boxes));
        }
        self.commit(&lines);
        Ok(())
    }

    /// Assign `behavior` at `strength` to what `target` names among
    /// `boxes` (a box, or a kind: its default and every anchor of it), save
    /// the file and log it, as a knob value of one assignment does. `Err`
    /// (nothing applied) when the target names nothing.
    pub fn assign(
        &mut self,
        target: &Target,
        behavior: SurfaceBehavior,
        strength: f32,
        boxes: &[LaneBox<'_>],
    ) -> Result<(), String> {
        let hits = resolve(target, boxes)?;
        let a = Assignment {
            target: target.clone(),
            behavior,
            strength,
            color: None,
            band: None,
        };
        let lines = self.write(&a, &hits, boxes);
        self.commit(&lines);
        Ok(())
    }

    /// What box `k` of `boxes` runs under the file as it is now (not as of
    /// the last update): its entry's behavior and strength, else its kind's
    /// default at full strength. `None` past the boxes.
    pub fn effective(&self, k: usize, boxes: &[LaneBox<'_>]) -> Option<(SurfaceBehavior, f32)> {
        let b = boxes.get(k)?;
        let (behavior, strength, _) = self.file.resolve(&b.uuid, b.kind);
        Some((behavior, strength))
    }

    /// Box `index`'s color index, audio band and strength as the surfaces
    /// pass resolves them under the file as it is now: the entry's lane
    /// row read back (`surfaces::lane_params` and the row's strength), so
    /// the panel shows what the face shows. An unset color is 0 (the
    /// kind's own), an entry without params (or no entry) runs its
    /// behavior's default band, an unset strength is 1. Past the boxes:
    /// those defaults for none.
    pub fn params_of(&self, index: usize, boxes: &[LaneBox<'_>]) -> (u32, u32, f32) {
        let (behavior, strength, params) = boxes.get(index).map_or_else(
            || {
                let none = SurfaceBehavior::None;
                (none, 1.0, crate::room_file::default_params(none))
            },
            |b| self.file.resolve(&b.uuid, b.kind),
        );
        let row = lane_row(behavior, strength, params);
        let (color, band) = lane_params(row, behavior);
        (color, band, row[1])
    }

    /// The hand menu's write (board #3472, D3): `edit`'s fields on box
    /// `index` (the pointed surface), the others kept. A surface without an
    /// entry gets one first, at its effective behavior and strength, so
    /// the params have somewhere to go (`RoomFile::set_color` writes
    /// nothing without an entry). Saved and logged through the path every
    /// assignment takes. The color is a whole number 0..=[`COLOR_KEY`],
    /// the band 0..[`BANDS`], the strength 0..1 (stored at one decimal);
    /// anything out of range, or a box past `boxes`, refuses the edit
    /// whole: nothing written, one warning. Returns whether the surface's
    /// look changed (an edit to the values it already has writes nothing).
    pub fn set_params(&mut self, index: usize, boxes: &[LaneBox<'_>], edit: ParamEdit) -> bool {
        let bad = |what: String| {
            warn!("room {}: {what}; nothing changed", room_label(self.room));
            false
        };
        if index >= boxes.len() {
            return bad(format!("no box #{index} ({} boxes)", boxes.len()));
        }
        if let Some(c) = edit.color.filter(|c| *c > COLOR_KEY) {
            return bad(format!("color {c} is not from 0 to {COLOR_KEY}"));
        }
        if let Some(b) = edit.band.filter(|b| *b >= BANDS) {
            return bad(format!("band {b} is not from 0 to {}", BANDS - 1));
        }
        if let Some(s) = edit.strength.filter(|s| !(0.0..=1.0).contains(s)) {
            return bad(format!("strength {s} is not in 0..1"));
        }
        let b = boxes[index];
        let (behavior, strength, _) = self.file.resolve(&b.uuid, b.kind);
        let (color, band, _) = self.params_of(index, boxes);
        let next = (
            edit.color.unwrap_or(color),
            edit.band.unwrap_or(band),
            edit.strength.map_or(strength, round_strength),
        );
        if next == (color, band, strength) {
            return false;
        }
        // The color goes in whenever the band does (the room file keeps
        // the two together); a strength alone leaves unset params unset.
        let (color, band) = match (edit.color, edit.band) {
            (None, None) => (None, None),
            (_, b) => (Some(next.0), b),
        };
        let a = Assignment {
            target: Target::Index(index),
            behavior,
            strength: next.2,
            color,
            band,
        };
        let lines = self.write(&a, &[index], boxes);
        self.commit(&lines);
        true
    }

    /// The class assignment from one surface: box `k`'s effective behavior
    /// and strength become its kind's default and every anchor of that
    /// kind's, saved and logged.
    pub fn assign_kind_of(&mut self, k: usize, boxes: &[LaneBox<'_>]) -> Result<(), String> {
        let (behavior, strength) = self
            .effective(k, boxes)
            .ok_or_else(|| format!("no box #{k} ({} boxes)", boxes.len()))?;
        self.assign(&Target::Kind(boxes[k].kind), behavior, strength, boxes)
    }

    /// Every kind default and every anchor of the room to none at full
    /// strength, in one save and one log line (the panel's "All: none"):
    /// the quiet room from which a single assignment can be judged.
    pub fn all_none(&mut self, boxes: &[LaneBox<'_>]) {
        let mut entries = 0;
        for (kind, _) in crate::surfaces::KIND_NAMES {
            let live = boxes
                .iter()
                .filter(|b| b.kind == kind && b.uuid != STAGE_FLOOR_UUID)
                .map(|b| b.uuid);
            entries += self
                .file
                .assign_kind(kind, SurfaceBehavior::None, 1.0, live);
        }
        // The stage floor too, under its own UUID, as the knob's `#k` does.
        if let Some(b) = boxes.iter().find(|b| b.uuid == STAGE_FLOOR_UUID) {
            self.file.assign(b.uuid, b.kind, SurfaceBehavior::None, 1.0);
            entries += 1;
        }
        let lines = [format!(
            "every surface ({} in the room, {entries} entries, every kind default) -> none@1.00",
            boxes.len()
        )];
        self.commit(&lines);
    }

    /// The class cycle from one surface: the step after box `k`'s
    /// effective behavior in its kind's catalogue
    /// ([`SurfaceBehavior::next_for`]) becomes its kind's default and every
    /// anchor of that kind's, at `k`'s strength, saved and logged. Returns
    /// the new behavior, `Err` past the boxes. Every hold then changes
    /// something the wearer can see, where re-applying the behavior the
    /// kind already ran did not (Kevin, worn, Sep 29).
    pub fn cycle_kind_of(
        &mut self,
        k: usize,
        boxes: &[LaneBox<'_>],
    ) -> Result<SurfaceBehavior, String> {
        let (behavior, strength) = self
            .effective(k, boxes)
            .ok_or_else(|| format!("no box #{k} ({} boxes)", boxes.len()))?;
        let next = behavior.next_for(boxes[k].kind);
        self.assign(&Target::Kind(boxes[k].kind), next, strength, boxes)?;
        Ok(next)
    }

    /// Advance box `k` one step through its kind's catalogue
    /// ([`SurfaceBehavior::next_for`]) from its effective behavior (the
    /// kind's default when it has no entry, not `none`), its strength kept
    /// (1 when unset); saved and logged. Returns the new behavior, `Err`
    /// past the boxes. A kind whose catalogue is only `none` (a ceiling)
    /// goes to `none`, which is still written.
    pub fn cycle(&mut self, k: usize, boxes: &[LaneBox<'_>]) -> Result<SurfaceBehavior, String> {
        let (behavior, strength) = self
            .effective(k, boxes)
            .ok_or_else(|| format!("no box #{k} ({} boxes)", boxes.len()))?;
        let next = behavior.next_for(boxes[k].kind);
        self.assign(&Target::Index(k), next, strength, boxes)?;
        Ok(next)
    }

    /// Write one assignment to the file for the boxes `hits` it resolved
    /// to; the log lines it makes (without the save suffix).
    fn write(&mut self, a: &Assignment, hits: &[usize], boxes: &[LaneBox<'_>]) -> Vec<String> {
        let params = match (a.color, a.band) {
            (Some(c), Some(b)) => format!(" color {c} band {b}"),
            (Some(c), None) => format!(" color {c}"),
            _ => String::new(),
        };
        let what = format!("{}@{:.2}{params}", a.behavior.name(), a.strength);
        if let Target::Kind(kind) = a.target {
            let live: Vec<[u8; 16]> = hits.iter().map(|&k| boxes[k].uuid).collect();
            let n = self
                .file
                .assign_kind(kind, a.behavior, a.strength, live.iter().copied());
            if let Some(color) = a.color {
                let of_kind: Vec<[u8; 16]> = self
                    .file
                    .anchors
                    .iter()
                    .filter(|e| e.kind == kind)
                    .map(|e| e.uuid)
                    .collect();
                for uuid in of_kind {
                    self.file.set_color(uuid, color, a.band);
                }
            }
            return vec![format!(
                "every {} ({} in the room, {n} entries, the kind default) -> {what}",
                kind_name(kind),
                hits.len()
            )];
        }
        hits.iter()
            .map(|&k| {
                let b = &boxes[k];
                self.file.assign(b.uuid, b.kind, a.behavior, a.strength);
                if let Some(color) = a.color {
                    self.file.set_color(b.uuid, color, a.band);
                }
                let (label, uuid8) = box_label(b);
                format!("{label} {uuid8} ({}) -> {what}", kind_name(b.kind))
            })
            .collect()
    }

    /// After a change: save the file, log `lines` with how the save went,
    /// and bump the revision so the next update rebuilds the rows.
    fn commit(&mut self, lines: &[String]) {
        let saved = self.save();
        for line in lines {
            info!("room {}: {line}{saved}", room_label(self.room));
        }
        self.revision += 1;
    }

    /// Save the room file; the log's suffix for how that went.
    fn save(&self) -> String {
        let Some(room) = self.room else {
            return ", not saved (no room)".to_owned();
        };
        let path = room_path(&self.config, room);
        match self.file.save(&path) {
            Ok(()) => format!(", saved rooms/{}.json", room_id_hex(room)),
            Err(e) => {
                warn!("room {}: saving {}: {e}", room_id_hex(room), path.display());
                format!(", NOT saved ({e})")
            }
        }
    }

    /// Load the current room's file, or the built-in defaults.
    fn load(&mut self) {
        self.revision += 1;
        self.file = RoomFile::default();
        let Some(room) = self.room else {
            info!("room -: no anchors, the built-in defaults");
            return;
        };
        let id = room_id_hex(room);
        match RoomFile::load(&room_path(&self.config, room)) {
            Loaded::Missing => info!("room {id}: no rooms/{id}.json, the built-in defaults"),
            Loaded::File(file, skipped) => {
                for s in &skipped {
                    warn!("room {id}: rooms/{id}.json: {s}");
                }
                info!(
                    "room {id}: loaded rooms/{id}.json ({} anchors assigned)",
                    file.anchors.len()
                );
                self.file = file;
            }
            Loaded::Failed(e) => warn!(
                "room {id}: rooms/{id}.json does not read ({e}); the built-in defaults, the file kept until the next change"
            ),
        }
    }

    /// The build key: the room, the assignments' revision and the boxes'
    /// identities in order.
    fn key_of(&self, boxes: &[LaneBox<'_>]) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut eat = |bytes: &[u8]| {
            for &b in bytes {
                h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        eat(&self.room.unwrap_or(0).to_le_bytes());
        eat(&self.revision.to_le_bytes());
        eat(&(boxes.len() as u64).to_le_bytes());
        for b in boxes {
            eat(&b.uuid);
            eat(&b.kind.to_le_bytes());
        }
        h
    }

    /// The lanes in one line: index, label, UUID prefix, kind, behavior
    /// and strength per box.
    fn table(&self, boxes: &[LaneBox<'_>]) -> String {
        let cells: Vec<String> = boxes
            .iter()
            .enumerate()
            .map(|(k, b)| {
                let (label, uuid8) = box_label(b);
                let (behavior, strength, _) = self.file.resolve(&b.uuid, b.kind);
                format!(
                    "{k} {label} {uuid8} {} {} {strength:.2}",
                    kind_name(b.kind),
                    behavior.name()
                )
            })
            .collect();
        let body = if cells.is_empty() {
            "no boxes".to_owned()
        } else {
            cells.join(" | ")
        };
        format!("room {}: {body}", room_label(self.room))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::room_file::{default_params, room_id};
    use crate::surfaces::{KIND_FLOOR, KIND_OTHER, KIND_TABLE, KIND_WALL};
    use SurfaceBehavior as B;

    fn uuid(n: u8) -> [u8; 16] {
        std::array::from_fn(|i| if i == 0 { 0x10 + n } else { 0xa0 + i as u8 })
    }

    /// A room: a desk, two walls, a floor, a couch, then the stage floor.
    fn room() -> Vec<LaneBox<'static>> {
        vec![
            LaneBox {
                uuid: uuid(1),
                kind: KIND_TABLE,
                label: "DESK",
            },
            LaneBox {
                uuid: uuid(2),
                kind: KIND_WALL,
                label: "WALL_FACE",
            },
            LaneBox {
                uuid: uuid(3),
                kind: KIND_WALL,
                label: "WALL_FACE",
            },
            LaneBox {
                uuid: uuid(4),
                kind: KIND_FLOOR,
                label: "FLOOR",
            },
            LaneBox {
                uuid: uuid(5),
                kind: KIND_OTHER,
                label: "COUCH",
            },
            LaneBox {
                uuid: STAGE_FLOOR_UUID,
                kind: KIND_FLOOR,
                label: "",
            },
        ]
    }

    fn room_of(boxes: &[LaneBox<'_>]) -> Option<u64> {
        room_id(boxes.iter().map(|b| b.uuid))
    }

    fn assign(target: Target, behavior: SurfaceBehavior, strength: f32) -> Assignment {
        Assignment {
            target,
            behavior,
            strength,
            color: None,
            band: None,
        }
    }

    #[test]
    fn the_knob_reads_every_target_form() {
        let full = uuid_hex(&uuid(1));
        assert_eq!(
            parse_knob(&format!("{full}=none")),
            Ok(Knob::Assign(vec![assign(
                Target::Uuid(full.clone()),
                B::None,
                1.0
            )]))
        );
        assert_eq!(
            parse_knob("11A1A2A3=Embers@0.5"),
            Ok(Knob::Assign(vec![assign(
                Target::Uuid("11a1a2a3".to_owned()),
                B::Embers,
                0.5
            )]))
        );
        // The ripple's old name is the rings.
        assert_eq!(
            parse_knob("#3=ripple"),
            Ok(Knob::Assign(vec![assign(Target::Index(3), B::Rings, 1.0)]))
        );
        assert_eq!(
            parse_knob("#3=rings,table=Streamlines@0.5"),
            Ok(Knob::Assign(vec![
                assign(Target::Index(3), B::Rings, 1.0),
                assign(Target::Kind(KIND_TABLE), B::Streamlines, 0.5),
            ]))
        );
        assert_eq!(
            parse_knob(" wall = spectrum @ 0 , table=sparks,#0=none@1 "),
            Ok(Knob::Assign(vec![
                assign(Target::Kind(KIND_WALL), B::Spectrum, 0.0),
                assign(Target::Kind(KIND_TABLE), B::Sparks, 1.0),
                assign(Target::Index(0), B::None, 1.0),
            ]))
        );
        assert_eq!(parse_knob("clear"), Ok(Knob::Clear));
        assert_eq!(parse_knob(" CLEAR "), Ok(Knob::Clear));
    }

    #[test]
    fn the_knob_takes_every_ported_effect_on_any_kind() {
        use crate::surfaces::{KIND_NAMES, kind_from_name};
        // Each of the eight by its `.pfx` name, on every kind, whatever
        // the kind's cycle list holds.
        for port in &crate::surface_port::PORTS {
            let b = B::from_name(port.name).expect("a port's name");
            for (kind, name) in KIND_NAMES {
                assert_eq!(
                    parse_knob(&format!("{name}={}@0.5", port.name)),
                    Ok(Knob::Assign(vec![assign(Target::Kind(kind), b, 0.5)]))
                );
            }
            assert_eq!(
                parse_knob(&format!("#2={}", port.name.to_uppercase())),
                Ok(Knob::Assign(vec![assign(Target::Index(2), b, 1.0)]))
            );
        }
        // The sweep's two values: every kind on a ported effect of its own
        // list, all eight between them, each within the 91 bytes a system
        // property's value holds.
        let sweeps = [
            "table=shards,floor=tessera,wall=aurora,other=reticle,frame=bezel,ceiling=astrolabe",
            "table=prism,floor=astrolabe,wall=fenestra,other=tessera,frame=reticle,ceiling=aurora",
        ];
        let mut seen = Vec::new();
        for value in sweeps {
            assert!(value.len() <= 91, "{} bytes", value.len());
            let Ok(Knob::Assign(list)) = parse_knob(value) else {
                panic!("{value}");
            };
            assert_eq!(list.len(), KIND_NAMES.len());
            for a in list {
                let Target::Kind(kind) = a.target else {
                    panic!("{value}");
                };
                assert!(B::catalogue(kind).contains(&a.behavior), "{value}");
                assert!(a.behavior.port().is_some());
                seen.push(a.behavior);
            }
        }
        for port in &crate::surface_port::PORTS {
            assert!(seen.contains(&B::from_name(port.name).unwrap()));
        }
        assert_eq!(
            kind_from_name("ceiling"),
            Some(crate::surfaces::KIND_CEILING)
        );
        // An unknown name still refuses the value and lists the catalogue.
        let e = parse_knob("table=auroras").unwrap_err();
        assert!(
            e.contains("unknown behavior") && e.contains("tessera"),
            "{e}"
        );
    }

    #[test]
    fn a_bad_knob_value_is_refused_whole() {
        for bad in [
            "",
            "   ",
            "table",
            "table=",
            "table=glitter",
            "wall=drips",
            "wall=pulses",
            "table=embers@1.5",
            "table=embers@-0.1",
            "table=embers@NaN",
            "table=embers@",
            "1234567=embers",
            "11a1a2a3zz=embers",
            &"a".repeat(33),
            "#x=none",
            "#-1=none",
            "sofa=none",
            "table=embers,",
            "table=embers,clear",
            "clear,table=embers",
        ] {
            assert!(parse_knob(bad).is_err(), "'{bad}' parsed");
        }
        // D2's names read.
        assert_eq!(
            parse_knob("floor=curls,frame=Pulse@0.5"),
            Ok(Knob::Assign(vec![
                assign(Target::Kind(crate::surfaces::KIND_FLOOR), B::Curls, 1.0),
                assign(Target::Kind(crate::surfaces::KIND_FRAME), B::Pulse, 0.5),
            ]))
        );
    }

    #[test]
    fn the_knob_takes_a_color_and_a_band() {
        let with = |target, behavior, strength, color, band| Assignment {
            color,
            band,
            ..assign(target, behavior, strength)
        };
        assert_eq!(
            parse_knob("table=curls:5:2"),
            Ok(Knob::Assign(vec![with(
                Target::Kind(KIND_TABLE),
                B::Curls,
                1.0,
                Some(5),
                Some(2)
            )]))
        );
        assert_eq!(
            parse_knob(" #3 = pulse : 8 @0.5, wall=spectrum:0:0,floor=rings "),
            Ok(Knob::Assign(vec![
                with(Target::Index(3), B::Pulse, 0.5, Some(8), None),
                with(Target::Kind(KIND_WALL), B::Spectrum, 1.0, Some(0), Some(0)),
                assign(Target::Kind(KIND_FLOOR), B::Rings, 1.0),
            ]))
        );
        // Out of range, not whole, empty, too many: refused whole.
        for bad in [
            "table=curls:9",
            "table=curls:5:4",
            "table=curls:-1",
            "table=curls:1.5",
            "table=curls:5:2.0",
            "table=curls:",
            "table=curls::2",
            "table=curls:5:2:1",
            "table=curls:teal",
            "table=curls:5:2,wall=pulse:12",
        ] {
            assert!(parse_knob(bad).is_err(), "'{bad}' parsed");
        }
        assert!(parse_knob("table=curls:9").unwrap_err().contains("0 to 8"));
        // Applied: the entries carry the params, the rows their color and
        // band; a refused value changes nothing.
        let dir = std::env::temp_dir().join(format!("fosfora-lanes-params-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let boxes = room();
        let id = room_of(&boxes);
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        // Unset: the kind's color and the behavior's band.
        assert_close!(lanes.rows()[0][2..], [0.0, 0.0]);
        assert_close!(lanes.rows()[3][2..], [0.0, 1.0]);
        assert!(lanes.poll_knob(Some("table=curls:5:2,#3=rings:7"), false, &boxes));
        lanes.update(id, &boxes);
        assert_close!(lanes.rows()[0], lane_row(B::Curls, 1.0, [5.0, 2.0]));
        // A color alone keeps the band: the rings' bass.
        assert_close!(lanes.rows()[3], lane_row(B::Rings, 1.0, [7.0, 1.0]));
        assert_eq!(
            crate::surfaces::lane_params(lanes.rows()[0], B::Curls),
            (5, 2)
        );
        // A cycle keeps them; the next knob value without a suffix too.
        assert_eq!(lanes.cycle(0, &boxes), Ok(B::Pulse));
        assert!(lanes.poll_knob(Some("#3=streamlines@0.5"), false, &boxes));
        lanes.update(id, &boxes);
        assert_close!(lanes.rows()[0][2..], [5.0, 2.0]);
        assert_close!(lanes.rows()[3], lane_row(B::Streamlines, 0.5, [7.0, 1.0]));
        let path = room_path(&dir, id.unwrap());
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(!lanes.poll_knob(Some("table=curls:5:9"), false, &boxes));
        assert!(!lanes.update(id, &boxes));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        // A relaunch reads them back.
        let mut again = RoomLanes::new(dir.clone());
        again.update(id, &boxes);
        assert_close!(again.rows()[0][2..], [5.0, 2.0]);
        assert_close!(again.rows()[3][2..], [7.0, 1.0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fresh room's lanes over `room()` in a temporary config dir named
    /// for `what`.
    fn fresh(what: &str) -> (PathBuf, Vec<LaneBox<'static>>, Option<u64>, RoomLanes) {
        let dir = std::env::temp_dir().join(format!("fosfora-lanes-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let boxes = room();
        let id = room_of(&boxes);
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        (dir, boxes, id, lanes)
    }

    fn color(c: u32) -> ParamEdit {
        ParamEdit {
            color: Some(c),
            ..ParamEdit::default()
        }
    }

    fn band(b: u32) -> ParamEdit {
        ParamEdit {
            band: Some(b),
            ..ParamEdit::default()
        }
    }

    fn strength(s: f32) -> ParamEdit {
        ParamEdit {
            strength: Some(s),
            ..ParamEdit::default()
        }
    }

    #[test]
    fn the_panels_writer_creates_the_entry_on_an_unset_surface_and_keeps_the_behavior() {
        let (dir, boxes, id, mut lanes) = fresh("panel");
        // The desk runs its kind's default with no entry.
        assert!(lanes.file.entry(&uuid(1)).is_none());
        assert_eq!(lanes.params_of(0, &boxes), (0, 0, 1.0));
        assert!(lanes.set_params(0, &boxes, color(4)));
        let entry = *lanes
            .file
            .entry(&uuid(1))
            .expect("the entry the writer made");
        assert_eq!(
            (entry.behavior, entry.strength, entry.params),
            (B::Streamlines, 1.0, Some([4.0, 0.0]))
        );
        assert_eq!(lanes.effective(0, &boxes), Some((B::Streamlines, 1.0)));
        assert_eq!(lanes.params_of(0, &boxes), (4, 0, 1.0));
        // The band, then the strength: each kept by the next.
        assert!(lanes.set_params(0, &boxes, band(2)));
        assert!(lanes.set_params(0, &boxes, strength(0.7)));
        let (c, b, s) = lanes.params_of(0, &boxes);
        assert_eq!((c, b), (4, 2));
        assert_close!(s, 0.7);
        // The rows follow on the next update: what the pass reads.
        assert!(lanes.update(id, &boxes));
        assert_close!(lanes.rows()[0], lane_row(B::Streamlines, 0.7, [4.0, 2.0]));
        // A strength alone on an unset surface: its entry, its params still
        // unset (the behavior's default band, the bass for the rings).
        assert!(lanes.set_params(3, &boxes, strength(0.5)));
        let floor = *lanes.file.entry(&uuid(4)).expect("an entry");
        assert_eq!((floor.behavior, floor.params), (B::Rings, None));
        assert_close!(lanes.params_of(3, &boxes).2, 0.5);
        assert_eq!(lanes.params_of(3, &boxes).0, 0);
        assert_eq!(lanes.params_of(3, &boxes).1, B::Rings.default_band());
        // A band alone takes the color the surface shows with it.
        assert!(lanes.set_params(3, &boxes, band(3)));
        assert_eq!(lanes.file.entry(&uuid(4)).unwrap().params, Some([0.0, 3.0]));
        // The stage floor, under its zero UUID, as the knob's `#k`.
        assert!(lanes.set_params(5, &boxes, color(8)));
        assert_eq!(lanes.params_of(5, &boxes).0, 8);
        // Stored at one decimal.
        assert!(lanes.set_params(4, &boxes, strength(0.333)));
        assert_close!(lanes.params_of(4, &boxes).2, 0.3);
        // An edit to the values it has changes nothing and writes nothing.
        let before = lanes.revision;
        assert!(!lanes.set_params(0, &boxes, color(4)));
        assert!(!lanes.set_params(0, &boxes, ParamEdit::default()));
        assert_eq!(lanes.revision, before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_panel_edit_saves_and_the_room_reads_back_on_relaunch() {
        let (dir, boxes, id, mut lanes) = fresh("panel-save");
        assert!(lanes.set_params(0, &boxes, color(4)));
        assert!(lanes.set_params(0, &boxes, band(2)));
        assert!(lanes.set_params(0, &boxes, strength(0.5)));
        assert!(lanes.set_params(1, &boxes, band(1)));
        let path = room_path(&dir, id.unwrap());
        assert!(path.is_file(), "saved");
        let mut again = RoomLanes::new(dir.clone());
        again.update(id, &boxes);
        assert_eq!(again.params_of(0, &boxes), (4, 2, 0.5));
        assert_eq!(again.params_of(1, &boxes), (0, 1, 1.0));
        assert_eq!(again.behavior(0), B::Streamlines);
        assert_eq!(again.behavior(1), B::Spectrum);
        assert_close!(again.rows()[0], lane_row(B::Streamlines, 0.5, [4.0, 2.0]));
        // A tap's cycle keeps them, as it keeps the knob's.
        assert_eq!(again.cycle(0, &boxes), Ok(B::Curls));
        assert_eq!(again.params_of(0, &boxes), (4, 2, 0.5));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_out_of_range_edit_is_refused_whole_and_the_file_is_unchanged() {
        let (dir, boxes, id, mut lanes) = fresh("panel-bad");
        assert!(lanes.set_params(0, &boxes, color(2)));
        assert!(lanes.update(id, &boxes));
        let path = room_path(&dir, id.unwrap());
        let before = std::fs::read_to_string(&path).unwrap();
        let file = lanes.file.clone();
        let revision = lanes.revision;
        for (k, bad) in [
            (0, color(COLOR_KEY + 1)),
            (0, band(BANDS)),
            (0, strength(1.5)),
            (0, strength(-0.1)),
            (0, strength(f32::NAN)),
            // One field out of range refuses the others with it.
            (
                0,
                ParamEdit {
                    color: Some(5),
                    band: Some(9),
                    strength: Some(0.5),
                },
            ),
            (
                1,
                ParamEdit {
                    color: Some(3),
                    band: None,
                    strength: Some(2.0),
                },
            ),
            // Past the boxes.
            (6, color(1)),
        ] {
            assert!(!lanes.set_params(k, &boxes, bad), "#{k} {bad:?}");
        }
        assert_eq!(lanes.file, file);
        assert_eq!(lanes.revision, revision);
        assert!(!lanes.update(id, &boxes));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn params_of_reports_what_the_knob_wrote() {
        let (dir, boxes, _, mut lanes) = fresh("panel-knob");
        // Unset: the kind's color and the behavior's band at full strength.
        assert_eq!(lanes.params_of(0, &boxes), (0, 0, 1.0), "the desk");
        assert_eq!(lanes.params_of(3, &boxes), (0, 1, 1.0), "the floor's rings");
        assert!(lanes.poll_knob(
            Some("table=curls:5:2,#3=rings:7@0.5,#1=pulse@0.25"),
            false,
            &boxes
        ));
        assert_eq!(lanes.params_of(0, &boxes), (5, 2, 1.0));
        assert_eq!(lanes.params_of(3, &boxes), (7, 1, 0.5));
        // An entry with no params: its behavior's default band.
        assert_eq!(lanes.params_of(1, &boxes), (0, 1, 0.25));
        // Past the boxes: the defaults.
        assert_eq!(lanes.params_of(9, &boxes), (0, 0, 1.0));
        // The panel's write lands where the knob's does: the same fields.
        assert!(lanes.set_params(3, &boxes, color(5)));
        assert_eq!(
            lanes.file.entry(&uuid(4)).map(|e| (e.behavior, e.params)),
            Some((B::Rings, Some([5.0, 1.0])))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_steppers_wrap_the_color_and_the_band_and_clamp_the_strength() {
        // The color: 0..=8 round, both ways.
        let mut c = 0;
        let mut seen = Vec::new();
        for _ in 0..=COLOR_KEY {
            c = next_color(c, true);
            seen.push(c);
        }
        assert_eq!(seen, [1, 2, 3, 4, 5, 6, 7, 8, 0]);
        assert_eq!(next_color(0, false), COLOR_KEY);
        assert_eq!(next_color(COLOR_KEY, true), 0);
        assert_eq!(next_color(5, false), 4);
        // The band: rms, bass, mid, high, round.
        assert_eq!([0, 1, 2, 3].map(|b| next_band(b, true)), [1, 2, 3, 0]);
        assert_eq!([0, 1, 2, 3].map(|b| next_band(b, false)), [3, 0, 1, 2]);
        // Out of range in: taken as the last.
        assert_eq!(next_band(7, true), 0);
        assert_eq!(next_color(20, false), COLOR_KEY - 1);
        // The strength: tenths, no wrap.
        assert_close!(next_strength(0.7, true), 0.8);
        assert_close!(next_strength(0.7, false), 0.6);
        assert_close!(next_strength(1.0, true), 1.0);
        assert_close!(next_strength(0.0, false), 0.0);
        assert_close!(next_strength(0.95, true), 1.0);
        assert_close!(next_strength(0.04, false), 0.0);
        // From full to nothing in ten steps and back, every value a tenth.
        let mut s = 1.0;
        for k in (0..10).rev() {
            s = next_strength(s, false);
            assert_close!(s, k as f32 / 10.0);
            assert_close!(s, round_strength(s));
        }
        for k in 1..=10 {
            s = next_strength(s, true);
            assert_close!(s, k as f32 / 10.0);
        }
        // One step of each from a surface's values.
        let now = (8, 3, 0.4);
        assert_eq!(ParamEdit::step(Param::Color, true, now), color(0));
        assert_eq!(ParamEdit::step(Param::Band, true, now), band(0));
        let down = ParamEdit::step(Param::Strength, false, now);
        assert_eq!((down.color, down.band), (None, None));
        assert_close!(down.strength.unwrap(), 0.3);
    }

    #[test]
    fn targets_resolve_against_the_frames_boxes() {
        let boxes = room();
        let hex = |n| uuid_hex(&uuid(n));
        assert_eq!(resolve(&Target::Uuid(hex(3)), &boxes), Ok(vec![2]));
        // A prefix: the first 8 digits tell the test UUIDs apart.
        assert_eq!(
            resolve(&Target::Uuid(hex(4)[..8].to_owned()), &boxes),
            Ok(vec![3])
        );
        // Not a prefix of any.
        assert!(resolve(&Target::Uuid(hex(1)[2..10].to_owned()), &boxes).is_err());
        // Two walls whose UUIDs differ only in the last byte: their
        // shared prefix names neither.
        let mut twins = boxes.clone();
        twins[2].uuid = twins[1].uuid;
        twins[2].uuid[15] ^= 1;
        let prefix = hex(2)[..8].to_owned();
        assert!(
            resolve(&Target::Uuid(prefix), &twins)
                .unwrap_err()
                .contains("2 anchors")
        );
        assert_eq!(
            resolve(&Target::Uuid(uuid_hex(&twins[2].uuid)), &twins),
            Ok(vec![2])
        );
        // The stage floor has no UUID to name it by, only its index.
        assert!(resolve(&Target::Uuid("00000000".to_owned()), &boxes).is_err());
        assert_eq!(resolve(&Target::Index(5), &boxes), Ok(vec![5]));
        assert!(resolve(&Target::Index(6), &boxes).is_err());
        assert_eq!(resolve(&Target::Kind(KIND_WALL), &boxes), Ok(vec![1, 2]));
        assert_eq!(resolve(&Target::Kind(KIND_FLOOR), &boxes), Ok(vec![3, 5]));
        assert_eq!(
            resolve(&Target::Kind(crate::surfaces::KIND_CEILING), &boxes),
            Ok(vec![])
        );
    }

    #[test]
    fn without_a_file_the_lanes_are_the_kind_defaults() {
        let boxes = room();
        let (rows, behaviors) = lane_rows(&RoomFile::default(), &boxes);
        // The mockup's room (board #3488): the desk on the streamlines, the
        // walls on the spectrum, the floors, scene and stage, on the rings
        // (step 2d), the couch on the curls.
        let expected = [
            B::Streamlines,
            B::Spectrum,
            B::Spectrum,
            B::Rings,
            B::Curls,
            B::Rings,
        ];
        assert_eq!(&behaviors[..6], &expected);
        for (row, b) in rows.iter().zip(expected) {
            assert_close!(*row, lane_row(b, 1.0, default_params(b)));
            // What the sim reads back is what the lanes meant.
            assert_eq!(crate::surfaces::lane_behavior(*row, KIND_TABLE), b);
        }
        // Past the boxes: unset.
        assert_close!(rows[6..], [[0.0; 4]; SURFACE_LANE_ROWS - 6]);
    }

    #[test]
    fn the_knob_assigns_saves_and_the_room_comes_back() {
        let dir = std::env::temp_dir().join(format!("fosfora-lanes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let boxes = room();
        let id = room_of(&boxes);
        let mut lanes = RoomLanes::new(dir.clone());
        assert!(lanes.update(id, &boxes));
        // Steady: no rebuild.
        assert!(!lanes.update(id, &boxes));
        assert_eq!(lanes.behavior(0), B::Streamlines);
        // While the room has no anchors yet, the knob waits.
        let desk = uuid_hex(&uuid(1))[..8].to_owned();
        let value = format!("{desk}=none,#5=ripple@0.5,wall=none");
        assert!(!lanes.poll_knob(Some(&value), true, &[]));
        assert!(lanes.poll_knob(Some(&value), false, &boxes));
        assert!(lanes.update(id, &boxes));
        assert_eq!(lanes.behavior(0), B::None);
        assert_eq!(lanes.behavior(1), B::None);
        assert_eq!(lanes.behavior(2), B::None);
        assert_eq!(lanes.behavior(3), B::Rings, "the floor's default");
        assert_eq!(lanes.behavior(5), B::Rings);
        assert_close!(
            lanes.rows()[5],
            lane_row(B::Rings, 0.5, default_params(B::Rings))
        );
        // The same value again applies nothing.
        assert!(!lanes.poll_knob(Some(&value), false, &boxes));
        let path = room_path(&dir, id.unwrap());
        assert!(path.is_file(), "saved");
        // A relaunch: the room reads back as left, the boxes in another
        // order.
        let mut shuffled = boxes.clone();
        shuffled.swap(0, 3);
        let mut again = RoomLanes::new(dir.clone());
        assert!(again.update(room_of(&shuffled), &shuffled));
        assert_eq!(again.behavior(3), B::None, "the desk");
        assert_eq!(again.behavior(0), B::Rings, "the floor");
        assert_eq!(again.behavior(5), B::Rings, "the stage floor");
        // A bad value changes nothing, in memory or on disk.
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(!again.poll_knob(Some("table=embers,#9=none"), false, &shuffled));
        assert!(!again.update(room_of(&shuffled), &shuffled));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        // Clear: the built-in defaults, saved.
        assert!(again.poll_knob(Some("clear"), false, &shuffled));
        assert!(again.update(room_of(&shuffled), &shuffled));
        assert_eq!(again.behavior(3), B::Streamlines);
        match RoomFile::load(&path) {
            Loaded::File(f, _) => assert_eq!(f, RoomFile::default()),
            other => panic!("{other:?}"),
        }
        // Unset and set again: applies again.
        assert!(!again.poll_knob(None, false, &shuffled));
        assert!(again.poll_knob(Some("clear"), false, &shuffled));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cycle_steps_through_the_catalogue_and_wraps() {
        assert_eq!(B::None.next(), B::Embers);
        assert_eq!(B::Embers.next(), B::Sparks);
        assert_eq!(B::Sparks.next(), B::Spectrum);
        assert_eq!(B::Spectrum.next(), B::Rings);
        assert_eq!(B::Rings.next(), B::Streamlines);
        assert_eq!(B::Streamlines.next(), B::Curls);
        assert_eq!(B::Curls.next(), B::Pulse);
        assert_eq!(B::Pulse.next(), B::Aurora);
        assert_eq!(B::Tessera.next(), B::None);
        let dir = std::env::temp_dir().join(format!("fosfora-lanes-cycle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let boxes = room();
        let id = room_of(&boxes);
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        // The couch (other, default the curls): eight pinches go round
        // its kind's catalogue once, the ported effects after its own.
        let mut seen = Vec::new();
        for _ in 0..8 {
            seen.push(lanes.cycle(4, &boxes).unwrap());
        }
        assert_eq!(
            seen,
            [
                B::Streamlines,
                B::Pulse,
                B::Embers,
                B::Shards,
                B::Reticle,
                B::Tessera,
                B::None,
                B::Curls
            ]
        );
        lanes.update(id, &boxes);
        assert_eq!(lanes.behavior(4), B::Curls);
        // Past the boxes: refused, nothing written.
        assert!(lanes.cycle(6, &boxes).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cycle_on_an_unset_box_starts_from_its_default_and_keeps_the_strength() {
        let dir = std::env::temp_dir().join(format!("fosfora-lanes-unset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let boxes = room();
        let id = room_of(&boxes);
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        // The desk runs the streamlines with no entry: its first pinch is
        // the curls, not the streamlines (the step after none).
        assert_eq!(lanes.effective(0, &boxes), Some((B::Streamlines, 1.0)));
        assert_eq!(lanes.cycle(0, &boxes), Ok(B::Curls));
        // A wall on the spectrum by default goes to the streamlines (a
        // wall's catalogue is none, the spectrum, the streamlines, the
        // rings, the pulse).
        assert_eq!(lanes.cycle(1, &boxes), Ok(B::Streamlines));
        // A strength set by the knob is kept across a cycle.
        assert!(lanes.poll_knob(Some("#3=sparks@0.4"), false, &boxes));
        assert_eq!(lanes.cycle(3, &boxes), Ok(B::Tessera));
        assert_eq!(lanes.cycle(3, &boxes), Ok(B::Prism));
        assert_eq!(lanes.cycle(3, &boxes), Ok(B::Astrolabe));
        assert_eq!(lanes.cycle(3, &boxes), Ok(B::None));
        assert_eq!(lanes.cycle(3, &boxes), Ok(B::Rings));
        assert_eq!(lanes.effective(3, &boxes), Some((B::Rings, 0.4)));
        lanes.update(id, &boxes);
        assert_close!(
            lanes.rows()[3],
            lane_row(B::Rings, 0.4, default_params(B::Rings))
        );
        assert_eq!(lanes.behavior(0), B::Curls);
        assert_eq!(lanes.behavior(1), B::Streamlines);
        // Saved: a relaunch finds them.
        let mut again = RoomLanes::new(dir.clone());
        again.update(id, &boxes);
        assert_eq!(again.behavior(0), B::Curls);
        assert_eq!(again.behavior(3), B::Rings);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn all_none_quiets_every_surface_and_every_kind_default() {
        let dir =
            std::env::temp_dir().join(format!("fosfora-lanes-allnone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let boxes = room();
        let id = room_of(&boxes);
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        lanes
            .assign(&Target::Index(0), B::Sparks, 0.5, &boxes)
            .unwrap();
        lanes.all_none(&boxes);
        for k in 0..boxes.len() {
            assert_eq!(lanes.effective(k, &boxes), Some((B::None, 1.0)), "box {k}");
        }
        for (kind, _) in crate::surfaces::KIND_NAMES {
            assert_eq!(lanes.file.kind_default(kind), B::None);
        }
        // Saved: a fresh load sees the same.
        let mut again = RoomLanes::new(dir.clone());
        again.update(id, &boxes);
        assert_eq!(again.effective(0, &boxes), Some((B::None, 1.0)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_class_assignment_takes_the_pointed_surfaces_behavior() {
        let dir = std::env::temp_dir().join(format!("fosfora-lanes-kind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let boxes = room();
        let id = room_of(&boxes);
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        // The first wall to none at 0.5, then to every wall.
        lanes
            .assign(&Target::Index(1), B::None, 0.5, &boxes)
            .unwrap();
        lanes.assign_kind_of(1, &boxes).unwrap();
        assert_eq!(lanes.effective(2, &boxes), Some((B::None, 0.5)));
        // The kind default too: a wall the file does not know runs none.
        let mut more = boxes.clone();
        more.insert(
            5,
            LaneBox {
                uuid: uuid(6),
                kind: KIND_WALL,
                label: "WALL_FACE",
            },
        );
        assert_eq!(lanes.effective(5, &more).map(|e| e.0), Some(B::None));
        // The stage floor's class is the floor kind: the scene floor too.
        lanes
            .assign(&Target::Index(5), B::Rings, 1.0, &boxes)
            .unwrap();
        lanes.assign_kind_of(5, &boxes).unwrap();
        assert_eq!(lanes.effective(3, &boxes), Some((B::Rings, 1.0)));
        assert!(lanes.assign_kind_of(9, &boxes).is_err());
        // The class cycle: one step past the surface, for the whole kind,
        // so a hold always changes something.
        let before = lanes.effective(1, &boxes).unwrap().0;
        let next = lanes.cycle_kind_of(1, &boxes).unwrap();
        assert_eq!(next, before.next_for(boxes[1].kind));
        assert_eq!((before, next), (B::None, B::Spectrum));
        for (k, b) in boxes.iter().enumerate() {
            if b.kind == boxes[1].kind {
                assert_eq!(lanes.effective(k, &boxes).unwrap().0, next, "box {k}");
            }
        }
        assert_eq!(lanes.file.kind_default(boxes[1].kind), next);
        assert!(lanes.cycle_kind_of(9, &boxes).is_err());
        assert!(
            lanes
                .assign(&Target::Index(9), B::None, 1.0, &boxes)
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cycle_follows_the_kind_and_a_ceiling_gets_the_rings() {
        use crate::surfaces::KIND_CEILING;
        let dir = std::env::temp_dir().join(format!("fosfora-lanes-kinds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut boxes = room();
        boxes.insert(
            5,
            LaneBox {
                uuid: uuid(6),
                kind: KIND_CEILING,
                label: "CEILING",
            },
        );
        let id = room_of(&boxes);
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        // The ceiling runs none: a pinch puts the rings on it (D1), a
        // hold the pulse on every ceiling.
        assert_eq!(lanes.cycle(5, &boxes), Ok(B::Rings));
        assert_eq!(lanes.cycle_kind_of(5, &boxes), Ok(B::Pulse));
        // Put on embers by the knob, it cycles to the first after none.
        assert!(lanes.poll_knob(Some("#5=embers"), false, &boxes));
        assert_eq!(lanes.cycle(5, &boxes), Ok(B::Rings));
        // The floor: the rings by default (step 2d), then the
        // streamlines, the curls, sparks, its three ported effects, none,
        // the rings.
        let floor: Vec<_> = (0..8).map(|_| lanes.cycle(3, &boxes).unwrap()).collect();
        assert_eq!(
            floor,
            [
                B::Streamlines,
                B::Curls,
                B::Sparks,
                B::Tessera,
                B::Prism,
                B::Astrolabe,
                B::None,
                B::Rings
            ]
        );
        // A table the knob put on the spectrum: the first after none.
        assert!(lanes.poll_knob(Some("#0=spectrum"), false, &boxes));
        assert_eq!(lanes.cycle(0, &boxes), Ok(B::Streamlines));
        // The class cycle from a wall: the streamlines, then the rings,
        // for both walls.
        assert_eq!(lanes.cycle_kind_of(1, &boxes), Ok(B::Streamlines));
        assert_eq!(lanes.cycle_kind_of(2, &boxes), Ok(B::Rings));
        assert_eq!(lanes.effective(1, &boxes).unwrap().0, B::Rings);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn another_room_loads_its_own_file_and_a_bad_file_is_kept() {
        let dir = std::env::temp_dir().join(format!("fosfora-lanes-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let boxes = room();
        let id = room_of(&boxes).unwrap();
        std::fs::create_dir_all(dir.join("rooms")).unwrap();
        std::fs::write(room_path(&dir, id), "{ broken").unwrap();
        let mut lanes = RoomLanes::new(dir.clone());
        assert!(lanes.update(Some(id), &boxes));
        assert_eq!(lanes.behavior(0), B::Streamlines, "the defaults");
        assert_eq!(
            std::fs::read_to_string(room_path(&dir, id)).unwrap(),
            "{ broken",
            "not overwritten on load"
        );
        // Another room (one anchor fewer): its own, missing, file.
        let smaller = &boxes[1..];
        assert!(lanes.update(room_of(smaller), smaller));
        assert!(lanes.poll_knob(Some("#0=none"), false, smaller));
        assert!(lanes.update(room_of(smaller), smaller));
        assert_eq!(lanes.behavior(0), B::None);
        assert!(room_path(&dir, room_of(smaller).unwrap()).is_file());
        // Back in the first room: its file is still the broken one, and
        // the wall there keeps its default.
        assert!(lanes.update(Some(id), &boxes));
        assert_eq!(lanes.behavior(1), B::Spectrum);
        // No room at all: assignments hold in memory, nothing saved.
        let stage = &boxes[5..];
        assert!(lanes.update(None, stage));
        assert!(lanes.poll_knob(Some("floor=ripple"), false, stage));
        assert!(lanes.update(None, stage));
        assert_eq!(lanes.behavior(0), B::Rings);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
