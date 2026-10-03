//! The room's anchors saved and replayed (board #3536): a measurement aid
//! for unworn runs, not a product feature.
//!
//! A headset lying unworn on a desk mostly answers the scene query with no
//! anchors, so an unworn sweep could not light the room's surfaces. So the
//! last located room is kept in `debug/anchors.json` under the app's config
//! dir, and with `debug.fosfora.anchors replay` an empty query brings it
//! back as static boxes:
//!
//! ```json
//! { "version": 1,
//!   "room_id": "<16 hex>",
//!   "anchors": [ { "uuid": "<32 hex>", "label": "TABLE",
//!                  "center": [x, y, z], "rotation": [x, y, z, w],
//!                  "half_extents": [x, y, z] } ] }
//! ```
//!
//! Boxes are in the app's base space, as `room.rs` hands them to the rest
//! of the app (a plane's half extents carry its slab's half thickness); the
//! label is the runtime's string as given, so the kind and the hidden-wall
//! rule read it as they read a live anchor's. The room id is
//! `room_file::room_id` of the saved UUIDs, so a replayed room opens the same
//! `rooms/<id>.json` as the live one.
//!
//! Plain data, so it builds and tests on the desktop; `room.rs` does the
//! writes and turns a replay into obstacle boxes.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::room_file::{parse_uuid, room_id, room_id_hex, uuid_hex};

/// The file's format version.
pub const VERSION: u64 = 1;
/// The directory under the config dir for debug files.
pub const DEBUG_DIR: &str = "debug";
/// The file's name in [`DEBUG_DIR`].
pub const FILE_NAME: &str = "anchors.json";
/// The knob's value that turns the replay on.
pub const KNOB_REPLAY: &str = "replay";
/// How long a set of located anchors that is not the whole room must hold
/// before it is saved.
pub const STABLE_FOR: Duration = Duration::from_secs(2);

/// Where the saved anchors live under `config`.
pub fn path(config: &Path) -> PathBuf {
    config.join(DEBUG_DIR).join(FILE_NAME)
}

/// One located anchor as the app consumes it.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedAnchor {
    pub uuid: [u8; 16],
    /// The semantic labels, exactly as the runtime gave them.
    pub label: String,
    pub center: [f32; 3],
    /// Box to base space, (x, y, z, w).
    pub rotation: [f32; 4],
    pub half_extents: [f32; 3],
}

impl SavedAnchor {
    /// The surface kind its label names, as for a live anchor.
    pub fn kind(&self) -> u32 {
        crate::surfaces::surface_kind(&self.label)
    }

    /// A wall the runtime hides, as for a live anchor.
    pub fn hidden(&self) -> bool {
        crate::surfaces::is_hidden_wall(&self.label)
    }
}

/// A saved room: its located anchors.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedRoom {
    pub anchors: Vec<SavedAnchor>,
}

impl SavedRoom {
    /// The room id of its anchors (`room_file::room_id`).
    pub fn room_id(&self) -> Option<u64> {
        room_id(self.anchors.iter().map(|a| a.uuid))
    }

    pub fn to_json(&self) -> String {
        let anchors: Vec<Value> = self
            .anchors
            .iter()
            .map(|a| {
                json!({
                    "uuid": uuid_hex(&a.uuid),
                    "label": a.label,
                    "center": a.center,
                    "rotation": a.rotation,
                    "half_extents": a.half_extents,
                })
            })
            .collect();
        let file = json!({
            "version": VERSION,
            "room_id": self.room_id().map(room_id_hex),
            "anchors": anchors,
        });
        serde_json::to_string_pretty(&file).unwrap_or_default()
    }

    /// A saved room from its JSON. `Err` says why the file is refused: not
    /// version [`VERSION`], no anchors, an anchor with a bad UUID, a missing
    /// label, a vector that is not finite numbers of the right length, a
    /// zero rotation or a negative extent, a UUID twice, or a room id that
    /// is not its anchors'.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let obj = v.as_object().ok_or("not a JSON object")?;
        match obj.get("version").and_then(Value::as_u64) {
            Some(VERSION) => {}
            other => return Err(format!("version {other:?}, expected {VERSION}")),
        }
        let list = obj
            .get("anchors")
            .and_then(Value::as_array)
            .ok_or("no anchors array")?;
        if list.is_empty() {
            return Err("no anchors".to_owned());
        }
        let mut anchors = Vec::with_capacity(list.len());
        for (i, a) in list.iter().enumerate() {
            let text = a.get("uuid").and_then(Value::as_str).unwrap_or("");
            let uuid = parse_uuid(text).ok_or(format!("anchors[{i}]: bad uuid '{text}'"))?;
            if anchors.iter().any(|b: &SavedAnchor| b.uuid == uuid) {
                return Err(format!("anchors[{i}]: uuid {} twice", uuid_hex(&uuid)));
            }
            let label = a
                .get("label")
                .and_then(Value::as_str)
                .ok_or(format!("anchors[{i}]: no label"))?
                .to_owned();
            let center = floats::<3>(a.get("center"))
                .ok_or(format!("anchors[{i}]: center is not 3 finite numbers"))?;
            let rotation = floats::<4>(a.get("rotation"))
                .filter(|q| q.iter().map(|x| x * x).sum::<f32>() > 1e-6)
                .ok_or(format!(
                    "anchors[{i}]: rotation is not a nonzero quaternion of 4 finite numbers"
                ))?;
            let half_extents = floats::<3>(a.get("half_extents"))
                .filter(|h| h.iter().all(|x| *x >= 0.0))
                .ok_or(format!(
                    "anchors[{i}]: half_extents is not 3 finite numbers >= 0"
                ))?;
            anchors.push(SavedAnchor {
                uuid,
                label,
                center,
                rotation,
                half_extents,
            });
        }
        let room = Self { anchors };
        let expected = room.room_id().map(room_id_hex);
        let named = obj.get("room_id").and_then(Value::as_str);
        if named != expected.as_deref() {
            return Err(format!(
                "room_id {named:?} is not its anchors' {expected:?}"
            ));
        }
        Ok(room)
    }

    /// Read the saved room at `path`; `Err` says why there is none.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_json(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(format!("{} missing", path.display()))
            }
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Write the room at `path`, creating its directory.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_json())
    }
}

/// `N` finite numbers from a JSON array of exactly `N`.
fn floats<const N: usize>(v: Option<&Value>) -> Option<[f32; N]> {
    let list = v?.as_array()?;
    if list.len() != N {
        return None;
    }
    let mut out = [0.0; N];
    for (o, x) in out.iter_mut().zip(list) {
        *o = x.as_f64()? as f32;
    }
    out.iter().all(|x| x.is_finite()).then_some(out)
}

/// When to save: once per change of the located set (its room id, the
/// set of UUIDs located), as soon as every anchor is located or once the
/// set has held for [`STABLE_FOR`]; never for a replayed room.
#[derive(Debug, Default)]
pub struct SaveRule {
    /// The set last saved this launch.
    saved: Option<u64>,
    /// A partial set and when it was first seen.
    candidate: Option<(u64, Instant)>,
}

impl SaveRule {
    /// One locate pass: `located` is the room id of the located anchors
    /// (`None` with none), `all` whether that is every anchor the query
    /// returned. True when the set should be written now; the set then
    /// counts as saved, so a failed write is not retried every pass.
    pub fn observe(
        &mut self,
        located: Option<u64>,
        all: bool,
        replayed: bool,
        now: Instant,
    ) -> bool {
        let Some(set) = located.filter(|_| !replayed) else {
            self.candidate = None;
            return false;
        };
        if self.saved == Some(set) {
            self.candidate = None;
            return false;
        }
        let ready = all
            || match self.candidate {
                Some((c, since)) if c == set => now.duration_since(since) >= STABLE_FOR,
                _ => {
                    self.candidate = Some((set, now));
                    false
                }
            };
        if ready {
            self.saved = Some(set);
            self.candidate = None;
        }
        ready
    }
}

/// `debug.fosfora.anchors`: on for the exact value [`KNOB_REPLAY`] only.
pub fn replay_knob(value: Option<&str>) -> bool {
    value == Some(KNOB_REPLAY)
}

/// The replay's state over a launch.
#[derive(Debug, Default)]
pub struct Replay {
    enabled: bool,
    /// The file was read once this launch (a missing or refused file is
    /// not read again on every empty retry).
    tried: bool,
    /// A saved room stands in for the live one.
    active: bool,
}

impl Replay {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }

    pub fn active(&self) -> bool {
        self.active
    }

    /// A query completed and `live` anchors are held: true when the saved
    /// room should be read now, which is once per launch, with the knob on
    /// and no live anchor.
    pub fn should_load(&mut self, live: usize) -> bool {
        if !self.enabled || self.tried || self.active || live > 0 {
            return false;
        }
        self.tried = true;
        true
    }

    /// The saved room was read and now stands in for the live one.
    pub fn loaded(&mut self) {
        self.active = true;
    }

    /// Live anchors arrived: they win. True when a replay was dropped for
    /// them.
    pub fn live_anchors(&mut self) -> bool {
        std::mem::take(&mut self.active)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_1_SQRT_2;

    use crate::surfaces::{KIND_FLOOR, KIND_TABLE, KIND_WALL};

    fn uuid(n: u8) -> [u8; 16] {
        let mut u = [0u8; 16];
        u[0] = n;
        u[15] = 0xb0 | n;
        u
    }

    fn room() -> SavedRoom {
        SavedRoom {
            anchors: vec![
                SavedAnchor {
                    uuid: uuid(1),
                    label: "TABLE".to_owned(),
                    center: [0.25, 0.75, -1.5],
                    rotation: [-0.5, 0.5, 0.5, 0.5],
                    half_extents: [0.5, 0.375, 0.375],
                },
                SavedAnchor {
                    uuid: uuid(2),
                    label: "FLOOR".to_owned(),
                    center: [0.0, 0.0, 0.0],
                    rotation: [-FRAC_1_SQRT_2, 0.0, 0.0, FRAC_1_SQRT_2],
                    half_extents: [2.0, 1.5, 0.02],
                },
                SavedAnchor {
                    uuid: uuid(3),
                    label: "INVISIBLE_WALL_FACE".to_owned(),
                    center: [-2.0, 1.25, 0.0],
                    rotation: [0.0, FRAC_1_SQRT_2, 0.0, FRAC_1_SQRT_2],
                    half_extents: [1.5, 1.25, 0.02],
                },
            ],
        }
    }

    #[test]
    fn a_saved_room_round_trips() {
        let saved = room();
        let text = saved.to_json();
        let back = SavedRoom::from_json(&text).expect("reads back");
        assert_eq!(back.anchors.len(), 3);
        for (a, b) in saved.anchors.iter().zip(&back.anchors) {
            assert_eq!(a.uuid, b.uuid);
            assert_eq!(a.label, b.label);
            assert_close!(a.center, b.center);
            assert_close!(a.rotation, b.rotation);
            assert_close!(a.half_extents, b.half_extents);
        }
        assert_eq!(back.room_id(), saved.room_id());
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["version"], VERSION);
        assert_eq!(
            v["room_id"],
            room_id_hex(room_id([uuid(1), uuid(2), uuid(3)]).unwrap())
        );
        assert_eq!(v["anchors"][0]["uuid"], uuid_hex(&uuid(1)));
        assert_eq!(v["anchors"][2]["label"], "INVISIBLE_WALL_FACE");
        // The kinds and the hidden wall read as for a live anchor.
        let kinds: Vec<u32> = back.anchors.iter().map(SavedAnchor::kind).collect();
        assert_eq!(kinds, [KIND_TABLE, KIND_FLOOR, KIND_WALL]);
        let hidden: Vec<bool> = back.anchors.iter().map(SavedAnchor::hidden).collect();
        assert_eq!(hidden, [false, false, true]);
        // On disk, into a directory that does not exist yet.
        let dir =
            std::env::temp_dir().join(format!("fosfora-anchor-replay-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = path(&dir);
        assert!(file.ends_with("debug/anchors.json"));
        let missing = SavedRoom::load(&file).unwrap_err();
        assert!(missing.contains("missing"), "{missing}");
        saved.save(&file).expect("saves");
        assert_eq!(SavedRoom::load(&file).expect("loads").anchors.len(), 3);
        std::fs::write(&file, "{ not json").unwrap();
        assert!(SavedRoom::load(&file).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_file_is_refused_with_a_reason() {
        let good: Value = serde_json::from_str(&room().to_json()).unwrap();
        let refused = |edit: &dyn Fn(&mut Value)| {
            let mut v = good.clone();
            edit(&mut v);
            SavedRoom::from_json(&v.to_string()).unwrap_err()
        };
        let e = refused(&|v| v["version"] = json!(2));
        assert!(e.contains("version Some(2)"), "{e}");
        let e = refused(&|v| {
            v.as_object_mut().unwrap().remove("version");
        });
        assert!(e.contains("version None"), "{e}");
        let e = refused(&|v| v["anchors"][1]["uuid"] = json!("not-a-uuid"));
        assert!(e.contains("anchors[1]: bad uuid 'not-a-uuid'"), "{e}");
        let e = refused(&|v| v["anchors"][1]["uuid"] = json!("0123"));
        assert!(e.contains("bad uuid"), "{e}");
        let e = refused(&|v| v["anchors"][2]["uuid"] = v["anchors"][0]["uuid"].clone());
        assert!(e.contains("anchors[2]") && e.contains("twice"), "{e}");
        let e = refused(&|v| v["anchors"][0]["center"] = json!([1.0, 2.0]));
        assert!(e.contains("anchors[0]: center"), "{e}");
        let e = refused(&|v| v["anchors"][0]["rotation"] = json!([0.0, 0.0, 0.0, 0.0]));
        assert!(e.contains("anchors[0]: rotation"), "{e}");
        let e = refused(&|v| v["anchors"][0]["half_extents"] = json!([0.1, -0.1, 0.1]));
        assert!(e.contains("anchors[0]: half_extents"), "{e}");
        let e = refused(&|v| {
            v["anchors"][0].as_object_mut().unwrap().remove("label");
        });
        assert!(e.contains("anchors[0]: no label"), "{e}");
        let e = refused(&|v| v["anchors"] = json!([]));
        assert_eq!(e, "no anchors");
        let e = refused(&|v| v["room_id"] = json!("0000000000000001"));
        assert!(e.contains("room_id"), "{e}");
        assert!(SavedRoom::from_json("[1, 2]").is_err());
    }

    #[test]
    fn the_save_fires_once_per_set_change() {
        let t0 = Instant::now();
        let at = |s: f32| t0 + Duration::from_secs_f32(s);
        let mut rule = SaveRule::default();
        let (a, b) = (Some(0xa), Some(0xb));
        // Nothing located: nothing to save.
        assert!(!rule.observe(None, false, false, at(0.0)));
        // Every anchor located: saved at once, and only once.
        assert!(rule.observe(a, true, false, at(0.0)));
        assert!(!rule.observe(a, true, false, at(1.0)));
        assert!(!rule.observe(a, true, false, at(10.0)));
        // A partial set is saved once it held for STABLE_FOR.
        assert!(!rule.observe(b, false, false, at(11.0)));
        assert!(!rule.observe(b, false, false, at(12.0)));
        assert!(rule.observe(b, false, false, at(13.0)));
        assert!(!rule.observe(b, false, false, at(14.0)));
        // A set that changes before it holds restarts the wait.
        let c = Some(0xc);
        assert!(!rule.observe(c, false, false, at(15.0)));
        assert!(!rule.observe(a, false, false, at(16.0)));
        assert!(!rule.observe(c, false, false, at(17.0)));
        assert!(!rule.observe(c, false, false, at(18.0)));
        assert!(rule.observe(c, false, false, at(19.0)));
        // Back to the first set, all located: a change, saved again.
        assert!(rule.observe(a, true, false, at(20.0)));
    }

    #[test]
    fn a_replayed_room_is_never_saved() {
        let t0 = Instant::now();
        let mut rule = SaveRule::default();
        for s in 0..10 {
            let now = t0 + Duration::from_secs(s);
            assert!(!rule.observe(Some(0xa), true, true, now));
            assert!(!rule.observe(Some(0xa), false, true, now));
        }
        // The live room after the replay is saved as any other.
        assert!(rule.observe(Some(0xa), true, false, t0 + Duration::from_secs(11)));
    }

    #[test]
    fn the_replay_is_on_only_for_the_exact_value() {
        assert!(replay_knob(Some("replay")));
        for v in [
            None,
            Some(""),
            Some("1"),
            Some("Replay"),
            Some(" replay"),
            Some("save"),
        ] {
            assert!(!replay_knob(v), "{v:?}");
        }
    }

    #[test]
    fn the_replay_loads_once_and_live_anchors_replace_it() {
        // Off: never.
        let mut off = Replay::new(false);
        assert!(!off.should_load(0));
        assert!(!off.live_anchors());
        // On, live anchors held: not needed.
        let mut on = Replay::new(true);
        assert!(!on.should_load(4));
        // On, an empty query: read once.
        assert!(on.should_load(0));
        assert!(!on.active());
        on.loaded();
        assert!(on.active());
        // Later empty retries do not read it again.
        assert!(!on.should_load(0));
        // Live anchors win, once.
        assert!(on.live_anchors());
        assert!(!on.active());
        assert!(!on.live_anchors());
        // A file that failed to read is not retried on every empty query.
        let mut failed = Replay::new(true);
        assert!(failed.should_load(0));
        assert!(!failed.should_load(0));
        assert!(!failed.active());
    }
}
