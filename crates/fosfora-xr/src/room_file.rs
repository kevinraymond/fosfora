//! The room file (board #3326): which behavior each of a room's surfaces
//! runs, kept per room across launches. Plain data and JSON, so it builds
//! and tests on the desktop; `lanes.rs` turns it into the sim's lanes.
//!
//! A room is its set of scene anchors: Space Setup keeps an anchor's UUID
//! across sessions and rescans (#3335), so the FNV-1a hash of the sorted
//! UUIDs ([`room_id`]) names the same room every launch, and a rescan that
//! adds or drops an anchor makes it a new one. One file per room,
//! `rooms/<room id>.json` under the app's config dir:
//!
//! ```json
//! { "version": 2,
//!   "kind_defaults": { "table": "streamlines", "floor": "rings", "wall": "spectrum",
//!                      "ceiling": "none", "frame": "pulse", "other": "curls" },
//!   "anchors": [ { "uuid": "<32 hex>", "kind": "table", "behavior": "curls",
//!                  "strength": 1.0, "params": [5.0, 2.0] } ] }
//! ```
//!
//! An anchor with an entry runs the entry; one without runs its kind's
//! default; a room without a file runs the built-in defaults
//! ([`SurfaceBehavior::default_for`]: the mockup's room since D2, board
//! #3488). A file keeps the kind defaults it was saved with: every save
//! writes all of them. An entry's `kind` is informational (the kind at
//! save time); its `params` are the lane's two spare values, the color
//! index and the audio band of the surfaces pass (board #3488: color 0 the
//! kind's own, 1 to 7 the surface palette, 8 the key's tint; band 0 the
//! rms, 1 the bass, 2 the mid, 3 the high), written only once something
//! set them; an entry without them runs the defaults ([`default_params`]).
//! Behaviors are by name: `ripple`, the rings' name before D1, still reads
//! as `rings`, and a save writes `rings`.
//!
//! **Version 2** (D2). Every save before D2 wrote all six kind defaults,
//! table embers and frame none among them, which would have kept a room
//! saved then on the old defaults for good. So a version 1 file keeps its
//! `anchors` entries (behavior and strength; the `params` it wrote were
//! never defined and are dropped) and its `kind_defaults` give way to the
//! built-ins; the next save writes version 2 with the full set. A version
//! 2 file loads as it was saved.
//!
//! The synthetic stage floor has no anchor: an assignment to it is kept
//! under the all-zero UUID, which no anchor has, and never counts toward
//! the room id.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::surfaces::{KIND_NAMES, SurfaceBehavior, kind_from_name, kind_name};

/// The room file's format version: 2 since D2 (board #3488), which reads
/// a version 1 file without its kind defaults.
pub const VERSION: u64 = 2;
/// The version before D2's defaults.
pub const VERSION_1: u64 = 1;
/// The directory under the config dir that holds one file per room.
pub const ROOMS_DIR: &str = "rooms";
/// The UUID the synthetic stage floor's entry is kept under.
pub const STAGE_FLOOR_UUID: [u8; 16] = [0; 16];

/// A UUID as 32 lowercase hex digits, bytes in order.
pub fn uuid_hex(uuid: &[u8; 16]) -> String {
    use std::fmt::Write as _;
    uuid.iter().fold(String::with_capacity(32), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// A UUID from 32 hex digits (any case).
pub fn parse_uuid(s: &str) -> Option<[u8; 16]> {
    let s = s.trim();
    if s.len() != 32 || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// FNV-1a, 64 bits.
fn fnv1a64(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The room id of a set of anchor UUIDs: FNV-1a 64 over the UUIDs sorted
/// bytewise, so the order the runtime returns them in does not matter.
/// The all-zero UUID (the stage floor) is left out; `None` without an
/// anchor.
pub fn room_id(uuids: impl IntoIterator<Item = [u8; 16]>) -> Option<u64> {
    let mut sorted: Vec<[u8; 16]> = uuids
        .into_iter()
        .filter(|u| *u != STAGE_FLOOR_UUID)
        .collect();
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_unstable();
    Some(fnv1a64(sorted.iter().flatten().copied()))
}

/// A room id as the 16 lowercase hex digits its file is named by.
pub fn room_id_hex(id: u64) -> String {
    format!("{id:016x}")
}

/// Where room `id`'s file lives under `config`.
pub fn room_path(config: &Path, id: u64) -> PathBuf {
    config
        .join(ROOMS_DIR)
        .join(format!("{}.json", room_id_hex(id)))
}

/// One anchor's assignment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnchorEntry {
    pub uuid: [u8; 16],
    /// The anchor's kind when it was assigned (informational).
    pub kind: u32,
    pub behavior: SurfaceBehavior,
    /// 0..1: the sim scales the behavior's gate by it.
    pub strength: f32,
    /// The lane's two spare parameters (`z`, `w`) when something set them;
    /// `None` runs the defaults.
    pub params: Option<[f32; 2]>,
}

/// A room's assignments: a default per kind and an entry per assigned
/// anchor.
#[derive(Debug, Clone, PartialEq)]
pub struct RoomFile {
    /// By kind, `KIND_NONE` (always none) to `KIND_OTHER`.
    kind_defaults: [SurfaceBehavior; 7],
    /// At most one per UUID.
    pub anchors: Vec<AnchorEntry>,
}

impl Default for RoomFile {
    /// The built-in defaults and no entries: what a room without a file
    /// runs.
    fn default() -> Self {
        Self {
            kind_defaults: std::array::from_fn(|k| SurfaceBehavior::default_for(k as u32)),
            anchors: Vec::new(),
        }
    }
}

/// What reading a room file found.
#[derive(Debug)]
pub enum Loaded {
    /// No file: the room runs the built-in defaults.
    Missing,
    /// The file, and what in it was skipped (one line each).
    File(RoomFile, Vec<String>),
    /// The file exists but does not read as a room file.
    Failed(String),
}

impl RoomFile {
    /// What a surface of `kind` without an entry runs.
    pub fn kind_default(&self, kind: u32) -> SurfaceBehavior {
        self.kind_defaults
            .get(kind as usize)
            .copied()
            .unwrap_or_default()
    }

    /// Set what the surfaces of `kind` without an entry run; ignored for a
    /// kind without a name ([`KIND_NAMES`]).
    pub fn set_kind_default(&mut self, kind: u32, behavior: SurfaceBehavior) {
        if KIND_NAMES.iter().any(|(k, _)| *k == kind) {
            self.kind_defaults[kind as usize] = behavior;
        }
    }

    /// The entry for `uuid`, if any.
    pub fn entry(&self, uuid: &[u8; 16]) -> Option<&AnchorEntry> {
        self.anchors.iter().find(|e| e.uuid == *uuid)
    }

    /// What the surface `uuid` of `kind` runs: its entry's behavior,
    /// strength and parameters, else its kind's default at full strength.
    /// Parameters nothing set are the defaults ([`default_params`]).
    pub fn resolve(&self, uuid: &[u8; 16], kind: u32) -> (SurfaceBehavior, f32, [f32; 2]) {
        let (behavior, strength, params) = self
            .entry(uuid)
            .map_or((self.kind_default(kind), 1.0, None), |e| {
                (e.behavior, e.strength, e.params)
            });
        (
            behavior,
            strength,
            params.unwrap_or_else(|| default_params(behavior)),
        )
    }

    /// Assign `behavior` at `strength` (clamped to 0..1) to the surface
    /// `uuid` of `kind`, replacing its entry if it has one (its parameters
    /// kept).
    pub fn assign(&mut self, uuid: [u8; 16], kind: u32, behavior: SurfaceBehavior, strength: f32) {
        let strength = strength.clamp(0.0, 1.0);
        match self.anchors.iter_mut().find(|e| e.uuid == uuid) {
            Some(e) => {
                e.kind = kind;
                e.behavior = behavior;
                e.strength = strength;
            }
            None => self.anchors.push(AnchorEntry {
                uuid,
                kind,
                behavior,
                strength,
                params: None,
            }),
        }
    }

    /// The class assignment: `behavior` at `strength` becomes `kind`'s
    /// default and every entry of that kind's, and each surface in `live`
    /// (the room's surfaces of that kind now) gets an entry, so the
    /// strength holds for the ones that had none. Returns how many entries
    /// it wrote.
    pub fn assign_kind(
        &mut self,
        kind: u32,
        behavior: SurfaceBehavior,
        strength: f32,
        live: impl IntoIterator<Item = [u8; 16]>,
    ) -> usize {
        self.set_kind_default(kind, behavior);
        let strength = strength.clamp(0.0, 1.0);
        let mut written = 0;
        for e in self.anchors.iter_mut().filter(|e| e.kind == kind) {
            e.behavior = behavior;
            e.strength = strength;
            written += 1;
        }
        for uuid in live {
            if self.entry(&uuid).is_none_or(|e| e.kind != kind) {
                self.assign(uuid, kind, behavior, strength);
                written += 1;
            }
        }
        written
    }

    /// Set the parameters of `uuid`'s entry; nothing without one.
    pub fn set_params(&mut self, uuid: [u8; 16], params: [f32; 2]) {
        if let Some(e) = self.anchors.iter_mut().find(|e| e.uuid == uuid) {
            e.params = Some(params);
        }
    }

    /// Set the color index of `uuid`'s entry and its band, or with `band`
    /// `None` keep the band it has (its behavior's default when nothing set
    /// one); nothing without an entry.
    pub fn set_color(&mut self, uuid: [u8; 16], color: u32, band: Option<u32>) {
        if let Some(e) = self.anchors.iter_mut().find(|e| e.uuid == uuid) {
            let kept = e.params.unwrap_or_else(|| default_params(e.behavior))[1];
            e.params = Some([color as f32, band.map_or(kept, |b| b as f32)]);
        }
    }

    /// Drop every entry and put the kind defaults back to the built-ins.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The file's JSON ([`VERSION`]), every kind default, entries in UUID
    /// order, an entry's `params` only when set.
    pub fn to_json(&self) -> String {
        let defaults: Map<String, Value> = KIND_NAMES
            .iter()
            .map(|&(k, name)| (name.to_owned(), json!(self.kind_default(k).name())))
            .collect();
        let mut anchors = self.anchors.clone();
        anchors.sort_by_key(|e| e.uuid);
        let anchors: Vec<Value> = anchors
            .iter()
            .map(|e| {
                let mut entry = json!({
                    "uuid": uuid_hex(&e.uuid),
                    "kind": kind_name(e.kind),
                    "behavior": e.behavior.name(),
                    "strength": e.strength,
                });
                if let (Some(p), Some(obj)) = (e.params, entry.as_object_mut()) {
                    obj.insert("params".to_owned(), json!(p));
                }
                entry
            })
            .collect();
        let file = json!({
            "version": VERSION,
            "kind_defaults": defaults,
            "anchors": anchors,
        });
        serde_json::to_string_pretty(&file).unwrap_or_default()
    }

    /// A room file from its JSON, and what in it was skipped: an unknown
    /// or reserved behavior name (`ripple` is known: the rings) leaves
    /// that default or entry unset (the kind's built-in default), as does
    /// a bad UUID; a version 1 file's kind defaults give way to the
    /// built-ins and its entries' parameters are dropped (the module
    /// docs), which the list says once. `Err` for text that is not a
    /// version 1 or 2 room file.
    pub fn from_json(text: &str) -> Result<(Self, Vec<String>), String> {
        let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let obj = v.as_object().ok_or("not a JSON object")?;
        let version = match obj.get("version").and_then(Value::as_u64) {
            Some(n @ (VERSION_1 | VERSION)) => n,
            other => {
                return Err(format!(
                    "version {other:?}, expected {VERSION_1} or {VERSION}"
                ));
            }
        };
        let mut file = Self::default();
        let mut skipped = Vec::new();
        let behavior = |v: Option<&Value>, what: &str, skipped: &mut Vec<String>| {
            let name = v.and_then(Value::as_str).unwrap_or("");
            let b = SurfaceBehavior::from_name(name);
            if b.is_none() {
                skipped.push(format!("{what}: unknown behavior '{name}', left unset"));
            }
            b
        };
        let defaults = obj.get("kind_defaults").and_then(Value::as_object);
        if version == VERSION_1 {
            if defaults.is_some_and(|d| !d.is_empty()) {
                skipped.push(format!(
                    "version {VERSION_1}: its kind defaults give way to the built-ins (the next save writes version {VERSION})"
                ));
            }
        } else if let Some(defaults) = defaults {
            for (name, v) in defaults {
                let Some(kind) = kind_from_name(name) else {
                    skipped.push(format!("kind_defaults: unknown kind '{name}'"));
                    continue;
                };
                if let Some(b) = behavior(Some(v), &format!("kind_defaults.{name}"), &mut skipped) {
                    file.set_kind_default(kind, b);
                }
            }
        }
        for (i, a) in obj
            .get("anchors")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .enumerate()
        {
            let Some(uuid) = a.get("uuid").and_then(Value::as_str).and_then(parse_uuid) else {
                skipped.push(format!("anchors[{i}]: no valid uuid"));
                continue;
            };
            let what = format!("anchors[{i}] {}", uuid_hex(&uuid));
            let Some(b) = behavior(a.get("behavior"), &what, &mut skipped) else {
                continue;
            };
            let kind = a
                .get("kind")
                .and_then(Value::as_str)
                .and_then(kind_from_name)
                .unwrap_or(crate::surfaces::KIND_NONE);
            let strength = a.get("strength").and_then(Value::as_f64).unwrap_or(1.0) as f32;
            file.assign(uuid, kind, b, strength);
            let params = a.get("params").filter(|_| version == VERSION);
            if let Some(p) = params {
                match params_of(p) {
                    Some(p) => file.set_params(uuid, p),
                    None => skipped.push(format!(
                        "{what}: params {p} are not two numbers, left unset"
                    )),
                }
            }
        }
        Ok((file, skipped))
    }

    /// Read the room file at `path`.
    pub fn load(path: &Path) -> Loaded {
        match std::fs::read_to_string(path) {
            Ok(text) => match Self::from_json(&text) {
                Ok((file, skipped)) => Loaded::File(file, skipped),
                Err(e) => Loaded::Failed(e),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded::Missing,
            Err(e) => Loaded::Failed(e.to_string()),
        }
    }

    /// Write the room file at `path`, creating its directory.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_json())
    }
}

/// The parameters of a surface running `behavior` that nothing set: the
/// kind's own color (index 0) and the behavior's band
/// ([`SurfaceBehavior::default_band`]): the bass for the rings and the
/// pulse, the rms for the rest.
pub fn default_params(behavior: SurfaceBehavior) -> [f32; 2] {
    [0.0, behavior.default_band() as f32]
}

/// An entry's `params` from the file: two finite numbers.
fn params_of(v: &Value) -> Option<[f32; 2]> {
    match v.as_array()?.as_slice() {
        [a, b] => {
            let p = [a.as_f64()? as f32, b.as_f64()? as f32];
            p.iter().all(|x| x.is_finite()).then_some(p)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surfaces::{KIND_FLOOR, KIND_OTHER, KIND_TABLE, KIND_WALL};

    fn uuid(n: u8) -> [u8; 16] {
        let mut u = [0u8; 16];
        u[0] = n;
        u[15] = 0xa0 | n;
        u
    }

    #[test]
    fn a_uuid_reads_back_from_its_hex() {
        let u: [u8; 16] = std::array::from_fn(|i| (i as u8).wrapping_mul(37).wrapping_add(5));
        let hex = uuid_hex(&u);
        assert_eq!(hex.len(), 32);
        assert_eq!(hex, hex.to_ascii_lowercase());
        assert_eq!(parse_uuid(&hex), Some(u));
        assert_eq!(parse_uuid(&hex.to_ascii_uppercase()), Some(u));
        assert_eq!(parse_uuid(&hex[..30]), None);
        assert_eq!(parse_uuid(&format!("{}zz", &hex[..30])), None);
    }

    #[test]
    fn the_room_id_is_the_set_of_anchors() {
        // FNV-1a 64's published vector.
        assert_eq!(fnv1a64(*b"a"), 0xaf63_dc4c_8601_ec8c);
        let id = room_id([uuid(1), uuid(2), uuid(3)]).expect("anchors");
        // Any order, and with the stage floor's zero UUID.
        assert_eq!(room_id([uuid(3), uuid(1), uuid(2)]), Some(id));
        assert_eq!(
            room_id([uuid(2), STAGE_FLOOR_UUID, uuid(3), uuid(1)]),
            Some(id)
        );
        // Another set, another room.
        assert_ne!(room_id([uuid(1), uuid(2)]), Some(id));
        assert_ne!(room_id([uuid(1), uuid(2), uuid(4)]), Some(id));
        assert_eq!(room_id([]), None);
        assert_eq!(room_id([STAGE_FLOOR_UUID]), None);
        let hex = room_id_hex(id);
        assert_eq!(hex.len(), 16);
        assert!(
            hex.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        );
        assert_eq!(room_id_hex(0x1f), "000000000000001f");
        assert_eq!(
            room_path(Path::new("/cfg"), 0x1f),
            Path::new("/cfg/rooms/000000000000001f.json")
        );
    }

    #[test]
    fn an_anchor_without_an_entry_runs_its_kind_default() {
        use SurfaceBehavior as B;
        let mut file = RoomFile::default();
        assert_eq!(
            file.resolve(&uuid(9), KIND_TABLE),
            (B::Streamlines, 1.0, [0.0; 2])
        );
        assert_eq!(file.resolve(&uuid(9), KIND_FLOOR).0, B::Rings);
        assert_eq!(file.resolve(&uuid(9), KIND_WALL).0, B::Spectrum);
        assert_eq!(file.resolve(&uuid(9), KIND_OTHER).0, B::Curls);
        file.assign(uuid(1), KIND_TABLE, B::None, 1.0);
        file.set_kind_default(KIND_WALL, B::None);
        // The entry wins for its anchor; the other table and every wall
        // take the defaults.
        assert_eq!(file.resolve(&uuid(1), KIND_TABLE).0, B::None);
        assert_eq!(file.resolve(&uuid(2), KIND_TABLE).0, B::Streamlines);
        assert_eq!(file.resolve(&uuid(3), KIND_WALL).0, B::None);
        // Assigning again replaces the entry; the strength is clamped.
        file.assign(uuid(1), KIND_TABLE, B::Sparks, 1.5);
        assert_eq!(file.anchors.len(), 1);
        assert_eq!(
            file.resolve(&uuid(1), KIND_TABLE),
            (B::Sparks, 1.0, [0.0; 2])
        );
        // The stage floor under the zero UUID.
        file.assign(STAGE_FLOOR_UUID, KIND_FLOOR, B::Rings, 0.5);
        assert_eq!(
            file.resolve(&STAGE_FLOOR_UUID, KIND_FLOOR),
            (B::Rings, 0.5, [0.0, 1.0]),
            "the rings' default band is the bass"
        );
        file.clear();
        assert_eq!(file, RoomFile::default());
    }

    #[test]
    fn a_file_keeps_the_floor_default_it_names_and_one_without_takes_the_rings() {
        use SurfaceBehavior as B;
        // Step 2d: the floor's built-in default is the rings (the ripple).
        // A file that names the floor's default keeps it; one that does
        // not takes the rings.
        let with = r#"{ "version": 2, "kind_defaults": { "table": "embers", "floor": "sparks" },
                        "anchors": [] }"#;
        let (file, skipped) = RoomFile::from_json(with).expect("reads");
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(file.resolve(&uuid(9), KIND_FLOOR).0, B::Sparks);
        assert_eq!(file.resolve(&STAGE_FLOOR_UUID, KIND_FLOOR).0, B::Sparks);
        let without = r#"{ "version": 2, "kind_defaults": { "table": "embers" }, "anchors": [] }"#;
        let (file, _) = RoomFile::from_json(without).expect("reads");
        assert_eq!(file.resolve(&uuid(9), KIND_FLOOR).0, B::Rings);
        // What a save writes now.
        assert!(
            RoomFile::default()
                .to_json()
                .contains(r#""floor": "rings""#)
        );
    }

    #[test]
    fn a_class_assignment_rewrites_the_kind_and_its_entries() {
        use SurfaceBehavior as B;
        let mut file = RoomFile::default();
        file.assign(uuid(1), KIND_WALL, B::None, 1.0);
        file.assign(uuid(2), KIND_WALL, B::Embers, 0.3);
        file.assign(uuid(3), KIND_TABLE, B::None, 1.0);
        // Walls 1 and 4 are in the room now; 2 is not (a file entry only).
        let written = file.assign_kind(KIND_WALL, B::Spectrum, 0.5, [uuid(1), uuid(4)]);
        assert_eq!(written, 3);
        for u in [uuid(1), uuid(2), uuid(4)] {
            assert_eq!(file.resolve(&u, KIND_WALL), (B::Spectrum, 0.5, [0.0; 2]));
        }
        // The kind default: a wall the file never saw.
        assert_eq!(file.kind_default(KIND_WALL), B::Spectrum);
        assert_eq!(
            file.resolve(&uuid(7), KIND_WALL),
            (B::Spectrum, 1.0, [0.0; 2])
        );
        // Another kind's entry and default are untouched.
        assert_eq!(file.resolve(&uuid(3), KIND_TABLE).0, B::None);
        assert_eq!(file.kind_default(KIND_TABLE), B::Streamlines);
    }

    #[test]
    fn a_room_file_round_trips() {
        use SurfaceBehavior as B;
        let mut file = RoomFile::default();
        file.assign(uuid(2), KIND_WALL, B::Spectrum, 0.25);
        file.assign(uuid(1), KIND_TABLE, B::None, 1.0);
        file.assign(STAGE_FLOOR_UUID, KIND_FLOOR, B::Rings, 1.0);
        file.set_params(uuid(2), [0.5, -2.0]);
        file.set_kind_default(KIND_FLOOR, B::Sparks);
        // A ported desktop effect (D2b) is saved by its name like the rest.
        file.assign(uuid(0), KIND_OTHER, B::Tessera, 0.5);
        let text = file.to_json();
        let (back, skipped) = RoomFile::from_json(&text).expect("reads");
        assert!(skipped.is_empty(), "{skipped:?}");
        // Entries come back in UUID order.
        let mut expected = file.clone();
        expected.anchors.sort_by_key(|e| e.uuid);
        assert_eq!(back, expected);
        // The shape the design names.
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["version"], 2);
        assert_eq!(v["kind_defaults"]["floor"], "sparks");
        assert_eq!(v["kind_defaults"]["table"], "streamlines");
        assert_eq!(v["kind_defaults"]["frame"], "pulse");
        assert_eq!(v["kind_defaults"]["other"], "curls");
        assert_eq!(v["kind_defaults"].as_object().unwrap().len(), 6);
        assert_eq!(v["anchors"][1]["behavior"], "tessera");
        let a = &v["anchors"][3];
        assert_eq!(a["uuid"], uuid_hex(&uuid(2)));
        assert_eq!(a["kind"], "wall");
        assert_eq!(a["behavior"], "spectrum");
        assert_eq!(a["params"], json!([0.5, -2.0]));
        // Parameters nothing set are not written.
        assert!(v["anchors"][2].get("params").is_none(), "{text}");
        // On disk, into a directory that does not exist yet.
        let dir = std::env::temp_dir().join(format!("fosfora-room-file-{}", std::process::id()));
        let path = room_path(&dir, 0xabc);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(RoomFile::load(&path), Loaded::Missing));
        file.save(&path).expect("saves");
        match RoomFile::load(&path) {
            Loaded::File(f, skipped) => {
                assert_eq!(f, expected);
                assert!(skipped.is_empty());
            }
            other => panic!("{other:?}"),
        }
        std::fs::write(&path, "{ not json").unwrap();
        assert!(matches!(RoomFile::load(&path), Loaded::Failed(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_room_saved_before_d1_reads_its_ripple_as_the_rings_and_saves_rings() {
        use SurfaceBehavior as B;
        // Board #3472: `ripple` was the rings' name until D1.
        let text = format!(
            r#"{{ "version": 2,
                "kind_defaults": {{ "floor": "ripple", "ceiling": "Ripple" }},
                "anchors": [
                  {{ "uuid": "{}", "kind": "floor", "behavior": "ripple", "strength": 0.5 }},
                  {{ "uuid": "{}", "kind": "table", "behavior": "streamlines" }}
                ] }}"#,
            uuid_hex(&uuid(1)),
            uuid_hex(&uuid(2))
        );
        let (file, skipped) = RoomFile::from_json(&text).expect("reads");
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(file.kind_default(KIND_FLOOR), B::Rings);
        assert_eq!(file.kind_default(crate::surfaces::KIND_CEILING), B::Rings);
        assert_eq!(
            file.resolve(&uuid(1), KIND_FLOOR),
            (B::Rings, 0.5, default_params(B::Rings))
        );
        assert_eq!(file.resolve(&uuid(2), KIND_TABLE).0, B::Streamlines);
        // Written back under the new names, and read back the same.
        let out = file.to_json();
        assert!(!out.contains("ripple"), "{out}");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["kind_defaults"]["floor"], "rings");
        assert_eq!(v["kind_defaults"]["ceiling"], "rings");
        assert_eq!(v["anchors"][0]["behavior"], "rings");
        assert_eq!(v["anchors"][1]["behavior"], "streamlines");
        assert_eq!(RoomFile::from_json(&out).expect("reads").0, file);
    }

    #[test]
    fn unknown_names_are_left_unset_and_bad_files_refused() {
        use SurfaceBehavior as B;
        let text = format!(
            r#"{{ "version": 2,
                "kind_defaults": {{ "table": "glitter", "wall": "none", "sofa": "embers",
                                    "floor": "pulses" }},
                "anchors": [
                  {{ "uuid": "{}", "kind": "table", "behavior": "drips" }},
                  {{ "uuid": "nope", "kind": "table", "behavior": "embers" }},
                  {{ "uuid": "{}", "kind": "other", "behavior": "SPARKS" }}
                ] }}"#,
            uuid_hex(&uuid(1)),
            uuid_hex(&uuid(2))
        );
        let (file, skipped) = RoomFile::from_json(&text).expect("reads");
        assert_eq!(skipped.len(), 5, "{skipped:?}");
        // The unknown table default stays built-in, the known wall one
        // takes; the unknown floor one is unset: the built-in rings.
        assert_eq!(file.kind_default(KIND_TABLE), B::Streamlines);
        assert_eq!(file.kind_default(KIND_WALL), B::None);
        assert_eq!(file.kind_default(KIND_FLOOR), B::Rings);
        // The entry with an unknown behavior is dropped: its kind default.
        assert_eq!(file.entry(&uuid(1)), None);
        // A missing strength is full.
        assert_eq!(
            file.resolve(&uuid(2), KIND_OTHER),
            (B::Sparks, 1.0, [0.0; 2])
        );
        assert!(RoomFile::from_json(r#"{ "version": 3 }"#).is_err());
        assert!(RoomFile::from_json(r#"{ "version": 0 }"#).is_err());
        assert!(RoomFile::from_json(r#"{ "anchors": [] }"#).is_err());
        assert!(RoomFile::from_json("[1]").is_err());
        assert!(RoomFile::from_json("").is_err());
    }

    #[test]
    fn a_version_1_room_keeps_its_anchors_and_takes_the_new_defaults() {
        use SurfaceBehavior as B;
        // What every save before D2 wrote: all six kind defaults (table
        // embers, frame none), an entry per assigned anchor with params
        // [0, 0], and the ripple's old name.
        let text = format!(
            r#"{{ "version": 1,
                "kind_defaults": {{ "table": "embers", "floor": "rings", "wall": "spectrum",
                                    "ceiling": "none", "frame": "none", "other": "none" }},
                "anchors": [
                  {{ "uuid": "{}", "kind": "table", "behavior": "sparks", "strength": 0.5,
                     "params": [0.0, 0.0] }},
                  {{ "uuid": "{}", "kind": "wall", "behavior": "none", "strength": 1.0,
                     "params": [3.0, 1.0] }},
                  {{ "uuid": "{}", "kind": "floor", "behavior": "ripple" }}
                ] }}"#,
            uuid_hex(&uuid(1)),
            uuid_hex(&uuid(2)),
            uuid_hex(&STAGE_FLOOR_UUID)
        );
        let (file, skipped) = RoomFile::from_json(&text).expect("reads");
        // One line says the kind defaults gave way.
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert!(skipped[0].contains("built-ins"), "{skipped:?}");
        // The built-in defaults: the mockup's room.
        let built_in = RoomFile::default();
        for (kind, _) in KIND_NAMES {
            assert_eq!(file.kind_default(kind), built_in.kind_default(kind));
        }
        assert_eq!(file.kind_default(KIND_TABLE), B::Streamlines);
        assert_eq!(file.kind_default(crate::surfaces::KIND_FRAME), B::Pulse);
        // The entries kept, without the params version 1 never defined.
        assert_eq!(file.anchors.len(), 3);
        assert_eq!(
            file.resolve(&uuid(1), KIND_TABLE),
            (B::Sparks, 0.5, [0.0; 2])
        );
        assert_eq!(file.resolve(&uuid(2), KIND_WALL), (B::None, 1.0, [0.0; 2]));
        assert!(file.anchors.iter().all(|e| e.params.is_none()));
        assert_eq!(file.resolve(&STAGE_FLOOR_UUID, KIND_FLOOR).0, B::Rings);
        // An anchor the file never named runs the new default.
        assert_eq!(file.resolve(&uuid(7), KIND_TABLE).0, B::Streamlines);
        // A save writes version 2 with the full set, and it reads back the
        // same with nothing skipped.
        let out = file.to_json();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["version"], 2);
        assert_eq!(v["kind_defaults"].as_object().unwrap().len(), 6);
        assert_eq!(v["kind_defaults"]["table"], "streamlines");
        let (back, skipped) = RoomFile::from_json(&out).expect("reads");
        assert!(skipped.is_empty(), "{skipped:?}");
        let mut expected = file.clone();
        expected.anchors.sort_by_key(|e| e.uuid);
        assert_eq!(back, expected);
        // A version 1 file without kind defaults says nothing about them.
        let (_, skipped) =
            RoomFile::from_json(r#"{ "version": 1, "anchors": [] }"#).expect("reads");
        assert!(skipped.is_empty(), "{skipped:?}");
    }

    #[test]
    fn a_version_2_room_round_trips_its_defaults_and_params() {
        use SurfaceBehavior as B;
        let mut file = RoomFile::default();
        file.set_kind_default(KIND_TABLE, B::Embers);
        file.set_kind_default(crate::surfaces::KIND_FRAME, B::None);
        file.assign(uuid(1), KIND_TABLE, B::Curls, 0.75);
        file.set_params(uuid(1), [5.0, 2.0]);
        file.assign(uuid(2), KIND_OTHER, B::Pulse, 1.0);
        // No entry, no params: nothing set.
        file.set_params(uuid(9), [1.0, 1.0]);
        assert_eq!(file.anchors.len(), 2);
        let (back, skipped) = RoomFile::from_json(&file.to_json()).expect("reads");
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(back, file);
        // A version 2 file keeps the defaults it names, table embers too.
        assert_eq!(back.kind_default(KIND_TABLE), B::Embers);
        assert_eq!(
            back.resolve(&uuid(1), KIND_TABLE),
            (B::Curls, 0.75, [5.0, 2.0])
        );
        assert_eq!(back.entry(&uuid(2)).unwrap().params, None);
        // Params that are not two numbers are left unset, said once.
        let text = format!(
            r#"{{ "version": 2, "anchors": [
                  {{ "uuid": "{}", "behavior": "pulse", "params": [1.0] }},
                  {{ "uuid": "{}", "behavior": "pulse", "params": "teal" }} ] }}"#,
            uuid_hex(&uuid(1)),
            uuid_hex(&uuid(2))
        );
        let (bad, skipped) = RoomFile::from_json(&text).expect("reads");
        assert_eq!(skipped.len(), 2, "{skipped:?}");
        assert!(bad.anchors.iter().all(|e| e.params.is_none()));
        assert_eq!(bad.anchors.len(), 2);
    }
}
