//! The surface lanes (board #3326): each obstacle box's behavior, from the
//! room file and the boxes of the frame, as the rows the Flux sim reads
//! after the pour row (`surfaces::lane_row`) and the behavior per box the
//! weights, the wall spectrum and the surfaces pass follow. Plain data, so
//! it builds and tests on the desktop; `app.rs` feeds it the frame's
//! boxes, the `debug.fosfora.surface` knob and the room editor's pinches
//! (`room_edit.rs`), which go through the same typed calls the knob does
//! ([`RoomLanes::assign`], [`RoomLanes::assign_kind_of`],
//! [`RoomLanes::cycle`]): one path that writes, saves and logs.
//!
//! The rows are rebuilt only when the box list (UUIDs and kinds, in order)
//! or an assignment changes: the boxes relocate once a second but keep
//! their identity, so a steady room costs one hash of its box list per
//! frame. The room file is loaded when the room id changes (the query
//! returned another anchor set) and saved on every change, never on load;
//! one that does not read is left alone until the next change.
//!
//! The knob, comma-separated assignments `<target>=<behavior>[@<strength>]`
//! ([`parse_knob`]): the target is a UUID (32 hex, or a prefix of at least
//! 8 unique among the room's anchors), a box index `#<k>` (this frame's
//! order, the stage floor too) or a kind name (the class assignment: the
//! kind's default and every anchor of that kind); the behavior one of
//! `none embers sparks spectrum rings streamlines` (`ripple` reads as
//! `rings`); the strength 0..1, 1 by default.
//! `clear` alone drops every entry and puts the kind defaults back. A bad
//! value applies nothing.

use std::path::PathBuf;

use log::{info, warn};

use crate::room_file::{Loaded, RoomFile, STAGE_FLOOR_UUID, room_id_hex, room_path, uuid_hex};
use crate::surfaces::{
    RESERVED_BEHAVIORS, SURFACE_LANE_ROWS, SurfaceBehavior, kind_from_name, kind_name, lane_row,
};

/// The shortest UUID prefix the knob takes.
pub const MIN_UUID_PREFIX: usize = 8;

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

/// One `<target>=<behavior>[@<strength>]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub target: Target,
    pub behavior: SurfaceBehavior,
    pub strength: f32,
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
    let (target, rhs) = part
        .split_once('=')
        .ok_or_else(|| format!("'{part}': expected <target>=<behavior>[@<strength>]"))?;
    let (behavior, strength) = match rhs.split_once('@') {
        Some((b, s)) => (b, Some(s)),
        None => (rhs, None),
    };
    let target = parse_target(target.trim())?;
    let behavior = behavior.trim();
    let behavior = SurfaceBehavior::from_name(behavior).ok_or_else(|| {
        if RESERVED_BEHAVIORS
            .iter()
            .any(|r| r.eq_ignore_ascii_case(behavior))
        {
            format!("'{behavior}' is reserved for step D2")
        } else {
            format!("unknown behavior '{behavior}' (none embers sparks spectrum rings streamlines)")
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
    })
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
        let what = format!("{}@{:.2}", a.behavior.name(), a.strength);
        if let Target::Kind(kind) = a.target {
            let live = hits.iter().map(|&k| boxes[k].uuid);
            let n = self.file.assign_kind(kind, a.behavior, a.strength, live);
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
    use crate::room_file::room_id;
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
    fn a_bad_knob_value_is_refused_whole() {
        for bad in [
            "",
            "   ",
            "table",
            "table=",
            "table=glitter",
            "wall=drips",
            "wall=pulse",
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
        assert!(
            parse_knob("floor=curls").unwrap_err().contains("reserved"),
            "the reserved names say so"
        );
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
        // The floors, scene and stage, on the rings (step 2d).
        let expected = [
            B::Embers,
            B::Spectrum,
            B::Spectrum,
            B::Rings,
            B::None,
            B::Rings,
        ];
        assert_eq!(&behaviors[..6], &expected);
        for (row, b) in rows.iter().zip(expected) {
            assert_close!(*row, lane_row(b, 1.0, [0.0; 2]));
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
        assert_eq!(lanes.behavior(0), B::Embers);
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
        assert_close!(lanes.rows()[5], lane_row(B::Rings, 0.5, [0.0; 2]));
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
        assert_eq!(again.behavior(3), B::Embers);
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
        assert_eq!(B::Streamlines.next(), B::None);
        let dir = std::env::temp_dir().join(format!("fosfora-lanes-cycle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let boxes = room();
        let id = room_of(&boxes);
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        // The couch (other, default none): four pinches go round its
        // kind's catalogue once.
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(lanes.cycle(4, &boxes).unwrap());
        }
        assert_eq!(seen, [B::Embers, B::Sparks, B::Streamlines, B::None]);
        lanes.update(id, &boxes);
        assert_eq!(lanes.behavior(4), B::None);
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
        // The desk runs embers with no entry: its first pinch is sparks,
        // not embers (the step after none).
        assert_eq!(lanes.effective(0, &boxes), Some((B::Embers, 1.0)));
        assert_eq!(lanes.cycle(0, &boxes), Ok(B::Sparks));
        // A wall on the spectrum by default goes to the streamlines (a
        // wall's catalogue is none, the spectrum, the streamlines, the
        // rings).
        assert_eq!(lanes.cycle(1, &boxes), Ok(B::Streamlines));
        // A strength set by the knob is kept across a cycle.
        assert!(lanes.poll_knob(Some("#3=sparks@0.4"), false, &boxes));
        assert_eq!(lanes.cycle(3, &boxes), Ok(B::Rings));
        assert_eq!(lanes.effective(3, &boxes), Some((B::Rings, 0.4)));
        lanes.update(id, &boxes);
        assert_close!(lanes.rows()[3], lane_row(B::Rings, 0.4, [0.0; 2]));
        assert_eq!(lanes.behavior(0), B::Sparks);
        assert_eq!(lanes.behavior(1), B::Streamlines);
        // Saved: a relaunch finds them.
        let mut again = RoomLanes::new(dir.clone());
        again.update(id, &boxes);
        assert_eq!(again.behavior(0), B::Sparks);
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
        // hold the streamlines on every ceiling.
        assert_eq!(lanes.cycle(5, &boxes), Ok(B::Rings));
        assert_eq!(lanes.cycle_kind_of(5, &boxes), Ok(B::Streamlines));
        // Put on embers by the knob, it cycles to the first after none.
        assert!(lanes.poll_knob(Some("#5=embers"), false, &boxes));
        assert_eq!(lanes.cycle(5, &boxes), Ok(B::Rings));
        // The floor: the rings by default (step 2d), then the streamlines,
        // none, sparks, the rings.
        let floor: Vec<_> = (0..4).map(|_| lanes.cycle(3, &boxes).unwrap()).collect();
        assert_eq!(floor, [B::Streamlines, B::None, B::Sparks, B::Rings]);
        // A table the knob put on the spectrum: the first after none.
        assert!(lanes.poll_knob(Some("#0=spectrum"), false, &boxes));
        assert_eq!(lanes.cycle(0, &boxes), Ok(B::Embers));
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
        assert_eq!(lanes.behavior(0), B::Embers, "the defaults");
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
