//! Where the debug panel sits and how a fingertip presses it. Plain
//! numbers in, so it builds and tests on the desktop as well.
//!
//! The panel belongs to the left hand: it appears while the left palm faces
//! the wearer, floats just above the hand turned toward the eyes, and hides
//! when the palm turns away. The right index fingertip pokes it. While that
//! fingertip is near the panel the panel stops following the hand, so it
//! holds still under the finger instead of wobbling with the other hand.

use glam::{Quat, Vec3};

/// Panel size (meters).
pub const PANEL_W_M: f32 = 0.20;
pub const PANEL_H_M: f32 = 0.30;
/// Palm-facing thresholds: the cosine between the palm normal and the
/// direction to the head. Show above `SHOW`, hide below `HIDE`.
const SHOW: f32 = 0.6;
const HIDE: f32 = 0.3;
/// Panel center relative to the palm: toward the head and up.
const TOWARD_HEAD_M: f32 = 0.06;
const ABOVE_PALM_M: f32 = 0.16;
/// Per-frame blend toward the target pose (hand-tracking jitter filter).
const FOLLOW: f32 = 0.35;
/// Fingertip-surface distance (meters, positive in front) that presses the
/// panel, and the one that releases it.
const PRESS_M: f32 = 0.004;
const RELEASE_M: f32 = 0.012;

/// The panel's frame this frame: center, unit right/up axes and the unit
/// normal toward the viewer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub center: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub normal: Vec3,
}

impl Placement {
    /// Facing the point `eye` from `center`, with its up as close to world
    /// up as the facing allows.
    pub fn facing(center: Vec3, eye: Vec3) -> Self {
        let normal = (eye - center).normalize_or(Vec3::Z);
        let right = Vec3::Y.cross(normal).normalize_or(Vec3::X);
        let up = normal.cross(right);
        Self {
            center,
            right,
            up,
            normal,
        }
    }

    /// Half-extent vectors for the renderer (center to right edge, center
    /// to top edge).
    pub fn half_vectors(&self) -> (Vec3, Vec3) {
        (self.right * (PANEL_W_M * 0.5), self.up * (PANEL_H_M * 0.5))
    }
}

/// A fingertip against the panel this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Touch {
    /// Pointer position in the panel, 0..1 from the top-left, while the
    /// fingertip hovers over or presses it.
    pub pointer: Option<[f32; 2]>,
    /// The fingertip is pressing the panel.
    pub pressed: bool,
    /// The fingertip is close enough that the panel should hold still.
    pub near: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PalmPanel {
    placement: Option<Placement>,
    shown: bool,
    pressed: bool,
    /// Last frame's fingertip-surface distance, to require a press from
    /// the front.
    last_depth: Option<f32>,
    /// Last frame's touch, whose `near` pins the panel.
    touch: Touch,
}

impl PalmPanel {
    /// Update the panel for this frame's left palm (position and rotation)
    /// and head position. Returns where to draw it, or `None` when hidden.
    pub fn place(&mut self, palm: Option<(Vec3, Quat)>, head: Vec3) -> Option<Placement> {
        let Some((pos, rot)) = palm else {
            // A pressed panel stays for the finger even if the left hand
            // drops out of tracking for a moment.
            if !self.touch.near {
                self.shown = false;
            }
            return self.visible();
        };
        let normal = rot * Vec3::NEG_Y;
        let facing = normal.dot((head - pos).normalize_or_zero());
        if self.shown {
            if facing < HIDE && !self.touch.near {
                self.shown = false;
            }
        } else if facing > SHOW {
            self.shown = true;
            // Appear where the hand is, not sliding in from the last spot.
            self.placement = None;
        }
        if !self.shown {
            return None;
        }
        if self.touch.near && self.placement.is_some() {
            return self.visible();
        }
        let to_head = (head - pos).normalize_or(Vec3::Z);
        let target =
            Placement::facing(pos + to_head * TOWARD_HEAD_M + Vec3::Y * ABOVE_PALM_M, head);
        let next = match self.placement {
            Some(p) => Placement::facing(p.center.lerp(target.center, FOLLOW), head),
            None => target,
        };
        self.placement = Some(next);
        self.visible()
    }

    fn visible(&self) -> Option<Placement> {
        self.placement.filter(|_| self.shown)
    }

    /// Where the fingertip (center and joint radius) is against the panel.
    /// Call after [`Self::place`].
    pub fn touch(&mut self, tip: Option<(Vec3, f32)>) -> Touch {
        let (Some(p), Some((tip, radius))) = (self.visible(), tip) else {
            self.pressed = false;
            self.last_depth = None;
            self.touch = Touch::default();
            return self.touch;
        };
        let d = tip - p.center;
        let x = d.dot(p.right) / (PANEL_W_M * 0.5);
        let y = d.dot(p.up) / (PANEL_H_M * 0.5);
        // Distance from the fingertip's surface to the panel, positive on
        // the wearer's side.
        let depth = d.dot(p.normal) - radius;
        let inside = |margin: f32| x.abs() <= margin && y.abs() <= margin;
        let from_front = self.last_depth.is_some_and(|z| z >= PRESS_M);
        if self.pressed {
            if depth > RELEASE_M || !inside(1.2) {
                self.pressed = false;
            }
        } else if inside(1.0) && depth < PRESS_M && from_front {
            self.pressed = true;
        }
        self.last_depth = Some(depth);
        let hover = inside(1.0) && (-0.03..0.06).contains(&depth);
        self.touch = Touch {
            pointer: (hover || self.pressed).then(|| {
                [
                    (x.clamp(-1.0, 1.0) + 1.0) * 0.5,
                    (1.0 - y.clamp(-1.0, 1.0)) * 0.5,
                ]
            }),
            pressed: self.pressed,
            near: inside(1.3) && (-0.04..0.10).contains(&depth),
        };
        self.touch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: Vec3 = Vec3::new(0.0, 1.6, 0.0);

    /// A left palm 35 cm ahead of the head, 30 cm down, turned so the
    /// cosine between its normal (-Y) and the direction to the head is
    /// `facing` (1 = straight at it).
    fn palm(facing: f32) -> (Vec3, Quat) {
        let pos = Vec3::new(0.0, 1.3, -0.35);
        let to_head = (HEAD - pos).normalize();
        let tilt = Quat::from_axis_angle(Vec3::X, facing.clamp(-1.0, 1.0).acos());
        (pos, Quat::from_rotation_arc(Vec3::NEG_Y, tilt * to_head))
    }

    fn shown_panel() -> (PalmPanel, Placement) {
        let mut panel = PalmPanel::default();
        let p = panel.place(Some(palm(1.0)), HEAD).expect("shown");
        (panel, p)
    }

    /// A fingertip `depth` m in front of the panel at panel coords (x, y)
    /// in -1..1.
    #[allow(
        clippy::unnecessary_wraps,
        reason = "matches `PalmPanel::touch`'s argument"
    )]
    fn tip_at(p: &Placement, x: f32, y: f32, depth: f32) -> Option<(Vec3, f32)> {
        let r = 0.008;
        let pos = p.center
            + p.right * (x * PANEL_W_M * 0.5)
            + p.up * (y * PANEL_H_M * 0.5)
            + p.normal * (depth + r);
        Some((pos, r))
    }

    #[test]
    fn shows_when_the_palm_faces_the_head_and_hides_when_it_turns_away() {
        let mut panel = PalmPanel::default();
        assert!(panel.place(Some(palm(0.0)), HEAD).is_none());
        assert!(panel.place(Some(palm(1.0)), HEAD).is_some());
        // Between the thresholds it stays as it was (hysteresis).
        let middle = palm(0.45);
        let facing = (middle.1 * Vec3::NEG_Y).dot((HEAD - middle.0).normalize());
        assert!((HIDE..SHOW).contains(&facing), "{facing}");
        assert!(panel.place(Some(middle), HEAD).is_some());
        assert!(panel.place(Some(palm(0.0)), HEAD).is_none());
        assert!(panel.place(Some(middle), HEAD).is_none());
        assert!(panel.place(None, HEAD).is_none());
    }

    #[test]
    fn the_panel_faces_the_head_upright_above_the_hand() {
        let (_, p) = shown_panel();
        let (pos, _) = palm(1.0);
        assert!(p.normal.dot((HEAD - p.center).normalize()) > 0.999);
        assert!(p.up.y > 0.9, "{}", p.up);
        assert!(p.center.y > pos.y + 0.1, "{} vs {}", p.center, pos);
        // Right is the wearer's right: +X for a panel ahead of them.
        assert!(p.right.x > 0.99, "{}", p.right);
    }

    #[test]
    fn a_fingertip_from_the_front_presses_and_releases() {
        let (mut panel, p) = shown_panel();
        let t = panel.touch(tip_at(&p, -1.0 + 0.2, 1.0 - 0.1, 0.03));
        assert!(!t.pressed && t.near);
        let [u, v] = t.pointer.expect("hovering");
        assert!((u - 0.1).abs() < 1e-3 && (v - 0.05).abs() < 1e-3, "{u} {v}");
        assert!(panel.touch(tip_at(&p, -0.8, 0.9, 0.0)).pressed);
        // Pushing through keeps it pressed.
        assert!(panel.touch(tip_at(&p, -0.8, 0.9, -0.02)).pressed);
        assert!(!panel.touch(tip_at(&p, -0.8, 0.9, 0.02)).pressed);
    }

    #[test]
    fn a_fingertip_arriving_from_behind_does_not_press() {
        let (mut panel, p) = shown_panel();
        assert!(!panel.touch(tip_at(&p, 0.0, 0.0, -0.02)).pressed);
        assert!(!panel.touch(tip_at(&p, 0.0, 0.0, -0.01)).pressed);
        assert!(!panel.touch(tip_at(&p, 0.0, 0.0, 0.0)).pressed);
    }

    #[test]
    fn the_panel_holds_still_under_a_near_fingertip() {
        let (mut panel, p) = shown_panel();
        panel.touch(tip_at(&p, 0.0, 0.0, 0.02));
        // The left hand drifts 5 cm; the panel stays put.
        let (pos, rot) = palm(1.0);
        let moved = panel
            .place(Some((pos + Vec3::new(0.05, 0.0, 0.0), rot)), HEAD)
            .expect("shown");
        assert_eq!(moved.center, p.center);
        // Once the finger leaves, it follows again.
        panel.touch(None);
        let followed = panel
            .place(Some((pos + Vec3::new(0.05, 0.0, 0.0), rot)), HEAD)
            .expect("shown");
        assert!(followed.center.x > p.center.x + 0.01);
    }

    #[test]
    fn a_press_outlives_the_left_hand_dropping_out() {
        let (mut panel, p) = shown_panel();
        panel.touch(tip_at(&p, 0.0, 0.0, 0.02));
        assert!(panel.touch(tip_at(&p, 0.0, 0.0, 0.0)).pressed);
        assert!(panel.place(None, HEAD).is_some());
        assert!(panel.touch(tip_at(&p, 0.0, 0.0, 0.0)).pressed);
    }
}
