//! The room editor (board #3326, step 2): point at a room surface with the
//! right far hand and pinch to change what it does. Plain data, so it
//! builds and tests on the desktop; `app.rs` feeds it the mode, the ray,
//! the boxes and the right hand's gestures each frame, applies its actions
//! through the lanes (`lanes.rs`) and draws its beam and highlight
//! (`highlight.rs`).
//!
//! **The ray** is the throw's: from the right far pinch point (the real
//! pinch point plus the seated reach's extension, so far walls are
//! reachable from the chair) along the line from the head through it
//! ([`Ray::through`]), cast against the room's boxes (and the stage floor
//! while the room has none) up to [`MAX_M`]. There is none while the hand
//! is untracked or the palm panel is up.
//!
//! **The hit** has hysteresis, so a ray jittering across a wall's thin
//! edge does not flicker the highlight: another box counts once the ray
//! has been on it for [`CONFIRM_S`], and the hit clears once the ray has
//! been off it for [`MISS_S`]. Until then the old hit holds.
//!
//! **The beam** runs from the far pinch point to where the ray meets a
//! box, or, with the hit held over a miss, as far as the hit was, or
//! [`MISS_BEAM_M`] along the ray.
//!
//! While the palm panel is up the editor is frozen: the hit and the
//! highlight stay, the beam goes, nothing ages and nothing fires, so the
//! wearer can turn the palm up and read the status cell for the surface
//! under the beam.
//!
//! **The gestures** act on the hit as it shows: a tap cycles its behavior
//! through what renders on its kind ([`EditAction::Cycle`]), a hold cycles
//! every surface of its kind one step from it ([`EditAction::AssignKind`]); either without a hit, or
//! without a ray (the hand lost, the panel up) while a hit is still held,
//! does nothing and says so ([`EditFrame::unaimed`]). A quick, short drag
//! counts as a tap ([`short_drag_is_tap`]; `app.rs` times it). The
//! highlight pulses once ([`PULSE_S`]) for a cycle, twice for a class
//! assignment.
//!
//! **The cloud toggle** (step 2c, [`Cloud`]): the hand menu's Cloud row.
//! Off, the world effect's emission goes to 0 through the density path,
//! the density kept so on restores it; off while Edit room is on, the
//! pointed surface is soloed: emission stays at the density, but every
//! other box's lane row goes to the sim as `none` ([`solo`]), so only the
//! hit spawns, at the full rate (step 2d), and the wearer sees its embers,
//! its sparks or nothing, alone. The particle system offers no way to clear the living cloud
//! without a core change, so turning the cloud off, or moving the solo to
//! another surface, leaves what is alive to die over its lifetime (12 s
//! for Flux XR Room). Kevin, worn, Sep 29: "Half the time I don't even
//! know what's happening because the giant particle cloud is everywhere";
//! with every table shedding into 400K living sprites, one table's change
//! was lost in the mass.

use glam::Vec3;

use crate::instruments::{Hit, RayBox, cast};
use crate::surfaces::{KIND_NONE, SURFACE_LANE_ROWS, SurfaceBehavior, lane_behavior, lane_row};

/// Seconds the ray has to stay on another box before it is the hit.
pub const CONFIRM_S: f32 = 0.15;
/// Seconds the ray has to stay off the hit before it clears.
pub const MISS_S: f32 = 0.3;
/// The cast's range (m): a room's far wall from the chair.
pub const MAX_M: f32 = 8.0;
/// The beam's length along the ray with nothing hit (m).
pub const MISS_BEAM_M: f32 = 3.0;
/// One highlight pulse (s): it starts at full and decays to nothing.
pub const PULSE_S: f32 = 0.3;
/// Pulses for a cycle and for a class assignment.
pub const CYCLE_PULSES: u32 = 1;
pub const CLASS_PULSES: u32 = 2;
/// A right drag shorter than this (s, from its start) and ...
pub const SHORT_DRAG_S: f32 = 0.5;
/// ... that ends nearer than this to where it started (m) is a tap.
pub const SHORT_DRAG_M: f32 = 0.10;

/// Whether a right drag that lasted `secs` from its start and ended
/// `travel_m` from where it started counts as a tap while Edit room is
/// on. A pinch that moves past `gesture::DRAG_START_M` (2.5 cm) before it
/// opens is a drag, which the editor has no use for: of Kevin's right
/// pinches, 16 in one worn pass and 2 in the next registered as drags and
/// did nothing (Sep 29). A quick one that stays within 10 cm is a tap
/// that wobbled; a slow or long one is not a tap.
pub fn short_drag_is_tap(secs: f32, travel_m: f32) -> bool {
    secs < SHORT_DRAG_S && travel_m < SHORT_DRAG_M
}

/// A pointing ray: `dir` is unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    /// The throw's ray: from the far pinch point `far_pinch`, along the
    /// line from `head` through it. `None` when the two coincide.
    pub fn through(head: Vec3, far_pinch: Vec3) -> Option<Self> {
        Some(Self {
            origin: far_pinch,
            dir: (far_pinch - head).try_normalize()?,
        })
    }
}

/// One frame of input.
#[derive(Debug, Clone, Copy)]
pub struct EditInput<'a> {
    /// Edit room is on.
    pub on: bool,
    /// The right hand's ray; `None` untracked or with the panel up.
    pub ray: Option<Ray>,
    /// The palm panel is up: the hit and the highlight hold as they are
    /// (nothing ages, nothing fires), so the wearer can turn the palm to
    /// read the status cell without the surface going to "no surface".
    pub frozen: bool,
    /// The boxes to cast against: the room's, then the stage floor while
    /// the room has no floor (the lanes' order, so an index is a lane's).
    pub boxes: &'a [RayBox],
    /// The right hand tapped / held this frame.
    pub tap: bool,
    pub hold: bool,
    pub dt: f32,
}

/// What a gesture asks of the lanes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAction {
    /// Box k one step through its kind's catalogue
    /// (`surfaces::SurfaceBehavior::next_for`).
    Cycle(usize),
    /// Every surface of box k's kind one step past k's behavior (the
    /// class cycle; `lanes::RoomLanes::cycle_kind_of`).
    AssignKind(usize),
}

/// The editor's frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EditFrame {
    /// The hit, as the highlight shows it.
    pub hit: Option<Hit>,
    /// The hit went to another box, came or went this frame.
    pub changed: bool,
    /// The beam, start and end.
    pub beam: Option<(Vec3, Vec3)>,
    pub action: Option<EditAction>,
    /// A gesture this frame with nothing hit (`"tap"` or `"hold"`), for
    /// the log.
    pub unaimed: Option<&'static str>,
    /// The highlight's pulse, 0..1.
    pub pulse: f32,
}

/// The editor's state across frames.
#[derive(Debug, Clone, Copy, Default)]
pub struct RoomEditor {
    hit: Option<Hit>,
    /// Another box under the ray and for how long.
    candidate: Option<(usize, f32)>,
    /// How long the ray has been off the hit.
    off_s: f32,
    /// The pulse's age and its count.
    pulse: Option<(f32, u32)>,
}

impl RoomEditor {
    /// Advance one frame. With the mode off everything clears and the
    /// frame is empty.
    pub fn step(&mut self, input: &EditInput<'_>) -> EditFrame {
        let before = self.hit.map(|h| h.index);
        if !input.on {
            *self = Self::default();
            return EditFrame {
                changed: before.is_some(),
                ..EditFrame::default()
            };
        }
        if input.frozen {
            return EditFrame {
                hit: self.hit,
                changed: false,
                beam: None,
                action: None,
                unaimed: None,
                pulse: self.pulse_now(),
            };
        }
        let dt = input.dt.max(0.0);
        // A rescan can shrink the box list under a held hit.
        if self.hit.is_some_and(|h| h.index >= input.boxes.len()) {
            self.hit = None;
        }
        let raw = input
            .ray
            .and_then(|r| cast(r.origin, r.dir, input.boxes, MAX_M));
        match raw {
            Some(h) if before == Some(h.index) && self.hit.is_some() => {
                self.hit = Some(h);
                self.candidate = None;
                self.off_s = 0.0;
            }
            Some(h) => {
                let held = match self.candidate {
                    Some((i, s)) if i == h.index => s + dt,
                    _ => dt,
                };
                if held >= CONFIRM_S {
                    self.hit = Some(h);
                    self.candidate = None;
                    self.off_s = 0.0;
                } else {
                    self.candidate = Some((h.index, held));
                    self.off_s += dt;
                }
            }
            None => {
                self.candidate = None;
                self.off_s += dt;
            }
        }
        if self.off_s >= MISS_S {
            self.hit = None;
        }
        let beam = input.ray.map(|r| {
            let reach = raw
                .or(self.hit)
                .map_or(MISS_BEAM_M, |h| h.distance.min(MAX_M));
            (r.origin, r.origin + r.dir * reach)
        });
        // The pulse ages, then a gesture may start another.
        self.pulse = self
            .pulse
            .map(|(age, n)| (age + dt, n))
            .filter(|&(age, n)| age < PULSE_S * n as f32);
        // A tap and a hold never fire in one frame (`gesture.rs`); a hold
        // would win.
        let gesture = if input.hold {
            Some((
                "hold",
                EditAction::AssignKind as fn(usize) -> EditAction,
                CLASS_PULSES,
            ))
        } else if input.tap {
            Some((
                "tap",
                EditAction::Cycle as fn(usize) -> EditAction,
                CYCLE_PULSES,
            ))
        } else {
            None
        };
        let (mut action, mut unaimed) = (None, None);
        if let Some((name, make, pulses)) = gesture {
            match self.hit.filter(|_| input.ray.is_some()) {
                Some(h) => {
                    action = Some(make(h.index));
                    self.pulse = Some((0.0, pulses));
                }
                None => unaimed = Some(name),
            }
        }
        EditFrame {
            hit: self.hit,
            changed: self.hit.map(|h| h.index) != before,
            beam,
            action,
            unaimed,
            pulse: self.pulse_now(),
        }
    }

    /// The hit as the highlight shows it.
    pub fn hit(&self) -> Option<Hit> {
        self.hit
    }

    /// The pulse now, 0..1: each of its pulses starts at 1 and falls
    /// linearly to 0 over [`PULSE_S`].
    fn pulse_now(&self) -> f32 {
        self.pulse
            .map_or(0.0, |(age, _)| 1.0 - (age % PULSE_S) / PULSE_S)
    }
}

/// What the world effect's cloud runs this frame, from the hand menu's
/// Cloud toggle and Edit room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cloud {
    /// Emission at the density, the lane rows as the lanes give them.
    On,
    /// Emission 0, the density kept.
    Off,
    /// Edit room with the cloud off: emission at the density, only box
    /// `k` (the editor's hit) spawning; with no hit nothing spawns.
    Solo(Option<usize>),
}

impl Cloud {
    /// The cloud for the toggle `cloud`, Edit room `edit_room` and the
    /// editor's hit (a lane index).
    pub fn of(cloud: bool, edit_room: bool, hit: Option<usize>) -> Self {
        match (cloud, edit_room) {
            (true, _) => Self::On,
            (false, false) => Self::Off,
            (false, true) => Self::Solo(hit),
        }
    }

    /// The world effect's emission against its preset's, for the cloud
    /// density `density`: 0 off, else the density.
    pub fn emission(self, density: f32) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::On | Self::Solo(_) => density,
        }
    }

    /// The lane rows to upload for the lanes' `rows`: soloed
    /// ([`solo`]) while soloing, else as they are. A copy, so the lanes'
    /// own rows and the room file never change.
    pub fn rows(self, rows: &[[f32; 4]; SURFACE_LANE_ROWS]) -> [[f32; 4]; SURFACE_LANE_ROWS] {
        match self {
            Self::Solo(hit) => solo(rows, hit),
            Self::On | Self::Off => *rows,
        }
    }

    /// The log line for a change to this cloud, `name` naming a box
    /// (`surfaces::friendly_name`).
    pub fn describe(self, name: impl FnOnce(usize) -> String) -> String {
        match self {
            Self::On => "cloud on".to_owned(),
            Self::Off => "cloud off: emission 0, the cloud fades over the lifetime".to_owned(),
            Self::Solo(Some(k)) => format!("cloud off, edit room: solo {}", name(k)),
            Self::Solo(None) => {
                "cloud off, edit room: solo, no surface (nothing spawns)".to_owned()
            }
        }
    }
}

/// `rows` soloed on `hit`, so only the hit spawns. With a hit that spawns
/// (its row sets embers or sparks), every other row becomes `none` at
/// strength 0 and the hit's is left as it is: the weights, which follow the
/// behaviors, then see one emitting box, the largest emitting table (the
/// reference, step 2c) or the only box of its kind, and every draw lands
/// on it, so it spawns at the full rate (step 2d; 2c kept the others'
/// behaviors, so the hit spawned only its share of the room's weight,
/// about 1.0 / 1.85 in Kevin's room). With no hit, or a hit that spawns
/// nothing (the ripple, the spectrum, `none`, an unset row), every other
/// row keeps its behavior at strength 0 (2c's rule): the weights stay on
/// the boxes that emit, a draw that lands on one spawns nothing (the sim
/// multiplies a box's gate by its lane's strength), and the sim never
/// falls back to the volume, which it does when no box weighs anything.
/// An unset row there (all zero: the sim would run its kind's default at
/// full, whatever the strength) becomes `none` at 0.
pub fn solo(
    rows: &[[f32; 4]; SURFACE_LANE_ROWS],
    hit: Option<usize>,
) -> [[f32; 4]; SURFACE_LANE_ROWS] {
    let spawns = |row: &[f32; 4]| row[0] > 0.5 && lane_behavior(*row, KIND_NONE).emits();
    let alone = hit.and_then(|k| rows.get(k)).is_some_and(spawns);
    let mut out = *rows;
    for (k, row) in out.iter_mut().enumerate() {
        if Some(k) == hit {
            continue;
        }
        if alone || row[0] <= 0.5 {
            *row = lane_row(SurfaceBehavior::None, 0.0, [row[2], row[3]]);
        } else {
            row[1] = 0.0;
        }
    }
    out
}

/// The emission to apply for `cloud` at `density` when it differs from
/// the one `applied` last (the app's density path): `None` while it holds.
pub fn emission_change(applied: f32, cloud: Cloud, density: f32) -> Option<f32> {
    let want = cloud.emission(density);
    ((applied - want).abs() > 1e-4).then_some(want)
}

/// The cloud toggle at launch from the `debug.fosfora.cloud` knob's
/// value: "0" off, anything else (unset too) on.
pub fn cloud_knob(value: Option<&str>) -> bool {
    value.map(str::trim) != Some("0")
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;

    const DT: f32 = 1.0 / 72.0;
    const HEAD: Vec3 = Vec3::new(0.0, 1.2, 0.0);

    /// A wall 2 m ahead (z = -2, 4 cm thick, local +Z into the room), a
    /// table to the right and a wall to the left.
    fn boxes() -> Vec<RayBox> {
        vec![
            RayBox {
                center: Vec3::new(0.0, 1.25, -2.0),
                rot: Quat::IDENTITY,
                half: Vec3::new(2.0, 1.25, 0.02),
                kind: crate::surfaces::KIND_WALL,
            },
            RayBox {
                center: Vec3::new(1.0, 0.4, -0.8),
                rot: Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
                half: Vec3::new(0.5, 0.3, 0.37),
                kind: crate::surfaces::KIND_TABLE,
            },
            RayBox {
                center: Vec3::new(-2.0, 1.25, 0.0),
                rot: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
                half: Vec3::new(2.0, 1.25, 0.02),
                kind: crate::surfaces::KIND_WALL,
            },
        ]
    }

    /// The ray through a far pinch point 0.5 m from the head toward `at`.
    fn toward(at: Vec3) -> Option<Ray> {
        Ray::through(HEAD, HEAD + (at - HEAD).normalize() * 0.5)
    }

    const WALL: Vec3 = Vec3::new(0.3, 1.4, -2.0);
    const TABLE: Vec3 = Vec3::new(1.0, 0.77, -0.8);
    const NOWHERE: Vec3 = Vec3::new(0.0, 1.2, 5.0);

    fn input(boxes: &[RayBox], at: Vec3) -> EditInput<'_> {
        EditInput {
            on: true,
            ray: toward(at),
            boxes,
            frozen: false,
            tap: false,
            hold: false,
            dt: DT,
        }
    }

    /// `n` frames pointing at `at`; the last frame.
    fn point(e: &mut RoomEditor, boxes: &[RayBox], at: Vec3, n: usize) -> EditFrame {
        let mut f = EditFrame::default();
        for _ in 0..n {
            f = e.step(&input(boxes, at));
        }
        f
    }

    /// Frames to cover `s` seconds at 72 Hz, rounded up.
    fn frames(s: f32) -> usize {
        (s / DT).ceil() as usize
    }

    #[test]
    fn a_box_counts_after_the_confirmation_time() {
        let b = boxes();
        let mut e = RoomEditor::default();
        // Just short: nothing yet.
        let f = point(&mut e, &b, WALL, frames(CONFIRM_S) - 1);
        assert_eq!(f.hit, None);
        assert!(!f.changed);
        // The next frame crosses it.
        let f = point(&mut e, &b, WALL, 1);
        assert_eq!(f.hit.map(|h| h.index), Some(0));
        assert!(f.changed);
        // Steady: no change.
        let f = point(&mut e, &b, WALL, 10);
        assert!(!f.changed && f.hit.is_some());
        // Another box takes over only after its own confirmation; the old
        // hit holds meanwhile.
        let f = point(&mut e, &b, TABLE, frames(CONFIRM_S) - 1);
        assert_eq!(f.hit.map(|h| h.index), Some(0));
        let f = point(&mut e, &b, TABLE, 1);
        assert_eq!(f.hit.map(|h| h.index), Some(1));
        assert!(f.changed);
        // The table from above: its top face.
        assert!(f.hit.unwrap().normal.abs_diff_eq(Vec3::Y, 1e-5));
    }

    #[test]
    fn a_miss_clears_the_hit_only_after_the_miss_time() {
        let b = boxes();
        let mut e = RoomEditor::default();
        point(&mut e, &b, WALL, frames(CONFIRM_S));
        let f = point(&mut e, &b, NOWHERE, frames(MISS_S) - 1);
        assert_eq!(f.hit.map(|h| h.index), Some(0), "held over the miss");
        let f = point(&mut e, &b, NOWHERE, 1);
        assert_eq!(f.hit, None);
        assert!(f.changed);
        // An untracked hand is a miss too.
        point(&mut e, &b, WALL, frames(CONFIRM_S));
        let mut gone = input(&b, WALL);
        gone.ray = None;
        for _ in 0..frames(MISS_S) - 1 {
            assert!(e.step(&gone).hit.is_some());
        }
        assert_eq!(e.step(&gone).hit, None);
    }

    #[test]
    fn jitter_across_a_thin_edge_does_not_flicker() {
        let b = boxes();
        let mut e = RoomEditor::default();
        point(&mut e, &b, WALL, frames(CONFIRM_S));
        // A second of the ray alternating on and off the wall, and on and
        // off the table: the wall stays, no change reported.
        for i in 0..72 {
            let at = match i % 4 {
                0 | 2 => WALL,
                1 => NOWHERE,
                _ => TABLE,
            };
            let f = e.step(&input(&b, at));
            assert_eq!(f.hit.map(|h| h.index), Some(0), "frame {i}");
            assert!(!f.changed, "frame {i}");
        }
    }

    #[test]
    fn the_panel_up_freezes_the_hit_and_fires_nothing() {
        let b = boxes();
        let mut e = RoomEditor::default();
        point(&mut e, &b, TABLE, frames(CONFIRM_S));
        assert_eq!(e.hit().map(|h| h.index), Some(1));
        // Frozen far longer than a miss clears, with a gesture and no
        // ray: the hit holds, nothing fires, no beam, no "unaimed".
        let mut f = EditFrame::default();
        for _ in 0..frames(MISS_S * 4.0) {
            f = e.step(&EditInput {
                ray: None,
                frozen: true,
                tap: true,
                ..input(&b, TABLE)
            });
        }
        assert_eq!(f.hit.map(|h| h.index), Some(1));
        assert!(!f.changed);
        assert_eq!(f.beam, None);
        assert_eq!(f.action, None);
        assert_eq!(f.unaimed, None);
        // Unfrozen with the ray back on the table: still the hit, at once.
        let f = e.step(&input(&b, TABLE));
        assert_eq!(f.hit.map(|h| h.index), Some(1));
        assert!(!f.changed);
        assert!(f.beam.is_some());
    }

    #[test]
    fn a_tap_cycles_and_a_hold_assigns_the_kind_on_the_hit() {
        let b = boxes();
        let mut e = RoomEditor::default();
        point(&mut e, &b, TABLE, frames(CONFIRM_S));
        let f = e.step(&EditInput {
            tap: true,
            ..input(&b, TABLE)
        });
        assert_eq!(f.action, Some(EditAction::Cycle(1)));
        assert_eq!(f.unaimed, None);
        let f = e.step(&EditInput {
            hold: true,
            ..input(&b, TABLE)
        });
        assert_eq!(f.action, Some(EditAction::AssignKind(1)));
        // A gesture on the held hit during a short miss still acts on it:
        // it is what the highlight shows.
        point(&mut e, &b, NOWHERE, 3);
        let f = e.step(&EditInput {
            tap: true,
            ..input(&b, NOWHERE)
        });
        assert_eq!(f.action, Some(EditAction::Cycle(1)));
    }

    #[test]
    fn nothing_happens_without_a_hit() {
        let b = boxes();
        let mut e = RoomEditor::default();
        let f = e.step(&EditInput {
            tap: true,
            ..input(&b, NOWHERE)
        });
        assert_eq!((f.action, f.unaimed), (None, Some("tap")));
        let f = e.step(&EditInput {
            hold: true,
            ..input(&b, NOWHERE)
        });
        assert_eq!((f.action, f.unaimed), (None, Some("hold")));
        assert_close!(f.pulse, 0.0);
        // A box under the ray but not yet confirmed is no hit either.
        point(&mut e, &b, WALL, 2);
        let f = e.step(&EditInput {
            tap: true,
            ..input(&b, WALL)
        });
        assert_eq!((f.action, f.unaimed), (None, Some("tap")));
        // Without a ray (the panel up, the hand lost) a held hit is not
        // acted on.
        point(&mut e, &b, WALL, frames(CONFIRM_S));
        let f = e.step(&EditInput {
            ray: None,
            tap: true,
            ..input(&b, WALL)
        });
        assert!(f.hit.is_some());
        assert_eq!((f.action, f.unaimed), (None, Some("tap")));
        // With the mode off, nothing at all.
        point(&mut e, &b, WALL, frames(CONFIRM_S));
        let f = e.step(&EditInput {
            on: false,
            tap: true,
            ..input(&b, WALL)
        });
        assert_eq!(
            f,
            EditFrame {
                changed: true,
                ..EditFrame::default()
            }
        );
        assert_eq!(e.hit(), None);
    }

    #[test]
    fn the_beam_ends_on_the_surface_or_three_meters_out() {
        let b = boxes();
        let mut e = RoomEditor::default();
        // A miss with nothing held: 3 m along the ray from the far pinch.
        let f = point(&mut e, &b, NOWHERE, 1);
        let ray = toward(NOWHERE).unwrap();
        let (start, end) = f.beam.unwrap();
        assert!(start.abs_diff_eq(ray.origin, 1e-6));
        assert!(end.abs_diff_eq(ray.origin + ray.dir * MISS_BEAM_M, 1e-5));
        // On the wall, even before it is confirmed: the wall's face.
        let f = point(&mut e, &b, WALL, 1);
        let (_, end) = f.beam.unwrap();
        assert!((end.z + 1.98).abs() < 1e-4, "{end}");
        // Held over a miss: as far as the hit was.
        point(&mut e, &b, WALL, frames(CONFIRM_S));
        let f = point(&mut e, &b, NOWHERE, 2);
        let (start, end) = f.beam.unwrap();
        let held = e.hit().unwrap().distance;
        assert!((start.distance(end) - held).abs() < 1e-4);
        // No ray, no beam.
        let mut gone = input(&b, WALL);
        gone.ray = None;
        assert_eq!(e.step(&gone).beam, None);
    }

    #[test]
    fn a_cycle_pulses_once_and_a_class_assignment_twice() {
        let b = boxes();
        let mut e = RoomEditor::default();
        point(&mut e, &b, WALL, frames(CONFIRM_S));
        let f = e.step(&EditInput {
            tap: true,
            ..input(&b, WALL)
        });
        assert_close!(f.pulse, 1.0);
        let half = point(&mut e, &b, WALL, frames(PULSE_S * 0.5));
        assert!((half.pulse - 0.5).abs() < 0.05, "{}", half.pulse);
        let done = point(&mut e, &b, WALL, frames(PULSE_S * 0.5) + 1);
        assert_close!(done.pulse, 0.0);
        // A hold: two pulses, the second starting again at full.
        e.step(&EditInput {
            hold: true,
            ..input(&b, WALL)
        });
        let mut peaks = 0;
        let mut last = 1.0;
        for _ in 0..frames(PULSE_S * 2.0) + 2 {
            let p = point(&mut e, &b, WALL, 1).pulse;
            if p > last + 0.5 {
                peaks += 1;
            }
            last = p;
        }
        assert_eq!(peaks, 1, "one more pulse after the first");
        assert_close!(last, 0.0);
    }

    #[test]
    fn a_box_list_that_shrinks_drops_the_hit() {
        let b = boxes();
        let mut e = RoomEditor::default();
        point(&mut e, &b, TABLE, frames(CONFIRM_S));
        let f = e.step(&input(&b[..1], NOWHERE));
        assert_eq!(f.hit, None);
        assert!(f.changed);
    }

    #[test]
    fn a_short_drag_is_a_tap_and_a_long_or_far_one_is_not() {
        assert!(short_drag_is_tap(0.0, 0.0));
        assert!(short_drag_is_tap(0.2, 0.04));
        assert!(short_drag_is_tap(0.49, 0.099));
        // Too long, too far, or both.
        assert!(!short_drag_is_tap(0.5, 0.04));
        assert!(!short_drag_is_tap(0.2, 0.10));
        assert!(!short_drag_is_tap(1.2, 0.3));
    }

    #[test]
    fn the_ray_runs_from_the_far_pinch_away_from_the_head() {
        let r = Ray::through(HEAD, HEAD + Vec3::new(0.0, 0.0, -2.0)).unwrap();
        assert!(r.origin.abs_diff_eq(Vec3::new(0.0, 1.2, -2.0), 1e-6));
        assert!(r.dir.abs_diff_eq(Vec3::NEG_Z, 1e-6));
        assert_eq!(Ray::through(HEAD, HEAD), None);
    }

    #[test]
    fn the_cloud_follows_the_toggle_and_edit_room() {
        assert_eq!(Cloud::of(true, false, None), Cloud::On);
        assert_eq!(Cloud::of(true, true, Some(3)), Cloud::On);
        assert_eq!(Cloud::of(false, false, Some(3)), Cloud::Off);
        assert_eq!(Cloud::of(false, true, Some(3)), Cloud::Solo(Some(3)));
        assert_eq!(Cloud::of(false, true, None), Cloud::Solo(None));
        let name = |k: usize| format!("desk {k}");
        assert_eq!(Cloud::On.describe(name), "cloud on");
        assert_eq!(
            Cloud::Off.describe(name),
            "cloud off: emission 0, the cloud fades over the lifetime"
        );
        assert_eq!(
            Cloud::Solo(Some(2)).describe(name),
            "cloud off, edit room: solo desk 2"
        );
        assert_eq!(
            Cloud::Solo(None).describe(name),
            "cloud off, edit room: solo, no surface (nothing spawns)"
        );
    }

    #[test]
    fn cloud_off_keeps_the_density_and_on_restores_it() {
        // The app's density path: the emission applied, changed only when
        // the cloud or the density asks for another.
        let mut density = 0.4;
        let mut applied = 1.0;
        let mut run = |cloud: Cloud, density: f32| {
            if let Some(e) = emission_change(applied, cloud, density) {
                applied = e;
            }
            applied
        };
        assert_close!(run(Cloud::On, density), 0.4);
        assert_close!(run(Cloud::Off, density), 0.0);
        // The stepper moves while off: the density changes, the emission
        // stays 0.
        density = 0.65;
        assert_close!(run(Cloud::Off, density), 0.0);
        // Solo runs at the density; off again, 0; on, the new density.
        assert_close!(run(Cloud::Solo(Some(1)), density), 0.65);
        assert_close!(run(Cloud::Solo(None), density), 0.65);
        assert_close!(run(Cloud::Off, density), 0.0);
        assert_close!(run(Cloud::On, density), 0.65);
        // Nothing to apply while it holds.
        assert_eq!(emission_change(0.65, Cloud::On, 0.65), None);
        assert_eq!(emission_change(0.0, Cloud::Off, 0.65), None);
        assert_eq!(emission_change(0.65, Cloud::Off, 0.65), Some(0.0));
    }

    #[test]
    fn solo_leaves_only_the_hit_spawning() {
        use SurfaceBehavior as B;
        let mut rows = [[0.0; 4]; SURFACE_LANE_ROWS];
        rows[0] = lane_row(B::Spectrum, 1.0, [0.0; 2]);
        rows[1] = lane_row(B::Embers, 1.0, [0.0; 2]);
        rows[2] = lane_row(B::Sparks, 0.5, [0.25, 0.75]);
        rows[3] = lane_row(B::Embers, 0.8, [0.0; 2]);
        let soloed = solo(&rows, Some(3));
        // The hit as it was.
        assert_close!(soloed[3], rows[3]);
        // Step 2d: every other row none at 0, its parameters kept, so the
        // weights see the hit alone.
        for k in (0..3).chain(4..SURFACE_LANE_ROWS) {
            assert_close!(soloed[k], lane_row(B::None, 0.0, [rows[k][2], rows[k][3]]));
        }
        // A hit that spawns nothing (the wall's spectrum, an unset row) and
        // no hit keep 2c's rule: the others' behaviors at strength 0, so the
        // weights stay on the emitting boxes and nothing spawns.
        for hit in [Some(0), Some(4), None] {
            let kept = solo(&rows, hit);
            for k in 0..4 {
                if Some(k) == hit {
                    assert_close!(kept[k], rows[k]);
                    continue;
                }
                assert_close!(kept[k][1], 0.0);
                assert_close!(kept[k][0], rows[k][0]);
                assert_close!(kept[k][2..], rows[k][2..]);
            }
            // An unset row runs its kind's gate whatever its strength: none
            // at 0 instead.
            if hit != Some(4) {
                assert_close!(kept[4], lane_row(B::None, 0.0, [0.0; 2]));
            }
        }
        // A copy: the lanes' rows are untouched.
        assert_close!(rows[1], lane_row(B::Embers, 1.0, [0.0; 2]));
        // The cloud's rows: soloed only while soloing.
        assert_close!(Cloud::Solo(Some(3)).rows(&rows)[..], soloed[..]);
        assert_close!(Cloud::On.rows(&rows)[..], rows[..]);
        assert_close!(Cloud::Off.rows(&rows)[..], rows[..]);
    }

    #[test]
    fn a_soloed_surface_takes_the_whole_weight() {
        use crate::surfaces::{
            KIND_FLOOR, KIND_TABLE, KIND_WALL, SurfaceBox, SurfaceWeights, lane_emitter_weights,
        };
        use SurfaceBehavior as B;
        // The weights as `ObstacleSet::set_emitter_weights` takes them from
        // the uploaded rows: a desk, a side table half its top, a scene
        // floor on sparks, a wall on the spectrum and the stage floor (its
        // flag off beside a scene floor) on its default, the ripple.
        let up = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        let flat = |kind, center: Vec3, half: Vec3, emit| SurfaceBox {
            kind,
            emit,
            behavior: B::None,
            center,
            rot: up,
            half,
        };
        let boxes = [
            flat(
                KIND_TABLE,
                Vec3::new(0.0, -0.8, -0.6),
                Vec3::new(0.8, 0.4, 0.37),
                1.0,
            ),
            flat(
                KIND_TABLE,
                Vec3::new(0.8, -0.8, 0.3),
                Vec3::new(0.4, 0.4, 0.37),
                1.0,
            ),
            flat(
                KIND_FLOOR,
                Vec3::new(0.0, -1.22, 0.0),
                Vec3::new(2.0, 1.5, 0.02),
                1.0,
            ),
            SurfaceBox {
                rot: Quat::IDENTITY,
                ..flat(
                    KIND_WALL,
                    Vec3::new(0.0, 0.0, -1.4),
                    Vec3::new(2.0, 1.25, 0.02),
                    1.0,
                )
            },
            SurfaceBox {
                rot: Quat::IDENTITY,
                ..flat(
                    KIND_FLOOR,
                    Vec3::new(0.0, -1.25, 0.0),
                    Vec3::new(10.0, 0.05, 10.0),
                    0.0,
                )
            },
        ];
        let mut rows = [[0.0; 4]; SURFACE_LANE_ROWS];
        for (k, b) in [B::Embers, B::Embers, B::Sparks, B::Spectrum, B::Ripple]
            .into_iter()
            .enumerate()
        {
            rows[k] = lane_row(b, 1.0, [0.0; 2]);
        }
        let weigh = |rows: &[[f32; 4]; SURFACE_LANE_ROWS]| {
            let mut b = boxes;
            let mut out = [0.0; 5];
            lane_emitter_weights(&mut b, rows, 1.5, SurfaceWeights::default(), &mut out);
            out
        };
        // Unsoloed: the desk 1, the side table half, the floor its 0.5.
        assert_close!(weigh(&rows), [1.0, 0.5, 0.5, 0.0, 0.0]);
        // Soloed on the side table: the only emitting table, the reference,
        // so it weighs 1 and every draw lands on it.
        assert_close!(weigh(&solo(&rows, Some(1))), [0.0, 1.0, 0.0, 0.0, 0.0]);
        assert_close!(weigh(&solo(&rows, Some(0))), [1.0, 0.0, 0.0, 0.0, 0.0]);
        // On the floor on sparks: its own weight, and nothing else weighs.
        assert_close!(weigh(&solo(&rows, Some(2))), [0.0, 0.0, 0.5, 0.0, 0.0]);
        // On the wall (the spectrum spawns nothing), or on nothing: the
        // weights stay where they were, so the sim does not fall back to
        // the volume, and every strength but the hit's is 0.
        for hit in [Some(3), None] {
            let soloed = solo(&rows, hit);
            assert_close!(weigh(&soloed), [1.0, 0.5, 0.5, 0.0, 0.0]);
            assert!(
                (0..5).all(|k| Some(k) == hit || soloed[k][1] == 0.0),
                "{soloed:?}"
            );
        }
    }

    #[test]
    fn the_cloud_knob_turns_it_off_with_0_only() {
        assert!(cloud_knob(None));
        assert!(cloud_knob(Some("1")));
        assert!(cloud_knob(Some("")));
        assert!(cloud_knob(Some("yes")));
        assert!(!cloud_knob(Some("0")));
        assert!(!cloud_knob(Some(" 0 ")));
    }
}
