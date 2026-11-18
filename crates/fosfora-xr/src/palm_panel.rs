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
//! One way to operate it with the right hand, and it needs no depth
//! judgment (it must work without stereo depth perception): a ray from the
//! shoulder through the index knuckle puts a cursor on the panel, a pinch
//! that starts there clicks, and pinching and moving drags. The cursor
//! holds still for the first [`CLICK_SLOP_M`] the ray travels after the
//! pinch closes, so the pinch's own jolt does not turn a click into a
//! drag. A poke with the index fingertip was the second way. It is off
//! since board #5572 (`debug.fosfora.menupoke 1` brings it back,
//! [`PalmPanel::set_poke`]): the menu is ray only.
//!
//! **The palm parks it ahead** (boards #5459, #5539, #5572,
//! `debug.fosfora.menustay`, on unless 0). The menu is [`At::Hidden`] or
//! [`At::Air`]; [`At::Palm`] is the knob at 0 alone.
//!
//! - The left palm turned to the face (the thresholds above) and held
//!   there for [`PALM_DWELL_S`] parks the menu ahead of the head in the
//!   air ([`PalmPanel::open_parked`]), where the spoken "menu" puts it,
//!   and there it holds still. It no longer rides the palm, and turning
//!   the palm away does not hide it. Worn (Nov 18, board #5572), five
//!   pinches in 3.3 s each pressed a different row of the menu on the
//!   palm: a small target on a hand that moves, aimed at by a hand that
//!   jolts as it pinches.
//! - The palm turned to the face again closes it ([`Close::PalmAgain`]),
//!   so the palm is a toggle. Not while a pointer is on it.
//! - No surprise menu. The palm brings nothing while the left hand is a
//!   fist or a voice window is open, nor for [`SUMMON_LOCKOUT_S`] after
//!   ([`PalmPanel::set_summon_blocked`]). A palm that came up in that
//!   time has to turn away and back, as after [`PalmPanel::hold`].
//! - A menu in the air closes by itself: no pointer on it for [`IDLE_S`],
//!   a pinch that starts away from it ([`AWAY_MARGIN`]; that pinch does
//!   nothing else until it is released), or the spoken "close the menu".
//!   A pinch that just missed a row, inside the margin, does nothing.
//!
//! **Where a press lands** (board #5572). Closing the fingers moves the
//! hand, so by the frame a pinch registers the ray has left the row the
//! wearer aimed at. The press lands where the cursor was [`AIM_BACK_S`]
//! before that frame, from a short trail of the cursor on the panel
//! ([`AIM_KEEP_S`]). If the cursor was off the panel then, the pinch
//! presses nothing: a ray sweeping past as the fingers close is not a
//! press. And after one press no other begins for [`PRESS_GAP_S`], so a
//! hand whose tips hover at the pinch's threshold does not walk down the
//! rows. A drag is one pinch and is not cut.
//!
//! The frame is [`Placement::facing_view`]. With the knob at 0 none of
//! this runs: the menu lives on the palm, shown while the palm faces the
//! wearer, with world up's frame.

use std::collections::VecDeque;

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
/// Board #5572: how long the left palm must face the wearer before it
/// brings the menu or closes it (s). A hand passing through the pose on
/// its way somewhere else does not hold it this long.
pub const PALM_DWELL_S: f32 = 0.3;
/// Board #5572: how long after a fist or a voice window on the left hand
/// the palm brings nothing (s). An opening fist turns its palm to the
/// face.
pub const SUMMON_LOCKOUT_S: f32 = 1.5;
/// Board #5572: a press lands where the cursor was this long before the
/// pinch registered (s), and the trail of the cursor reaches this far
/// back (s).
pub const AIM_BACK_S: f32 = 0.12;
pub const AIM_KEEP_S: f32 = 0.25;
/// Board #5572: after one press begins, no other does for this long (s).
pub const PRESS_GAP_S: f32 = 0.35;
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

/// The log's line for `debug.fosfora.menustay` (board #5459, #5539).
pub fn knob_line(stay: bool) -> &'static str {
    if stay {
        "hand menu: debug.fosfora.menustay 1, the left palm held to the face parks it ahead in the air and closes it again, as a spoken \"menu\" does; pressed by the ray alone"
    } else {
        "hand menu: debug.fosfora.menustay 0, it lives on the palm"
    }
}

/// Where the hand menu is (board #5459).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum At {
    #[default]
    Hidden,
    /// On the left palm, following it (`debug.fosfora.menustay 0`).
    Palm,
    /// Parked ahead of the head, by the palm or by voice, holding still.
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
    /// Board #5572: the left palm turned to the face again.
    PalmAgain,
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
            Self::PalmAgain => "the palm raised again".to_owned(),
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
    /// Where the ray was (panel meters) on the frame the pinch registered:
    /// the cursor is held until the ray travels [`CLICK_SLOP_M`] from here.
    slop_from: Option<(f32, f32)>,
}

#[derive(Debug, Clone, Default)]
pub struct PalmPanel {
    placement: Option<Placement>,
    /// Where the menu is: hidden, on the palm, or (board #5459) in the air.
    at: At,
    /// Board #5459: the menu has its place in the air for the spoken
    /// "menu", its closes and the view's frame (`debug.fosfora.menustay`);
    /// off, it lives on the palm with world up's frame.
    stay: bool,
    /// Board #5459: the left palm faces the head, with the show and hide
    /// thresholds' hysteresis and nothing else.
    palm_up: bool,
    /// Board #5572: how long it has (s), toward [`PALM_DWELL_S`], and
    /// whether this raise has already brought or closed the menu.
    palm_s: f32,
    summoned: bool,
    /// Board #5572: the left hand is a fist or a voice window is open
    /// ([`Self::set_summon_blocked`]), and the time left after it during
    /// which the palm brings nothing (s).
    blocked: bool,
    lockout_s: f32,
    /// Board #5572: the palm brought the menu that is up (not the spoken
    /// "menu"): the gesture the palm hint teaches.
    by_palm: bool,
    /// Board #5572: the fingertip poke presses the menu
    /// (`debug.fosfora.menupoke 1`); off, the ray alone does.
    poke_on: bool,
    /// Board #5572: the menu's clock (s), stepped by [`Self::tick`]; the
    /// cursor's trail on the panel (the time, and where the ray was on it
    /// or `None` off it) and when the last press began.
    clock_s: f64,
    trail: VecDeque<(f64, Option<(f32, f32)>)>,
    last_press_s: Option<f64>,
    /// Board #5531, #5572: the palm faced the head while the menu was held
    /// back ([`Self::hold`]) or the summon was blocked, and has not turned
    /// away since.
    held_palm: bool,
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

    /// Board #5459, #5572: the menu has its place in the air, where the
    /// palm and the spoken "menu" park it (`debug.fosfora.menustay`, read
    /// live), or lives on the palm.
    pub fn set_stay(&mut self, on: bool) {
        self.stay = on;
    }

    /// Board #5572: whether a fingertip poke presses the menu
    /// (`debug.fosfora.menupoke`); off, the default, the ray alone does.
    pub fn set_poke(&mut self, on: bool) {
        self.poke_on = on;
    }

    /// Board #5572: the left hand is a fist or a voice window is open this
    /// frame, so its palm brings no menu, now or for
    /// [`SUMMON_LOCKOUT_S`] after. Call before [`Self::place`].
    pub fn set_summon_blocked(&mut self, blocked: bool) {
        self.blocked = blocked;
        if blocked {
            self.lockout_s = SUMMON_LOCKOUT_S;
        }
    }

    /// Board #5572: the left palm brought the menu that is up.
    pub fn by_palm(&self) -> bool {
        self.by_palm && self.at != At::Hidden
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
        self.palm_faces(palm, head);
        // Board #5572: a palm up through a fist, a voice window or the
        // time just after one brings nothing.
        if self.stay && self.palm_up && (self.blocked || self.lockout_s > 0.0) {
            self.held_palm = true;
        }
        // Board #5531: a palm raised while the menu was held back brings
        // nothing until it has turned away.
        if self.held_palm {
            if self.palm_up {
                return self.visible();
            }
            self.held_palm = false;
        }
        if self.stay {
            let summon = self.summon();
            self.place_staying(summon, head, head_rot)
        } else {
            self.place_on_palm(palm, head)
        }
    }

    /// Board #5572: the palm has faced the wearer for [`PALM_DWELL_S`],
    /// once per raise ([`Self::tick`] counts the time).
    fn summon(&mut self) -> bool {
        if !self.palm_up {
            self.palm_s = 0.0;
            self.summoned = false;
            return false;
        }
        if self.summoned || self.palm_s < PALM_DWELL_S {
            return false;
        }
        self.summoned = true;
        true
    }

    /// Board #5531: the menu held back for this frame, in place of
    /// [`Self::place`] and [`Self::input`], while the show waits for the
    /// wearer: hidden, and the palm does not bring it. The left palm and
    /// the right hand's pinch are still followed, so neither is new when
    /// the hold ends: a palm already turned to the face brings the menu
    /// only after it has turned away and back, and a pinch already closed
    /// presses and closes nothing.
    pub fn hold(&mut self, palm: Option<(Vec3, Quat)>, head: Vec3, pinching: bool) {
        self.palm_faces(palm, head);
        self.held_palm = self.palm_up;
        self.palm_s = 0.0;
        self.summoned = false;
        self.by_palm = false;
        self.trail.clear();
        self.at = At::Hidden;
        self.placement = None;
        self.closed = None;
        self.swallowed = [false; 2];
        self.pressed = false;
        self.last_depth = None;
        self.touch = Touch::default();
        self.ray = Ray {
            was_pinching: pinching,
            ..Ray::default()
        };
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

    /// Board #5572: the palm parks the menu ahead and closes it again.
    /// `summon`: the palm has been held to the face ([`Self::summon`]).
    fn place_staying(&mut self, summon: bool, head: Vec3, head_rot: Quat) -> Option<Placement> {
        // A menu the knob at 0 left on the palm holds still where it is.
        if self.at == At::Palm {
            self.at = At::Air;
            self.idle_s = 0.0;
        }
        if summon {
            if self.at == At::Hidden {
                self.open_parked(head, head_rot);
                self.by_palm = true;
            } else if !self.pointed() {
                self.close(Close::PalmAgain);
            }
        }
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
        self.by_palm = false;
        self.touch = Touch::default();
        self.ray.pressed = false;
        self.ray.held_at = None;
        self.ray.slop_from = None;
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
        self.by_palm = false;
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
    /// air with no pointer on it for [`IDLE_S`] closes. Board #5572: the
    /// menu's clock, the palm's dwell and the lockout after a fist run
    /// here too.
    pub fn tick(&mut self, dt: f32) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.clock_s += f64::from(dt);
        if self.palm_up && !self.held_palm {
            self.palm_s += dt;
        } else {
            self.palm_s = 0.0;
        }
        if !self.blocked {
            self.lockout_s = (self.lockout_s - dt).max(0.0);
        }
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
    /// pinching. The fingertip is read only with the poke on
    /// ([`Self::set_poke`]), and then a poke wins while the fingertip is
    /// near the panel. Call after [`Self::place`].
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
        let poke = self.poke(tip.filter(|_| self.poke_on));
        let (aim, within) = self.aim(ray, pinching);
        let poke_wins = poke.pressed || poke.poke_depth.is_some_and(|d| d < POKE_WINS_M);
        self.touch = if poke_wins {
            // A poke in progress cancels a ray press.
            self.ray.pressed = false;
            self.ray.held_at = None;
            self.ray.slop_from = None;
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
            self.trail.clear();
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
        // Board #5572: the cursor's trail on the panel, a quarter second
        // of it.
        let now = self.clock_s;
        while self
            .trail
            .front()
            .is_some_and(|(t, _)| now - t > f64::from(AIM_KEEP_S))
            || self.trail.len() >= 64
        {
            self.trail.pop_front();
        }
        self.trail.push_back((now, hit.filter(|_| hover)));
        if self.ray.pressed {
            if !pinching {
                self.ray.pressed = false;
                self.ray.held_at = None;
                self.ray.slop_from = None;
            }
        } else if began {
            // Board #5572: the press lands where the cursor was before the
            // fingers began to close. With no sample that old (the menu
            // just came up), where it is now.
            let earlier = self
                .trail
                .iter()
                .rev()
                .find(|(t, _)| now - t >= f64::from(AIM_BACK_S))
                .map(|(_, at)| *at);
            let at = earlier.unwrap_or(hit.filter(|_| hover));
            let rested = self
                .last_press_s
                .is_none_or(|t| now - t >= f64::from(PRESS_GAP_S));
            if let Some(at) = at
                && rested
            {
                self.ray.pressed = true;
                self.ray.held_at = Some(at);
                self.ray.slop_from = hit.or(Some(at));
                self.last_press_s = Some(now);
            }
        }
        // Past the slop the cursor follows the ray (a slider drag).
        if let (Some(from), Some(h)) = (self.ray.slop_from, hit)
            && ((h.0 - from.0).powi(2) + (h.1 - from.1).powi(2)).sqrt() > CLICK_SLOP_M
        {
            self.ray.held_at = None;
            self.ray.slop_from = None;
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
/// Board #5575: the Tips control, on unless saved off (the hints a wearer
/// had been taught were saved here as `taught` from board #5452 until
/// then; a file that has the field loads, and a save leaves it out).
/// Board #5534: the wearer's flash limit setting, absent
/// until they choose. A missing or broken file is the defaults.
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
    /// The Tips control (board #5575, `first_run::Hints`): the first
    /// run's hints show on every launch until the wearer turns this off.
    pub tips: bool,
    /// The "Limit flashing" setting (board #5534, `flash_choice.rs`): the
    /// photosensitive limit on or off, as the wearer chose. `None` until
    /// they answer the question or change the row: they have not been
    /// asked, and the limit is on (`flash::decide`).
    pub limit_flashing: Option<bool>,
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
            tips: true,
            limit_flashing: None,
        }
    }
}

impl MenuFile {
    /// The file's text read leniently: a field that is missing or not a
    /// boolean keeps its default; an `agent` that is missing or names no
    /// pick is no saved choice; an `immerse` that is missing or names no
    /// depth is full; a `tips` that is missing is on; a
    /// `limit_flashing` that is missing or not a boolean is no choice.
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
            tips: field("tips", d.tips),
            limit_flashing: value
                .get(crate::flash_choice::FIELD)
                .and_then(serde_json::Value::as_bool),
        }
    }

    /// The file's text: `{"debug":false,"immerse":"full","voice":true}`,
    /// with `"agent":"local"` (`anthropic`, `openai`, `off`) once picked,
    /// `"roam":true` and `"space_setup_asked":true` only while true, and
    /// `"tips":false` only while the tips are off, and
    /// `"limit_flashing":true` or `false` once the wearer has
    /// chosen.
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
        if !self.tips {
            json["tips"] = false.into();
        }
        if let Some(on) = self.limit_flashing {
            json[crate::flash_choice::FIELD] = on.into();
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

    /// Board #5575: a file that lists the hints taught, as the app saved
    /// them until the tips showed on every launch, loads, and a save
    /// leaves the list out.
    /// Board #5470: the menu no longer has an effect row. A
    /// `hand_menu.json` that names an effect, cut or not, loads with every
    /// field it does have, and a save leaves the name out.
    #[test]
    fn a_menu_file_that_names_a_cut_effect_still_loads() {
        let json = r#"{"agent":"local","debug":true,"effect":"Flock","world_effect":"Embers","effect_index":2,"immerse":"low","roam":true,"taught":["swipe"],"voice":false}"#;
        let file = MenuFile::parse(json);
        assert_eq!(
            file,
            MenuFile {
                debug: true,
                voice: false,
                agent: Some(crate::agent::Pick::Local),
                immerse: crate::immerse::Depth::Low,
                roam: true,
                space_setup_asked: false,
                tips: true,
                limit_flashing: None,
            }
        );
        let saved = file.to_json();
        assert!(
            !saved.contains("effect") && !saved.contains("Flock") && !saved.contains("taught"),
            "{saved}"
        );
        assert_eq!(MenuFile::parse(&saved), file);
        // The effect's name alone is the defaults.
        assert_eq!(
            MenuFile::parse(r#"{"effect":"Flux Cloud"}"#),
            MenuFile::default()
        );
    }

    /// Board #5575: the Tips control round-trips, is written only while
    /// off, and a file from before it, or with something else in the
    /// field, has the tips on.
    #[test]
    fn the_menu_file_saves_the_tips_off_and_reads_them_on_when_absent() {
        for tips in [true, false] {
            let file = MenuFile {
                tips,
                voice: false,
                ..MenuFile::default()
            };
            let json = file.to_json();
            assert_eq!(MenuFile::parse(&json), file, "{json}");
            assert_eq!(json.contains(r#""tips":false"#), !tips, "{json}");
            assert_eq!(json.contains("tips"), !tips, "{json}");
        }
        // Files written before the control, as the app saved them, with
        // every hint taught or none: every field they have still loads,
        // and the tips are on.
        for (json, voice, roam) in [
            (r#"{"debug":false,"voice":true}"#, true, false),
            (
                r#"{"agent":"local","debug":false,"immerse":"black","roam":true,"space_setup_asked":true,"taught":["swipe","talk","palm"],"voice":false}"#,
                false,
                true,
            ),
        ] {
            let file = MenuFile::parse(json);
            assert!(file.tips, "{json}");
            assert_eq!((file.voice, file.roam), (voice, roam), "{json}");
        }
        let old = MenuFile::parse(
            r#"{"agent":"local","debug":false,"immerse":"black","roam":true,"space_setup_asked":true,"voice":false}"#,
        );
        assert_eq!(old.immerse, crate::immerse::Depth::Black);
        assert_eq!(old.agent, Some(crate::agent::Pick::Local));
        assert!(old.space_setup_asked);
        // A mistyped field turns nothing off.
        assert!(MenuFile::parse(r#"{"tips": "off"}"#).tips);
    }

    /// Board #5534: the flash limit setting round-trips either way, is
    /// written only once the wearer has chosen, and a file from before the
    /// field, or with something else in it, is no choice.
    #[test]
    fn the_menu_file_saves_the_flash_limit_once_chosen_and_reads_no_choice_when_absent() {
        for choice in [Some(true), Some(false)] {
            let file = MenuFile {
                limit_flashing: choice,
                voice: false,
                ..MenuFile::default()
            };
            let json = file.to_json();
            assert!(
                json.contains(&format!(r#""limit_flashing":{}"#, choice.unwrap())),
                "{json}"
            );
            assert_eq!(MenuFile::parse(&json), file, "{json}");
        }
        // Not chosen: the field is not written, and the file reads back as
        // not chosen.
        let json = MenuFile::default().to_json();
        assert!(!json.contains("limit_flashing"), "{json}");
        assert_eq!(MenuFile::parse(&json).limit_flashing, None);
        // Files written before the field, as the app saved them: every
        // field they have still loads, and the wearer has not been asked.
        for json in [
            r#"{"debug":false,"voice":true}"#,
            r#"{"agent":"local","debug":false,"immerse":"black","roam":true,"space_setup_asked":true,"taught":["swipe","talk","palm"],"voice":false}"#,
            "",
            "not json",
        ] {
            assert_eq!(MenuFile::parse(json).limit_flashing, None, "{json}");
        }
        let old = MenuFile::parse(
            r#"{"agent":"local","debug":false,"immerse":"black","roam":true,"space_setup_asked":true,"taught":["swipe","talk","palm"],"voice":false}"#,
        );
        assert!(old.tips);
        assert_eq!(old.immerse, crate::immerse::Depth::Black);
        assert!(old.roam && old.space_setup_asked && !old.voice);
        // A mistyped field is no choice, never an "off".
        for json in [
            r#"{"limit_flashing":"off"}"#,
            r#"{"limit_flashing":0}"#,
            r#"{"limit_flashing":null}"#,
            r#"{"limit_flashing":"false"}"#,
        ] {
            assert_eq!(MenuFile::parse(json).limit_flashing, None, "{json}");
        }
        // The choice beside the other fields.
        let file = MenuFile::parse(r#"{"debug":true,"limit_flashing":false,"voice":true}"#);
        assert_eq!(file.limit_flashing, Some(false));
        assert!(file.debug && file.voice);
        // What the limiter does with each state of the file, the knob
        // unset: on unless the wearer turned it off.
        use crate::flash::{By, decide};
        assert_eq!(
            decide(None, MenuFile::default().limit_flashing),
            (true, By::NotAsked)
        );
        assert_eq!(decide(None, file.limit_flashing), (false, By::Wearer));
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

    /// The menu on the palm (the knob at 0) with the poke on: the two
    /// pointers as they were before board #5572.
    fn shown_panel() -> (PalmPanel, Placement) {
        let mut panel = PalmPanel::default();
        panel.set_poke(true);
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

    /// Board #5572: the menu the palm brought, with the knob at 1: parked
    /// ahead in the air.
    fn staying() -> (PalmPanel, Placement) {
        let mut panel = PalmPanel::default();
        panel.set_stay(true);
        let p = hold_palm(&mut panel).expect("shown");
        assert_eq!(panel.at(), At::Air);
        assert!(panel.by_palm());
        (panel, p)
    }

    /// The menu in the air by the spoken "menu", no palm.
    fn parked() -> (PalmPanel, Placement) {
        let mut panel = PalmPanel::default();
        panel.set_stay(true);
        assert!(panel.open_parked(HEAD, Quat::IDENTITY));
        let p = panel.placement().expect("parked");
        assert_eq!(panel.at(), At::Air);
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

    /// The left palm held to the face until it acts: its dwell and a few
    /// frames more.
    fn hold_palm(panel: &mut PalmPanel) -> Option<Placement> {
        let mut drawn = None;
        for _ in 0..(PALM_DWELL_S / DT) as usize + 3 {
            drawn = frame(panel, Some(palm(1.0)), None, false, false);
        }
        drawn
    }

    /// `seconds` of frames with nothing pointing and the palm away.
    fn idle(panel: &mut PalmPanel, seconds: f32) -> Option<Placement> {
        let mut drawn = panel.placement();
        for _ in 0..(seconds / DT).round() as usize {
            drawn = frame(panel, None, None, false, false);
        }
        drawn
    }

    /// Board #5572: the palm held to the face parks the menu ahead of the
    /// head, where the spoken "menu" puts it, and there it holds still
    /// whatever the hand does. Held to the face again it closes it. A palm
    /// passing through the pose does neither.
    #[test]
    fn the_palm_held_to_the_face_parks_the_menu_ahead_and_closes_it_again() {
        let mut panel = PalmPanel::default();
        panel.set_stay(true);
        // A palm that passes through the pose brings nothing.
        for _ in 0..(PALM_DWELL_S / DT) as usize - 3 {
            assert!(frame(&mut panel, Some(palm(1.0)), None, false, false).is_none());
        }
        assert!(frame(&mut panel, Some(palm(0.0)), None, false, false).is_none());
        // Held: parked ahead, not on the palm.
        let p = hold_palm(&mut panel).expect("parked");
        assert_eq!(panel.at(), At::Air);
        assert!(panel.by_palm());
        let ahead = HEAD + Vec3::new(0.0, -PARK_BELOW_M, -PARK_AHEAD_M);
        assert!(p.center.distance(ahead) < 1e-5, "{}", p.center);
        // It holds still while the hand moves, turns away and drops out
        // of tracking.
        let (pos, rot) = palm(1.0);
        let moved = Some((pos + Vec3::new(0.1, 0.05, 0.0), rot));
        for hand in [moved, Some(palm(0.0)), None] {
            for _ in 0..10 {
                assert_eq!(frame(&mut panel, hand, None, false, false), Some(p));
            }
        }
        assert_eq!(panel.take_closed(), None);
        // The palm held to the face again closes it, and says why.
        assert!(hold_palm(&mut panel).is_none());
        assert_eq!(panel.take_closed(), Some(Close::PalmAgain));
        assert_eq!(Close::PalmAgain.label(), "the palm raised again");
        // One raise acts once: still up, it brings nothing back.
        for _ in 0..72 {
            assert!(frame(&mut panel, Some(palm(1.0)), None, false, false).is_none());
        }
        // Away and held again: the menu again.
        frame(&mut panel, Some(palm(0.0)), None, false, false);
        let p = hold_palm(&mut panel).expect("parked again");
        // Not from under a pointer: the ray on a row and the palm held.
        frame(&mut panel, Some(palm(0.0)), None, false, false);
        let on = ray_at(&p, 0.0, 0.0);
        for _ in 0..72 {
            assert!(frame(&mut panel, Some(palm(1.0)), on, false, false).is_some());
        }
        // The spoken "menu" is not the palm's.
        let (spoken, _) = parked();
        assert!(!spoken.by_palm());
        const { assert!(PALM_DWELL_S >= 0.2 && PALM_DWELL_S <= 0.5) };
    }

    /// Board #5572: the fingertip presses nothing unless the poke is on.
    #[test]
    fn the_fingertip_presses_nothing_unless_the_poke_is_on() {
        let (mut panel, p) = parked();
        panel.input(tip_at(&p, 0.0, 0.0, 0.02), None, false);
        let t = panel.input(tip_at(&p, 0.0, 0.0, 0.0), None, false);
        assert_eq!(t, Touch::default());
        assert!(!panel.pointed());
        panel.set_poke(true);
        panel.input(tip_at(&p, 0.0, 0.0, 0.02), None, false);
        assert!(panel.input(tip_at(&p, 0.0, 0.0, 0.0), None, false).pressed);
    }

    /// Board #5572: no surprise menu. A palm turned to the face through a
    /// fist or a voice window, or in the second and a half after one,
    /// brings nothing until it has turned away and back.
    #[test]
    fn a_fist_or_a_voice_window_and_the_time_after_bring_no_menu() {
        fn step(panel: &mut PalmPanel, blocked: bool, facing: f32) -> Option<Placement> {
            panel.set_summon_blocked(blocked);
            frame(panel, Some(palm(facing)), None, false, false)
        }
        let frames = |seconds: f32| (seconds / DT) as usize;
        // A fist with its palm to the face, 2 s: nothing.
        let mut panel = PalmPanel::default();
        panel.set_stay(true);
        for _ in 0..frames(2.0) {
            assert!(step(&mut panel, true, 1.0).is_none());
        }
        // The hand opens and the palm stays to the face: nothing, however
        // long, until it turns away and back.
        for _ in 0..frames(3.0) {
            assert!(step(&mut panel, false, 1.0).is_none());
        }
        step(&mut panel, false, 0.0);
        assert!(hold_palm(&mut panel).is_some());
        // The fist opens with the palm away, and the palm comes up a
        // second later, inside the lockout: nothing while it is held.
        let mut panel = PalmPanel::default();
        panel.set_stay(true);
        for _ in 0..10 {
            step(&mut panel, true, 0.0);
        }
        for _ in 0..frames(1.0) {
            step(&mut panel, false, 0.0);
        }
        for _ in 0..frames(2.0) {
            assert!(step(&mut panel, false, 1.0).is_none());
        }
        step(&mut panel, false, 0.0);
        assert!(hold_palm(&mut panel).is_some());
        // Raised once the lockout has run out, it comes on the dwell.
        let mut panel = PalmPanel::default();
        panel.set_stay(true);
        step(&mut panel, true, 0.0);
        for _ in 0..frames(SUMMON_LOCKOUT_S + 0.1) {
            step(&mut panel, false, 0.0);
        }
        assert!(hold_palm(&mut panel).is_some());
        // A fist's palm does not close a menu that is up, either.
        for _ in 0..frames(1.0) {
            assert!(step(&mut panel, true, 0.0).is_some());
        }
        step(&mut panel, true, 0.0);
        let on = ray_at(&panel.placement().expect("up"), 0.0, 0.0);
        for _ in 0..frames(1.0) {
            panel.set_summon_blocked(true);
            assert!(frame(&mut panel, Some(palm(1.0)), on, false, false).is_some());
        }
        const { assert!(SUMMON_LOCKOUT_S >= 1.0) };
    }

    /// One frame of the right hand's ray at panel meters (x, y) on the
    /// parked menu `p`, pinching or not.
    fn point(panel: &mut PalmPanel, p: &Placement, x: f32, y: f32, pinch: bool) -> Touch {
        panel.place(None, HEAD, Quat::IDENTITY);
        let t = panel.input(None, ray_at(p, x, y), pinch);
        panel.tick(DT);
        t
    }

    /// Board #5572: the press lands where the cursor was before the
    /// fingers began to close, not where the hand had jolted to when the
    /// pinch registered. A ray that was off the menu then presses nothing.
    #[test]
    fn a_press_lands_where_the_cursor_was_before_the_fingers_closed() {
        let (mut panel, p) = parked();
        // The cursor rests on a row 5 cm over the middle.
        for _ in 0..22 {
            point(&mut panel, &p, 0.0, 0.05, false);
        }
        // The fingers close: over six frames the ray drops 4 cm, two rows
        // down, and the pinch registers there.
        for i in 1..=6 {
            point(&mut panel, &p, 0.0, 0.05 - 0.04 * i as f32 / 6.0, false);
        }
        let t = point(&mut panel, &p, 0.0, 0.01, true);
        assert!(t.pressed);
        let v_of = |y: f32| (1.0 - y / p.half_h) * 0.5;
        let [u, v] = t.pointer.expect("on the menu");
        assert!((u - 0.5).abs() < 0.01, "{u}");
        assert!((v - v_of(0.05)).abs() < 0.01, "{v} vs {}", v_of(0.05));
        assert!((v - v_of(0.01)).abs() > 0.04);
        // Held there while the pinch is closed, and released there: a
        // click on the row the wearer aimed at.
        let held = point(&mut panel, &p, 0.0, 0.01, true);
        assert_eq!(held.pointer, t.pointer);
        assert!(!point(&mut panel, &p, 0.0, 0.01, false).pressed);
        // A ray off the menu that sweeps onto it as the fingers close
        // presses nothing, then or while the pinch stays closed.
        for _ in 0..36 {
            point(&mut panel, &p, 0.25, 0.0, false);
        }
        for i in 1..=4 {
            point(&mut panel, &p, 0.25 - 0.25 * i as f32 / 4.0, 0.0, false);
        }
        for _ in 0..20 {
            assert!(!point(&mut panel, &p, 0.0, 0.0, true).pressed);
        }
        assert!(panel.placement().is_some());
        point(&mut panel, &p, 0.0, 0.0, false);
        // Rested on the menu again, the next pinch presses.
        for _ in 0..22 {
            point(&mut panel, &p, 0.0, 0.0, false);
        }
        assert!(point(&mut panel, &p, 0.0, 0.0, true).pressed);
        const { assert!(AIM_BACK_S < AIM_KEEP_S) };
    }

    /// Board #5572: after one press no other begins for
    /// [`PRESS_GAP_S`], and a drag is one press.
    #[test]
    fn after_one_press_no_other_begins_for_a_third_of_a_second() {
        let (mut panel, p) = parked();
        for _ in 0..22 {
            point(&mut panel, &p, 0.0, 0.0, false);
        }
        assert!(point(&mut panel, &p, 0.0, 0.0, true).pressed);
        for _ in 0..2 {
            point(&mut panel, &p, 0.0, 0.0, true);
        }
        for _ in 0..3 {
            assert!(!point(&mut panel, &p, 0.0, 0.0, false).pressed);
        }
        // A second pinch 83 ms after the first: nothing, for as long as
        // it is closed.
        for _ in 0..30 {
            assert!(!point(&mut panel, &p, 0.0, 0.0, true).pressed);
        }
        point(&mut panel, &p, 0.0, 0.0, false);
        // Past the gap, the next one presses, and held and moved it stays
        // one press: a drag.
        assert!(point(&mut panel, &p, 0.0, 0.0, true).pressed);
        for i in 0..72 {
            let x = 0.06 * (i as f32 / 72.0);
            assert!(point(&mut panel, &p, x, 0.0, true).pressed);
        }
    }

    /// No pointer on the menu in the air for its idle time closes it; a
    /// pointer starts the count again; on the palm it never counts.
    #[test]
    fn the_menu_in_the_air_closes_after_its_idle_time_without_a_pointer() {
        // Brought by the palm, it counts down as the spoken one does.
        let (mut panel, _) = staying();
        assert!(idle(&mut panel, IDLE_S - 0.5).is_some());
        assert!(idle(&mut panel, 0.6).is_none());
        assert_eq!(panel.take_closed(), Some(Close::Idle(IDLE_S)));
        // In the air, by the spoken "menu".
        let (mut panel, p) = parked();
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
        let (mut panel, _) = parked();
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
        let (mut panel, p) = parked();
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
        let (mut panel, p) = parked();
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
        // The left hand's pinch never closes it, in the air or on the palm.
        let (mut panel, _) = parked();
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
        let (mut panel, p) = parked();
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

    /// The palm and the spoken "menu" work the same menu: the palm closes
    /// one the voice opened, brings one the idle count closed, and after a
    /// spoken close with the palm still up it waits for the palm to turn
    /// away and back.
    #[test]
    fn the_palm_and_the_voice_work_the_same_menu() {
        // In the air by the spoken "menu": the palm held closes it.
        let (mut panel, _) = parked();
        assert!(hold_palm(&mut panel).is_none());
        assert_eq!(panel.take_closed(), Some(Close::PalmAgain));
        // Closed by the idle count, the palm brings it back.
        let (mut panel, _) = parked();
        assert!(idle(&mut panel, IDLE_S + 0.1).is_none());
        assert_eq!(panel.take_closed(), Some(Close::Idle(IDLE_S)));
        assert!(hold_palm(&mut panel).is_some());
        // Closed by voice with the palm still up: closed until the palm
        // turns away and is held again.
        assert!(panel.close_spoken());
        for _ in 0..72 {
            assert!(frame(&mut panel, Some(palm(1.0)), None, false, false).is_none());
        }
        assert!(frame(&mut panel, Some(palm(0.0)), None, false, false).is_none());
        assert!(hold_palm(&mut panel).is_some());
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

    /// Board #5531: held back, the palm brings nothing. When the hold ends
    /// a palm still turned to the face brings nothing either until it has
    /// turned away and back, and a pinch closed through the end of the
    /// hold neither presses a row nor closes the menu. With the menu
    /// staying and on the palm alike.
    #[test]
    fn held_back_the_menu_does_not_come_and_nothing_from_the_hold_is_read_after() {
        for stay in [true, false] {
            let mut panel = PalmPanel::default();
            panel.set_stay(stay);
            for facing in [1.0, 0.0, 1.0, 1.0] {
                panel.hold(Some(palm(facing)), HEAD, true);
                assert!(panel.placement().is_none(), "stay {stay}");
                assert_eq!(panel.at(), At::Hidden);
            }
            // The hold ends with the palm up and the right pinch closed.
            for _ in 0..10 {
                assert!(frame(&mut panel, Some(palm(1.0)), None, true, false).is_none());
            }
            // The palm turns away and back: the menu. The pinch from the
            // hold is still closed, on a row: no press.
            frame(&mut panel, Some(palm(0.0)), None, true, false);
            let mut shown = None;
            for _ in 0..(PALM_DWELL_S / DT) as usize + 3 {
                shown = frame(&mut panel, Some(palm(1.0)), None, true, false);
            }
            let p = shown.expect("shown");
            assert_eq!(panel.at(), if stay { At::Air } else { At::Palm });
            assert!(!aim(&mut panel, ray_at(&p, 0.0, 0.0), true).pressed);
            // Nor does it close the menu from away from it.
            frame(
                &mut panel,
                Some(palm(1.0)),
                ray_at(&p, 0.0, -0.6),
                true,
                false,
            );
            assert_eq!(panel.swallowed(), [false, false], "stay {stay}");
            assert!(panel.placement().is_some());
            // Opened and closed again, it clicks.
            aim(&mut panel, ray_at(&p, 0.0, 0.0), false);
            assert!(aim(&mut panel, ray_at(&p, 0.0, 0.0), true).pressed);
        }
        // A palm that was down when the hold ended brings it on its dwell.
        let mut panel = PalmPanel::default();
        panel.set_stay(true);
        panel.hold(Some(palm(0.0)), HEAD, false);
        assert!(hold_palm(&mut panel).is_some());
        // The hold hides a menu that is up.
        let (mut panel, _) = staying();
        panel.hold(None, HEAD, false);
        assert!(panel.placement().is_none());
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
        let (mut panel, _) = parked();
        frame(&mut panel, None, None, false, false);
        assert_eq!(panel.at(), At::Air);
        panel.set_stay(false);
        assert!(frame(&mut panel, None, None, false, false).is_none());
    }

    /// Boards #5459, #5572: lying back with the hand straight above the
    /// face, the palm parks the menu along the view, over the face and
    /// upright for the wearer, and the hand's jitter does not move it.
    #[test]
    fn the_palm_overhead_parks_the_menu_over_the_face() {
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
        let step = |panel: &mut PalmPanel, d: Vec3| {
            panel.place(Some((over + d, Quat::IDENTITY)), HEAD, looking_up());
            panel.input(None, None, false);
            panel.tick(DT);
            panel.placement()
        };
        let mut shown = None;
        for i in 0..(PALM_DWELL_S / DT) as usize + 3 {
            shown = step(&mut panel, jitter[i % jitter.len()]);
        }
        let p = shown.expect("shown");
        assert_eq!(panel.at(), At::Air);
        assert!(degrees(p.right, Vec3::X) < 3.0, "{p:?}");
        assert!(p.up.z > 0.9, "{p:?}");
        assert!(p.normal.dot((HEAD - p.center).normalize()) > 0.999);
        assert!(p.center.y > HEAD.y + 0.4, "{}", p.center);
        for d in jitter {
            assert_eq!(step(&mut panel, d), Some(p));
        }
        // The parked menu sits inside the glasses' design area, 58 by 54
        // degrees about the view's center: 25 degrees wide, and at its
        // tallest (the developer's menu with Edit room on, 24 cm since
        // Voice and Theme have their own row, board #5572) from 9 degrees
        // over the center to 21 under. The release menu is 16 cm.
        let half_w = (PANEL_W_M * 0.5 / PARK_AHEAD_M).atan().to_degrees();
        assert!(half_w < 29.0, "{half_w}");
        for h in [0.162f32, 0.242] {
            let top = ((h * 0.5 - PARK_BELOW_M) / PARK_AHEAD_M)
                .atan()
                .to_degrees();
            let bottom = ((h * 0.5 + PARK_BELOW_M) / PARK_AHEAD_M)
                .atan()
                .to_degrees();
            assert!(top < 27.0 && bottom < 27.0, "{h}: {top} {bottom}");
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
