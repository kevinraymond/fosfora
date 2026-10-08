//! I5 hands-first gestures on top of the pinch detector: one pinch at a time
//! becomes a tap, a drag or a hold. Plain numbers in, so it builds and tests
//! on the desktop as well.
//!
//! A pinch starts as *pressed*. Moving the pinch point more than
//! [`DRAG_START_M`] from where it closed makes it a drag, which reports the
//! point's motion every frame until release (the first report carries the
//! motion since the pinch closed, so a grab loses none of it). Staying
//! within that radius for [`HOLD_S`] makes it a hold, which fires once and
//! then waits for the release. Releasing before either is a tap.
//!
//! The hand that closes a pinch owns the gesture until it opens; the other
//! hand is ignored meanwhile, so two hands never fight over one anchor.

use glam::Vec3;

/// Pinch-point travel (meters) that turns a pinch into a drag. Above the
/// tracking jitter of a held pinch, below a deliberate hand move.
pub const DRAG_START_M: f32 = 0.025;
/// Seconds a pinch has to stay put to count as a hold. Only the room
/// editor consumes a hold (Edit room on, the right hand, `room_edit.rs`).
/// In the world a hold does nothing since board #3336: a still pinch,
/// aiming a throw or a hand at rest, cycled the world effect unasked, and
/// the effect cycle is the panel's `<` `>` row. The HUD still shows a
/// pinch's progress toward one.
pub const HOLD_S: f32 = 0.7;

/// One hand's pinch this frame, as the detector reports it.
#[derive(Debug, Clone, Copy, Default)]
pub struct PinchInput {
    pub pinching: bool,
    /// Midpoint of the thumb and index tips, when both are located.
    pub point: Option<[f32; 3]>,
}

/// What a frame's pinch input amounts to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gesture {
    /// A pinch closed and opened again without moving or holding.
    Tap { hand: usize },
    /// A drag began on `hand` (the frame's `Drag` follows in the same step).
    DragStart { hand: usize },
    /// The dragging pinch point moved by `delta` (meters) this frame.
    Drag { hand: usize, delta: [f32; 3] },
    /// The drag ended: the pinch opened or the hand was lost.
    DragEnd { hand: usize },
    /// A pinch held still for [`HOLD_S`].
    Hold { hand: usize },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Idle,
    Pressed {
        hand: usize,
        start: Vec3,
        held_s: f32,
    },
    Dragging {
        hand: usize,
        last: Vec3,
    },
    /// A hold fired; nothing more until the pinch opens.
    Held {
        hand: usize,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct Gestures {
    state: State,
}

impl Default for Gestures {
    fn default() -> Self {
        Self { state: State::Idle }
    }
}

impl Gestures {
    /// Advance by one frame of `dt` seconds with both hands' pinches (left,
    /// right). Returns at most two gestures (`DragStart` + its first `Drag`).
    pub fn step(&mut self, hands: [PinchInput; 2], dt: f32) -> Vec<Gesture> {
        let mut out = Vec::new();
        // A hand whose pinch point is lost counts as open for a drag (it must
        // not stick to a hand that left the tracking volume), but a press or
        // a hold rides out the gap: a tip that drops tracking for a frame
        // while the fingers stay pinched used to fire a Tap and restart the
        // hold timer, so one long pinch could Hold twice.
        let closed = |h: usize| hands[h].pinching && hands[h].point.is_some();
        let gap = |h: usize| hands[h].pinching && hands[h].point.is_none();
        let point = |h: usize| hands[h].point.map_or(Vec3::ZERO, Vec3::from);
        self.state = match self.state {
            State::Idle => {
                // The right hand wins a same-frame tie.
                match [1, 0].into_iter().find(|&h| closed(h)) {
                    Some(hand) => State::Pressed {
                        hand,
                        start: point(hand),
                        held_s: 0.0,
                    },
                    None => State::Idle,
                }
            }
            State::Pressed {
                hand,
                start,
                held_s,
            } => {
                if gap(hand) {
                    State::Pressed {
                        hand,
                        start,
                        held_s: held_s + dt,
                    }
                } else if !closed(hand) {
                    out.push(Gesture::Tap { hand });
                    State::Idle
                } else if point(hand).distance(start) > DRAG_START_M {
                    let p = point(hand);
                    out.push(Gesture::DragStart { hand });
                    out.push(Gesture::Drag {
                        hand,
                        delta: (p - start).to_array(),
                    });
                    State::Dragging { hand, last: p }
                } else if held_s + dt >= HOLD_S {
                    out.push(Gesture::Hold { hand });
                    State::Held { hand }
                } else {
                    State::Pressed {
                        hand,
                        start,
                        held_s: held_s + dt,
                    }
                }
            }
            State::Dragging { hand, last } => {
                if closed(hand) {
                    let p = point(hand);
                    out.push(Gesture::Drag {
                        hand,
                        delta: (p - last).to_array(),
                    });
                    State::Dragging { hand, last: p }
                } else {
                    out.push(Gesture::DragEnd { hand });
                    State::Idle
                }
            }
            State::Held { hand } => {
                if closed(hand) || gap(hand) {
                    State::Held { hand }
                } else {
                    State::Idle
                }
            }
        };
        out
    }

    /// Progress of a pinch toward a hold, 0..1, and the hand, while one is
    /// pressed (for a visible cue).
    pub fn hold_progress(&self) -> Option<(usize, f32)> {
        match self.state {
            State::Pressed { hand, held_s, .. } => Some((hand, (held_s / HOLD_S).min(1.0))),
            _ => None,
        }
    }

    /// A short description of the gesture in progress, for the debug panel.
    pub fn label(&self) -> String {
        let side = |h: usize| if h == 0 { "L" } else { "R" };
        match self.state {
            State::Idle => "idle".to_owned(),
            State::Pressed { hand, held_s, .. } => format!(
                "pinch {} (hold {:.0}%)",
                side(hand),
                (held_s / HOLD_S * 100.0).min(100.0)
            ),
            State::Dragging { hand, .. } => format!("drag {}", side(hand)),
            State::Held { hand } => format!("held {}", side(hand)),
        }
    }

    /// The hand whose pinch is a gesture in progress (pressed, dragging or
    /// held), if any.
    pub fn owner(&self) -> Option<usize> {
        match self.state {
            State::Idle => None,
            State::Pressed { hand, .. } | State::Dragging { hand, .. } | State::Held { hand } => {
                Some(hand)
            }
        }
    }

    /// The hand dragging, if any.
    pub fn dragging(&self) -> Option<usize> {
        match self.state {
            State::Dragging { hand, .. } => Some(hand),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 72.0;
    const OPEN: PinchInput = PinchInput {
        pinching: false,
        point: Some([0.0, 1.0, -0.4]),
    };

    fn pinch(p: [f32; 3]) -> PinchInput {
        PinchInput {
            pinching: true,
            point: Some(p),
        }
    }

    /// Frames of `dt` that fit in `s` seconds.
    fn frames(s: f32) -> usize {
        (s / DT) as usize
    }

    #[test]
    fn a_short_still_pinch_is_a_tap() {
        let mut g = Gestures::default();
        let p = [0.0, 1.0, -0.4];
        for _ in 0..frames(0.2) {
            assert!(g.step([OPEN, pinch(p)], DT).is_empty());
        }
        assert_eq!(g.step([OPEN, OPEN], DT), vec![Gesture::Tap { hand: 1 }]);
        assert!(g.step([OPEN, OPEN], DT).is_empty());
    }

    #[test]
    fn a_still_pinch_holds_once() {
        let mut g = Gestures::default();
        let p = [0.0, 1.0, -0.4];
        let mut holds = 0;
        for _ in 0..frames(3.0) {
            // Tracking jitter well inside the drag radius.
            let jitter = [p[0] + 0.004, p[1] - 0.003, p[2]];
            for e in g.step([pinch(jitter), OPEN], DT) {
                assert_eq!(e, Gesture::Hold { hand: 0 });
                holds += 1;
            }
        }
        assert_eq!(holds, 1);
        // Releasing a hold is not also a tap.
        assert!(g.step([OPEN, OPEN], DT).is_empty());
    }

    #[test]
    fn a_hold_fires_at_the_hold_time_not_before() {
        let mut g = Gestures::default();
        let p = [0.0, 1.0, -0.4];
        let mut fired_at = None;
        for i in 0..frames(2.0) {
            if !g.step([pinch(p), OPEN], DT).is_empty() {
                fired_at = Some(i as f32 * DT);
                break;
            }
        }
        let t = fired_at.expect("hold fired");
        assert!((HOLD_S - 2.0 * DT..=HOLD_S + 2.0 * DT).contains(&t), "{t}");
    }

    #[test]
    fn a_drag_reports_every_millimeter_of_the_motion() {
        let mut g = Gestures::default();
        let start = Vec3::new(0.1, 1.0, -0.4);
        let end = Vec3::new(0.4, 1.2, -0.6);
        let n = 60;
        let mut total = Vec3::ZERO;
        let mut started = false;
        for i in 0..=n {
            let p = start.lerp(end, i as f32 / n as f32);
            for e in g.step([OPEN, pinch(p.to_array())], DT) {
                match e {
                    Gesture::DragStart { hand: 1 } => started = true,
                    Gesture::Drag { hand: 1, delta } => total += Vec3::from(delta),
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
        assert!(started);
        // Includes the motion inside the drag radius before the drag began.
        assert!(
            total.distance(end - start) < 1e-5,
            "{total} vs {}",
            end - start
        );
        assert_eq!(g.step([OPEN, OPEN], DT), vec![Gesture::DragEnd { hand: 1 }]);
    }

    #[test]
    fn a_moving_pinch_never_holds() {
        let mut g = Gestures::default();
        for i in 0..frames(3.0) {
            let x = 0.3 * (i as f32 * DT).sin();
            for e in g.step([pinch([x, 1.0, -0.4]), OPEN], DT) {
                assert!(
                    !matches!(e, Gesture::Hold { .. } | Gesture::Tap { .. }),
                    "{e:?}"
                );
            }
        }
    }

    #[test]
    fn a_drag_ends_when_the_hand_is_lost() {
        let mut g = Gestures::default();
        g.step([pinch([0.0, 1.0, -0.4]), OPEN], DT);
        g.step([pinch([0.1, 1.0, -0.4]), OPEN], DT);
        assert_eq!(g.dragging(), Some(0));
        let lost = PinchInput {
            pinching: true,
            point: None,
        };
        assert_eq!(g.step([lost, OPEN], DT), vec![Gesture::DragEnd { hand: 0 }]);
        assert_eq!(g.dragging(), None);
    }

    #[test]
    fn the_other_hand_is_ignored_while_one_owns_the_gesture() {
        let mut g = Gestures::default();
        let a = [0.0, 1.0, -0.4];
        g.step([pinch(a), OPEN], DT);
        // The right hand pinches and moves a long way; the left stays put.
        for i in 0..frames(0.3) {
            let b = [0.5 + i as f32 * 0.01, 1.0, -0.4];
            assert!(g.step([pinch(a), pinch(b)], DT).is_empty());
        }
        assert_eq!(g.hold_progress().map(|(h, _)| h), Some(0));
    }
}
