//! Where the debug panel sits and how a fingertip presses it. Plain
//! numbers in, so it builds and tests on the desktop as well.
//!
//! The panel belongs to the left hand: it appears while the left palm faces
//! the wearer, floats just above the hand turned toward the eyes, and hides
//! when the palm turns away.
//!
//! Two ways to operate it with the right hand. The primary one needs no
//! depth judgment (it must work without stereo depth perception): a ray
//! from the shoulder through the index knuckle puts a cursor on the panel,
//! a pinch that starts there clicks, and pinching and moving drags. The
//! cursor holds still for the first [`CLICK_SLOP_M`] after the pinch
//! closes, so the pinch's own jolt does not turn a click into a drag. The
//! other is a poke with the index fingertip, which wins whenever the
//! fingertip is close. While either is on the panel it stops following the
//! left hand, so it holds still instead of wobbling under the pointer.

use glam::{Quat, Vec3};

/// Panel size (meters).
pub const PANEL_W_M: f32 = 0.20;
pub const PANEL_H_M: f32 = 0.40;
/// Palm-facing thresholds: the cosine between the palm normal and the
/// direction to the head. Show above `SHOW`, hide below `HIDE`.
const SHOW: f32 = 0.6;
const HIDE: f32 = 0.3;
/// Panel center relative to the palm: toward the head and up.
const TOWARD_HEAD_M: f32 = 0.06;
const ABOVE_PALM_M: f32 = 0.20;
/// Per-frame blend toward the target pose (hand-tracking jitter filter).
const FOLLOW: f32 = 0.35;
/// Fingertip-surface distance (meters, positive in front) that presses the
/// panel, and the one that releases it.
const PRESS_M: f32 = 0.004;
const RELEASE_M: f32 = 0.012;
/// Cursor travel on the panel (meters) after a ray pinch closes before the
/// cursor follows the ray (egui reads anything past a few points as a drag).
pub const CLICK_SLOP_M: f32 = 0.015;
/// Per-frame blend of the ray direction toward the new one.
const RAY_FOLLOW: f32 = 0.5;
/// A ray this far past the panel's edges (fraction of the half size) still
/// counts as on it: the controls are full-width rows, so forgive the edges.
const RAY_MARGIN: f32 = 1.15;
/// A poke takes over from the ray only when the fingertip is this close in
/// front of the panel (or pressing it). Pointing at the panel brings the
/// fingertip within 10 cm of it anyway, so a wider band took the ray away.
const POKE_WINS_M: f32 = 0.03;
/// The visible beam: it starts this far past the knuckle and, off the
/// panel, runs this long.
const BEAM_START_M: f32 = 0.03;
const BEAM_MISS_M: f32 = 0.5;
/// The shoulder the ray starts from, relative to the head: this far to the
/// hand's side and down.
const SHOULDER_SIDE_M: f32 = 0.18;
const SHOULDER_DOWN_M: f32 = 0.25;

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

/// How the panel is pointed at this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Touch {
    /// Pointer position in the panel, 0..1 from the top-left, while the
    /// ray or fingertip is on it.
    pub pointer: Option<[f32; 2]>,
    /// The panel is pressed (ray pinch or poke).
    pub pressed: bool,
    /// The panel should hold still (a pointer is on or near it).
    pub near: bool,
    /// The right hand's pinches belong to the panel, not to the gestures:
    /// the ray is on the panel or pressing it.
    pub captures_pinch: bool,
    /// Poke only: the fingertip surface's distance in front of the panel
    /// (meters), for a cursor that shrinks as the finger closes in.
    pub poke_depth: Option<f32>,
    /// The ray's visible beam (start, end), while the ray is the pointer.
    pub beam: Option<(Vec3, Vec3)>,
}

/// A pointing ray from the shoulder through the index knuckle of the hand
/// on `side` (-1 left, +1 right), with the head at `head` turned by
/// `head_rot`. Only the head's yaw places the shoulder.
pub fn hand_ray(knuckle: Vec3, head: Vec3, head_rot: Quat, side: f32) -> (Vec3, Vec3) {
    let ahead = head_rot * Vec3::NEG_Z;
    let flat = Vec3::new(ahead.x, 0.0, ahead.z).normalize_or(Vec3::NEG_Z);
    let right = flat.cross(Vec3::Y);
    let shoulder = head + right * (SHOULDER_SIDE_M * side) - Vec3::Y * SHOULDER_DOWN_M;
    (knuckle, (knuckle - shoulder).normalize_or(flat))
}

/// The ray's state between frames.
#[derive(Debug, Clone, Copy, Default)]
struct Ray {
    dir: Option<Vec3>,
    pressed: bool,
    was_pinching: bool,
    /// Where a press landed (panel meters), while the cursor is held there.
    held_at: Option<(f32, f32)>,
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
    ray: Ray,
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

    /// This frame's pointing: the right index fingertip (center and joint
    /// radius), the right hand's ray (origin, direction) and whether it is
    /// pinching. A poke wins while the fingertip is near the panel. Call
    /// after [`Self::place`].
    pub fn input(
        &mut self,
        tip: Option<(Vec3, f32)>,
        ray: Option<(Vec3, Vec3)>,
        pinching: bool,
    ) -> Touch {
        let poke = self.poke(tip);
        let aim = self.aim(ray, pinching);
        let poke_wins = poke.pressed || poke.poke_depth.is_some_and(|d| d < POKE_WINS_M);
        self.touch = if poke_wins {
            // A poke in progress cancels a ray press.
            self.ray.pressed = false;
            self.ray.held_at = None;
            Touch {
                captures_pinch: aim.captures_pinch,
                ..poke
            }
        } else {
            Touch {
                near: aim.near || poke.near,
                ..aim
            }
        };
        self.touch
    }

    fn aim(&mut self, ray: Option<(Vec3, Vec3)>, pinching: bool) -> Touch {
        let began = pinching && !self.ray.was_pinching;
        self.ray.was_pinching = pinching;
        let (Some(p), Some((origin, dir))) = (self.visible(), ray) else {
            self.ray = Ray {
                was_pinching: pinching,
                ..Ray::default()
            };
            return Touch::default();
        };
        let dir = match self.ray.dir {
            Some(d) => d.lerp(dir, RAY_FOLLOW).normalize_or(dir),
            None => dir,
        };
        self.ray.dir = Some(dir);
        // Only a ray arriving at the front face hits.
        let facing = dir.dot(p.normal);
        let hit = (facing < -1e-3)
            .then(|| (p.center - origin).dot(p.normal) / facing)
            .filter(|&t| t > 0.0)
            .map(|t| {
                let d = origin + dir * t - p.center;
                (d.dot(p.right), d.dot(p.up))
            });
        let (hw, hh) = (PANEL_W_M * 0.5, PANEL_H_M * 0.5);
        let inside =
            |(x, y): (f32, f32), margin: f32| x.abs() <= hw * margin && y.abs() <= hh * margin;
        let hover = hit.is_some_and(|h| inside(h, RAY_MARGIN));
        if self.ray.pressed {
            if !pinching {
                self.ray.pressed = false;
                self.ray.held_at = None;
            }
        } else if began && hover {
            self.ray.pressed = true;
            self.ray.held_at = hit;
        }
        // Past the slop the cursor follows the ray (a slider drag).
        if let (Some(at), Some(h)) = (self.ray.held_at, hit)
            && ((h.0 - at.0).powi(2) + (h.1 - at.1).powi(2)).sqrt() > CLICK_SLOP_M
        {
            self.ray.held_at = None;
        }
        let cursor = self
            .ray
            .held_at
            .or(hit.filter(|_| hover || self.ray.pressed));
        Touch {
            pointer: cursor.map(|(x, y)| {
                [
                    ((x / hw).clamp(-1.0, 1.0) + 1.0) * 0.5,
                    (1.0 - (y / hh).clamp(-1.0, 1.0)) * 0.5,
                ]
            }),
            pressed: self.ray.pressed,
            near: hover || self.ray.pressed,
            captures_pinch: hover || self.ray.pressed,
            poke_depth: None,
            beam: Some((
                origin + dir * BEAM_START_M,
                hit.filter(|_| hover)
                    .map_or(origin + dir * BEAM_MISS_M, |(x, y)| {
                        p.center + p.right * x + p.up * y
                    }),
            )),
        }
    }

    fn poke(&mut self, tip: Option<(Vec3, f32)>) -> Touch {
        let (Some(p), Some((tip, radius))) = (self.visible(), tip) else {
            self.pressed = false;
            self.last_depth = None;
            return Touch::default();
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
        Touch {
            pointer: (hover || self.pressed).then(|| {
                [
                    (x.clamp(-1.0, 1.0) + 1.0) * 0.5,
                    (1.0 - y.clamp(-1.0, 1.0)) * 0.5,
                ]
            }),
            pressed: self.pressed,
            near: inside(1.3) && (-0.04..0.10).contains(&depth),
            captures_pinch: false,
            poke_depth: hover.then_some(depth),
            beam: None,
        }
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

    fn poke(panel: &mut PalmPanel, tip: Option<(Vec3, f32)>) -> Touch {
        panel.input(tip, None, false)
    }

    /// A ray from 40 cm in front of the panel aimed at panel coords (x, y)
    /// in meters from its center.
    #[allow(
        clippy::unnecessary_wraps,
        reason = "matches `PalmPanel::input`'s argument"
    )]
    fn ray_at(p: &Placement, x: f32, y: f32) -> Option<(Vec3, Vec3)> {
        let target = p.center + p.right * x + p.up * y;
        let origin = p.center + p.normal * 0.4 + Vec3::new(0.1, -0.05, 0.0);
        Some((origin, (target - origin).normalize()))
    }

    fn aim(panel: &mut PalmPanel, ray: Option<(Vec3, Vec3)>, pinching: bool) -> Touch {
        panel.input(None, ray, pinching)
    }

    #[test]
    fn a_ray_on_the_panel_points_and_a_pinch_there_clicks() {
        let (mut panel, p) = shown_panel();
        let t = aim(&mut panel, ray_at(&p, 0.0, 0.0), false);
        let [u, v] = t.pointer.expect("on the panel");
        assert!((u - 0.5).abs() < 1e-3 && (v - 0.5).abs() < 1e-3, "{u} {v}");
        assert!(t.captures_pinch && !t.pressed);
        assert!(aim(&mut panel, ray_at(&p, 0.0, 0.0), true).pressed);
        let t = aim(&mut panel, ray_at(&p, 0.0, 0.0), false);
        assert!(!t.pressed && t.pointer.is_some());
    }

    #[test]
    fn the_pinch_jolt_does_not_move_the_cursor_but_a_drag_does() {
        let (mut panel, p) = shown_panel();
        aim(&mut panel, ray_at(&p, 0.0, 0.0), false);
        let pressed = aim(&mut panel, ray_at(&p, 0.0, 0.0), true);
        // The ray settles over several frames (direction filter), so feed
        // each target until it stops moving.
        let settle = |panel: &mut PalmPanel, x: f32| {
            let mut t = Touch::default();
            for _ in 0..30 {
                t = aim(panel, ray_at(&p, x, 0.0), true);
            }
            t
        };
        // A 1 cm jolt: the cursor stays where the press landed.
        let jolted = settle(&mut panel, 0.01);
        assert_eq!(jolted.pointer, pressed.pointer);
        // A 5 cm move: past the slop, the cursor follows (a slider drag).
        let dragged = settle(&mut panel, 0.05);
        let [u, _] = dragged.pointer.expect("dragging");
        assert!((u - (0.5 + 0.05 / PANEL_W_M)).abs() < 0.01, "{u}");
        assert!(dragged.pressed);
    }

    #[test]
    fn a_pinch_that_began_off_the_panel_is_not_a_click() {
        let (mut panel, p) = shown_panel();
        // Pinching while aimed well off the panel (a cloud gesture)...
        aim(&mut panel, ray_at(&p, 0.5, 0.0), true);
        // ...then swept onto it: no press.
        for _ in 0..30 {
            assert!(!aim(&mut panel, ray_at(&p, 0.0, 0.0), true).pressed);
        }
    }

    #[test]
    fn a_ray_from_behind_the_panel_does_not_hit() {
        let (mut panel, p) = shown_panel();
        let origin = p.center - p.normal * 0.4;
        let t = aim(&mut panel, Some((origin, p.normal)), false);
        assert_eq!(t.pointer, None);
        assert!(!t.captures_pinch);
    }

    #[test]
    fn a_near_fingertip_wins_over_the_ray() {
        let (mut panel, p) = shown_panel();
        let t = panel.input(tip_at(&p, -0.5, 0.5, 0.02), ray_at(&p, 0.05, -0.05), false);
        let [u, v] = t.pointer.expect("poke hover");
        assert!(
            (u - 0.25).abs() < 1e-3 && (v - 0.25).abs() < 1e-3,
            "{u} {v}"
        );
        assert!(t.poke_depth.is_some_and(|d| (d - 0.02).abs() < 1e-4));
    }

    #[test]
    fn a_fingertip_a_few_cm_off_the_panel_leaves_the_ray_in_charge() {
        let (mut panel, p) = shown_panel();
        // Pointing at the panel puts the fingertip ~6 cm in front of it.
        let t = panel.input(tip_at(&p, -0.5, 0.5, 0.06), ray_at(&p, 0.0, 0.0), false);
        let [u, v] = t.pointer.expect("ray cursor");
        assert!((u - 0.5).abs() < 1e-3 && (v - 0.5).abs() < 1e-3, "{u} {v}");
        assert!(t.poke_depth.is_none() && t.beam.is_some());
        // The beam ends on the panel where the ray hits.
        let (_, end) = t.beam.expect("beam");
        assert!(end.distance(p.center) < 1e-4, "{end}");
    }

    #[test]
    fn the_right_ray_from_the_shoulder_points_through_the_knuckle() {
        let head = Vec3::new(0.0, 1.6, 0.0);
        let knuckle = Vec3::new(0.0, 1.3, -0.4);
        let (origin, dir) = hand_ray(knuckle, head, Quat::IDENTITY, 1.0);
        assert_eq!(origin, knuckle);
        // From a right shoulder, a hand straight ahead points left and down-ish.
        assert!(dir.x < 0.0 && dir.z < 0.0, "{dir}");
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
        let t = poke(&mut panel, tip_at(&p, -1.0 + 0.2, 1.0 - 0.1, 0.02));
        assert!(!t.pressed && t.near);
        let [u, v] = t.pointer.expect("hovering");
        assert!((u - 0.1).abs() < 1e-3 && (v - 0.05).abs() < 1e-3, "{u} {v}");
        assert!(poke(&mut panel, tip_at(&p, -0.8, 0.9, 0.0)).pressed);
        // Pushing through keeps it pressed.
        assert!(poke(&mut panel, tip_at(&p, -0.8, 0.9, -0.02)).pressed);
        assert!(!poke(&mut panel, tip_at(&p, -0.8, 0.9, 0.02)).pressed);
    }

    #[test]
    fn a_fingertip_arriving_from_behind_does_not_press() {
        let (mut panel, p) = shown_panel();
        assert!(!poke(&mut panel, tip_at(&p, 0.0, 0.0, -0.02)).pressed);
        assert!(!poke(&mut panel, tip_at(&p, 0.0, 0.0, -0.01)).pressed);
        assert!(!poke(&mut panel, tip_at(&p, 0.0, 0.0, 0.0)).pressed);
    }

    #[test]
    fn the_panel_holds_still_under_a_near_fingertip() {
        let (mut panel, p) = shown_panel();
        poke(&mut panel, tip_at(&p, 0.0, 0.0, 0.02));
        // The left hand drifts 5 cm; the panel stays put.
        let (pos, rot) = palm(1.0);
        let moved = panel
            .place(Some((pos + Vec3::new(0.05, 0.0, 0.0), rot)), HEAD)
            .expect("shown");
        assert_eq!(moved.center, p.center);
        // Once the finger leaves, it follows again.
        poke(&mut panel, None);
        let followed = panel
            .place(Some((pos + Vec3::new(0.05, 0.0, 0.0), rot)), HEAD)
            .expect("shown");
        assert!(followed.center.x > p.center.x + 0.01);
    }

    #[test]
    fn a_press_outlives_the_left_hand_dropping_out() {
        let (mut panel, p) = shown_panel();
        poke(&mut panel, tip_at(&p, 0.0, 0.0, 0.02));
        assert!(poke(&mut panel, tip_at(&p, 0.0, 0.0, 0.0)).pressed);
        assert!(panel.place(None, HEAD).is_some());
        assert!(poke(&mut panel, tip_at(&p, 0.0, 0.0, 0.0)).pressed);
    }
}
