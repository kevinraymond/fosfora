//! Where the debug panel sits and how a fingertip presses it. Plain
//! numbers in, so it builds and tests on the desktop as well.
//!
//! The panel belongs to the left hand: it appears while the left palm faces
//! the wearer, floats just above the hand turned toward the eyes, and hides
//! when the palm turns away. A palm that faces the ceiling more than the
//! head is a hold (`pose.rs`, Murmur's hand behaviors), not the menu: held
//! low in front of the chest, a palm turned up faces both, so the panel
//! shows only while the palm faces the head more than it faces up.
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
//!
//! **Summon and stay** (board #5459, `debug.fosfora.menustay`, on unless
//! 0). The palm summons the menu and then leaves it in the world, so a
//! press no longer needs both arms raised, and one hand can work it. The
//! menu is [`At::Hidden`], [`At::Palm`] or [`At::Air`]:
//!
//! - The left palm turning to the face (the thresholds above, as a rising
//!   edge) brings it to the palm from anywhere, and it follows the palm.
//! - The palm turning away or dropping out of tracking leaves it in the
//!   air where it was, facing the wearer from there once, then still.
//! - The spoken "menu" puts it in the air ahead of the head
//!   ([`PalmPanel::open_parked`]), no palm needed.
//! - It closes by itself: no pointer on it for [`IDLE_S`] in the air, a
//!   pinch that starts away from it ([`AWAY_MARGIN`]; that pinch does
//!   nothing else until it is released), or the spoken "close the menu".
//!   A pinch that just missed a row, inside the margin, does nothing.
//!
//! The frame is [`Placement::facing_view`], steady with the hand over the
//! face. With the knob at 0 none of this runs: the menu lives on the palm
//! as above, with world up's frame.

use glam::{Quat, Vec3};

/// Panel width and default height (meters). The height changes with what
/// the panel shows (`PalmPanel::set_height`); the bottom edge stays put.
pub const PANEL_W_M: f32 = 0.20;
pub const PANEL_H_M: f32 = 0.40;
/// Palm-facing thresholds: the cosine between the palm normal and the
/// direction to the head. Show above `SHOW`, hide below `HIDE`.
const SHOW: f32 = 0.6;
const HIDE: f32 = 0.3;
/// The panel also hides once the palm faces the ceiling (world up) this
/// much more than it faces the head (cosines), and shows only while it
/// faces the head at least as much as up.
const CEILING_MARGIN: f32 = 0.1;
/// Panel placement relative to the palm: toward the head, and its bottom
/// edge this far up (the panel grows upward from there).
const TOWARD_HEAD_M: f32 = 0.06;
const BOTTOM_ABOVE_PALM_M: f32 = 0.0;
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
/// Board #5459, [`Placement::facing_view`]: the sine of the angle between
/// the line of sight and the vertical under which the panel's top edge
/// follows the head's right whole (15°), and over which world up whole
/// (30°); between, a blend.
const HEAD_RIGHT_BELOW: f32 = 0.26;
const WORLD_UP_ABOVE: f32 = 0.5;
/// Board #5459: a menu in the air closes after this long with no pointer
/// on it (seconds). Worn at 8 s it read as a menu that does not go away:
/// it hung over the show long after the last press, and a palm that
/// summoned it by accident while walking left it up as long. 3 s is time
/// to lower one hand and point with the other.
/// `debug.fosfora.menuidle` sets another time ([`PalmPanel::set_idle`]).
pub const IDLE_S: f32 = 3.0;
/// The shortest and longest idle time the knob may set (s).
pub const IDLE_RANGE_S: (f32, f32) = (0.5, 60.0);
/// Board #5459: a pinch whose ray lands outside the panel grown by half
/// its size (this factor on the half extents), or misses it, is away from
/// it and closes the menu; inside, it just missed a row.
pub const AWAY_MARGIN: f32 = 1.5;
/// Board #5459: the spoken "menu" parks the panel this far ahead along the
/// view and this far below it (meters), as `debug.fosfora.hudtest` does.
const PARK_AHEAD_M: f32 = 0.45;
const PARK_BELOW_M: f32 = 0.05;

/// The log's line for `debug.fosfora.menustay` (board #5459).
pub fn knob_line(stay: bool) -> &'static str {
    if stay {
        "hand menu: debug.fosfora.menustay 1, the palm summons it and it stays in the air"
    } else {
        "hand menu: debug.fosfora.menustay 0, it lives on the palm"
    }
}

/// Where the hand menu is (board #5459).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum At {
    #[default]
    Hidden,
    /// On the left palm, following it.
    Palm,
    /// Where the palm left it, or parked ahead by voice, holding still.
    Air,
}

impl At {
    /// For the log: "on the palm", "in the air".
    pub fn label(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::Palm => "on the palm",
            Self::Air => "in the air",
        }
    }
}

/// Why the menu closed by itself (board #5459).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Close {
    /// No pointer on it for this long (s): [`IDLE_S`] or the knob's time.
    Idle(f32),
    /// A pinch away from it, by this hand (0 left, 1 right).
    PinchAway(usize),
    /// The spoken "close the menu".
    Spoken,
}

impl Close {
    /// For the log: "no pointer for 3 s", "a right pinch away from it".
    pub fn label(self) -> String {
        match self {
            Self::Idle(s) => format!("no pointer for {s:.0} s"),
            Self::PinchAway(h) => format!(
                "a {} pinch away from it",
                if h == 0 { "left" } else { "right" }
            ),
            Self::Spoken => "spoken".to_owned(),
        }
    }
}

/// The panel's frame this frame: center, unit right/up axes and the unit
/// normal toward the viewer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub center: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub normal: Vec3,
    /// Half the width and height (meters).
    pub half_w: f32,
    pub half_h: f32,
}

impl Placement {
    /// A `height`-tall panel at `center` facing the point `eye`, with its
    /// up as close to world up as the facing allows.
    pub fn facing(center: Vec3, eye: Vec3, height: f32) -> Self {
        let normal = (eye - center).normalize_or(Vec3::Z);
        let right = Vec3::Y.cross(normal).normalize_or(Vec3::X);
        let up = normal.cross(right);
        Self {
            center,
            right,
            up,
            normal,
            half_w: PANEL_W_M * 0.5,
            half_h: height * 0.5,
        }
    }

    /// A `height`-tall panel at `center` facing `eye`, the head turned by
    /// `head_rot` (board #5459). Its up is as close to world up as the
    /// facing allows, as [`Self::facing`] has it, except near a vertical
    /// line of sight: with the hand straight above the face (lying back)
    /// or below it, world up gives the panel no steady roll, so there its
    /// top edge runs along the head's right instead, as the room labels'
    /// does (`label::facing`), and between the two a blend. The world-up
    /// right is turned to agree with the head's, so the panel does not
    /// flip over as the line of sight passes the vertical.
    pub fn facing_view(center: Vec3, eye: Vec3, head_rot: Quat, height: f32) -> Self {
        let normal = (eye - center).normalize_or(Vec3::Z);
        // The world-up right; its length is the sine of the line of
        // sight's angle from vertical.
        let level = Vec3::Y.cross(normal);
        let sine = level.length();
        let head_right = head_rot * Vec3::X;
        let across = head_right - normal * head_right.dot(normal);
        // Looking along the head's right (the head rolled 90°, lying on a
        // side) there is no roll of the head's to follow: world up.
        let right = if across.length() < 0.2 {
            level.normalize_or(Vec3::X)
        } else {
            let across = across.normalize();
            let level = level.normalize_or(across);
            let level = if level.dot(across) < 0.0 {
                -level
            } else {
                level
            };
            let w = world_up_weight(sine);
            (level * w + across * (1.0 - w)).normalize_or(across)
        };
        let up = normal.cross(right);
        Self {
            center,
            right,
            up,
            normal,
            half_w: PANEL_W_M * 0.5,
            half_h: height * 0.5,
        }
    }

    /// A [`Self::facing_view`] panel whose bottom edge's middle is at
    /// `bottom` (board #5459). Away from a vertical line of sight it grows
    /// up along world up, as the palm's panel always has; near one, world
    /// up runs along the line of sight and would put the panel behind the
    /// hand, so it grows along its own up instead (toward the top of the
    /// head when lying back), blended as the frame is.
    pub fn above_view(bottom: Vec3, eye: Vec3, head_rot: Quat, height: f32) -> Self {
        let half = height * 0.5;
        let level = bottom + Vec3::Y * half;
        let sine = Vec3::Y.cross((eye - level).normalize_or(Vec3::Z)).length();
        if sine >= WORLD_UP_ABOVE {
            return Self::facing_view(level, eye, head_rot, height);
        }
        let own = Self::facing_view(level, eye, head_rot, height).up;
        let w = world_up_weight(sine);
        let grow = (Vec3::Y * w + own * (1.0 - w)).normalize_or(Vec3::Y);
        Self::facing_view(bottom + grow * half, eye, head_rot, height)
    }

    /// Half-extent vectors for the renderer (center to right edge, center
    /// to top edge).
    pub fn half_vectors(&self) -> (Vec3, Vec3) {
        (self.right * self.half_w, self.up * self.half_h)
    }
}

/// How much of world up's frame the view frame takes for a line of sight
/// whose angle from vertical has this sine: 0 under [`HEAD_RIGHT_BELOW`],
/// 1 over [`WORLD_UP_ABOVE`], a smoothstep between.
fn world_up_weight(sine: f32) -> f32 {
    let t = ((sine - HEAD_RIGHT_BELOW) / (WORLD_UP_ABOVE - HEAD_RIGHT_BELOW)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
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
    /// Where the menu is: hidden, on the palm, or (board #5459) in the air.
    at: At,
    /// Board #5459: the menu stays in the air once the palm lets it go
    /// (`debug.fosfora.menustay`); off, it lives on the palm.
    stay: bool,
    /// Board #5459: the left palm faces the head, with the show and hide
    /// thresholds' hysteresis and nothing else; its rise brings the menu
    /// to the palm.
    palm_up: bool,
    /// Board #5459: seconds in the air with no pointer on it.
    idle_s: f32,
    /// The idle time `debug.fosfora.menuidle` asks for in place of
    /// [`IDLE_S`] (s).
    idle_knob_s: Option<f32>,
    /// Board #5459, per hand (0 left, 1 right): a pinch that closed the
    /// menu from away from it, which nothing else reads until released.
    swallowed: [bool; 2],
    /// Why the menu last closed by itself, until taken for the log.
    closed: Option<Close>,
    pressed: bool,
    /// Last frame's fingertip-surface distance, to require a press from
    /// the front.
    last_depth: Option<f32>,
    /// Last frame's touch, whose `near` pins the panel.
    touch: Touch,
    ray: Ray,
    /// Panel height (meters); `None` is `PANEL_H_M`.
    height: Option<f32>,
}

impl PalmPanel {
    fn height(&self) -> f32 {
        self.height.unwrap_or(PANEL_H_M)
    }

    /// Change the panel's height (what it shows changed). A placed panel
    /// keeps its bottom edge where it is and grows or shrinks upward, so a
    /// control at the bottom stays under the pointer.
    pub fn set_height(&mut self, height: f32) {
        if let Some(p) = self.placement.as_mut() {
            p.center += p.up * (height * 0.5 - p.half_h);
            p.half_h = height * 0.5;
        }
        self.height = Some(height);
    }

    /// Board #5459: the menu stays in the air once the palm lets it go
    /// (`debug.fosfora.menustay`, read live), or lives on the palm.
    pub fn set_stay(&mut self, on: bool) {
        self.stay = on;
    }

    /// The idle time before a menu in the air closes: `seconds`
    /// (`debug.fosfora.menuidle`, kept inside [`IDLE_RANGE_S`]), or
    /// [`IDLE_S`] for `None`.
    pub fn set_idle(&mut self, seconds: Option<f32>) {
        self.idle_knob_s = seconds
            .filter(|s| s.is_finite())
            .map(|s| s.clamp(IDLE_RANGE_S.0, IDLE_RANGE_S.1));
    }

    /// The idle time in force (s).
    pub fn idle_max_s(&self) -> f32 {
        self.idle_knob_s.unwrap_or(IDLE_S)
    }

    /// Where the menu is.
    pub fn at(&self) -> At {
        self.at
    }

    /// Board #5459, per hand (0 left, 1 right): a pinch that closed the
    /// menu from away from it, which nothing else may read until it is
    /// released.
    pub fn swallowed(&self) -> [bool; 2] {
        self.swallowed
    }

    /// Board #5459: a pointer (the ray or a fingertip) is on or near the
    /// shown menu.
    pub fn pointed(&self) -> bool {
        self.at != At::Hidden && (self.touch.near || self.touch.pressed)
    }

    /// Why the menu last closed by itself, once.
    pub fn take_closed(&mut self) -> Option<Close> {
        self.closed.take()
    }

    /// Update the panel for this frame's left palm (position and rotation)
    /// and the head (position and rotation). Returns where to draw it, or
    /// `None` when hidden.
    pub fn place(
        &mut self,
        palm: Option<(Vec3, Quat)>,
        head: Vec3,
        head_rot: Quat,
    ) -> Option<Placement> {
        let rose = self.palm_faces(palm, head);
        if self.stay {
            self.place_staying(palm, rose, head, head_rot)
        } else {
            self.place_on_palm(palm, head)
        }
    }

    /// Board #5459: whether the left palm faces the head, with the show
    /// and hide thresholds' hysteresis; true on the frame it starts to.
    fn palm_faces(&mut self, palm: Option<(Vec3, Quat)>, head: Vec3) -> bool {
        let was = self.palm_up;
        self.palm_up = palm.is_some_and(|(pos, rot)| {
            let normal = rot * Vec3::NEG_Y;
            let facing = normal.dot((head - pos).normalize_or_zero());
            let up = normal.y;
            if was {
                facing >= HIDE && up <= facing + CEILING_MARGIN
            } else {
                facing > SHOW && facing >= up
            }
        });
        self.palm_up && !was
    }

    /// The menu on the palm, as it was before board #5459 (the knob at 0):
    /// shown while the palm faces the head or a pointer holds it, hidden
    /// otherwise.
    fn place_on_palm(&mut self, palm: Option<(Vec3, Quat)>, head: Vec3) -> Option<Placement> {
        // A menu left in the air by the knob at 1 is the palm's now.
        if self.at == At::Air {
            self.at = At::Palm;
        }
        let Some((pos, rot)) = palm else {
            // A pressed panel stays for the finger even if the left hand
            // drops out of tracking for a moment.
            if !self.touch.near {
                self.at = At::Hidden;
            }
            return self.visible();
        };
        let normal = rot * Vec3::NEG_Y;
        let facing = normal.dot((head - pos).normalize_or_zero());
        let up = normal.y;
        if self.at != At::Hidden {
            if (facing < HIDE || up > facing + CEILING_MARGIN) && !self.touch.near {
                self.at = At::Hidden;
            }
        } else if facing > SHOW && facing >= up {
            self.at = At::Palm;
            // Appear where the hand is, not sliding in from the last spot.
            self.placement = None;
        }
        if self.at == At::Hidden {
            return None;
        }
        if self.touch.near && self.placement.is_some() {
            return self.visible();
        }
        let to_head = (head - pos).normalize_or(Vec3::Z);
        let h = self.height();
        let target = Placement::facing(
            pos + to_head * TOWARD_HEAD_M + Vec3::Y * (BOTTOM_ABOVE_PALM_M + h * 0.5),
            head,
            h,
        );
        let next = match self.placement {
            Some(p) => Placement::facing(p.center.lerp(target.center, FOLLOW), head, h),
            None => target,
        };
        self.placement = Some(next);
        self.visible()
    }

    /// Board #5459: the palm summons the menu and leaves it in the air.
    /// `rose`: the palm turned to the face this frame.
    fn place_staying(
        &mut self,
        palm: Option<(Vec3, Quat)>,
        rose: bool,
        head: Vec3,
        head_rot: Quat,
    ) -> Option<Placement> {
        let near = self.touch.near;
        if rose {
            // Showing the palm brings the menu to it from anywhere: at
            // once, unless a pointer holds it, and then once the pointer
            // leaves.
            if !near {
                self.placement = None;
            }
            self.at = At::Palm;
            self.closed = None;
        } else if self.at == At::Palm && !self.palm_up {
            // The palm let go: the menu stays where it is, facing the
            // wearer from there once (a pointer on it keeps the facing it
            // has), then holds still.
            self.at = At::Air;
            self.idle_s = 0.0;
            if !near && let Some(p) = self.placement.as_mut() {
                *p = Placement::facing_view(p.center, head, head_rot, p.half_h * 2.0);
            }
        }
        if self.at != At::Palm {
            return self.visible();
        }
        let Some((pos, _)) = palm else {
            return self.visible();
        };
        if near && self.placement.is_some() {
            return self.visible();
        }
        let to_head = (head - pos).normalize_or(Vec3::Z);
        let h = self.height();
        let target = Placement::above_view(
            pos + to_head * TOWARD_HEAD_M + Vec3::Y * BOTTOM_ABOVE_PALM_M,
            head,
            head_rot,
            h,
        );
        let next = match self.placement {
            Some(p) => {
                Placement::facing_view(p.center.lerp(target.center, FOLLOW), head, head_rot, h)
            }
            None => target,
        };
        self.placement = Some(next);
        self.visible()
    }

    fn visible(&self) -> Option<Placement> {
        self.placement.filter(|_| self.at != At::Hidden)
    }

    /// Where to draw the panel now (after [`Self::input`] and
    /// [`Self::tick`], which may have closed it), or `None` when hidden.
    pub fn placement(&self) -> Option<Placement> {
        self.visible()
    }

    fn close(&mut self, why: Close) {
        self.at = At::Hidden;
        self.closed = Some(why);
        self.idle_s = 0.0;
        self.touch = Touch::default();
        self.ray.pressed = false;
        self.ray.held_at = None;
        self.pressed = false;
    }

    /// Board #5459: the spoken "menu": the panel in the air
    /// [`PARK_AHEAD_M`] ahead of the head along its view and a little
    /// below, facing it, no palm needed (one hand works it). Where it was
    /// shown it moves there. Does nothing, and returns false, with the
    /// menu not staying (the knob at 0: it lives on the palm).
    pub fn open_parked(&mut self, head: Vec3, head_rot: Quat) -> bool {
        if !self.stay {
            return false;
        }
        let center =
            head + head_rot * Vec3::NEG_Z * PARK_AHEAD_M + head_rot * Vec3::NEG_Y * PARK_BELOW_M;
        self.placement = Some(Placement::facing_view(
            center,
            head,
            head_rot,
            self.height(),
        ));
        self.at = At::Air;
        self.idle_s = 0.0;
        self.closed = None;
        true
    }

    /// Board #5459: the spoken "close the menu". Does nothing, and returns
    /// false, with the menu not staying (on the palm it would show again
    /// at once).
    pub fn close_spoken(&mut self) -> bool {
        if !self.stay {
            return false;
        }
        if self.at != At::Hidden {
            self.close(Close::Spoken);
        }
        true
    }

    /// Board #5459: `dt` seconds pass, after [`Self::input`]. A menu in the
    /// air with no pointer on it for [`IDLE_S`] closes.
    pub fn tick(&mut self, dt: f32) {
        if self.at != At::Air || self.pointed() {
            self.idle_s = 0.0;
            return;
        }
        self.idle_s += dt;
        let max = self.idle_max_s();
        if self.idle_s >= max {
            self.close(Close::Idle(max));
        }
    }

    /// This frame's pointing: the right index fingertip (center and joint
    /// radius), the right hand's ray (origin, direction) and whether it is
    /// pinching. A poke wins while the fingertip is near the panel. Call
    /// after [`Self::place`].
    ///
    /// Board #5459: with the menu staying, a pinch that starts with no
    /// pointer near the menu and the ray off the panel grown by
    /// [`AWAY_MARGIN`] closes it, and is swallowed until released; inside
    /// that margin it just missed a row and does nothing.
    pub fn input(
        &mut self,
        tip: Option<(Vec3, f32)>,
        ray: Option<(Vec3, Vec3)>,
        pinching: bool,
    ) -> Touch {
        let began = pinching && !self.ray.was_pinching;
        if !pinching {
            self.swallowed[1] = false;
        }
        let poke = self.poke(tip);
        let (aim, within) = self.aim(ray, pinching);
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
        if self.stay && began && self.at != At::Hidden && !self.pointed() && !within {
            self.close(Close::PinchAway(1));
            self.swallowed[1] = true;
        }
        self.touch
    }

    /// The ray against the shown panel, and whether it lands within the
    /// panel grown by [`AWAY_MARGIN`].
    fn aim(&mut self, ray: Option<(Vec3, Vec3)>, pinching: bool) -> (Touch, bool) {
        let began = pinching && !self.ray.was_pinching;
        self.ray.was_pinching = pinching;
        let (Some(p), Some((origin, dir))) = (self.visible(), ray) else {
            self.ray = Ray {
                was_pinching: pinching,
                ..Ray::default()
            };
            return (Touch::default(), false);
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
        let (hw, hh) = (p.half_w, p.half_h);
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
        let touch = Touch {
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
        };
        (touch, hit.is_some_and(|h| inside(h, AWAY_MARGIN)))
    }

    fn poke(&mut self, tip: Option<(Vec3, f32)>) -> Touch {
        let (Some(p), Some((tip, radius))) = (self.visible(), tip) else {
            self.pressed = false;
            self.last_depth = None;
            return Touch::default();
        };
        let d = tip - p.center;
        let x = d.dot(p.right) / p.half_w;
        let y = d.dot(p.up) / p.half_h;
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

/// The hand menu's saved toggles, `hand_menu.json` under the config dir:
/// the debug panel (off unless saved on), the voice path (board #3751,
/// V4: on unless saved off, so a file from before the toggle keeps voice
/// on) and the agent's provider (board #3776: none saved until the Agent
/// control is first pressed), and the immersion depth (board #5384: full
/// unless saved otherwise). Board #5393: the wearer's Roam choice, and
/// whether the app has asked for Space Setup, both off unless saved on.
/// Board #5452: the first run's hints the wearer has been taught, none
/// unless saved. A missing or broken file is the defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuFile {
    pub debug: bool,
    pub voice: bool,
    /// The Agent control's pick (board #3776), `None` until the wearer
    /// first presses it: the launch then takes the V5 rule's.
    pub agent: Option<crate::agent::Pick>,
    /// The Immersion control's depth (board #5384).
    pub immerse: crate::immerse::Depth,
    /// The wearer chose roam (board #5393, `roam::RoamState::chosen`).
    pub roam: bool,
    /// The app has launched Space Setup, or its first scene query has
    /// ended, once (board #5393): from then on only the hand menu's Rescan
    /// and the spoken rescan open Space Setup.
    pub space_setup_asked: bool,
    /// The first run's hints the wearer has been taught (board #5452,
    /// `first_run::Hints`): those never show again.
    pub taught: crate::first_run::Taught,
}

impl Default for MenuFile {
    fn default() -> Self {
        Self {
            debug: false,
            voice: true,
            agent: None,
            immerse: crate::immerse::Depth::default(),
            roam: false,
            space_setup_asked: false,
            taught: crate::first_run::Taught::default(),
        }
    }
}

impl MenuFile {
    /// The file's text read leniently: a field that is missing or not a
    /// boolean keeps its default; an `agent` that is missing or names no
    /// pick is no saved choice; an `immerse` that is missing or names no
    /// depth is full; a `taught` that is missing has taught no hint.
    pub fn parse(json: &str) -> Self {
        let value = serde_json::from_str::<serde_json::Value>(json).unwrap_or_default();
        let field = |name: &str, default: bool| {
            value
                .get(name)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(default)
        };
        let d = Self::default();
        Self {
            debug: field("debug", d.debug),
            voice: field("voice", d.voice),
            agent: value
                .get("agent")
                .and_then(serde_json::Value::as_str)
                .and_then(crate::agent::Pick::from_name),
            immerse: value
                .get("immerse")
                .and_then(serde_json::Value::as_str)
                .and_then(crate::immerse::Depth::from_name)
                .unwrap_or(d.immerse),
            roam: field("roam", d.roam),
            space_setup_asked: field("space_setup_asked", d.space_setup_asked),
            taught: crate::first_run::Taught::from_json(value.get("taught")),
        }
    }

    /// The file's text: `{"debug":false,"immerse":"full","voice":true}`,
    /// with `"agent":"local"` (`anthropic`, `openai`, `off`) once picked,
    /// `"roam":true` and `"space_setup_asked":true` only while true, and
    /// `"taught":["swipe","tap","palm"]` with the hints taught, once one
    /// is.
    pub fn to_json(self) -> String {
        let mut json = serde_json::json!({
            "debug": self.debug,
            "voice": self.voice,
            "immerse": self.immerse.name(),
        });
        if let Some(pick) = self.agent {
            json["agent"] = pick.name().into();
        }
        if self.roam {
            json["roam"] = true.into();
        }
        if self.space_setup_asked {
            json["space_setup_asked"] = true.into();
        }
        let taught = self.taught.names();
        if !taught.is_empty() {
            json["taught"] = taught.into();
        }
        json.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_file_round_trips_and_reads_voice_on_when_absent() {
        for file in [
            MenuFile {
                debug: true,
                voice: false,
                agent: None,
                ..MenuFile::default()
            },
            MenuFile {
                debug: false,
                voice: true,
                agent: None,
                ..MenuFile::default()
            },
        ] {
            assert_eq!(MenuFile::parse(&file.to_json()), file);
        }
        // A file from before the Voice toggle: voice on.
        assert_eq!(
            MenuFile::parse(r#"{"debug": true}"#),
            MenuFile {
                debug: true,
                voice: true,
                agent: None,
                ..MenuFile::default()
            }
        );
        assert!(!MenuFile::parse(r#"{"voice": false}"#).voice);
        // No file, or a broken one: the defaults.
        assert_eq!(MenuFile::parse(""), MenuFile::default());
        assert_eq!(MenuFile::parse("{not json"), MenuFile::default());
        assert_eq!(
            MenuFile::parse(r#"{"debug": "yes", "voice": 1}"#),
            MenuFile::default()
        );
    }

    /// Board #3776: the Agent control's pick round-trips; a file without
    /// it (V5's), or naming something unknown, is no saved choice.
    #[test]
    fn the_menu_file_saves_the_agent_pick_and_reads_none_when_absent_or_unknown() {
        use crate::agent::{Pick, ProviderKind};
        for pick in [
            Pick::Local,
            Pick::Network(ProviderKind::Anthropic),
            Pick::Network(ProviderKind::OpenAi),
            Pick::Off,
        ] {
            let file = MenuFile {
                debug: true,
                voice: false,
                agent: Some(pick),
                ..MenuFile::default()
            };
            let json = file.to_json();
            assert!(
                json.contains(&format!(r#""agent":"{}""#, pick.name())),
                "{json}"
            );
            assert_eq!(MenuFile::parse(&json), file);
        }
        assert_eq!(
            MenuFile::default().to_json(),
            r#"{"debug":false,"immerse":"full","voice":true}"#
        );
        // V5's file: no saved choice, the rest as written.
        let v5 = MenuFile::parse(r#"{"debug":false,"voice":false}"#);
        assert_eq!((v5.voice, v5.agent), (false, None));
        // Unknown or mistyped: no saved choice, the rest kept.
        for json in [
            r#"{"debug": true, "agent": "ollama"}"#,
            r#"{"debug": true, "agent": ""}"#,
            r#"{"debug": true, "agent": 1}"#,
            r#"{"debug": true, "agent": null}"#,
            r#"{"debug": true, "agent": "Anthropic"}"#,
        ] {
            let file = MenuFile::parse(json);
            assert_eq!(file.agent, None, "{json}");
            assert!(file.debug && file.voice, "{json}");
        }
        assert_eq!(
            MenuFile::parse(r#"{"agent": "openai"}"#).agent,
            Some(Pick::Network(ProviderKind::OpenAi))
        );
    }

    /// Board #5384: the Immersion control's depth round-trips; a file
    /// without it (every one before), or naming something unknown, is full.
    #[test]
    fn the_menu_file_saves_the_immersion_depth_and_reads_full_when_absent_or_unknown() {
        use crate::immerse::Depth;
        for depth in Depth::ALL {
            let file = MenuFile {
                immerse: depth,
                ..MenuFile::default()
            };
            let json = file.to_json();
            assert!(
                json.contains(&format!(r#""immerse":"{}""#, depth.name())),
                "{json}"
            );
            assert_eq!(MenuFile::parse(&json), file);
        }
        for json in [
            r#"{"debug": true, "voice": false}"#,
            r#"{"debug": true, "voice": false, "immerse": "dark"}"#,
            r#"{"debug": true, "voice": false, "immerse": 0.5}"#,
            r#"{"debug": true, "voice": false, "immerse": "Low"}"#,
        ] {
            let file = MenuFile::parse(json);
            assert_eq!(file.immerse, Depth::Full, "{json}");
            assert!(file.debug && !file.voice, "{json}");
        }
        assert_eq!(MenuFile::parse(r#"{"immerse": "off"}"#).immerse, Depth::Off);
    }

    /// Board #5393: the Roam choice and the Space Setup flag round-trip,
    /// are written only while on, and read off when absent or mistyped.
    #[test]
    fn the_menu_file_saves_roam_and_the_space_setup_flag_only_while_on() {
        for (roam, space_setup_asked) in [(true, false), (false, true), (true, true)] {
            let file = MenuFile {
                roam,
                space_setup_asked,
                ..MenuFile::default()
            };
            let json = file.to_json();
            assert_eq!(json.contains(r#""roam":true"#), roam, "{json}");
            assert_eq!(
                json.contains(r#""space_setup_asked":true"#),
                space_setup_asked,
                "{json}"
            );
            assert_eq!(MenuFile::parse(&json), file);
        }
        let off = MenuFile::default().to_json();
        assert!(
            !off.contains("roam") && !off.contains("space_setup"),
            "{off}"
        );
        for json in [
            r#"{"debug": true}"#,
            r#"{"debug": true, "roam": "yes", "space_setup_asked": 1}"#,
            r#"{"debug": true, "roam": false, "space_setup_asked": false}"#,
        ] {
            let file = MenuFile::parse(json);
            assert!(!file.roam && !file.space_setup_asked, "{json}");
            assert!(file.debug, "{json}");
        }
    }

    /// Board #5452: the hints taught round-trip, are written once one is
    /// taught, and a file from before them has taught none.
    #[test]
    fn the_menu_file_saves_the_hints_taught_and_reads_none_when_absent() {
        use crate::first_run::{Hint, Taught};
        let mut some = Taught::default();
        some.set(Hint::Swipe);
        some.set(Hint::Palm);
        for taught in [some, Taught::ALL] {
            let file = MenuFile {
                taught,
                voice: false,
                ..MenuFile::default()
            };
            let json = file.to_json();
            assert_eq!(MenuFile::parse(&json), file, "{json}");
        }
        let json = MenuFile {
            taught: some,
            ..MenuFile::default()
        }
        .to_json();
        assert!(json.contains(r#""taught":["swipe","palm"]"#), "{json}");
        assert!(!MenuFile::default().to_json().contains("taught"));
        // Files written before the hints, as the app saved them: every
        // field they have still loads, and no hint is taught.
        for (json, voice, roam) in [
            (r#"{"debug":false,"voice":true}"#, true, false),
            (
                r#"{"agent":"local","debug":false,"immerse":"black","roam":true,"space_setup_asked":true,"voice":false}"#,
                false,
                true,
            ),
        ] {
            let file = MenuFile::parse(json);
            assert_eq!(file.taught, Taught::default(), "{json}");
            assert_eq!((file.voice, file.roam), (voice, roam), "{json}");
        }
        let old = MenuFile::parse(
            r#"{"agent":"local","debug":false,"immerse":"black","roam":true,"space_setup_asked":true,"voice":false}"#,
        );
        assert_eq!(old.immerse, crate::immerse::Depth::Black);
        assert_eq!(old.agent, Some(crate::agent::Pick::Local));
        assert!(old.space_setup_asked);
        // A mistyped field teaches nothing.
        assert_eq!(
            MenuFile::parse(r#"{"taught": "all"}"#).taught,
            Taught::default()
        );
    }

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
        let p = panel
            .place(Some(palm(1.0)), HEAD, Quat::IDENTITY)
            .expect("shown");
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
    fn a_resize_keeps_the_bottom_edge_under_the_pointer() {
        let (mut panel, p) = shown_panel();
        let bottom = p.center - p.up * p.half_h;
        // Point near the bottom edge, so the panel is pinned by the ray.
        let near_bottom = -p.half_h + 0.02;
        aim(&mut panel, ray_at(&p, 0.0, near_bottom), false);
        panel.set_height(0.05);
        let small = panel
            .place(Some(palm(1.0)), HEAD, Quat::IDENTITY)
            .expect("shown");
        assert!((small.center - small.up * small.half_h - bottom).length() < 1e-4);
        assert!((small.half_h - 0.025).abs() < 1e-6);
        // The same ray still lands 2 cm above the bottom edge.
        let t = aim(&mut panel, ray_at(&p, 0.0, near_bottom), false);
        let [_, v] = t.pointer.expect("on the small panel");
        assert!((v - (1.0 - 0.02 / 0.05)).abs() < 0.01, "{v}");
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
        assert!(panel.place(Some(palm(0.0)), HEAD, Quat::IDENTITY).is_none());
        assert!(panel.place(Some(palm(1.0)), HEAD, Quat::IDENTITY).is_some());
        // Between the thresholds it stays as it was (hysteresis).
        let middle = palm(0.45);
        let facing = (middle.1 * Vec3::NEG_Y).dot((HEAD - middle.0).normalize());
        assert!((HIDE..SHOW).contains(&facing), "{facing}");
        assert!(panel.place(Some(middle), HEAD, Quat::IDENTITY).is_some());
        assert!(panel.place(Some(palm(0.0)), HEAD, Quat::IDENTITY).is_none());
        assert!(panel.place(Some(middle), HEAD, Quat::IDENTITY).is_none());
        assert!(panel.place(None, HEAD, Quat::IDENTITY).is_none());
    }

    #[test]
    fn a_palm_facing_the_ceiling_more_than_the_head_is_not_the_menu() {
        // A palm low in front of the chest: the head is up and back from
        // it, so a palm turned flat to the ceiling faces the head too.
        let pos = Vec3::new(-0.1, 1.1, -0.3);
        let to_head = (HEAD - pos).normalize();
        let flat = Quat::from_rotation_arc(Vec3::NEG_Y, Vec3::Y);
        assert!(Vec3::Y.dot(to_head) > SHOW, "{to_head}");
        let mut panel = PalmPanel::default();
        assert!(
            panel
                .place(Some((pos, flat)), HEAD, Quat::IDENTITY)
                .is_none()
        );
        // Turned toward the face, the same palm opens the menu...
        let toward = Quat::from_rotation_arc(Vec3::NEG_Y, to_head);
        assert!(
            panel
                .place(Some((pos, toward)), HEAD, Quat::IDENTITY)
                .is_some()
        );
        // ...a small tilt back toward the ceiling keeps it (hysteresis)...
        let n = Quat::IDENTITY.slerp(Quat::from_rotation_arc(to_head, Vec3::Y), 0.6) * to_head;
        let between = Quat::from_rotation_arc(Vec3::NEG_Y, n);
        assert!(
            n.y > n.dot(to_head) && n.y < n.dot(to_head) + CEILING_MARGIN,
            "{n}"
        );
        assert!(
            panel
                .place(Some((pos, between)), HEAD, Quat::IDENTITY)
                .is_some()
        );
        // ...and flat up again hides it.
        assert!(
            panel
                .place(Some((pos, flat)), HEAD, Quat::IDENTITY)
                .is_none()
        );
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

    /// The head turned to look straight up (lying back): its view is world
    /// up, its right +X, the top of the head +Z.
    fn looking_up() -> Quat {
        Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)
    }

    /// The angle between two directions (degrees).
    fn degrees(a: Vec3, b: Vec3) -> f32 {
        a.angle_between(b).to_degrees()
    }

    /// Board #5459: away from a vertical line of sight the view frame is
    /// the world-up frame, whatever the head's roll.
    #[test]
    fn the_view_frame_is_the_world_up_frame_away_from_vertical() {
        let rolled = Quat::from_rotation_z(0.3) * Quat::from_rotation_x(-0.4);
        for center in [
            palm(1.0).0,
            Vec3::new(0.0, 1.6, -0.45),
            Vec3::new(-0.3, 1.4, -0.3),
            Vec3::new(0.25, 1.9, -0.35),
        ] {
            let world = Placement::facing(center, HEAD, 0.3);
            for rot in [Quat::IDENTITY, rolled] {
                let view = Placement::facing_view(center, HEAD, rot, 0.3);
                for (a, b) in [
                    (view.right, world.right),
                    (view.up, world.up),
                    (view.normal, world.normal),
                ] {
                    assert!(a.distance(b) < 1e-5, "{center}: {a} vs {b}");
                }
            }
        }
    }

    /// Board #5459: with the hand straight above the face and the head
    /// looking up, a centimeter of hand jitter either side of the vertical
    /// spins the world-up frame round, while the view frame keeps its top
    /// edge along the head's right, the panel upright for the wearer.
    #[test]
    fn the_frame_is_stable_with_the_hand_overhead_and_the_head_looking_up() {
        let rot = looking_up();
        let overhead = HEAD + Vec3::Y * 0.45;
        let jitter = [
            Vec3::ZERO,
            Vec3::new(0.01, 0.0, 0.0),
            Vec3::new(-0.01, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.01),
            Vec3::new(0.0, 0.0, -0.01),
            Vec3::new(0.007, 0.0, -0.007),
        ];
        let mut world_spread: f32 = 0.0;
        let first = Placement::facing(overhead + jitter[1], HEAD, 0.3).right;
        for d in jitter {
            let p = Placement::facing_view(overhead + d, HEAD, rot, 0.3);
            assert!(p.normal.dot((HEAD - p.center).normalize()) > 0.999);
            assert!(p.right.dot(p.up).abs() < 1e-5 && p.right.dot(p.normal).abs() < 1e-5);
            assert!(degrees(p.right, Vec3::X) < 2.0, "{d}: {}", p.right);
            // The top edge toward the top of the head (+Z when lying back).
            assert!(degrees(p.up, Vec3::Z) < 2.0, "{d}: {}", p.up);
            world_spread = world_spread.max(degrees(
                Placement::facing(overhead + d, HEAD, 0.3).right,
                first,
            ));
        }
        assert!(world_spread > 90.0, "{world_spread}");
        // A head rolled 20° on the pillow: the top edge follows it.
        let roll = Quat::from_rotation_y(20_f32.to_radians());
        let p = Placement::facing_view(overhead, HEAD, roll * rot, 0.3);
        assert!(degrees(p.right, roll * Vec3::X) < 1.0, "{}", p.right);
    }

    /// Board #5459: the line of sight swept from straight up to level
    /// ahead (the head rolled, so the head's right and the level right
    /// differ): the frame turns smoothly from the head's to world up's, and
    /// past the vertical (the panel behind the top of the head) it stays
    /// upright for the wearer, where world up turns it upside down.
    #[test]
    fn the_view_frame_turns_smoothly_from_the_head_right_to_world_up() {
        let rot = Quat::from_rotation_y(0.35) * looking_up();
        let mut last: Option<Vec3> = None;
        for step in 0..=90 {
            let a = (step as f32).to_radians();
            let center = HEAD + Vec3::new(0.0, a.cos(), -a.sin()) * 0.45;
            let p = Placement::facing_view(center, HEAD, rot, 0.3);
            if let Some(r) = last {
                assert!(degrees(p.right, r) < 3.0, "{step}°: {r} -> {}", p.right);
            }
            last = Some(p.right);
            if step >= 31 {
                let world = Placement::facing(center, HEAD, 0.3);
                assert!(p.right.distance(world.right) < 1e-5, "{step}°");
            }
        }
        let behind = HEAD + Vec3::new(0.0, 0.45, 0.15);
        let p = Placement::facing_view(behind, HEAD, looking_up(), 0.3);
        assert!(p.up.z > 0.9 && p.right.x > 0.9, "{p:?}");
        assert!(Placement::facing(behind, HEAD, 0.3).up.z < 0.0);
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
            .place(
                Some((pos + Vec3::new(0.05, 0.0, 0.0), rot)),
                HEAD,
                Quat::IDENTITY,
            )
            .expect("shown");
        assert_eq!(moved.center, p.center);
        // Once the finger leaves, it follows again.
        poke(&mut panel, None);
        let followed = panel
            .place(
                Some((pos + Vec3::new(0.05, 0.0, 0.0), rot)),
                HEAD,
                Quat::IDENTITY,
            )
            .expect("shown");
        assert!(followed.center.x > p.center.x + 0.01);
    }

    #[test]
    fn a_press_outlives_the_left_hand_dropping_out() {
        let (mut panel, p) = shown_panel();
        poke(&mut panel, tip_at(&p, 0.0, 0.0, 0.02));
        assert!(poke(&mut panel, tip_at(&p, 0.0, 0.0, 0.0)).pressed);
        assert!(panel.place(None, HEAD, Quat::IDENTITY).is_some());
        assert!(poke(&mut panel, tip_at(&p, 0.0, 0.0, 0.0)).pressed);
    }

    /// A frame at 72 Hz.
    const DT: f32 = 1.0 / 72.0;

    /// Board #5459: the menu summoned on the palm with the knob at 1.
    fn staying() -> (PalmPanel, Placement) {
        let mut panel = PalmPanel::default();
        panel.set_stay(true);
        let p = panel
            .place(Some(palm(1.0)), HEAD, Quat::IDENTITY)
            .expect("shown");
        assert_eq!(panel.at(), At::Palm);
        (panel, p)
    }

    /// One frame as the hand menu runs it: the left palm, the right hand's
    /// ray and pinch, time. Where the panel is drawn. `_left_pinch` is the
    /// left hand's pinch, which the menu does not read: lowering the hand
    /// that summoned it must not close it.
    fn frame(
        panel: &mut PalmPanel,
        palm: Option<(Vec3, Quat)>,
        ray: Option<(Vec3, Vec3)>,
        right_pinch: bool,
        _left_pinch: bool,
    ) -> Option<Placement> {
        panel.place(palm, HEAD, Quat::IDENTITY);
        panel.input(None, ray, right_pinch);
        panel.tick(DT);
        panel.placement()
    }

    /// `seconds` of frames with nothing pointing and the palm away.
    fn idle(panel: &mut PalmPanel, seconds: f32) -> Option<Placement> {
        let mut drawn = panel.placement();
        for _ in 0..(seconds / DT).round() as usize {
            drawn = frame(panel, None, None, false, false);
        }
        drawn
    }

    /// The palm turned away, or the hand dropped out of tracking: the menu
    /// stays where it was, facing the wearer, and holds still.
    #[test]
    fn the_palm_turning_away_leaves_the_menu_in_the_air() {
        for gone in [Some(palm(0.0)), None] {
            let (mut panel, p) = staying();
            let left = frame(&mut panel, gone, None, false, false).expect("still up");
            assert_eq!(panel.at(), At::Air);
            assert!(left.center.distance(p.center) < 1e-5, "{gone:?}");
            assert!(left.normal.dot((HEAD - left.center).normalize()) > 0.999);
            // The left hand wanders off; the menu does not follow.
            let (pos, rot) = palm(0.0);
            for k in 0..30 {
                let away = Some((pos + Vec3::new(0.01 * k as f32, -0.01 * k as f32, 0.0), rot));
                let held = frame(&mut panel, away, None, false, false).expect("still up");
                assert_eq!(held, left);
            }
        }
    }

    /// No pointer on the menu in the air for its idle time closes it; a
    /// pointer starts the count again; on the palm it never counts.
    #[test]
    fn the_menu_in_the_air_closes_after_its_idle_time_without_a_pointer() {
        let (mut panel, p) = staying();
        // Held up on the palm for 20 s: it stays.
        for _ in 0..(20.0 / DT) as usize {
            assert!(frame(&mut panel, Some(palm(1.0)), None, false, false).is_some());
        }
        assert!(idle(&mut panel, IDLE_S - 1.0).is_some());
        // The ray rests on it for a moment: the count starts again.
        assert!(frame(&mut panel, None, ray_at(&p, 0.0, 0.0), false, false).is_some());
        assert!(idle(&mut panel, IDLE_S - 0.1).is_some());
        assert_eq!(panel.at(), At::Air);
        assert!(idle(&mut panel, 0.2).is_none());
        assert_eq!(panel.at(), At::Hidden);
        assert_eq!(panel.take_closed(), Some(Close::Idle(IDLE_S)));
        assert_eq!(panel.take_closed(), None);
        assert_eq!(Close::Idle(IDLE_S).label(), "no pointer for 3 s");
    }

    /// `debug.fosfora.menuidle` sets another idle time, kept in range, and
    /// clearing it brings the default back.
    #[test]
    #[allow(clippy::float_cmp)] // The times are set and read back, not computed.
    fn the_knob_sets_the_idle_time() {
        let (mut panel, _) = staying();
        panel.set_idle(Some(6.0));
        assert_eq!(panel.idle_max_s(), 6.0);
        frame(&mut panel, None, None, false, false);
        assert!(idle(&mut panel, 5.8).is_some());
        assert!(idle(&mut panel, 0.4).is_none());
        assert_eq!(panel.take_closed(), Some(Close::Idle(6.0)));
        panel.set_idle(Some(0.0));
        assert_eq!(panel.idle_max_s(), IDLE_RANGE_S.0);
        panel.set_idle(Some(f32::NAN));
        assert_eq!(panel.idle_max_s(), IDLE_S);
        panel.set_idle(None);
        assert_eq!(panel.idle_max_s(), IDLE_S);
    }

    /// A pinch with the ray well off the menu closes it, and that pinch is
    /// the menu's until released (the gestures never see it); the left
    /// hand's pinch does the same while the menu is in the air.
    #[test]
    fn a_pinch_away_from_the_menu_closes_it_and_is_swallowed() {
        let (mut panel, p) = staying();
        frame(&mut panel, None, None, false, false);
        let far = ray_at(&p, 0.0, -0.6);
        assert!(frame(&mut panel, None, far, false, false).is_some());
        assert!(frame(&mut panel, None, far, true, false).is_none());
        assert_eq!(panel.take_closed(), Some(Close::PinchAway(1)));
        assert_eq!(panel.swallowed(), [false, true]);
        // Held and swept across where the menu was: still swallowed, and
        // nothing shows or presses.
        for _ in 0..30 {
            assert!(frame(&mut panel, None, ray_at(&p, 0.0, 0.0), true, false).is_none());
            assert_eq!(panel.swallowed(), [false, true]);
        }
        frame(&mut panel, None, None, false, false);
        assert_eq!(panel.swallowed(), [false, false]);
        // A ray that misses the panel's plane altogether is away too.
        let (mut panel, p) = staying();
        frame(&mut panel, None, None, false, false);
        let behind = Some((p.center - p.normal * 0.4, p.normal));
        assert!(frame(&mut panel, None, behind, true, false).is_none());
        // On the palm, a pinch away closes it as well.
        let (mut panel, p) = staying();
        assert!(
            frame(
                &mut panel,
                Some(palm(1.0)),
                ray_at(&p, 0.5, 0.0),
                true,
                false
            )
            .is_none()
        );
        assert_eq!(panel.swallowed(), [false, true]);
        // The left hand's pinch never closes it, in the air or on the palm:
        // the hand that summoned the menu is on its way down.
        let (mut panel, _) = staying();
        frame(&mut panel, None, None, false, false);
        assert!(frame(&mut panel, None, None, false, true).is_some());
        assert_eq!(panel.take_closed(), None);
        assert_eq!(panel.swallowed(), [false, false]);
        let (mut panel, _) = staying();
        assert!(frame(&mut panel, Some(palm(1.0)), None, false, true).is_some());
        assert_eq!(panel.swallowed(), [false, false]);
    }

    /// A pinch that just misses a row, inside the panel grown by half its
    /// size, does nothing: no press, no close, nothing swallowed.
    #[test]
    fn a_pinch_that_just_misses_a_row_does_nothing() {
        let (mut panel, p) = staying();
        frame(&mut panel, None, None, false, false);
        // Off the edge past the hover margin, inside the away margin.
        let x = p.half_w * (RAY_MARGIN + AWAY_MARGIN) * 0.5;
        let miss = ray_at(&p, x, 0.0);
        for _ in 0..30 {
            frame(&mut panel, None, miss, false, false);
        }
        assert!(frame(&mut panel, None, miss, true, false).is_some());
        assert!(!panel.touch.pressed && panel.touch.pointer.is_none());
        assert_eq!(panel.swallowed(), [false, false]);
        assert_eq!(panel.at(), At::Air);
    }

    /// Showing the palm again brings the menu back to it: from the air, at
    /// once and to the hand's new place; from closed; and after a close
    /// with the palm still up, only once the palm turns away and back.
    #[test]
    fn showing_the_palm_again_moves_the_menu_back_to_it() {
        let (mut panel, p) = staying();
        frame(&mut panel, Some(palm(0.0)), None, false, false);
        assert_eq!(panel.at(), At::Air);
        let (pos, rot) = palm(1.0);
        let moved = Some((pos + Vec3::new(-0.15, 0.05, 0.0), rot));
        let back = frame(&mut panel, moved, None, false, false).expect("shown");
        assert_eq!(panel.at(), At::Palm);
        assert!(
            back.center.x < p.center.x - 0.1,
            "{} vs {}",
            back.center,
            p.center
        );
        // Closed by the idle count, the palm brings it back too.
        frame(&mut panel, None, None, false, false);
        assert!(idle(&mut panel, IDLE_S + 0.1).is_none());
        assert!(frame(&mut panel, Some(palm(1.0)), None, false, false).is_some());
        // Closed by voice with the palm still up: closed until the palm
        // turns away and back.
        assert!(panel.close_spoken());
        for _ in 0..30 {
            assert!(frame(&mut panel, Some(palm(1.0)), None, false, false).is_none());
        }
        assert!(frame(&mut panel, Some(palm(0.0)), None, false, false).is_none());
        assert!(frame(&mut panel, Some(palm(1.0)), None, false, false).is_some());
    }

    /// The spoken "menu" parks it ahead of the head, in the air, no palm;
    /// "close the menu" closes it. Lying back, the parked menu is over the
    /// face and upright for the wearer.
    #[test]
    fn the_spoken_open_parks_the_menu_ahead_and_the_spoken_close_hides_it() {
        let mut panel = PalmPanel::default();
        panel.set_stay(true);
        assert!(frame(&mut panel, None, None, false, false).is_none());
        assert!(panel.open_parked(HEAD, Quat::IDENTITY));
        let p = frame(&mut panel, None, None, false, false).expect("parked");
        assert_eq!(panel.at(), At::Air);
        let ahead = HEAD + Vec3::new(0.0, -PARK_BELOW_M, -PARK_AHEAD_M);
        assert!(p.center.distance(ahead) < 1e-5, "{}", p.center);
        assert!(p.normal.dot((HEAD - p.center).normalize()) > 0.999);
        assert!(p.up.y > 0.99 && p.right.x > 0.99, "{p:?}");
        // One hand works it: the ray clicks it.
        assert!(panel.input(None, ray_at(&p, 0.0, 0.0), true).pressed);
        panel.input(None, ray_at(&p, 0.0, 0.0), false);
        assert!(panel.close_spoken());
        assert!(panel.placement().is_none());
        assert_eq!(panel.take_closed(), Some(Close::Spoken));
        // Lying back, looking straight up.
        assert!(panel.open_parked(HEAD, looking_up()));
        let p = panel.placement().expect("parked");
        assert!(p.center.y > HEAD.y + 0.4, "{}", p.center);
        assert!(p.up.z > 0.99 && p.right.x > 0.99, "{p:?}");
    }

    /// The knob at 0: the menu lives on the palm as before. The palm
    /// turning away hides it, the spoken open and close do nothing, a far
    /// pinch neither closes it nor is swallowed, nothing counts down, and
    /// the frame is world up's even overhead.
    #[test]
    fn with_the_knob_at_zero_the_menu_lives_on_the_palm() {
        let (mut panel, p) = shown_panel();
        assert!(frame(&mut panel, Some(palm(0.0)), None, false, false).is_none());
        assert!(!panel.open_parked(HEAD, Quat::IDENTITY));
        assert!(frame(&mut panel, None, None, false, false).is_none());
        assert!(frame(&mut panel, Some(palm(1.0)), None, false, false).is_some());
        assert!(!panel.close_spoken());
        assert!(frame(&mut panel, Some(palm(1.0)), None, false, false).is_some());
        for _ in 0..(20.0 / DT) as usize {
            assert!(frame(&mut panel, Some(palm(1.0)), None, false, false).is_some());
        }
        assert!(
            frame(
                &mut panel,
                Some(palm(1.0)),
                ray_at(&p, 0.0, -0.6),
                true,
                false
            )
            .is_some()
        );
        assert!(frame(&mut panel, Some(palm(1.0)), None, false, true).is_some());
        assert_eq!(panel.swallowed(), [false, false]);
        assert_eq!(panel.take_closed(), None);
        // Overhead, lying back: the world-up frame, as before.
        let mut panel = PalmPanel::default();
        let pos = HEAD + Vec3::new(0.0, 0.45, 0.005);
        let p = panel
            .place(Some((pos, Quat::IDENTITY)), HEAD, looking_up())
            .expect("shown");
        let world = Placement::facing(p.center, HEAD, p.half_h * 2.0);
        assert!(p.right.distance(world.right) < 1e-5, "{p:?}");
        // The knob turned to 0 while the menu is in the air: the palm's
        // rules take it, and with the palm away it hides.
        let (mut panel, _) = staying();
        frame(&mut panel, None, None, false, false);
        assert_eq!(panel.at(), At::Air);
        panel.set_stay(false);
        assert!(frame(&mut panel, None, None, false, false).is_none());
    }

    /// Board #5459: with the hand straight above the face and the head
    /// looking up, the palm's menu keeps its top edge toward the top of the
    /// head through the hand's jitter, and sits beside the hand in the view
    /// instead of behind it (world up runs along the line of sight there).
    #[test]
    fn the_palm_menu_overhead_is_steady_and_beside_the_hand() {
        let over = HEAD + Vec3::Y * 0.45;
        let jitter = [
            Vec3::ZERO,
            Vec3::new(0.01, 0.0, 0.0),
            Vec3::new(-0.01, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.01),
            Vec3::new(0.0, 0.0, -0.01),
        ];
        let mut panel = PalmPanel::default();
        panel.set_stay(true);
        panel.set_height(0.3);
        for d in jitter {
            let p = panel
                .place(Some((over + d, Quat::IDENTITY)), HEAD, looking_up())
                .expect("shown");
            assert!(degrees(p.right, Vec3::X) < 3.0, "{d}: {p:?}");
            assert!(p.up.z > 0.9, "{d}: {p:?}");
            assert!(p.normal.dot((HEAD - p.center).normalize()) > 0.999);
            let apart = degrees(p.center - HEAD, over + d - HEAD);
            assert!(apart > 10.0, "{d}: {apart}°");
        }
        // The knob at 0 puts it straight behind the hand.
        let mut panel = PalmPanel::default();
        panel.set_height(0.3);
        let p = panel
            .place(Some((over, Quat::IDENTITY)), HEAD, looking_up())
            .expect("shown");
        assert!(degrees(p.center - HEAD, over - HEAD) < 1.0);
    }
}
