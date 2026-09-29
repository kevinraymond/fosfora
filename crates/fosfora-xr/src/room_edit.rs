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
//! does nothing and says so ([`EditFrame::unaimed`]). The highlight pulses
//! once ([`PULSE_S`]) for a cycle, twice for a class assignment.

use glam::Vec3;

use crate::instruments::{Hit, RayBox, cast};

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
    fn the_ray_runs_from_the_far_pinch_away_from_the_head() {
        let r = Ray::through(HEAD, HEAD + Vec3::new(0.0, 0.0, -2.0)).unwrap();
        assert!(r.origin.abs_diff_eq(Vec3::new(0.0, 1.2, -2.0), 1e-6));
        assert!(r.dir.abs_diff_eq(Vec3::NEG_Z, 1e-6));
        assert_eq!(Ray::through(HEAD, HEAD), None);
    }
}
